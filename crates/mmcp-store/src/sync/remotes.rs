//! Effective remote-set resolution.
//!
//! Merges a loaded [`UserConfig`] and [`ProjectConfig`] into the
//! flat, fully-resolved list of remotes a sync operation should use,
//! plus which one is the default. Per-file syntax and single-level
//! semantic checks (duplicate names, more than one `default = true`)
//! already ran inside `SyncConfig::validate` when each config was
//! loaded; this module is the first place both files are available
//! together, so it owns every CROSS-level check.

use std::collections::HashSet;

use mmcp_core::config::{ProjectConfig, Remote, SyncConfig, UserConfig};

use crate::error::StoreError;

/// Synthetic name for the implicit `mmcp-server` remote built from
/// the user-level `sync.server_url` legacy shorthand.
const USER_LEGACY_REMOTE_NAME: &str = "user-legacy";

/// Synthetic name for the implicit `mmcp-server` remote built from
/// the project-level `sync.server_url` legacy shorthand. Distinct
/// from [`USER_LEGACY_REMOTE_NAME`] so a project and its user both
/// using the shorthand never collide by name.
const PROJECT_LEGACY_REMOTE_NAME: &str = "project-legacy";

/// Which config file a [`ResolvedRemote`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteLevel {
    User,
    Project,
}

/// One remote from the effective set.
#[derive(Debug, Clone)]
pub struct ResolvedRemote {
    /// The parsed remote itself. For a `server_url` legacy shorthand
    /// entry, a synthesized `Remote::MmcpServer` carrying
    /// [`USER_LEGACY_REMOTE_NAME`] / [`PROJECT_LEGACY_REMOTE_NAME`]
    /// as its `name`.
    pub remote: Remote,
    /// Which config file this entry came from.
    pub level: RemoteLevel,
    /// For a `direct-git` remote: its target group, UUID or slug,
    /// still unparsed - resolving a slug to a concrete `GroupId`
    /// needs the live `GroupIndex`, an async lookup outside this
    /// pure resolver's reach; `mmcp-store::sync::build_engine` runs
    /// that lookup. Always populated by the time resolution succeeds
    /// (the config's own explicit value, or the project's own UUID
    /// string when omitted at project level). `None` for an
    /// `mmcp-server` remote.
    pub direct_git_group: Option<String>,
}

impl ResolvedRemote {
    /// This remote's name, whichever level or synthesis path it came
    /// from.
    #[must_use]
    pub fn name(&self) -> &str {
        self.remote.name()
    }

    /// True for the synthesized entry built from a `server_url`
    /// legacy shorthand rather than a real `[[sync.remotes]]` entry.
    /// `mmcp-store::sync::build_engine` uses this to pick which
    /// credential env vars a remote reads: an un-suffixed
    /// `MMCP_SYNC_TOKEN` / `MMCP_SYNC_PUSH_TOKEN` for a legacy entry,
    /// the name-derived `MMCP_SYNC_TOKEN_<NAME>` /
    /// `MMCP_SYNC_PUSH_TOKEN_<NAME>` pair for every other remote.
    #[must_use]
    pub fn is_legacy_shorthand(&self) -> bool {
        matches!(
            self.name(),
            USER_LEGACY_REMOTE_NAME | PROJECT_LEGACY_REMOTE_NAME
        )
    }
}

/// Effective, fully-resolved remote set for a sync operation.
#[derive(Debug, Clone)]
pub struct EffectiveRemotes {
    /// Every remote in the effective set, user remotes (unless
    /// excluded by `project_remote_only`) then project remotes,
    /// concatenated.
    pub remotes: Vec<ResolvedRemote>,
    /// Index into `remotes` of the resolved default remote. `None`
    /// only when `remotes` is empty; a non-empty set always resolves
    /// to exactly one default or fails with
    /// [`StoreError::AmbiguousDefaultRemote`].
    pub default_index: Option<usize>,
}

impl EffectiveRemotes {
    /// The resolved default remote, if any.
    #[must_use]
    pub fn default_remote(&self) -> Option<&ResolvedRemote> {
        self.default_index.map(|i| &self.remotes[i])
    }

