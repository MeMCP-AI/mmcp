//! [`ServerConfig`], the server's top-level configuration.

use std::net::SocketAddr;
use std::path::PathBuf;

use super::cascade::{
    load_user_limits, resolve_max_handle_length, resolve_max_password_length,
    resolve_min_password_length,
};
use super::defaults::{
    ALLOW_SELF_REGISTRATION_ENV, DEFAULT_BIND, DEFAULT_DATABASE_URL, DEFAULT_REPO_ROOT,
};
use super::error::ConfigError;
use super::oauth_provider::OAuthProviderConfig;
use super::overrides::ServerConfigOverrides;

/// Server configuration.
///
/// Loaded from the following environment variables with the listed
/// defaults so local development is a zero-configuration experience:
///
/// | Variable                          | Default                  |
/// | --------------------------------- | ------------------------ |
/// | `MMCP_BIND`                       | `127.0.0.1:8787`         |
/// | `MMCP_DATABASE_URL`               | `sqlite::memory:`        |
/// | `MMCP_REPO_ROOT`                  | `./data/repos`           |
/// | `MMCP_TOKEN_KEY_HEX`              | random 32 bytes on start |
/// | `MMCP_OAUTH_GITHUB_CLIENT_ID`     | (absent = disabled)      |
/// | `MMCP_OAUTH_GITHUB_CLIENT_SECRET` | (absent = disabled)      |
/// | `MMCP_ORIGIN`                     | `http://localhost:<port>`|
/// | `MMCP_PUSH_TOKEN`                 | (absent = pushes disabled)|
/// | `MMCP_MIN_PASSWORD_LENGTH`        | see `min_password_length` cascade below |
/// | `MMCP_MAX_PASSWORD_LENGTH`        | see `max_password_length` cascade below |
/// | `MMCP_MAX_HANDLE_LENGTH`          | see `max_handle_length` cascade below |
/// | `MMCP_ALLOW_SELF_REGISTRATION`    | `false` (self-registration disabled) |
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub database_url: String,
    pub repo_root: PathBuf,
    pub token_key: [u8; 32],
    pub oauth_providers: Vec<OAuthProviderConfig>,
    /// Public origin of the server (e.g. `https://mmcp.example.com`).
    /// Used to build OAuth callback URLs and WebAuthn relying party ID.
    pub origin: String,
    /// Shared-secret bearer token that authorizes `git-receive-pack`
    /// (push) requests. A single global token authorizes writes to
    /// EVERY group hosted by this server; there is no per-group
    /// token concept yet. `None` (the default when
    /// `MMCP_PUSH_TOKEN` is unset or empty) disables push entirely.
    pub push_token: Option<String>,
    /// Effective minimum accepted account password length, in
    /// bytes, resolved through the override/env/config-file/default
    /// cascade (CLI `--min-password-length` beats
    /// `MMCP_MIN_PASSWORD_LENGTH` beats `~/.mmcp/config.toml`
    /// `[limits] min_password_length` beats
    /// `mmcp_auth::MIN_PASSWORD_LENGTH`).
    pub min_password_length: usize,
    /// Effective maximum accepted account password length, in
    /// bytes. Same cascade as [`ServerConfig::min_password_length`],
    /// over `MMCP_MAX_PASSWORD_LENGTH`, `[limits] max_password_length`,
    /// and `mmcp_auth::MAX_PASSWORD_LENGTH`.
    pub max_password_length: usize,
    /// Effective maximum accepted account handle length, in bytes.
    /// Same cascade as [`ServerConfig::min_password_length`].
    /// Tiers: `MMCP_MAX_HANDLE_LENGTH`, `[limits] max_handle_length`, `mmcp_auth::MAX_HANDLE_LENGTH`.
    pub max_handle_length: usize,
    /// Whether `POST /auth/register` accepts unauthenticated
    /// self-registration requests. Sourced from
    /// `MMCP_ALLOW_SELF_REGISTRATION` (any value case-insensitively
    /// equal to `"true"` or `"1"` after trimming whitespace enables
    /// it); **defaults to `false` (closed) when the variable is unset
    /// or holds any other value**.
    ///
    /// Full per-group ACL work (tracked separately, out of scope
    /// here) is what will eventually let a self-hosted deployment
    /// safely open registration to arbitrary callers; until then,
    /// `post_push` authorizes writes on group/memory ids taken
    /// straight from the request body with no membership check, so
    /// an open registration endpoint turns that gap into a live
    /// anonymous write/read capability in three HTTP calls
    /// (register, login, push). A self-hosted/dev deployment that
    /// wants open registration opts in explicitly by setting
    /// `MMCP_ALLOW_SELF_REGISTRATION=true`; production must never
    /// rely on an implicit default here.
    pub allow_self_registration: bool,
}

