//! [`LocalConfig`], the typed content of `.mmcp.local.toml`.

use serde::{Deserialize, Serialize};

use super::{ConfigError, NoticeConfig};

/// File name of the local configuration, at the project root.
pub const LOCAL_CONFIG_FILENAME: &str = ".mmcp.local.toml";

/// Global git ignore pattern that keeps every local configuration file out of commits.
pub const LOCAL_CONFIG_EXCLUDE_PATTERN: &str = "**/.mmcp.local.toml";

/// Parsed contents of `.mmcp.local.toml`: the user's personal settings for one project, never committed.
/// It carries no project identity, which stays in `.mmcp.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    /// Notice settings, see [`NoticeConfig`].
    #[serde(default, skip_serializing_if = "NoticeConfig::is_empty")]
    pub notice: NoticeConfig,
}

impl LocalConfig {
    /// Parse a local configuration from TOML text.
    ///
    /// # Errors
    /// [`ConfigError::Parse`] on malformed TOML, an unknown key or an invalid value.
    pub fn from_toml(source: &str) -> Result<Self, ConfigError> {
        toml::from_str(source).map_err(ConfigError::from)
    }

    /// Render this configuration to TOML text.
    ///
    /// # Errors
    /// [`ConfigError::Render`] when the content cannot be serialized.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(ConfigError::from)
    }

    /// Whether the file would carry nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.notice.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::config::{ConfigKey, NoticeValue};

    #[test]
    fn the_exclude_pattern_matches_the_file_name_at_any_depth() {
        assert_eq!(
            LOCAL_CONFIG_EXCLUDE_PATTERN,
            format!("**/{LOCAL_CONFIG_FILENAME}")
        );
    }

    #[test]
    fn an_empty_file_parses_to_the_default_and_renders_nothing() {
        let config = LocalConfig::from_toml("").expect("parse");
        assert!(config.is_empty());
        assert_eq!(config.to_toml().expect("render"), "");
    }

    #[test]
    fn the_notice_table_round_trips() {
        let mut config = LocalConfig::default();
        config
            .notice
            .set(ConfigKey::NoticeMdProject, NoticeValue::Off);

        let reparsed = LocalConfig::from_toml(&config.to_toml().expect("render")).expect("reparse");

        assert_eq!(reparsed, config);
    }

    #[test]
    fn local_config_rejects_an_unknown_root_key() {
        assert!(LocalConfig::from_toml("project_uuid = \"x\"\n").is_err());
        assert!(LocalConfig::from_toml("[notic]\n").is_err());
    }

    #[test]
    fn local_config_rejects_an_unknown_notice_key_and_an_invalid_value() {
        assert!(LocalConfig::from_toml("[notice.md]\nprojet = \"off\"\n").is_err());
        assert!(LocalConfig::from_toml("[notice.md]\nproject = \"maybe\"\n").is_err());
    }
}
