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
    /// Stable identity for this project. Generated at `mmcp init`.
    pub project_uuid: ProjectUuid,

    /// Sync settings. Absent for local-only projects.
    #[serde(default)]
    pub sync: Option<SyncConfig>,

    /// Extra group loading configuration beyond the built-in defaults.
    #[serde(default)]
    pub groups: GroupsConfig,

    /// Language convention groups to auto-load.
    #[serde(default)]
    pub languages: LanguagesConfig,
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

/// Remote sync configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncConfig {
    /// URL of the mmcp server this project syncs against.
    pub server_url: String,
}

/// Group loading configuration.
///
/// Default values load the `global` group and the project's own
/// group, with no additional groups and no extra exclusions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupsConfig {
    /// Skip auto-loading the `global` group. The project's own group
    /// is always loaded regardless.
    #[serde(default)]
    pub no_default: bool,

    /// Extra groups to pull beyond the defaults.
    #[serde(default)]
    pub additional: Vec<String>,
}

/// Language convention configuration.
///
/// Resolves against the `lang/` namespace. `use = ["rust"]` pulls the
/// `lang/rust` group. `auto_detect = true` scans the project for
/// marker files (`Cargo.toml`, `pyproject.toml`, ...) and adds any
/// matches to the load set.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguagesConfig {
    /// Language convention groups to explicitly load.
    #[serde(default, rename = "use")]
    pub use_: Vec<String>,

    /// If true, scan the project for known language marker files and
    /// add the detected languages to the load set.
    #[serde(default)]
    pub auto_detect: bool,
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
        assert!(cfg.sync.is_none());
        assert!(!cfg.groups.no_default);
        assert!(cfg.groups.additional.is_empty());
        assert!(!cfg.languages.auto_detect);
        assert!(cfg.languages.use_.is_empty());
    }

    #[test]
    fn full_config_round_trips() {
        let source = r#"
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[sync]
server_url = "https://mmcp.example.com"

[groups]
no_default = false
additional = ["team-acme/shared"]

[languages]
use = ["rust"]
auto_detect = true
"#;
        let cfg = ProjectConfig::from_toml(source).expect("parse full config");
        assert_eq!(
            cfg.sync.as_ref().expect("sync present").server_url,
            "https://mmcp.example.com"
        );
        assert_eq!(cfg.groups.additional, vec!["team-acme/shared".to_string()]);
        assert_eq!(cfg.languages.use_, vec!["rust".to_string()]);
        assert!(cfg.languages.auto_detect);

        let rendered = cfg.to_toml().expect("render full config");
        let reparsed = ProjectConfig::from_toml(&rendered).expect("reparse rendered config");
        assert_eq!(cfg, reparsed);
    }

    #[test]
    fn unknown_top_level_field_is_rejected() {
        let source = r#"
            project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"
            rogue_field = true
        "#;
        assert!(ProjectConfig::from_toml(source).is_err());
    }
}
