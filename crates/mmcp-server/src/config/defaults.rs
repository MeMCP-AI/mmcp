//! Default values and environment-variable names for [`super::ServerConfig`].
//!
//! Kept in its own file per the project's module-organization
//! convention: constants and default values always live in their own
//! dedicated file, never scattered inline alongside the struct/logic
//! that consumes them.

/// Fallback bind address, used both when `MMCP_BIND` is unset and
/// when it is present but fails to parse. A hardcoded, always-valid
/// `SocketAddr` literal, so the `.expect()` in
/// [`super::ServerConfig::from_source_with_overrides`] that re-parses
/// it can never actually observe a failure.
pub(crate) const DEFAULT_BIND: &str = "127.0.0.1:8787";

/// Fallback database URL, used when `MMCP_DATABASE_URL` is unset. An
/// in-memory SQLite database, so `cargo run` boots as a
/// zero-configuration experience with no external database to stand
/// up first.
pub(crate) const DEFAULT_DATABASE_URL: &str = "sqlite::memory:";

/// Fallback repo-root directory, used when `MMCP_REPO_ROOT` is
/// unset. Relative to the process's current working directory.
pub(crate) const DEFAULT_REPO_ROOT: &str = "data/repos";

/// Environment variable overriding the minimum accepted password
/// length. Second in precedence behind an explicit `--min-password-length`
/// CLI override, ahead of the `~/.mmcp/config.toml` `[limits]` tier
/// and the compiled-in default.
pub const MIN_PASSWORD_LENGTH_ENV: &str = "MMCP_MIN_PASSWORD_LENGTH";

/// Environment variable overriding the maximum accepted password
/// length. Same precedence position as [`MIN_PASSWORD_LENGTH_ENV`].
pub const MAX_PASSWORD_LENGTH_ENV: &str = "MMCP_MAX_PASSWORD_LENGTH";
