//! Typed representation of `.mmcp/config.toml`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::config::ConfigError;
use crate::id::ProjectUuid;
use crate::loadset::GroupRef;

/// Parsed contents of `.mmcp/config.toml`.
///
/// A minimal file contains only `project_uuid`; every other field
/// defaults to an empty or disabled value so local-only projects do
/// not need to configure anything beyond their identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Stable identity for this project. Generated at
    /// `mmcp init project` (locally if offline, or accepted from
    /// the server on first push) and never rewritten afterwards.
    pub project_uuid: ProjectUuid,

    /// Human-readable group slug mirrored into the project's bare
    /// repo manifest. Stored here so a checked-in `.mmcp.toml`
    /// carries the canonical project name forward: teammates
    /// cloning the working tree see the slug without reading back
    /// to `~/.mmcp/repos`. Absent on configs written by pre-slug
    /// versions of `mmcp init`; `mmcp init project` backfills it
    /// in place on the next run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_slug: Option<String>,

    /// Sync settings. `SyncConfig::default()` (no legacy `server_url`,
    /// no `remotes`) for local-only projects: an always-defaulted
    /// struct rather than `Option<SyncConfig>`, so this project's own
    /// [`ProjectConfig::project_remote_only`] toggle stays expressible
    /// even when it declares no `[sync]` table at all.
    #[serde(default, skip_serializing_if = "SyncConfig::is_empty")]
    pub sync: SyncConfig,

    /// When true, this project uses ONLY the remotes declared in its
    /// own `sync.remotes` (plus its own `sync.server_url`, if set);
    /// the user-level `~/.mmcp/config.toml` remotes are not
    /// inherited. Lives at the top level, outside [`SyncConfig`], so
    /// it stays expressible on a project that declares no `[sync]`
    /// table of its own. Default false: a project inherits the
    /// user's remotes unless it opts out.
    #[serde(default)]
    pub project_remote_only: bool,

    /// Single subscription engine: which extra groups, languages,
    /// memories, and tags this project pulls into scope.
    #[serde(default)]
    pub subscriptions: SubscriptionsConfig,
}

impl ProjectConfig {
    /// Parse a project configuration from TOML text.
    ///
    /// # Errors
    /// Returns [`ConfigError::Parse`] on malformed TOML, or
    /// [`ConfigError::DuplicateRemoteName`] /
    /// [`ConfigError::MultipleDefaultRemotes`] when `sync.remotes`
    /// fails the single-file, single-level checks in
    /// [`SyncConfig::validate`].
    pub fn from_toml(source: &str) -> Result<Self, ConfigError> {
        let cfg: Self = toml::from_str(source).map_err(ConfigError::from)?;
        cfg.sync.validate()?;
        Ok(cfg)
    }

    /// Render this configuration to TOML text.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(ConfigError::from)
    }
}

/// Does `cfg` adopt the given group slug? Checks both the explicit
/// `subscriptions.groups` list and the `lang/<name>` mapping implied
/// by `subscriptions.languages`.
#[must_use]
pub fn is_group_adopted(slug: &str, cfg: &ProjectConfig) -> bool {
    cfg.subscriptions.groups.iter().any(|s| s == slug)
        || cfg
            .subscriptions
            .languages
            .iter()
            .any(|lang| slug == GroupRef::Language(lang.clone()).canonical_name())
}

/// Environment variable carrying the control-plane bearer token for
/// `/sync/*` requests: a per-user PASETO session token issued by the
/// server's login flow and verified against `mmcp_auth::TokenVerifier`.
pub const SYNC_TOKEN_ENV: &str = "MMCP_SYNC_TOKEN";

/// Environment variable carrying the content-plane git push
/// credential, compared by the server's git smart-HTTP endpoint by
/// exact string equality against its own `MMCP_PUSH_TOKEN`. The two
/// env var names differ because they read in different processes
/// (this one client side, `MMCP_PUSH_TOKEN` server side); whoever
/// configures both sides sets them to the same value.
pub const SYNC_PUSH_TOKEN_ENV: &str = "MMCP_SYNC_PUSH_TOKEN";