    /// Short human-readable label for this effective remote set: the
    /// sole remote's name, `"N remote(s), default '<name>'"` for
    /// several, or `"(no remotes configured)"` for an empty set. The
    /// single owning definition shared by every consumer that shows a
    /// remote-set summary (`mmcp-client`'s CLI log lines and MCP
    /// tool responses, the GUI's status bar) instead of each
    /// reimplementing the same formatting.
    #[must_use]
    pub fn summary_label(&self) -> String {
        match self.default_remote() {
            Some(default) if self.remotes.len() == 1 => default.name().to_string(),
            Some(default) => format!(
                "{} remote(s), default '{}'",
                self.remotes.len(),
                default.name()
            ),
            None => "(no remotes configured)".to_string(),
        }
    }
}

/// Resolve the effective remote set for a sync operation, merging
/// `user` and `project` into one flat, precedence-ordered list.
///
/// The cross-level name-collision check runs against EVERY name
/// declared at both levels unconditionally, even when
/// `project_remote_only` excludes the user level from the returned
/// active set: `project_remote_only` narrows which remotes are used
/// for sync, it must never widen which names are safe for a project
/// to reuse. Without this, a project could declare a remote sharing
/// a real user-level remote's name and inherit that name's ambient
/// `MMCP_SYNC_TOKEN_<NAME>` credential while pointing the connection
/// at an attacker-controlled URL (FR-301 security finding F1).
///
/// # Errors
/// [`StoreError::InvalidRemoteName`] for a name outside the safe git-ref
/// charset, [`StoreError::DirectGitMissingGroup`] for a user-level
/// `direct-git` entry with no `group`, [`StoreError::RemoteNameCollision`]
/// for a name declared more than once across the merged set, and
/// [`StoreError::AmbiguousDefaultRemote`] when two or more remotes
/// exist and none resolves as the default.
pub fn resolve_effective_remotes(
    user: &UserConfig,
    project: &ProjectConfig,
) -> Result<EffectiveRemotes, StoreError> {
    let mut user_remotes = Vec::new();
    collect_level(&user.sync, RemoteLevel::User, project, &mut user_remotes)?;

    let mut project_remotes = Vec::new();
    collect_level(
        &project.sync,
        RemoteLevel::Project,
        project,
        &mut project_remotes,
    )?;

    check_name_collisions(user_remotes.iter().chain(project_remotes.iter()))?;

    let mut remotes = Vec::new();
    if !project.project_remote_only {
        remotes.extend(user_remotes);
    }
    remotes.extend(project_remotes);

    let default_index = resolve_default_index(&remotes)?;

    Ok(EffectiveRemotes {
        remotes,
        default_index,
    })
}

/// Append every remote `sync` declares (legacy shorthand first, then
/// `sync.remotes` in file order) onto `out`, tagged with `level`.
fn collect_level(
    sync: &SyncConfig,
    level: RemoteLevel,
    project: &ProjectConfig,
    out: &mut Vec<ResolvedRemote>,
) -> Result<(), StoreError> {
    if let Some(url) = sync.server_url.as_deref().filter(|u| !u.is_empty()) {
        out.push(ResolvedRemote {
            remote: Remote::MmcpServer {
                name: legacy_name(level).to_string(),
                url: url.to_string(),
                default: false,
                include_in_push_all: true,
            },
            level,
            direct_git_group: None,
        });
    }

    for remote in &sync.remotes {
        validate_remote_name(remote.name())?;
        let direct_git_group = match remote {
            Remote::DirectGit { group, .. } => Some(resolve_direct_git_group(
                group.as_deref(),
                level,
                project,
                remote.name(),
            )?),
            Remote::MmcpServer { .. } => None,
        };
        out.push(ResolvedRemote {
            remote: remote.clone(),
            level,
            direct_git_group,
        });
    }
    Ok(())
}

/// Synthetic name for `level`'s legacy `server_url` shorthand entry.
fn legacy_name(level: RemoteLevel) -> &'static str {
    match level {
        RemoteLevel::User => USER_LEGACY_REMOTE_NAME,
        RemoteLevel::Project => PROJECT_LEGACY_REMOTE_NAME,
    }
}

