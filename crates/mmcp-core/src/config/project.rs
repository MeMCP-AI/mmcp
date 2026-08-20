//! Typed representation of `.mmcp/config.toml`.

use serde::{Deserialize, Serialize};

use crate::config::{ConfigError, SyncConfig};
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
/// source of truth for "what does this project subscribe to." Kept
/// alongside [`ProjectConfig`] rather than in its own file: it is
/// this project's own subscription surface, has no independent
/// existence outside a [`ProjectConfig`], and every other consumer
/// across the workspace reaches it only through
/// `ProjectConfig.subscriptions`.
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
    use crate::config::{Remote, RemoteAuth};

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
                        group: None,
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
    fn direct_git_group_field_round_trips_when_set() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[[sync.remotes]]
kind = "direct-git"
name = "mirror"
url = "ssh://git@example.com/mirror.git"
group = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c92"
"#;
        let cfg = ProjectConfig::from_toml(source).expect("parse direct-git with group");
        assert_eq!(
            cfg.sync.remotes[0].group(),
            Some("018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c92")
        );

        let rendered = cfg.to_toml().expect("render direct-git with group");
        let reparsed = ProjectConfig::from_toml(&rendered).expect("reparse rendered config");
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn direct_git_group_field_is_absent_from_toml_when_unset() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[[sync.remotes]]
kind = "direct-git"
name = "mirror"
url = "ssh://git@example.com/mirror.git"
"#;
        let cfg = ProjectConfig::from_toml(source).expect("parse direct-git without group");
        assert_eq!(cfg.sync.remotes[0].group(), None);
        let rendered = cfg.to_toml().expect("render");
        // `subscriptions.groups = []` legitimately contains the
        // substring "group"; check for the `Remote::DirectGit`
        // field's own key line specifically.
        assert!(
            !rendered.contains("\ngroup = "),
            "unset group must not serialize: {rendered}"
        );
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

    /// Exploit chain this closes: `prod-eu` and `prod_eu` (or
    /// `PROD-EU`) both derive the identical `MMCP_SYNC_TOKEN_PROD_EU`
    /// env var via `token_env_name`. Before comparing the normalized
    /// form, `validate` only rejected a RAW-string duplicate, so two
    /// remotes spelled this way in the SAME file would silently share
    /// one credential slot.
    #[test]
    fn two_remotes_normalizing_to_the_same_env_var_are_rejected_in_one_file() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[[sync.remotes]]
kind = "mmcp-server"
name = "prod-eu"
url = "https://a.example.com"

[[sync.remotes]]
kind = "mmcp-server"
name = "prod_eu"
url = "https://attacker.example.com"
"#;
        let err = ProjectConfig::from_toml(source)
            .expect_err("normalized-name collision must be rejected");
        assert!(matches!(
            err,
            ConfigError::DuplicateRemoteName { name } if name == "prod_eu"
        ));
    }

    /// Same closure as the hyphen/underscore case above, for the
    /// case-only variant: `PROD-EU` and `prod-eu` also normalize to
    /// the same key.
    #[test]
    fn case_variant_remote_names_normalizing_to_the_same_env_var_are_rejected() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[[sync.remotes]]
kind = "mmcp-server"
name = "prod-eu"
url = "https://a.example.com"

[[sync.remotes]]
kind = "mmcp-server"
name = "PROD-EU"
url = "https://attacker.example.com"
"#;
        let err = ProjectConfig::from_toml(source)
            .expect_err("case-variant normalized collision must be rejected");
        assert!(matches!(
            err,
            ConfigError::DuplicateRemoteName { name } if name == "PROD-EU"
        ));
    }
}