/// Remote sync configuration, reused verbatim on both
/// [`ProjectConfig`] and [`crate::config::UserConfig`] so the sync
/// surface is uniform between project and user level.
///
/// Carries no credential field: every token is env-only
/// ([`SyncConfig::resolve_token`], [`SyncConfig::resolve_push_token`],
/// [`SyncConfig::token_env_name`], [`SyncConfig::push_token_env_name`]),
/// so a `.mmcp.toml` rewrite can never persist a secret to a file
/// that convention tracks in git.
///
/// Always defaulted (not `Option<SyncConfig>`): a config with no
/// `[sync]` table at all still parses to `SyncConfig::default()`
/// (`server_url: None`, `remotes` empty), so a toggle that lives on
/// the enclosing config (see [`ProjectConfig::project_remote_only`])
/// stays reachable even when this struct itself is unset.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncConfig {
    /// Legacy shorthand for a single `mmcp-server` remote. Purely
    /// additive: composed with `remotes` at resolution time (a later
    /// wave), never validated as mutually exclusive with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,

    /// Any number of named remotes. Kept empty (and absent from
    /// rendered TOML) on configs that only use the legacy
    /// `server_url` shorthand.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remotes: Vec<Remote>,
}

impl SyncConfig {
    /// True when this config declares neither the legacy `server_url`
    /// shorthand nor any `remotes` entry. Replaces the old
    /// `Option<SyncConfig>::is_none()` check now that this struct is
    /// always present and always defaulted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.server_url.is_none() && self.remotes.is_empty()
    }

    /// Env var name carrying the control-plane bearer token for the
    /// remote named `remote_name`, `MMCP_SYNC_TOKEN_<NAME>`.
    ///
    /// Derivation rule, exact and stable (a later wave, mmcp-sync,
    /// reads env vars produced by this exact rule): ASCII-uppercase
    /// `remote_name`, then replace every `-` with `_` so a hyphenated
    /// remote name still yields a portable env var identifier.
    /// Deliberately NOT a free-text config field: see
    /// [`RemoteAuth::Bearer`].
    #[must_use]
    pub fn token_env_name(remote_name: &str) -> String {
        format!(
            "MMCP_SYNC_TOKEN_{}",
            Self::normalize_remote_name_for_env(remote_name)
        )
    }

    /// Env var name carrying the content-plane git push credential
    /// for the remote named `remote_name`,
    /// `MMCP_SYNC_PUSH_TOKEN_<NAME>`. Same derivation rule as
    /// [`SyncConfig::token_env_name`].
    #[must_use]
    pub fn push_token_env_name(remote_name: &str) -> String {
        format!(
            "MMCP_SYNC_PUSH_TOKEN_{}",
            Self::normalize_remote_name_for_env(remote_name)
        )
    }

    /// Shared name-to-env-suffix rule backing
    /// [`SyncConfig::token_env_name`] and
    /// [`SyncConfig::push_token_env_name`].
    fn normalize_remote_name_for_env(remote_name: &str) -> String {
        remote_name.to_ascii_uppercase().replace('-', "_")
    }

    /// Single-file, single-level validation of this `remotes` list:
    /// rejects two entries sharing a `name`, and rejects more than
    /// one entry marked `default = true`.
    ///
    /// Cross-level validation, a project remote colliding with a user
    /// remote by name, or picking a winner between two
    /// `default = true` remotes declared at different levels, needs
    /// both config files loaded together and is deliberately left to
    /// the resolver, a later wave.
    ///
    /// # Errors
    /// [`ConfigError::DuplicateRemoteName`] or
    /// [`ConfigError::MultipleDefaultRemotes`].
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        let mut seen_names = HashSet::new();
        for remote in &self.remotes {
            if !seen_names.insert(remote.name()) {
                return Err(ConfigError::DuplicateRemoteName {
                    name: remote.name().to_string(),
                });
            }
        }

        let default_names: Vec<String> = self
            .remotes
            .iter()
            .filter(|remote| remote.is_default())
            .map(|remote| remote.name().to_string())
            .collect();
        if default_names.len() > 1 {
            return Err(ConfigError::MultipleDefaultRemotes {
                names: default_names,
            });
        }

        Ok(())
    }

    /// Effective control-plane bearer token for `/sync/*` requests,
    /// reading `MMCP_SYNC_TOKEN` from the real process environment.
    /// Env-only: no file-backed fallback exists, so this credential
    /// never round-trips through `.mmcp.toml`.
    #[must_use]
    pub fn resolve_token(&self) -> Option<String> {
        Self::resolve_token_with(|key| std::env::var(key).ok())
    }

    /// Same resolution as [`SyncConfig::resolve_token`], with the
    /// env lookup injected so tests never touch `std::env`, which
    /// would race under cargo's default parallel test runner. An
    /// empty env value is treated as absent, matching
    /// `SyncClient::git_credentials`'s existing
    /// `Some(token) if !token.is_empty()` pattern in the
    /// `mmcp-sync` crate.
    fn resolve_token_with(get: impl Fn(&str) -> Option<String>) -> Option<String> {
        get(SYNC_TOKEN_ENV).filter(|token| !token.is_empty())
    }

    /// Effective content-plane git push credential, reading
    /// `MMCP_SYNC_PUSH_TOKEN` from the real process environment.
    /// Structurally a twin of [`SyncConfig::resolve_token`]: env-only,
    /// same empty-value handling, distinct env var and distinct
    /// destination (`SyncClient::with_push_credential`, never
    /// `SyncClient::with_bearer`).
    #[must_use]
    pub fn resolve_push_token(&self) -> Option<String> {
        Self::resolve_push_token_with(|key| std::env::var(key).ok())
    }

    /// Same resolution as [`SyncConfig::resolve_push_token`], with
    /// the env lookup injected for the same race-avoidance reason as
    /// [`SyncConfig::resolve_token_with`].
    fn resolve_push_token_with(get: impl Fn(&str) -> Option<String>) -> Option<String> {
        get(SYNC_PUSH_TOKEN_ENV).filter(|token| !token.is_empty())
    }
}