impl ServerConfig {
    /// Build a config from the process environment using sensible
    /// defaults, with no CLI override tier. Thin wrapper over
    /// [`from_env_with_overrides`](Self::from_env_with_overrides).
    ///
    /// # Errors
    /// [`ConfigError::RandomKeyUnavailable`] when `MMCP_TOKEN_KEY_HEX`
    /// is unset and the OS CSPRNG cannot be read either.
    /// [`ConfigError::InvalidBind`] when `MMCP_BIND` is explicitly set
    /// to a value that is not a valid socket address.
    /// [`ConfigError::TokenKeyHexWrongLength`] when `MMCP_TOKEN_KEY_HEX`
    /// is explicitly set to a value that is not exactly 64 characters.
    /// [`ConfigError::TokenKeyHexInvalidCharacter`] when it is 64
    /// characters but contains a non-hex byte.
    /// [`ConfigError::MinPasswordLengthExceedsMax`] when the resolved
    /// minimum password length exceeds the resolved maximum.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_env_with_overrides(ServerConfigOverrides::default())
    }

    /// Build a config from the process environment, honoring the
    /// given CLI-supplied override tier for the min/max password
    /// length and max handle length cascades.
    ///
    /// # Errors
    /// See [`ServerConfig::from_env`].
    pub fn from_env_with_overrides(overrides: ServerConfigOverrides) -> Result<Self, ConfigError> {
        Self::from_source_with_overrides(|key| std::env::var(key).ok(), overrides)
    }

    /// Build a config by pulling each variable from an injectable
    /// source, with no CLI override tier. Exposed separately from
    /// [`from_env`](Self::from_env) so tests can feed a deterministic
    /// map without mutating the process environment.
    ///
    /// # Errors
    /// See [`ServerConfig::from_env`].
    pub fn from_source<F>(get: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Self::from_source_with_overrides(get, ServerConfigOverrides::default())
    }

    /// Build a config by pulling each variable from an injectable
    /// source, honoring the given CLI-supplied override tier for the
    /// min/max password length and max handle length cascades. The
    /// full [`ServerConfig::from_source`] / [`ServerConfig::from_env`]
    /// wrappers delegate here with a default (all-`None`) override
    /// tier.
    ///
    /// # Errors
    /// See [`ServerConfig::from_env`].
    pub fn from_source_with_overrides<F>(
        get: F,
        overrides: ServerConfigOverrides,
    ) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let bind: SocketAddr = match get("MMCP_BIND") {
            Some(raw) => raw
                .parse()
                .map_err(|source| ConfigError::InvalidBind { raw, source })?,
            None => {
                // NOTE: `DEFAULT_BIND` is a hardcoded string literal
                // owned by this crate, never user input, so its parse
                // outcome is fixed at compile time and covered by the
                // crate's own test suite; it cannot fail at runtime.
                #[allow(clippy::expect_used)]
                DEFAULT_BIND
                    .parse()
                    .expect("DEFAULT_BIND is a hardcoded, always-valid SocketAddr literal")
            }
        };
        let database_url =
            get("MMCP_DATABASE_URL").unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());
        let repo_root = get("MMCP_REPO_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_REPO_ROOT));
        let token_key = match get("MMCP_TOKEN_KEY_HEX") {
            // Never carry `raw` into the error: it is the (rejected)
            // key material itself.
            Some(raw) => parse_hex_key(&raw)?,
            None => random_key()?,
        };
        // Fallback origin intentionally uses `localhost` (not the
        // bind IP) because the WebAuthn RP ID is derived from the
        // origin's host and the spec rejects IP literals. Production
        // deployments set `MMCP_ORIGIN` to their public hostname;
        // `cargo run` just needs something that boots.
        let origin =
            get("MMCP_ORIGIN").unwrap_or_else(|| format!("http://localhost:{}", bind.port()));
        let push_token = get("MMCP_PUSH_TOKEN")
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());

        let mut oauth_providers = Vec::new();
        if let (Some(id), Some(secret)) = (
            get("MMCP_OAUTH_GITHUB_CLIENT_ID"),
            get("MMCP_OAUTH_GITHUB_CLIENT_SECRET"),
        ) {
            oauth_providers.push(OAuthProviderConfig::github(id, secret));
        }

        let user_limits = load_user_limits();
        let min_password_length =
            resolve_min_password_length(&get, overrides.min_password_length, user_limits.as_ref());
        let max_password_length =
            resolve_max_password_length(&get, overrides.max_password_length, user_limits.as_ref());
        if min_password_length > max_password_length {
            return Err(ConfigError::MinPasswordLengthExceedsMax {
                min: min_password_length,
                max: max_password_length,
            });
        }
        let max_handle_length =
            resolve_max_handle_length(&get, overrides.max_handle_length, user_limits.as_ref());
        let allow_self_registration = parse_bool_env(get(ALLOW_SELF_REGISTRATION_ENV).as_deref());

        Ok(Self {
            bind,
            database_url,
            repo_root,
            token_key,
            oauth_providers,
            origin,
            push_token,
            min_password_length,
            max_password_length,
            max_handle_length,
            allow_self_registration,
        })
    }
}