/// Resolve a `direct-git` remote's group reference per the
/// project/user asymmetry: a project-level entry with no `group`
/// defaults to the project's own UUID; a user-level entry with no
/// `group` is a loud error, never silently defaulted (see
/// [`StoreError::DirectGitMissingGroup`]'s doc comment for why).
fn resolve_direct_git_group(
    group: Option<&str>,
    level: RemoteLevel,
    project: &ProjectConfig,
    remote_name: &str,
) -> Result<String, StoreError> {
    match (group, level) {
        (Some(g), _) => Ok(g.to_string()),
        (None, RemoteLevel::Project) => Ok(project.project_uuid.to_string()),
        (None, RemoteLevel::User) => Err(StoreError::DirectGitMissingGroup {
            name: remote_name.to_string(),
        }),
    }
}

/// A remote name flows verbatim into `refs/remotes/<name>/main`
/// (`mmcp_sync::BoundRemote::tracking_ref`); constrain it to the
/// charset a git ref path can safely carry (checked on the RAW name:
/// normalizing first would let a name with a disallowed character
/// slip through as long as its normalized form happened to be
/// clean). Also reject any name whose
/// [`SyncConfig::normalized_remote_name`] matches either reserved
/// synthetic name's own normalized form
/// ([`USER_LEGACY_REMOTE_NAME`] / [`PROJECT_LEGACY_REMOTE_NAME`]),
/// not just an exact raw-string match: without normalizing this
/// comparison, a declared `[[sync.remotes]]` entry named
/// `USER-LEGACY` or `user_legacy` would both slip past a raw
/// `matches!` guard AND, per
/// [`SyncConfig::normalized_remote_name`]'s own doc comment, collide
/// with the real legacy shorthand's credential env var. A level with
/// no `server_url` set has nothing synthesized for
/// [`check_name_collisions`] to catch such a spoof against, so this
/// guard is the only place that ever rejects it, and
/// `ResolvedRemote::is_legacy_shorthand` would otherwise misclassify
/// the declared remote as the shorthand, routing it to the
/// un-suffixed `MMCP_SYNC_TOKEN(_PUSH)?` env vars instead of its own
/// name-derived pair.
fn validate_remote_name(name: &str) -> Result<(), StoreError> {
    let charset_valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let normalized = SyncConfig::normalized_remote_name(name);
    let reserved = normalized == SyncConfig::normalized_remote_name(USER_LEGACY_REMOTE_NAME)
        || normalized == SyncConfig::normalized_remote_name(PROJECT_LEGACY_REMOTE_NAME);
    if charset_valid && !reserved {
        Ok(())
    } else {
        Err(StoreError::InvalidRemoteName {
            name: name.to_string(),
        })
    }
}

/// Reject a name appearing more than once across `remotes`, by
/// [`SyncConfig::normalized_remote_name`] rather than the raw string:
/// a cross-level collision, or a declared remote colliding with a
/// legacy shorthand's synthetic name, closes the same credential
/// env var collision described on
/// [`SyncConfig::normalized_remote_name`]'s own doc comment, now
/// across the two config files instead of within one. Same-level
/// duplicates among REAL `[[sync.remotes]]` entries (by normalized
/// key) are already impossible by the time this runs
/// (`SyncConfig::validate` rejected them at parse time); this only
/// ever fires on a genuinely cross-cutting name.
///
/// Takes an iterator rather than a single slice so the caller can
/// chain the user-level and project-level entries even when they are
/// never concatenated into one combined `Vec` (`project_remote_only`
/// keeps them apart in the ACTIVE set, but this check must still see
/// both, per [`resolve_effective_remotes`]'s doc comment).
///
/// The error still reports the colliding entry's own original,
/// unnormalized `name`, matching [`SyncConfig::validate`]'s same
/// readability choice.
fn check_name_collisions<'a>(
    remotes: impl Iterator<Item = &'a ResolvedRemote>,
) -> Result<(), StoreError> {
    let mut seen: HashSet<String> = HashSet::new();
    for entry in remotes {
        if !seen.insert(SyncConfig::normalized_remote_name(entry.name())) {
            return Err(StoreError::RemoteNameCollision {
                name: entry.name().to_string(),
            });
        }
    }
    Ok(())
}