/// One configured sync remote.
///
/// Internally tagged (`kind = "..."`) rather than untagged: an
/// untagged enum collapses every variant's parse failure into one
/// generic "data did not match any variant" error, which loses the
/// per-field precision `deny_unknown_fields` otherwise gives each
/// variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Remote {
    /// A remote mmcp server, reached over the control-plane and
    /// content-plane HTTP APIs the client already speaks.
    MmcpServer {
        /// Unique name for this remote within its effective set.
        name: String,
        /// Base URL of the mmcp server.
        url: String,
        /// Whether this remote is the default push target. At most
        /// one remote in a given `remotes` list may set this; see
        /// [`SyncConfig::validate`].
        #[serde(default)]
        default: bool,
        /// Whether `mmcp push --all-remotes` includes this remote.
        #[serde(default = "default_include_in_push_all")]
        include_in_push_all: bool,
    },
    /// A bare git remote reached directly, with no mmcp control
    /// plane: no manifest, no ACL, no version-bump registration
    /// through it.
    DirectGit {
        /// Unique name for this remote within its effective set.
        name: String,
        /// Git remote URL (any scheme the local git supports).
        url: String,
        /// How to authenticate against this remote.
        #[serde(default)]
        auth: RemoteAuth,
        /// Whether this remote is the default push target. At most
        /// one remote in a given `remotes` list may set this; see
        /// [`SyncConfig::validate`].
        #[serde(default)]
        default: bool,
        /// Whether `mmcp push --all-remotes` includes this remote.
        #[serde(default = "default_include_in_push_all")]
        include_in_push_all: bool,
    },
}

/// Default for [`Remote::MmcpServer::include_in_push_all`] and
/// [`Remote::DirectGit::include_in_push_all`]: a remote participates
/// in a bulk push unless it explicitly opts out.
fn default_include_in_push_all() -> bool {
    true
}

impl Remote {
    /// This remote's `name` field, whichever variant it is.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::MmcpServer { name, .. } | Self::DirectGit { name, .. } => name,
        }
    }

    /// This remote's `default` field, whichever variant it is.
    #[must_use]
    pub fn is_default(&self) -> bool {
        match self {
            Self::MmcpServer { default, .. } | Self::DirectGit { default, .. } => *default,
        }
    }

    /// This remote's `include_in_push_all` field, whichever variant
    /// it is.
    #[must_use]
    pub fn include_in_push_all(&self) -> bool {
        match self {
            Self::MmcpServer {
                include_in_push_all,
                ..
            }
            | Self::DirectGit {
                include_in_push_all,
                ..
            } => *include_in_push_all,
        }
    }

    /// This remote's kind tag, matching the `kind` value its TOML
    /// form serializes under.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::MmcpServer { .. } => "mmcp-server",
            Self::DirectGit { .. } => "direct-git",
        }
    }
}

