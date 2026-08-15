//! Typed representation of `.mmcp/config.toml`.

use serde::{Deserialize, Serialize};

use crate::config::ConfigError;
use crate::id::ProjectUuid;

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

    /// Sync settings. Absent for local-only projects.
    #[serde(default)]
    pub sync: Option<SyncConfig>,

    /// Single subscription engine: which extra groups, languages,
    /// memories, and tags this project pulls into scope.
    #[serde(default)]
    pub subscriptions: SubscriptionsConfig,
}

impl ProjectConfig {
    /// Parse a project configuration from TOML text.
    pub fn from_toml(source: &str) -> Result<Self, ConfigError> {
        toml::from_str(source).map_err(ConfigError::from)
    }

    /// Render this configuration to TOML text.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(ConfigError::from)
    }
}

/// Does `cfg` adopt the given group slug? Checks both the explicit
/// `subscriptions.groups` list and the `lang/<name>` mapping implied
/// by `subscriptions.languages`.
/// Bare string equality for now: namespace-aware resolution is a
/// candidate future refinement once the adoption format stabilises.
#[must_use]
pub fn is_group_adopted(slug: &str, cfg: &ProjectConfig) -> bool {
    cfg.subscriptions.groups.iter().any(|s| s == slug)
        || cfg
            .subscriptions
            .languages
            .iter()
            .any(|lang| slug == format!("lang/{lang}"))
}

/// Remote sync configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncConfig {
    /// URL of the mmcp server this project syncs against.
    pub server_url: String,
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
    use super::*;

    #[test]
    fn minimal_config_requires_only_project_uuid() {
        let source = r#"
            project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"
        "#;
        let cfg = ProjectConfig::from_toml(source).expect("parse minimal config");
        assert!(cfg.project_slug.is_none());
        assert!(cfg.sync.is_none());
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
        assert_eq!(
            cfg.sync.as_ref().expect("sync present").server_url,
            "https://mmcp.example.com"
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
            sync: None,
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
            sync: None,
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
            sync: None,
            subscriptions: SubscriptionsConfig {
                languages: vec!["rust".to_string()],
                ..Default::default()
            },
        };
        assert!(is_group_adopted("lang/rust", &cfg));
        assert!(!is_group_adopted("lang/python", &cfg));
        assert!(!is_group_adopted("rust", &cfg));
    }
}
