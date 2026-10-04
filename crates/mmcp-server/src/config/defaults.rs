//! Default values and environment-variable names for [`super::ServerConfig`].

/// Fallback bind address, used when `MMCP_BIND` is unset. A
/// hardcoded, always-valid `SocketAddr` literal, so the `.expect()`
/// in [`super::ServerConfig::from_source_with_overrides`] that
/// re-parses it can never actually observe a failure. `MMCP_BIND`
/// present but unparsable is a distinct case that fails construction
/// with [`super::ConfigError::InvalidBind`] instead of falling back
/// to this default.
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

/// Environment variable overriding the maximum accepted account handle length.
/// Same precedence position as [`MIN_PASSWORD_LENGTH_ENV`].
pub const MAX_HANDLE_LENGTH_ENV: &str = "MMCP_MAX_HANDLE_LENGTH";

/// Environment variable gating `POST /auth/register`. Any value that
/// case-insensitively equals `"true"` or `"1"` after trimming
/// whitespace enables self-registration; anything else (including
/// unset) leaves it disabled. See
/// [`super::ServerConfig::allow_self_registration`]'s doc comment for
/// why the default is closed.
pub const ALLOW_SELF_REGISTRATION_ENV: &str = "MMCP_ALLOW_SELF_REGISTRATION";

/// Environment variable carrying the GitHub OAuth application client id.
/// The GitHub provider is enabled only when this and [`GITHUB_OAUTH_CLIENT_SECRET_ENV`] are both non-blank.
pub const GITHUB_OAUTH_CLIENT_ID_ENV: &str = "MMCP_OAUTH_GITHUB_CLIENT_ID";

/// Environment variable carrying the GitHub OAuth application client secret.
pub const GITHUB_OAUTH_CLIENT_SECRET_ENV: &str = "MMCP_OAUTH_GITHUB_CLIENT_SECRET";

/// Environment variable naming the directory of the built web UI.
/// The argument parser binds it to `serve --webui-dir`, and the flag beats it.
/// Unset or blank means the server answers the API only.
pub const WEBUI_DIR_ENV: &str = "MMCP_WEBUI_DIR";

/// Environment variable listing, comma-separated, the origins allowed to call the API from a browser.
/// The argument parser binds it to `serve --cors-allowed-origins`, and the flag beats it.
/// Unset or blank attaches no CORS layer.
pub const CORS_ALLOWED_ORIGINS_ENV: &str = "MMCP_CORS_ALLOWED_ORIGINS";

/// Environment variable naming the sign-in and registration requests one client may send per minute once its burst is spent.
/// The argument parser binds it to `serve --auth-rate-limit-per-minute`, and the flag beats it.
/// Unset or blank leaves the next tier of the cascade in charge.
pub const AUTH_RATE_LIMIT_PER_MINUTE_ENV: &str = "MMCP_AUTH_RATE_LIMIT_PER_MINUTE";

/// Environment variable naming the sign-in and registration requests one client may send at once.
/// The argument parser binds it to `serve --auth-rate-limit-burst`, and the flag beats it.
/// Unset or blank leaves the next tier of the cascade in charge.
pub const AUTH_RATE_LIMIT_BURST_ENV: &str = "MMCP_AUTH_RATE_LIMIT_BURST";

/// Environment variable listing, comma-separated, the reverse proxies in front of the server.
/// The argument parser binds it to `serve --trusted-proxies`, and the flag beats it.
/// Unset or blank trusts no proxy, so every client is its TCP peer.
pub const TRUSTED_PROXIES_ENV: &str = "MMCP_TRUSTED_PROXIES";

/// Environment variable naming the leading bits of an IPv6 address that identify one client for the auth rate limit.
/// The argument parser binds it to `serve --auth-rate-limit-ipv6-client-prefix-length`, and the flag beats it.
/// Unset or blank leaves the next tier of the cascade in charge.
pub const AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH_ENV: &str =
    "MMCP_AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH";

/// Environment variable naming the leading bits of an IPv6 address that identify one network for the second auth rate limit.
/// The argument parser binds it to `serve --auth-rate-limit-ipv6-network-prefix-length`, and the flag beats it.
/// Unset or blank leaves the next tier of the cascade in charge, and `0` switches the second limit off.
pub const AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH_ENV: &str =
    "MMCP_AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH";

/// Environment variable naming the requests per minute one IPv6 network may send once its burst is spent.
/// The argument parser binds it to `serve --auth-rate-limit-ipv6-network-per-minute`, and the flag beats it.
/// Unset or blank leaves the next tier of the cascade in charge.
pub const AUTH_RATE_LIMIT_IPV6_NETWORK_PER_MINUTE_ENV: &str =
    "MMCP_AUTH_RATE_LIMIT_IPV6_NETWORK_PER_MINUTE";

/// Environment variable naming the requests one IPv6 network may send at once.
/// The argument parser binds it to `serve --auth-rate-limit-ipv6-network-burst`, and the flag beats it.
/// Unset or blank leaves the next tier of the cascade in charge.
pub const AUTH_RATE_LIMIT_IPV6_NETWORK_BURST_ENV: &str = "MMCP_AUTH_RATE_LIMIT_IPV6_NETWORK_BURST";

/// Default leading bits of an IPv6 address that identify one client.
/// One host or one subnet of a LAN holds a /64, so a client rotating addresses inside it keeps one budget (RFC 4291 section 2.5.4).
pub(crate) const DEFAULT_AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH: u8 = 64;

/// Default leading bits of an IPv6 address that identify one network for the second limit.
/// A /48 is the largest site allocation, 65 536 client prefixes of the default length.
pub(crate) const DEFAULT_AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH: u8 = 48;

/// How many times the client quota the default quota of the IPv6 network budget is.
/// The network budget exists to bound the password hashing work one network holder can force.
/// With the default client quota, this multiple keeps a whole network at about three hashes per second.
pub(crate) const IPV6_NETWORK_QUOTA_MULTIPLE: u32 = 10;

/// Largest number of leading bits an IPv6 prefix holds.
pub(crate) const IPV6_ADDRESS_BITS: u8 = 128;

/// Default requests per minute one client may send to the unauthenticated auth routes once its burst is spent.
/// One argon2 hash every three seconds at most, per client key.
pub(crate) const DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE: u32 = 20;

/// Default requests one client may send to the unauthenticated auth routes at once.
/// Covers a real sign-in with a few retries and an OAuth round trip.
pub(crate) const DEFAULT_AUTH_RATE_LIMIT_BURST: u32 = 10;

/// Floor for the password-length tunables: a zero-byte bound is never legitimate.
pub(crate) const MIN_VALID_LENGTH_LIMIT: usize = 1;