/// Auth selector for a [`Remote::DirectGit`] remote.
///
/// Deliberately not a free-text env-var-name field: that shape would
/// let a git-tracked config redirect an arbitrary already-set env var
/// to an arbitrary URL, exfiltrating whatever that variable holds.
/// The actual env var name is always derived from the remote's own
/// `name` ([`SyncConfig::token_env_name`]), never user-supplied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteAuth {
    /// No credential; rely on ambient git/SSH environment (agent,
    /// known_hosts, netrc, whatever the user's own git is already
    /// configured with). Correct default for a plain `ssh://` remote.
    #[default]
    None,
    /// Same runtime effect as `None` today (maps to
    /// `mmcp_git::Credentials::None`); kept as a distinct explicit
    /// variant so a config can document intent (this remote
    /// deliberately relies on the user's SSH agent) even though
    /// there is nothing extra to configure.
    SshAgent,
    /// Bearer credential read from the derived
    /// `MMCP_SYNC_TOKEN_<NAME>` env var
    /// ([`SyncConfig::token_env_name`]), mapped to
    /// `mmcp_git::Credentials::BearerHttp`.
    Bearer,
}

/// Unified opt-in surface for which groups and memories the project
/// pulls into scope.
///
/// `languages` and `groups` answer "fully subscribe to this
/// group": every memory in the named group surfaces (mandatory or
/// not). `memories` and `tags` are finer-grained: pull a specific
/// memory by `<group_uuid>:<slug>`, or pull every non-mandatory
/// memory whose tags overlap the listed set from any in-scope group.
///
/// `no_default_global` and `auto_detect_languages` are the legacy
/// toggles, kept under the same section so the config has a single
/// source of truth for "what does this project subscribe to."
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionsConfig {
    /// Skip auto-loading the `global` group. The project's own group
    /// is always loaded regardless.
    #[serde(default)]
    pub no_default_global: bool,

    /// If true, scan the project for known language marker files
    /// (`Cargo.toml`, `pyproject.toml`, ...) and add the detected
    /// languages to the load set.
    #[serde(default)]
    pub auto_detect_languages: bool,

    /// Language convention groups to explicitly load. Each entry
    /// resolves to `lang/<name>` and pulls every memory in that
    /// group into scope.
    #[serde(default)]
    pub languages: Vec<String>,

    /// Extra groups to fully subscribe to beyond the defaults.
    /// Entries may be UUIDs or slugs; the resolver matches against
    /// the local mirror.
    #[serde(default)]
    pub groups: Vec<String>,

    /// Individual non-mandatory memory pins, formatted as
    /// `<group_uuid>:<slug>`. The targeted memory surfaces in
    /// `bootstrap_context` regardless of whether its owning group
    /// is otherwise in scope.
    #[serde(default)]
    pub memories: Vec<String>,

    /// Tag filter. Non-mandatory memories from in-scope groups
    /// whose frontmatter tags overlap this set surface in
    /// `bootstrap_context`.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn minimal_config_requires_only_project_uuid() {
        let source = r#"
            project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"
        "#;
        let cfg = ProjectConfig::from_toml(source).expect("parse minimal config");
        assert!(cfg.project_slug.is_none());
        assert!(cfg.sync.is_empty());
        assert!(!cfg.project_remote_only);
        assert!(!cfg.subscriptions.no_default_global);
        assert!(!cfg.subscriptions.auto_detect_languages);
        assert!(cfg.subscriptions.languages.is_empty());
        assert!(cfg.subscriptions.groups.is_empty());
        assert!(cfg.subscriptions.memories.is_empty());
        assert!(cfg.subscriptions.tags.is_empty());
    }

    #[test]
    fn pre_slug_config_parses_without_project_slug_field() {
        // Configs written before `project_slug` landed must keep
        // parsing: the field is backward-compatible via serde-default.
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[sync]
server_url = "http://localhost:8787"
"#;
        let cfg = ProjectConfig::from_toml(source).expect("pre-slug config must parse");
        assert!(cfg.project_slug.is_none());
    }

    #[test]
    fn full_config_round_trips() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"
