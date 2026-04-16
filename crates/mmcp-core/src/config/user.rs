//! User-level configuration loaded from `~/.mmcp/config.toml`.
//!
//! Holds defaults that apply across all projects: sync server,
//! author identity, and default group. Project-level `.mmcp.toml`
//! overrides these when both are set.

use serde::{Deserialize, Serialize};

use super::SyncConfig;

/// User-level configuration. All fields are optional; a missing
/// config file is equivalent to an empty struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    /// Default sync server. Project `.mmcp.toml` `[sync]` overrides.
    pub sync: Option<SyncConfig>,

    /// Author identity for commits.
    pub author: Option<AuthorConfig>,

    /// Default values for CLI flags.
    pub defaults: Option<DefaultsConfig>,
}

/// Author identity configuration.
///
/// The `git_fallback` field is a tri-state:
/// - `Some(true)`: read `git config --global user.name/email` when
///   `name`/`email` are not set here.
/// - `Some(false)`: never read git config. Use mmcp hardcoded
///   fallback. User explicitly opted out.
/// - `None` (default): not decided. Diagnose reports a warning.
///   Resolution falls back to constants without reading git.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorConfig {
    /// Override commit author name.
    pub name: Option<String>,

    /// Override commit author email.
    pub email: Option<String>,

    /// Whether to fall back to `git config user.name/email`.
    /// Must be explicitly set to `true` to enable. Never reads
    /// git config silently.
    pub git_fallback: Option<bool>,
}

/// Default values for CLI commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    /// Default group UUID or slug for `--group` flags.
    pub group: Option<String>,
}

impl UserConfig {
    /// Parse from a TOML string.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Render to a TOML string.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_parses_to_defaults() {
        let cfg: UserConfig = toml::from_str("").unwrap();
        assert!(cfg.sync.is_none());
        assert!(cfg.author.is_none());
        assert!(cfg.defaults.is_none());
    }

    #[test]
    fn full_config_round_trips() {
        let text = r#"
[sync]
server_url = "https://mmcp.example.com"

[author]
name = "Alice"
email = "alice@example.com"
git_fallback = true

[defaults]
group = "my-group"
"#;
        let cfg = UserConfig::from_toml(text).unwrap();
        assert_eq!(cfg.author.as_ref().unwrap().name.as_deref(), Some("Alice"));
        assert_eq!(
            cfg.author.as_ref().unwrap().git_fallback,
            Some(true)
        );
        assert_eq!(
            cfg.sync.as_ref().unwrap().server_url,
            "https://mmcp.example.com"
        );
        assert_eq!(
            cfg.defaults.as_ref().unwrap().group.as_deref(),
            Some("my-group")
        );
    }

    #[test]
    fn git_fallback_none_when_omitted() {
        let text = "[author]\nname = \"Bob\"\n";
        let cfg = UserConfig::from_toml(text).unwrap();
        assert!(cfg.author.as_ref().unwrap().git_fallback.is_none());
    }
}
