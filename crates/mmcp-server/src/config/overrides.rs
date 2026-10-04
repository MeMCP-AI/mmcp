//! [`ServerConfigOverrides`], the CLI-supplied override tier for [`super::ServerConfig`].

use std::path::PathBuf;

/// CLI-supplied override tier for [`super::ServerConfig::from_source_with_overrides`]
/// / [`super::ServerConfig::from_env_with_overrides`]. A parameter object
/// (per the project's Rust API-design convention) rather than
/// growing `from_source`'s own argument list as more tunables gain a
/// CLI flag.
#[derive(Debug, Clone, Default)]
pub struct ServerConfigOverrides {
    /// `--min-password-length`; highest-precedence tier, beats
    /// `MMCP_MIN_PASSWORD_LENGTH`, the user config file, and the
    /// compiled-in [`mmcp_auth::MIN_PASSWORD_LENGTH`] default.
    pub min_password_length: Option<usize>,
    /// `--max-password-length`; same precedence as
    /// [`ServerConfigOverrides::min_password_length`], over
    /// [`mmcp_auth::MAX_PASSWORD_LENGTH`].
    pub max_password_length: Option<usize>,
    /// `--max-handle-length`; highest-precedence tier, beats
    /// `MMCP_MAX_HANDLE_LENGTH`, the user config file, and the
    /// compiled-in [`mmcp_auth::MAX_HANDLE_LENGTH`] default.
    pub max_handle_length: Option<usize>,
    /// `--webui-dir`, else `MMCP_WEBUI_DIR`, as resolved by the argument parser.
    /// A blank value counts as absent.
    pub webui_dir: Option<PathBuf>,
    /// `--cors-allowed-origins`, else `MMCP_CORS_ALLOWED_ORIGINS`, as resolved by the argument parser.
    /// A blank value allows no origin.
    pub cors_allowed_origins: Option<String>,
    /// `--auth-rate-limit-per-minute`, else `MMCP_AUTH_RATE_LIMIT_PER_MINUTE`, as resolved by the argument parser.
    /// Kept as text, so a blank value counts as absent and a malformed one warns instead of stopping the start.
    pub auth_rate_limit_per_minute: Option<String>,
    /// `--auth-rate-limit-burst`, else `MMCP_AUTH_RATE_LIMIT_BURST`, as resolved by the argument parser.
    /// Same handling as [`ServerConfigOverrides::auth_rate_limit_per_minute`].
    pub auth_rate_limit_burst: Option<String>,
    /// `--trusted-proxies`, else `MMCP_TRUSTED_PROXIES`, as resolved by the argument parser.
    /// A blank value trusts no proxy.
    pub trusted_proxies: Option<String>,
}