project_slug = "team-acme"

[sync]
server_url = "https://mmcp.example.com"

[subscriptions]
no_default_global = false
auto_detect_languages = true
languages = ["rust"]
groups = ["team-acme/shared"]
memories = ["019d955d-4cce-77f2-a0b3-0b79ed394612:supersede-convention"]
tags = ["git", "testing"]
"#;
        let cfg = ProjectConfig::from_toml(source).expect("parse full config");
        assert_eq!(cfg.project_slug.as_deref(), Some("team-acme"));
        assert!(!cfg.sync.is_empty());
        assert_eq!(
            cfg.sync.server_url.as_deref(),
            Some("https://mmcp.example.com")
        );
        assert_eq!(
            cfg.subscriptions.groups,
            vec!["team-acme/shared".to_string()]
        );
        assert_eq!(cfg.subscriptions.languages, vec!["rust".to_string()]);
        assert!(cfg.subscriptions.auto_detect_languages);
        assert_eq!(
            cfg.subscriptions.memories,
            vec!["019d955d-4cce-77f2-a0b3-0b79ed394612:supersede-convention".to_string()]
        );
        assert_eq!(
            cfg.subscriptions.tags,
            vec!["git".to_string(), "testing".to_string()]
        );

        let rendered = cfg.to_toml().expect("render full config");
        let reparsed = ProjectConfig::from_toml(&rendered).expect("reparse rendered config");
        assert_eq!(cfg, reparsed);
    }

    /// `SyncConfig` carries no credential field: a `.mmcp.toml`
    /// round trip must never emit a `token` key, regardless of what
    /// environment variables are set on the process running the
    /// test.
    #[test]
    fn sync_config_round_trip_never_serializes_a_token_key() {
        let cfg = ProjectConfig {
            project_uuid: ProjectUuid::from_uuid(
                uuid::Uuid::parse_str("018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91").unwrap(),
            ),
            project_slug: None,
            sync: SyncConfig {
                server_url: Some("https://mmcp.example.com".to_string()),
                remotes: vec![
                    Remote::MmcpServer {
                        name: "primary".to_string(),
                        url: "https://mmcp.example.com".to_string(),
                        default: true,
                        include_in_push_all: true,
                    },
                    Remote::DirectGit {
                        name: "mirror".to_string(),
                        url: "ssh://git@example.com/mirror.git".to_string(),
                        auth: RemoteAuth::Bearer,
                        default: false,
                        include_in_push_all: false,
                    },
                ],
            },
            project_remote_only: false,
            subscriptions: SubscriptionsConfig::default(),
        };
        let rendered = cfg.to_toml().expect("render");
        assert!(
            !rendered.contains("token"),
            "SyncConfig must never serialize a credential field: {rendered}"
        );
    }

    #[test]
    fn resolve_token_with_reads_the_env_value() {
        let resolved = SyncConfig::resolve_token_with(|key| {
            (key == SYNC_TOKEN_ENV).then(|| "env-token".to_string())
        });
        assert_eq!(resolved.as_deref(), Some("env-token"));
    }

    #[test]
    fn resolve_token_with_is_none_when_env_absent() {
        let resolved = SyncConfig::resolve_token_with(|_| None);
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_token_with_treats_an_empty_env_value_as_absent() {
        let resolved =
            SyncConfig::resolve_token_with(|key| (key == SYNC_TOKEN_ENV).then(String::new));
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_push_token_with_reads_the_env_value() {
        let resolved = SyncConfig::resolve_push_token_with(|key| {
            (key == SYNC_PUSH_TOKEN_ENV).then(|| "env-push-token".to_string())
        });
        assert_eq!(resolved.as_deref(), Some("env-push-token"));
    }

    #[test]
    fn resolve_push_token_with_is_none_when_env_absent() {
        let resolved = SyncConfig::resolve_push_token_with(|_| None);
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_push_token_with_treats_an_empty_env_value_as_absent() {
        let resolved = SyncConfig::resolve_push_token_with(|key| {
            (key == SYNC_PUSH_TOKEN_ENV).then(String::new)
        });
        assert!(resolved.is_none());
    }

    /// The two resolvers read distinct env vars: setting one must
    /// never leak into the other's result. Structural regression
    /// test for the credential-plane split.
    #[test]
    fn resolve_token_and_resolve_push_token_read_distinct_env_vars() {
        let get = |key: &str| match key {
            k if k == SYNC_TOKEN_ENV => Some("control-plane-value".to_string()),
            k if k == SYNC_PUSH_TOKEN_ENV => Some("content-plane-value".to_string()),
            _ => None,
        };
        assert_eq!(
            SyncConfig::resolve_token_with(get).as_deref(),
            Some("control-plane-value")
        );
        assert_eq!(
            SyncConfig::resolve_push_token_with(get).as_deref(),
            Some("content-plane-value")
        );
    }

    #[test]
    fn absent_project_slug_is_skipped_on_serialize() {
        // `skip_serializing_if = Option::is_none` keeps rendered
        // `.mmcp.toml` files minimal: no empty-string field noise
        // on configs that haven't been init-project'd yet.
        let cfg = ProjectConfig {
            project_uuid: ProjectUuid::from_uuid(
                uuid::Uuid::parse_str("018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91").unwrap(),
            ),
            project_slug: None,
            sync: SyncConfig::default(),
            project_remote_only: false,
            subscriptions: SubscriptionsConfig::default(),
        };
        let rendered = cfg.to_toml().expect("render");
        assert!(
            !rendered.contains("project_slug"),
            "project_slug must not appear when the value is None"
        );
    }

    #[test]
    fn unknown_top_level_field_is_rejected() {
        let source = r#"
            project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"
            rogue_field = true
        "#;
        assert!(ProjectConfig::from_toml(source).is_err());
    }

    #[test]
    fn legacy_groups_and_languages_sections_are_rejected() {
        // Hard cut: pre-rebase `.mmcp.toml` files using the old
        // `[groups]` / `[languages]` sections must fail loudly so
        // operators rewrite to `[subscriptions]`.
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[groups]
no_default = false
"#;
        assert!(ProjectConfig::from_toml(source).is_err());
    }

    #[test]
    fn is_group_adopted_matches_explicit_group_slug() {
        let cfg = ProjectConfig {
            project_uuid: ProjectUuid::from_uuid(
                uuid::Uuid::parse_str("018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91").unwrap(),
            ),
            project_slug: None,
            sync: SyncConfig::default(),
            project_remote_only: false,
            subscriptions: SubscriptionsConfig {
                groups: vec!["team-acme/shared".to_string()],
                ..Default::default()
            },
        };
        assert!(is_group_adopted("team-acme/shared", &cfg));
        assert!(!is_group_adopted("team-acme/other", &cfg));
    }

    #[test]
    fn is_group_adopted_matches_language_mapped_slug() {
        let cfg = ProjectConfig {
            project_uuid: ProjectUuid::from_uuid(
                uuid::Uuid::parse_str("018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91").unwrap(),
            ),
            project_slug: None,
            sync: SyncConfig::default(),
            project_remote_only: false,
            subscriptions: SubscriptionsConfig {
                languages: vec!["rust".to_string()],
                ..Default::default()
            },
        };
        assert!(is_group_adopted("lang/rust", &cfg));
        assert!(!is_group_adopted("lang/python", &cfg));
        assert!(!is_group_adopted("rust", &cfg));
    }

    #[test]
    fn legacy_server_url_only_round_trips_and_is_not_empty() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[sync]
server_url = "https://mmcp.example.com"
"#;
        let cfg = ProjectConfig::from_toml(source).expect("parse legacy shorthand config");
        assert!(!cfg.sync.is_empty());
        assert_eq!(
            cfg.sync.server_url.as_deref(),
            Some("https://mmcp.example.com")
        );
        assert!(cfg.sync.remotes.is_empty());

        let rendered = cfg.to_toml().expect("render legacy shorthand config");
        let reparsed = ProjectConfig::from_toml(&rendered).expect("reparse rendered config");
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn remotes_with_two_different_kinds_round_trip() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[[sync.remotes]]
kind = "mmcp-server"
name = "primary"
url = "https://mmcp.example.com"
default = true

[[sync.remotes]]
kind = "direct-git"
name = "mirror"
url = "ssh://git@example.com/mirror.git"
auth = "ssh-agent"
include_in_push_all = false
"#;
        let cfg = ProjectConfig::from_toml(source).expect("parse two-kind remotes config");
        assert_eq!(cfg.sync.remotes.len(), 2);
        assert_eq!(cfg.sync.remotes[0].kind(), "mmcp-server");
        assert_eq!(cfg.sync.remotes[0].name(), "primary");
        assert!(cfg.sync.remotes[0].is_default());
        assert!(cfg.sync.remotes[0].include_in_push_all());
        assert_eq!(cfg.sync.remotes[1].kind(), "direct-git");
        assert_eq!(cfg.sync.remotes[1].name(), "mirror");
        assert!(!cfg.sync.remotes[1].is_default());
        assert!(!cfg.sync.remotes[1].include_in_push_all());

        let rendered = cfg.to_toml().expect("render two-kind remotes config");
        let reparsed = ProjectConfig::from_toml(&rendered).expect("reparse rendered config");
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn server_url_and_remotes_together_are_purely_additive() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[sync]
server_url = "https://legacy.example.com"

[[sync.remotes]]
kind = "mmcp-server"
name = "primary"
url = "https://mmcp.example.com"
"#;
        let cfg =
            ProjectConfig::from_toml(source).expect("parse combined shorthand+remotes config");
        assert_eq!(
            cfg.sync.server_url.as_deref(),
            Some("https://legacy.example.com")
        );
        assert_eq!(cfg.sync.remotes.len(), 1);
        assert_eq!(cfg.sync.remotes[0].name(), "primary");

        let rendered = cfg.to_toml().expect("render combined config");
        let reparsed = ProjectConfig::from_toml(&rendered).expect("reparse rendered config");
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn duplicate_remote_names_in_one_list_are_rejected() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[[sync.remotes]]
kind = "mmcp-server"
name = "primary"
url = "https://a.example.com"

[[sync.remotes]]
kind = "direct-git"
name = "primary"
url = "ssh://git@example.com/b.git"
"#;
        let err = ProjectConfig::from_toml(source).expect_err("duplicate name must be rejected");
        assert!(matches!(
            err,
            ConfigError::DuplicateRemoteName { name } if name == "primary"
        ));
    }

    #[test]
    fn two_default_remotes_in_one_list_are_rejected() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[[sync.remotes]]
kind = "mmcp-server"
name = "primary"
url = "https://a.example.com"
default = true

[[sync.remotes]]
kind = "mmcp-server"
name = "secondary"
url = "https://b.example.com"
default = true
"#;
        let err = ProjectConfig::from_toml(source).expect_err("two defaults must be rejected");
        assert!(matches!(err, ConfigError::MultipleDefaultRemotes { .. }));
    }

    #[test]
    fn absent_sync_table_and_toggle_parse_to_defaults() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"
"#;
        let cfg = ProjectConfig::from_toml(source).expect("parse config with no [sync] table");
        assert_eq!(cfg.sync, SyncConfig::default());
        assert!(!cfg.project_remote_only);
    }

    #[test]
    fn token_env_name_uses_the_documented_derivation_rule() {
        assert_eq!(
            SyncConfig::token_env_name("primary"),
            "MMCP_SYNC_TOKEN_PRIMARY"
        );
        assert_eq!(
            SyncConfig::token_env_name("prod-eu"),
            "MMCP_SYNC_TOKEN_PROD_EU"
        );
    }

    #[test]
    fn push_token_env_name_uses_the_documented_derivation_rule() {
        assert_eq!(
            SyncConfig::push_token_env_name("primary"),
            "MMCP_SYNC_PUSH_TOKEN_PRIMARY"
        );
        assert_eq!(
            SyncConfig::push_token_env_name("prod-eu"),
            "MMCP_SYNC_PUSH_TOKEN_PROD_EU"
        );
    }
}