/// Parse an environment-variable-style boolean: a value that, after
/// trimming ASCII whitespace, case-insensitively equals `"true"` or
/// exactly equals `"1"` is `true`; every other value (including
/// `None`, empty, or unrecognized text) is `false`. Fails closed by
/// construction: there is no branch that can return `true` from an
/// absent or malformed value.
fn parse_bool_env(raw: Option<&str>) -> bool {
    match raw.map(str::trim) {
        Some(v) => v.eq_ignore_ascii_case("true") || v == "1",
        None => false,
    }
}

/// Number of hex characters `MMCP_TOKEN_KEY_HEX` must be: twice
/// [`mmcp_auth::token::V4_LOCAL_KEY_BYTES`], one hex digit pair per
/// byte.
const TOKEN_KEY_HEX_CHARS: usize = mmcp_auth::token::V4_LOCAL_KEY_BYTES * 2;

/// Decode `input` as the hex-encoded token-signing key, distinguishing
/// a wrong-length input from one containing a non-hex character so
/// the caller can raise the matching [`ConfigError`] variant instead
/// of one fieldless catch-all. Length is checked on the raw character
/// count BEFORE decoding: a [`TOKEN_KEY_HEX_CHARS`]-character input
/// decodes to exactly [`mmcp_auth::token::V4_LOCAL_KEY_BYTES`] bytes
/// whenever every character is valid hex, so this single upfront
/// check also subsumes `hex::decode`'s `OddLength` failure without
/// needing a separate branch for it. Mixed case is accepted exactly
/// as before.
fn parse_hex_key(input: &str) -> Result<[u8; mmcp_auth::token::V4_LOCAL_KEY_BYTES], ConfigError> {
    if input.len() != TOKEN_KEY_HEX_CHARS {
        return Err(ConfigError::TokenKeyHexWrongLength { got: input.len() });
    }
    let bytes = hex::decode(input).map_err(|_| ConfigError::TokenKeyHexInvalidCharacter)?;
    // `input.len() == TOKEN_KEY_HEX_CHARS` guarantees a successful
    // decode produces exactly `V4_LOCAL_KEY_BYTES` bytes.
    #[allow(clippy::expect_used)]
    Ok(bytes
        .try_into()
        .expect("checked length decodes to exactly V4_LOCAL_KEY_BYTES bytes"))
}