/// Resolve the default remote's index per the precedence order: a
/// project-level `default = true` remote wins if present; else a
/// user-level `default = true` remote; else, when the effective set
/// has exactly one remote total, it is the implicit default; else
/// (two or more remotes, none marked default) a loud error.
fn resolve_default_index(remotes: &[ResolvedRemote]) -> Result<Option<usize>, StoreError> {
    if remotes.is_empty() {
        return Ok(None);
    }
    if let Some(idx) = remotes
        .iter()
        .position(|r| r.level == RemoteLevel::Project && r.remote.is_default())
    {
        return Ok(Some(idx));
    }
    if let Some(idx) = remotes
        .iter()
        .position(|r| r.level == RemoteLevel::User && r.remote.is_default())
    {
        return Ok(Some(idx));
    }
    if remotes.len() == 1 {
        return Ok(Some(0));
    }
    Err(StoreError::AmbiguousDefaultRemote {
        candidates: remotes.iter().map(|r| r.name().to_string()).collect(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use mmcp_core::config::{RemoteAuth, SyncConfig};
    use mmcp_core::id::ProjectUuid;

    const PROJECT_UUID: &str = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91";

    fn project_with(sync: SyncConfig, project_remote_only: bool) -> ProjectConfig {
        ProjectConfig {
            project_uuid: ProjectUuid::from_uuid(uuid::Uuid::parse_str(PROJECT_UUID).unwrap()),
            project_slug: None,
            sync,
            project_remote_only,
            subscriptions: mmcp_core::config::SubscriptionsConfig::default(),
        }
    }

    fn user_with(sync: SyncConfig) -> UserConfig {
        UserConfig {
            sync,
            ..UserConfig::default()
        }
    }

    fn mmcp_server(name: &str, default: bool) -> Remote {
        Remote::MmcpServer {
            name: name.to_string(),
            url: format!("https://{name}.example.com"),
            default,
            include_in_push_all: true,
        }
    }

    fn direct_git(name: &str, group: Option<&str>, default: bool) -> Remote {
        Remote::DirectGit {
            name: name.to_string(),
            url: format!("ssh://git@example.com/{name}.git"),
            auth: RemoteAuth::None,
            group: group.map(str::to_string),
            default,
            include_in_push_all: true,
        }
    }

    #[test]
    fn both_empty_resolves_to_an_empty_set_with_no_default() {
        let user = user_with(SyncConfig::default());
        let project = project_with(SyncConfig::default(), false);
        let effective = resolve_effective_remotes(&user, &project).expect("resolve");
        assert!(effective.remotes.is_empty());
        assert_eq!(effective.default_index, None);
        assert!(effective.default_remote().is_none());
    }

    /// Positive counterpart of
    /// `project_remote_only_still_rejects_a_project_remote_colliding_with_a_user_remote_name`
    /// below: `project_remote_only` still excludes the user remotes
    /// from the ACTIVE set when the names genuinely do not collide,
    /// proving the security fix only widens the collision CHECK, not
    /// the active set it returns.
    #[test]
    fn project_remote_only_excludes_user_remotes() {
        let user = user_with(SyncConfig {
            remotes: vec![mmcp_server("primary", true)],
            ..SyncConfig::default()
        });
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("proj", true)],
                ..SyncConfig::default()
            },
            true,
        );
        let effective = resolve_effective_remotes(&user, &project).expect("resolve");
        assert_eq!(effective.remotes.len(), 1);
        assert_eq!(effective.remotes[0].name(), "proj");
    }

    /// FR-301 security finding F1 regression: `project_remote_only`
    /// must narrow which remotes are ACTIVE, never widen which names
    /// are safe to reuse. Before the fix, `project_remote_only`
    /// skipped collecting the user's remotes entirely, so a project
    /// could declare its own `primary` remote pointed at an
    /// attacker-controlled URL and it would resolve successfully,
    /// inheriting the operator's real `MMCP_SYNC_TOKEN_PRIMARY`
    /// credential (derived purely from the name) for that attacker
    /// URL. The name collision must still be a loud error even though
    /// the user's `primary` is excluded from the returned active set.
    #[test]
    fn project_remote_only_still_rejects_a_project_remote_colliding_with_a_user_remote_name() {
        let user = user_with(SyncConfig {
            remotes: vec![mmcp_server("primary", true)],
            ..SyncConfig::default()
        });
        let project = project_with(
            SyncConfig {
                remotes: vec![Remote::MmcpServer {
                    name: "primary".to_string(),
                    url: "https://attacker.example".to_string(),
                    default: true,
                    include_in_push_all: true,
                }],
                ..SyncConfig::default()
            },
            true,
        );
        let err = resolve_effective_remotes(&user, &project)
            .expect_err("project_remote_only must not hide a cross-level name collision");
        assert!(matches!(err, StoreError::RemoteNameCollision { name } if name == "primary"));
    }

    #[test]
    fn user_remotes_precede_project_remotes_in_concatenation_order() {
        let user = user_with(SyncConfig {
            remotes: vec![mmcp_server("u1", false)],
            ..SyncConfig::default()
        });
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("p1", true)],
                ..SyncConfig::default()
            },
            false,
        );
        let effective = resolve_effective_remotes(&user, &project).expect("resolve");
        let names: Vec<&str> = effective.remotes.iter().map(ResolvedRemote::name).collect();
        assert_eq!(names, vec!["u1", "p1"]);
    }

    #[test]
    fn cross_level_name_collision_is_rejected() {
        let user = user_with(SyncConfig {
            remotes: vec![mmcp_server("shared", false)],
            ..SyncConfig::default()
        });
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("shared", true)],
                ..SyncConfig::default()
            },
            false,
        );
        let err = resolve_effective_remotes(&user, &project).expect_err("collision must error");
        assert!(matches!(err, StoreError::RemoteNameCollision { name } if name == "shared"));
    }

    #[test]
    fn project_default_wins_over_user_default() {
        let user = user_with(SyncConfig {
            remotes: vec![mmcp_server("u1", true)],
            ..SyncConfig::default()
        });
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("p1", true)],
                ..SyncConfig::default()
            },
            false,
        );
        let effective = resolve_effective_remotes(&user, &project).expect("resolve");
        assert_eq!(
            effective.default_remote().map(ResolvedRemote::name),
            Some("p1")
        );
    }

    #[test]
    fn user_default_wins_when_project_has_no_default() {
        let user = user_with(SyncConfig {
            remotes: vec![mmcp_server("u1", true)],
            ..SyncConfig::default()
        });
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("p1", false)],
                ..SyncConfig::default()
            },
            false,
        );
        let effective = resolve_effective_remotes(&user, &project).expect("resolve");
        assert_eq!(
            effective.default_remote().map(ResolvedRemote::name),
            Some("u1")
        );
    }

    #[test]
    fn a_single_remote_is_the_implicit_default() {
        let user = user_with(SyncConfig::default());
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("only", false)],
                ..SyncConfig::default()
            },
            false,
        );
        let effective = resolve_effective_remotes(&user, &project).expect("resolve");
        assert_eq!(
            effective.default_remote().map(ResolvedRemote::name),
            Some("only")
        );
    }

    #[test]
    fn two_remotes_with_no_default_anywhere_is_ambiguous() {
        let user = user_with(SyncConfig {
            remotes: vec![mmcp_server("u1", false)],
            ..SyncConfig::default()
        });
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("p1", false)],
                ..SyncConfig::default()
            },
            false,
        );
        let err = resolve_effective_remotes(&user, &project).expect_err("ambiguous must error");
        match err {
            StoreError::AmbiguousDefaultRemote { candidates } => {
                assert_eq!(candidates, vec!["u1".to_string(), "p1".to_string()]);
            }
            other => panic!("expected AmbiguousDefaultRemote, got {other:?}"),
        }
    }

    #[test]
    fn project_level_direct_git_with_no_group_defaults_to_the_project_uuid() {
        let user = user_with(SyncConfig::default());
        let project = project_with(
            SyncConfig {
                remotes: vec![direct_git("mirror", None, false)],
                ..SyncConfig::default()
            },
            false,
        );
        let effective = resolve_effective_remotes(&user, &project).expect("resolve");
        assert_eq!(
            effective.remotes[0].direct_git_group.as_deref(),
            Some(PROJECT_UUID)
        );
    }

    #[test]
    fn user_level_direct_git_with_no_group_is_rejected() {
        let user = user_with(SyncConfig {
            remotes: vec![direct_git("mirror", None, false)],
            ..SyncConfig::default()
        });
        let project = project_with(SyncConfig::default(), false);
        let err = resolve_effective_remotes(&user, &project).expect_err("missing group must error");
        assert!(matches!(
            err,
            StoreError::DirectGitMissingGroup { name } if name == "mirror"
        ));
    }

    #[test]
    fn user_level_direct_git_with_explicit_group_resolves_verbatim() {
        let user = user_with(SyncConfig {
            remotes: vec![direct_git("mirror", Some("team-rust"), false)],
            ..SyncConfig::default()
        });
        let project = project_with(SyncConfig::default(), false);
        let effective = resolve_effective_remotes(&user, &project).expect("resolve");
        assert_eq!(
            effective.remotes[0].direct_git_group.as_deref(),
            Some("team-rust")
        );
    }

    #[test]
    fn legacy_server_url_shorthand_synthesizes_a_level_distinct_name() {
        // Both levels declaring only the legacy shorthand is exactly
        // the two-remotes-none-marked-default shape, so this expects
        // `AmbiguousDefaultRemote` (covered on its own below); what
        // this test pins is that the two synthesized entries carry
        // DISTINCT names rather than colliding, which the error's
        // own `candidates` list already proves without needing a
        // successful resolution.
        let user = user_with(SyncConfig {
            server_url: Some("https://user.example.com".to_string()),
            ..SyncConfig::default()
        });
        let project = project_with(
            SyncConfig {
                server_url: Some("https://project.example.com".to_string()),
                ..SyncConfig::default()
            },
            false,
        );
        let err = resolve_effective_remotes(&user, &project).expect_err("two undefaulted remotes");
        match err {
            StoreError::AmbiguousDefaultRemote { candidates } => {
                assert_eq!(
                    candidates,
                    vec![
                        USER_LEGACY_REMOTE_NAME.to_string(),
                        PROJECT_LEGACY_REMOTE_NAME.to_string()
                    ]
                );
            }
            other => panic!("expected AmbiguousDefaultRemote, got {other:?}"),
        }
    }

    #[test]
    fn a_sole_legacy_server_url_shorthand_is_the_implicit_default() {
        let user = user_with(SyncConfig {
            server_url: Some("https://user.example.com".to_string()),
            ..SyncConfig::default()
        });
        let project = project_with(SyncConfig::default(), false);
        let effective = resolve_effective_remotes(&user, &project).expect("resolve");
        assert_eq!(effective.remotes.len(), 1);
        assert_eq!(effective.remotes[0].name(), USER_LEGACY_REMOTE_NAME);
        assert!(effective.remotes[0].is_legacy_shorthand());
        assert_eq!(
            effective.default_remote().map(ResolvedRemote::name),
            Some(USER_LEGACY_REMOTE_NAME)
        );
    }

    #[test]
    fn invalid_remote_name_is_rejected() {
        let user = user_with(SyncConfig::default());
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("has space", false)],
                ..SyncConfig::default()
            },
            false,
        );
        let err = resolve_effective_remotes(&user, &project).expect_err("bad name must error");
        assert!(matches!(
            err,
            StoreError::InvalidRemoteName { name } if name == "has space"
        ));
    }

    #[test]
    fn a_declared_remote_may_not_spoof_a_reserved_legacy_synthetic_name() {
        // No `server_url` shorthand set at either level, so there is
        // nothing synthesized for a same-named declared remote to
        // collide with in `check_name_collisions`; the reserved name
        // must still be rejected in `validate_remote_name` itself, or
        // `ResolvedRemote::is_legacy_shorthand` would misclassify this
        // declared remote and route it to the wrong credential env
        // vars.
        let user = user_with(SyncConfig::default());
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server(USER_LEGACY_REMOTE_NAME, false)],
                ..SyncConfig::default()
            },
            false,
        );
        let err =
            resolve_effective_remotes(&user, &project).expect_err("reserved name spoof must error");
        assert!(matches!(
            err,
            StoreError::InvalidRemoteName { name } if name == USER_LEGACY_REMOTE_NAME
        ));
    }

    /// Cross-level twin of `project.rs`'s
    /// `two_remotes_normalizing_to_the_same_env_var_are_rejected_in_one_file`:
    /// a user-level `prod-eu` and a project-level `prod_eu` both
    /// derive the identical `MMCP_SYNC_TOKEN_PROD_EU` credential env
    /// var. `SyncConfig::validate` only ever sees one file at a time,
    /// so this collision is only catchable here, where both configs
    /// are loaded together.
    #[test]
    fn cross_level_normalized_name_collision_is_rejected() {
        let user = user_with(SyncConfig {
            remotes: vec![mmcp_server("prod-eu", false)],
            ..SyncConfig::default()
        });
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("prod_eu", true)],
                ..SyncConfig::default()
            },
            false,
        );
        let err = resolve_effective_remotes(&user, &project)
            .expect_err("normalized cross-level collision must error");
        assert!(matches!(err, StoreError::RemoteNameCollision { name } if name == "prod_eu"));
    }

    /// Case/separator variants of the reserved legacy names must be
    /// rejected exactly like the exact-match spoof above:
    /// `USER-LEGACY` normalizes to the same key as `user-legacy`.
    #[test]
    fn a_case_variant_of_a_reserved_legacy_name_is_rejected() {
        let user = user_with(SyncConfig::default());
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("USER-LEGACY", false)],
                ..SyncConfig::default()
            },
            false,
        );
        let err = resolve_effective_remotes(&user, &project)
            .expect_err("case-variant reserved name spoof must error");
        assert!(matches!(
            err,
            StoreError::InvalidRemoteName { name } if name == "USER-LEGACY"
        ));
    }

    /// Separator variant: `user_legacy` (underscore) also normalizes
    /// to the same key as `user-legacy`.
    #[test]
    fn a_separator_variant_of_a_reserved_legacy_name_is_rejected() {
        let user = user_with(SyncConfig::default());
        let project = project_with(
            SyncConfig {
                remotes: vec![mmcp_server("user_legacy", false)],
                ..SyncConfig::default()
            },
            false,
        );
        let err = resolve_effective_remotes(&user, &project)
            .expect_err("separator-variant reserved name spoof must error");
        assert!(matches!(
            err,
            StoreError::InvalidRemoteName { name } if name == "user_legacy"
        ));
    }

    fn resolved(remote: Remote, level: RemoteLevel) -> ResolvedRemote {
        ResolvedRemote {
            remote,
            level,
            direct_git_group: None,
        }
    }

    /// Single owning definition, shared by `mmcp-client`'s CLI log
    /// lines / MCP tool responses and the GUI's status bar. A single
    /// configured remote is named directly, no count prefix.
    #[test]
    fn summary_label_names_the_sole_remote_when_only_one_is_configured() {
        let effective = EffectiveRemotes {
            remotes: vec![resolved(mmcp_server("primary", false), RemoteLevel::User)],
            default_index: Some(0),
        };
        assert_eq!(effective.summary_label(), "primary");
    }

    /// A multi-remote effective set formats as a count plus the
    /// resolved default's name, not a collapsed single string.
    #[test]
    fn summary_label_summarises_a_multi_remote_set_with_its_default() {
        let effective = EffectiveRemotes {
            remotes: vec![
                resolved(mmcp_server("u1", false), RemoteLevel::User),
                resolved(mmcp_server("p1", true), RemoteLevel::Project),
            ],
            default_index: Some(1),
        };
        assert_eq!(effective.summary_label(), "2 remote(s), default 'p1'");
    }

    #[test]
    fn summary_label_reports_no_remotes_configured_on_an_empty_set() {
        let effective = EffectiveRemotes {
            remotes: vec![],
            default_index: None,
        };
        assert_eq!(effective.summary_label(), "(no remotes configured)");
    }
}
