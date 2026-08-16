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

    /// Tunable numeric limits an operator may override per-install.
    pub limits: Option<LimitsConfig>,
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

/// Per-install overrides for numeric limits that otherwise fall back
/// to a compiled-in constant. Each field names the constant it
/// overrides in its own doc comment so the override and its fallback
/// stay discoverable from either side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    /// Overrides `mmcp_store::memory::DEFAULT_MAX_AUTO_SLUG_LENGTH`,
    /// the cap applied to a slug auto-derived from a title
    /// (`add_issue` / `add_feature` / milestone creation / file
    /// import default-slug path) when no per-call override and no
    /// `MMCP_MAX_AUTO_SLUG_LENGTH` environment variable are set.
    pub max_auto_slug_length: Option<usize>,

    /// Overrides `mmcp_auth::password::MIN_PASSWORD_LENGTH`, in
    /// bytes, the minimum accepted account password length. Beaten
    /// by a CLI `--min-password-length` override or the
    /// `MMCP_MIN_PASSWORD_LENGTH` environment variable; wins over
    /// the compiled-in default when neither of those is set.
    pub min_password_length: Option<usize>,

    /// Overrides `mmcp_auth::password::MAX_PASSWORD_LENGTH`, in
    /// bytes, the maximum accepted account password length. Same
    /// precedence cascade as [`LimitsConfig::min_password_length`].
    pub max_password_length: Option<usize>,

    /// Overrides `mmcp_auth::backend::MAX_HANDLE_LENGTH`, in bytes, the maximum accepted account handle length.
    /// Same precedence cascade as [`LimitsConfig::min_password_length`].
    pub max_handle_length: Option<usize>,
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn empty_string_parses_to_defaults() {
        let cfg: UserConfig = toml::from_str("").unwrap();
        assert!(cfg.sync.is_none());
        assert!(cfg.author.is_none());
        assert!(cfg.defaults.is_none());
        assert!(cfg.limits.is_none());
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

[limits]
max_auto_slug_length = 80
min_password_length = 10
max_password_length = 128
max_handle_length = 32
"#;
        let cfg = UserConfig::from_toml(text).unwrap();
        assert_eq!(cfg.author.as_ref().unwrap().name.as_deref(), Some("Alice"));
        assert_eq!(cfg.author.as_ref().unwrap().git_fallback, Some(true));
        assert_eq!(
            cfg.sync.as_ref().unwrap().server_url,
            "https://mmcp.example.com"
        );
        assert_eq!(
            cfg.defaults.as_ref().unwrap().group.as_deref(),
            Some("my-group")
        );
        assert_eq!(cfg.limits.as_ref().unwrap().max_auto_slug_length, Some(80));
        assert_eq!(cfg.limits.as_ref().unwrap().min_password_length, Some(10));
        assert_eq!(cfg.limits.as_ref().unwrap().max_password_length, Some(128));
        assert_eq!(cfg.limits.as_ref().unwrap().max_handle_length, Some(32));
    }

    #[test]
    fn limits_password_length_fields_are_none_when_omitted() {
        let text = "[limits]\nmax_auto_slug_length = 80\n";
        let cfg = UserConfig::from_toml(text).unwrap();
        assert!(cfg.limits.as_ref().unwrap().min_password_length.is_none());
        assert!(cfg.limits.as_ref().unwrap().max_password_length.is_none());
        assert!(cfg.limits.as_ref().unwrap().max_handle_length.is_none());
    }

    #[test]
    fn limits_section_omitted_when_absent() {
        let text = "[author]\nname = \"Bob\"\n";
        let cfg = UserConfig::from_toml(text).unwrap();
        assert!(cfg.limits.is_none());
    }

    #[test]
    fn git_fallback_none_when_omitted() {
        let text = "[author]\nname = \"Bob\"\n";
        let cfg = UserConfig::from_toml(text).unwrap();
        assert!(cfg.author.as_ref().unwrap().git_fallback.is_none());
    }
}
