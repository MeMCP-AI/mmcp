//! [`ServerConfigOverrides`], the CLI-supplied override tier for [`super::ServerConfig`].

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
}