/// Pull 32 bytes straight from the OS CSPRNG (getrandom defers to
/// `getrandom(2)` on Linux, `BCryptGenRandom` on Windows, etc.). A
/// sandbox with no entropy source can genuinely fail this, so the
/// failure is propagated as [`ConfigError::RandomKeyUnavailable`]
/// rather than panicking: admins set `MMCP_TOKEN_KEY_HEX` explicitly
/// when they need a stable key across restarts, and are the ones who
/// must be told, not crashed on, when that is not an option either.
fn random_key() -> Result<[u8; mmcp_auth::token::V4_LOCAL_KEY_BYTES], ConfigError> {
    let mut out = [0u8; mmcp_auth::token::V4_LOCAL_KEY_BYTES];
    getrandom::fill(&mut out).map_err(ConfigError::RandomKeyUnavailable)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::config::test_support::WarnCounter;
    use crate::config::{MAX_HANDLE_LENGTH_ENV, MAX_PASSWORD_LENGTH_ENV, MIN_PASSWORD_LENGTH_ENV};
    use std::collections::HashMap;

    /// Build a config from a `HashMap` so tests never touch
    /// `std::env`, which would race under cargo's default parallel
    /// test runner.
    fn from_map(entries: &[(&str, &str)]) -> ServerConfig {
        from_map_result(entries).expect("the OS CSPRNG is available in the test environment")
    }

    /// Same as [`from_map`] but returns the raw `Result`, for tests
    /// asserting a specific [`ConfigError`] rather than a successful
    /// construction.
    fn from_map_result(entries: &[(&str, &str)]) -> Result<ServerConfig, ConfigError> {
        let map: HashMap<&str, &str> = entries.iter().copied().collect();
        ServerConfig::from_source(|key| map.get(key).map(|s| (*s).to_string()))
    }

    #[test]
    fn from_source_applies_defaults_when_nothing_is_set() {
        let cfg = from_map(&[]);
        assert_eq!(cfg.bind.to_string(), "127.0.0.1:8787");
        assert_eq!(cfg.database_url, "sqlite::memory:");
        assert_eq!(cfg.repo_root, PathBuf::from("data/repos"));
        assert_eq!(cfg.origin, "http://localhost:8787");
        assert!(cfg.oauth_providers.is_empty());
        // A dead/no-op CSPRNG would silently leave the buffer
        // zero-initialized; the length check alone can never catch
        // that (it is a compile-time invariant of `[u8; 32]`), so
        // assert the actual bit pattern is non-zero to prove
        // `getrandom::fill` really ran.
        assert_eq!(cfg.token_key.len(), 32);
        assert_ne!(cfg.token_key, [0u8; 32]);
    }

    #[test]
    fn from_source_honors_bind_database_and_repo_root_overrides() {
        let cfg = from_map(&[
            ("MMCP_BIND", "0.0.0.0:9000"),
            ("MMCP_DATABASE_URL", "postgres://x:y@db/app"),
            ("MMCP_REPO_ROOT", "/srv/mmcp/repos"),
        ]);
        assert_eq!(cfg.bind.to_string(), "0.0.0.0:9000");
        assert_eq!(cfg.database_url, "postgres://x:y@db/app");
        assert_eq!(cfg.repo_root, PathBuf::from("/srv/mmcp/repos"));
    }

    #[test]
    fn origin_defaults_to_localhost_and_inherits_bind_port() {
        // Bind may be a loopback/wildcard IP, but the derived origin
        // always uses `localhost` because WebAuthn rejects IP-literal
        // RP IDs.
        let cfg = from_map(&[("MMCP_BIND", "0.0.0.0:9000")]);
        assert_eq!(cfg.origin, "http://localhost:9000");
    }

    #[test]
    fn explicit_origin_override_wins_over_bind_derived_default() {
        let cfg = from_map(&[
            ("MMCP_BIND", "0.0.0.0:9000"),
            ("MMCP_ORIGIN", "https://mmcp.example.com"),
        ]);
        assert_eq!(cfg.origin, "https://mmcp.example.com");
    }

    #[test]
    fn malformed_bind_fails_construction_instead_of_falling_back() {
        let result = from_map_result(&[("MMCP_BIND", "not-a-socket-addr")]);
        match result {
            Err(ConfigError::InvalidBind { raw, .. }) => {
                assert_eq!(raw, "not-a-socket-addr");
            }
            other => panic!("expected ConfigError::InvalidBind, got {other:?}"),
        }
    }

    #[test]
    fn valid_token_key_hex_is_parsed_verbatim() {
        // All-ones key is visibly different from any random fallback.
        let hex = "ff".repeat(32);
        let cfg = from_map(&[("MMCP_TOKEN_KEY_HEX", hex.as_str())]);
        assert_eq!(cfg.token_key, [0xFFu8; 32]);
    }

    #[test]
    fn wrong_length_token_key_hex_fails_construction_instead_of_falling_back() {
        // Wrong length: construction must fail rather than silently
        // substituting a random session-signing key, with the exact
        // character count reported, not the shared invalid-character
        // variant.
        let result = from_map_result(&[("MMCP_TOKEN_KEY_HEX", "deadbeef")]);
        assert!(
            matches!(result, Err(ConfigError::TokenKeyHexWrongLength { got: 8 })),
            "expected ConfigError::TokenKeyHexWrongLength {{ got: 8 }}, got {result:?}"
        );
    }

    #[test]
    fn invalid_hex_digit_token_key_fails_construction_instead_of_falling_back() {
        // Right length but non-hex chars: parse_hex_key rejects on
        // content, not length, and construction must fail with the
        // distinct invalid-character variant rather than falling back.
        let bad = "z".repeat(64);
        let result = from_map_result(&[("MMCP_TOKEN_KEY_HEX", bad.as_str())]);
        assert!(
            matches!(result, Err(ConfigError::TokenKeyHexInvalidCharacter)),
            "expected ConfigError::TokenKeyHexInvalidCharacter, got {result:?}"
        );
    }

    /// `random_key` returns a typed `Result` (see [`ConfigError::RandomKeyUnavailable`])
    /// instead of panicking on a CSPRNG failure. A genuine CSPRNG
    /// outage cannot be simulated portably in a unit test, so this
    /// asserts the signature is actually `Result`-returning (a
    /// panicking `-> [u8; 32]` would not compile against this call
    /// site) and that two calls still yield fresh, independent keys.
    #[test]
    fn random_key_returns_ok_with_fresh_bytes_each_call() {
        let a = random_key().expect("OS CSPRNG must be available in the test environment");
        let b = random_key().expect("OS CSPRNG must be available in the test environment");
        assert_eq!(a.len(), 32);
        assert_ne!(a, [0u8; 32]);
        assert_ne!(a, b);
    }

    #[test]
    fn absent_token_key_hex_falls_back_to_random_without_warning() {
        // No MMCP_TOKEN_KEY_HEX at all is the normal zero-config path,
        // not a rejected value: it must never warn.
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());
        let cfg = tracing::subscriber::with_default(subscriber, || from_map(&[]));
        assert_eq!(cfg.token_key.len(), 32);
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an unset MMCP_TOKEN_KEY_HEX is the documented default, not a rejected value"
        );
    }

    #[test]
    fn github_oauth_enabled_only_when_both_client_id_and_secret_are_set() {
        let only_id = from_map(&[("MMCP_OAUTH_GITHUB_CLIENT_ID", "abc")]);
        assert!(only_id.oauth_providers.is_empty());

        let only_secret = from_map(&[("MMCP_OAUTH_GITHUB_CLIENT_SECRET", "xyz")]);
        assert!(only_secret.oauth_providers.is_empty());

        let both = from_map(&[
            ("MMCP_OAUTH_GITHUB_CLIENT_ID", "abc"),
            ("MMCP_OAUTH_GITHUB_CLIENT_SECRET", "xyz"),
        ]);
        assert_eq!(both.oauth_providers.len(), 1);
        let gh = &both.oauth_providers[0];
        assert_eq!(gh.slug, "github");
        assert_eq!(gh.client_id, "abc");
        assert_eq!(gh.client_secret, "xyz");
        assert_eq!(gh.auth_url, "https://github.com/login/oauth/authorize");
        assert_eq!(gh.token_url, "https://github.com/login/oauth/access_token");
        assert_eq!(gh.userinfo_url, "https://api.github.com/user");
    }

    #[test]
    fn parse_hex_key_rejects_wrong_length() {
        assert!(matches!(
            parse_hex_key(""),
            Err(ConfigError::TokenKeyHexWrongLength { got: 0 })
        ));
        assert!(matches!(
            parse_hex_key("ff"),
            Err(ConfigError::TokenKeyHexWrongLength { got: 2 })
        ));
        assert!(matches!(
            parse_hex_key(&"ff".repeat(31)),
            Err(ConfigError::TokenKeyHexWrongLength { got: 62 })
        ));
        assert!(matches!(
            parse_hex_key(&"ff".repeat(33)),
            Err(ConfigError::TokenKeyHexWrongLength { got: 66 })
        ));
    }

    #[test]
    fn parse_hex_key_rejects_non_hex_characters() {
        assert!(matches!(
            parse_hex_key(&"g".repeat(64)),
            Err(ConfigError::TokenKeyHexInvalidCharacter)
        ));
        // Mixed valid/invalid, right length.
        let mut mixed = "f".repeat(63);
        mixed.push('z');
        assert!(matches!(
            parse_hex_key(&mixed),
            Err(ConfigError::TokenKeyHexInvalidCharacter)
        ));
    }

    #[test]
    fn parse_hex_key_accepts_valid_lowercase_and_uppercase() {
        let lower = "a".repeat(64);
        let upper = "A".repeat(64);
        let lo = parse_hex_key(&lower).expect("lowercase valid");
        let up = parse_hex_key(&upper).expect("uppercase valid");
        assert_eq!(lo, up);
        assert_eq!(lo, [0xAAu8; 32]);
    }

    #[test]
    fn push_token_absent_when_env_var_unset_or_empty() {
        assert!(from_map(&[]).push_token.is_none());
        assert!(from_map(&[("MMCP_PUSH_TOKEN", "")]).push_token.is_none());
    }

    #[test]
    fn push_token_present_when_env_var_is_a_nonempty_string() {
        let cfg = from_map(&[("MMCP_PUSH_TOKEN", "s3cr3t")]);
        assert_eq!(cfg.push_token.as_deref(), Some("s3cr3t"));
    }

    #[test]
    fn push_token_strips_surrounding_whitespace() {
        // A copy-pasted .env line often carries a trailing newline or
        // trailing spaces; the stored token must match what a client
        // sends verbatim, so padding is stripped before comparison.
        let cfg = from_map(&[("MMCP_PUSH_TOKEN", "  s3cr3t\n")]);
        assert_eq!(cfg.push_token.as_deref(), Some("s3cr3t"));
    }

    #[test]
    fn push_token_absent_when_env_var_is_whitespace_only() {
        let cfg = from_map(&[("MMCP_PUSH_TOKEN", "   \n\t")]);
        assert!(cfg.push_token.is_none());
    }

    #[test]
    fn from_source_with_overrides_plumbs_the_override_tier_into_server_config() {
        // The override tier always wins regardless of whatever the
        // real machine's `~/.mmcp/config.toml` happens to contain,
        // so this assertion stays deterministic without needing to
        // isolate `MMCP_HOME`.
        let cfg = ServerConfig::from_source_with_overrides(
            |_| None,
            ServerConfigOverrides {
                min_password_length: Some(16),
                max_password_length: Some(64),
                max_handle_length: Some(32),
            },
        )
        .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(cfg.min_password_length, 16);
        assert_eq!(cfg.max_password_length, 64);
        assert_eq!(cfg.max_handle_length, 32);
    }

    #[test]
    fn from_source_with_overrides_prefers_env_over_a_zero_override() {
        let cfg = ServerConfig::from_source_with_overrides(
            |key| (key == MIN_PASSWORD_LENGTH_ENV).then(|| "12".to_string()),
            ServerConfigOverrides {
                min_password_length: Some(0),
                max_password_length: None,
                max_handle_length: None,
            },
        )
        .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(cfg.min_password_length, 12);
    }

    #[test]
    fn min_password_length_exceeding_max_via_overrides_fails_construction() {
        let result = ServerConfig::from_source_with_overrides(
            |_| None,
            ServerConfigOverrides {
                min_password_length: Some(50),
                max_password_length: Some(10),
                max_handle_length: None,
            },
        );
        match result {
            Err(ConfigError::MinPasswordLengthExceedsMax { min, max }) => {
                assert_eq!(min, 50);
                assert_eq!(max, 10);
            }
            other => panic!("expected ConfigError::MinPasswordLengthExceedsMax, got {other:?}"),
        }
    }

    #[test]
    fn min_password_length_exceeding_max_via_env_fails_construction() {
        let result = from_map_result(&[
            (MIN_PASSWORD_LENGTH_ENV, "50"),
            (MAX_PASSWORD_LENGTH_ENV, "10"),
        ]);
        match result {
            Err(ConfigError::MinPasswordLengthExceedsMax { min, max }) => {
                assert_eq!(min, 50);
                assert_eq!(max, 10);
            }
            other => panic!("expected ConfigError::MinPasswordLengthExceedsMax, got {other:?}"),
        }
    }

    // ── max_handle_length cascade (falsification anchor) ────────────
    //
    // Mirrors the min/max password length precedence tests above,
    // proving `max_handle_length`'s own override -> env ->
    // config-file -> default precedence independently of the shared
    // `resolve_usize_from_tiers` unit tests (those prove the generic
    // mechanism; these prove `ServerConfig` actually wires
    // `max_handle_length` through it end to end).

    #[test]
    fn max_handle_length_defaults_to_the_compiled_in_constant_when_nothing_is_set() {
        let cfg = from_map(&[]);
        assert_eq!(cfg.max_handle_length, mmcp_auth::MAX_HANDLE_LENGTH);
    }

    #[test]
    fn max_handle_length_env_var_is_honored_via_from_map() {
        let cfg = from_map(&[(MAX_HANDLE_LENGTH_ENV, "20")]);
        assert_eq!(cfg.max_handle_length, 20);
    }

    #[test]
    fn max_handle_length_override_beats_env_beats_default() {
        let overridden = ServerConfig::from_source_with_overrides(
            |key| (key == MAX_HANDLE_LENGTH_ENV).then(|| "20".to_string()),
            ServerConfigOverrides {
                min_password_length: None,
                max_password_length: None,
                max_handle_length: Some(8),
            },
        )
        .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(
            overridden.max_handle_length, 8,
            "the CLI override tier must beat a set env var"
        );

        let env_only = ServerConfig::from_source_with_overrides(
            |key| (key == MAX_HANDLE_LENGTH_ENV).then(|| "20".to_string()),
            ServerConfigOverrides::default(),
        )
        .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(
            env_only.max_handle_length, 20,
            "the env tier must beat the compiled-in default"
        );

        let default_only =
            ServerConfig::from_source_with_overrides(|_| None, ServerConfigOverrides::default())
                .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(
            default_only.max_handle_length,
            mmcp_auth::MAX_HANDLE_LENGTH,
            "with no override and no env var, the compiled-in default must win"
        );
    }

    #[test]
    fn max_handle_length_zero_override_falls_through_to_env() {
        let cfg = ServerConfig::from_source_with_overrides(
            |key| (key == MAX_HANDLE_LENGTH_ENV).then(|| "12".to_string()),
            ServerConfigOverrides {
                min_password_length: None,
                max_password_length: None,
                max_handle_length: Some(0),
            },
        )
        .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(
            cfg.max_handle_length, 12,
            "an explicit override of 0 is not a legitimate bound and must fall through to env"
        );
    }

    #[test]
    fn max_handle_length_zero_env_falls_through_to_default() {
        let cfg = ServerConfig::from_source_with_overrides(
            |key| (key == MAX_HANDLE_LENGTH_ENV).then(|| "0".to_string()),
            ServerConfigOverrides::default(),
        )
        .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(
            cfg.max_handle_length,
            mmcp_auth::MAX_HANDLE_LENGTH,
            "an env value of 0 is not a legitimate bound and must fall through to the default"
        );
    }

    // ── max_handle_length floor ──
    //
    // A configured max_handle_length below the floor is rejected by the cascade, not just an exact 0.

    #[test]
    fn max_handle_length_too_small_nonzero_override_falls_through_to_env() {
        let cfg = ServerConfig::from_source_with_overrides(
            |key| (key == MAX_HANDLE_LENGTH_ENV).then(|| "12".to_string()),
            ServerConfigOverrides {
                min_password_length: None,
                max_password_length: None,
                // Below MIN_VIABLE_MAX_HANDLE_LENGTH (4) but nonzero.
                max_handle_length: Some(2),
            },
        )
        .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(
            cfg.max_handle_length, 12,
            "an override too small to leave suffix room is not a legitimate bound and must \
             fall through to env, exactly like an override of 0 already does"
        );
    }

    #[test]
    fn max_handle_length_too_small_nonzero_env_falls_through_to_default() {
        let cfg = ServerConfig::from_source_with_overrides(
            |key| (key == MAX_HANDLE_LENGTH_ENV).then(|| "1".to_string()),
            ServerConfigOverrides::default(),
        )
        .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(
            cfg.max_handle_length,
            mmcp_auth::MAX_HANDLE_LENGTH,
            "an env value too small to leave suffix room must fall through to the compiled-in \
             default, exactly like a value of 0 already does"
        );
    }

    #[test]
    fn max_handle_length_at_the_viable_floor_is_accepted() {
        let cfg = ServerConfig::from_source_with_overrides(
            |_| None,
            ServerConfigOverrides {
                min_password_length: None,
                max_password_length: None,
                max_handle_length: Some(mmcp_auth::MIN_VIABLE_MAX_HANDLE_LENGTH),
            },
        )
        .expect("the OS CSPRNG is available in the test environment");
        assert_eq!(
            cfg.max_handle_length,
            mmcp_auth::MIN_VIABLE_MAX_HANDLE_LENGTH,
            "a value exactly at the viable floor leaves just enough suffix room and must be \
             accepted, not rejected"
        );
    }

    // allow_self_registration: defaults closed.

    #[test]
    fn allow_self_registration_defaults_to_false_when_unset() {
        assert!(!from_map(&[]).allow_self_registration);
    }

    #[test]
    fn allow_self_registration_is_true_when_env_var_is_true() {
        assert!(from_map(&[("MMCP_ALLOW_SELF_REGISTRATION", "true")]).allow_self_registration);
        assert!(from_map(&[("MMCP_ALLOW_SELF_REGISTRATION", "TRUE")]).allow_self_registration);
        assert!(from_map(&[("MMCP_ALLOW_SELF_REGISTRATION", "1")]).allow_self_registration);
    }

    #[test]
    fn allow_self_registration_stays_false_for_any_other_value() {
        assert!(!from_map(&[("MMCP_ALLOW_SELF_REGISTRATION", "false")]).allow_self_registration);
        assert!(!from_map(&[("MMCP_ALLOW_SELF_REGISTRATION", "0")]).allow_self_registration);
        assert!(!from_map(&[("MMCP_ALLOW_SELF_REGISTRATION", "yes")]).allow_self_registration);
        assert!(!from_map(&[("MMCP_ALLOW_SELF_REGISTRATION", "")]).allow_self_registration);
    }

    #[test]
    fn allow_self_registration_trims_surrounding_whitespace() {
        assert!(from_map(&[("MMCP_ALLOW_SELF_REGISTRATION", "  true\n")]).allow_self_registration);
    }
}
