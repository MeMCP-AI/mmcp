//! Server configuration loaded from environment variables.

mod defaults;

use std::net::SocketAddr;
use std::path::PathBuf;

use defaults::{DEFAULT_BIND, DEFAULT_DATABASE_URL, DEFAULT_REPO_ROOT};
pub use defaults::{MAX_PASSWORD_LENGTH_ENV, MIN_PASSWORD_LENGTH_ENV};

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
}

/// CLI-supplied override tier for [`ServerConfig::from_source_with_overrides`]
/// / [`ServerConfig::from_env_with_overrides`]. A parameter object
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
}

/// Configuration for a single OAuth provider.
#[derive(Debug, Clone)]
pub struct OAuthProviderConfig {
    pub slug: String,
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,
}

impl ServerConfig {
    /// Build a config from the process environment using sensible
    /// defaults, with no CLI override tier. Thin wrapper over
    /// [`from_env_with_overrides`](Self::from_env_with_overrides).
    pub fn from_env() -> Self {
        Self::from_env_with_overrides(ServerConfigOverrides::default())
    }

    /// Build a config from the process environment, honoring the
    /// given CLI-supplied override tier for the password-length
    /// cascade.
    pub fn from_env_with_overrides(overrides: ServerConfigOverrides) -> Self {
        Self::from_source_with_overrides(|key| std::env::var(key).ok(), overrides)
    }

    /// Build a config by pulling each variable from an injectable
    /// source, with no CLI override tier. Exposed separately from
    /// [`from_env`](Self::from_env) so tests can feed a deterministic
    /// map without mutating the process environment.
    pub fn from_source<F>(get: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        Self::from_source_with_overrides(get, ServerConfigOverrides::default())
    }

    /// Build a config by pulling each variable from an injectable
    /// source, honoring the given CLI-supplied override tier for the
    /// password-length cascade. The full [`ServerConfig::from_source`]
    /// / [`ServerConfig::from_env`] wrappers delegate here with a
    /// default (all-`None`) override tier.
    pub fn from_source_with_overrides<F>(get: F, overrides: ServerConfigOverrides) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let bind: SocketAddr = get("MMCP_BIND")
            .unwrap_or_else(|| DEFAULT_BIND.to_string())
            .parse()
            .unwrap_or_else(|_| {
                DEFAULT_BIND
                    .parse()
                    .expect("DEFAULT_BIND is a hardcoded, always-valid SocketAddr literal")
            });
        let database_url =
            get("MMCP_DATABASE_URL").unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());
        let repo_root = get("MMCP_REPO_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_REPO_ROOT));
        let token_key = get("MMCP_TOKEN_KEY_HEX")
            .as_deref()
            .and_then(parse_hex_key)
            .unwrap_or_else(random_key);
        // Fallback origin intentionally uses `localhost` (not the
        // bind IP) because the WebAuthn RP ID is derived from the
        // origin's host and the spec rejects IP literals. Production
        // deployments set `MMCP_ORIGIN` to their public hostname;
        // `cargo run` just needs something that boots.
        let origin =
            get("MMCP_ORIGIN").unwrap_or_else(|| format!("http://localhost:{}", bind.port()));
        let push_token = get("MMCP_PUSH_TOKEN").filter(|token| !token.is_empty());

        let mut oauth_providers = Vec::new();
        if let (Some(id), Some(secret)) = (
            get("MMCP_OAUTH_GITHUB_CLIENT_ID"),
            get("MMCP_OAUTH_GITHUB_CLIENT_SECRET"),
        ) {
            oauth_providers.push(OAuthProviderConfig {
                slug: "github".to_string(),
                client_id: id,
                client_secret: secret,
                auth_url: "https://github.com/login/oauth/authorize".to_string(),
                token_url: "https://github.com/login/oauth/access_token".to_string(),
                userinfo_url: "https://api.github.com/user".to_string(),
            });
        }

        let min_password_length = resolve_min_password_length(&get, overrides.min_password_length);
        let max_password_length = resolve_max_password_length(&get, overrides.max_password_length);

        Self {
            bind,
            database_url,
            repo_root,
            token_key,
            oauth_providers,
            origin,
            push_token,
            min_password_length,
            max_password_length,
        }
    }
}

fn parse_hex_key(input: &str) -> Option<[u8; 32]> {
    // `hex::decode` rejects odd-length input and any non-hex byte;
    // the subsequent `try_into` enforces the 32-byte length. Mixed
    // case is accepted exactly as before.
    hex::decode(input).ok()?.try_into().ok()
}

fn random_key() -> [u8; 32] {
    // Pull 32 bytes straight from the OS CSPRNG (getrandom defers to
    // `getrandom(2)` on Linux, `BCryptGenRandom` on Windows, etc.).
    // On a platform that cannot satisfy that, e.g. a sandbox with no
    // entropy source, panic at startup rather than hand out
    // guessable tokens.
    // Admins set `MMCP_TOKEN_KEY_HEX` explicitly when they need a stable key across restarts.
    let mut out = [0u8; 32];
    getrandom::fill(&mut out).expect("OS CSPRNG unavailable; set MMCP_TOKEN_KEY_HEX explicitly");
    out
}

// ── Password-length tier cascade ───────────────────────────────────
//
// Mirrors `mmcp_store::memory::resolve_max_auto_slug_length`'s
// override -> env -> user-config-file -> compiled-default cascade,
// applied to two independent tunables (minimum and maximum password
// length). The pure resolution logic (`resolve_password_length_from_tiers`)
// stays separate from the I/O-performing wrappers (env var read,
// config file read) so the precedence rules are unit-testable
// without mutating process-global environment state or touching the
// filesystem.

/// Resolve the effective minimum password length.
/// Highest-precedence source wins:
/// 1. `override_len`, the `--min-password-length` CLI flag.
/// 2. [`MIN_PASSWORD_LENGTH_ENV`] environment variable.
/// 3. `~/.mmcp/config.toml` `[limits] min_password_length`
///    ([`mmcp_core::config::UserConfig`]).
/// 4. [`mmcp_auth::MIN_PASSWORD_LENGTH`], the compiled-in fallback.
fn resolve_min_password_length<F>(get: &F, override_len: Option<usize>) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    let env_len = parse_password_length_env(
        "minimum password length",
        MIN_PASSWORD_LENGTH_ENV,
        get(MIN_PASSWORD_LENGTH_ENV).as_deref(),
    );
    let config_len = user_config_password_length(|limits| limits.min_password_length);
    resolve_password_length_from_tiers(
        "minimum password length",
        override_len,
        env_len,
        config_len,
        mmcp_auth::MIN_PASSWORD_LENGTH,
    )
}

/// Resolve the effective maximum password length. Same cascade as
/// [`resolve_min_password_length`], over [`MAX_PASSWORD_LENGTH_ENV`],
/// `[limits] max_password_length`, and [`mmcp_auth::MAX_PASSWORD_LENGTH`].
fn resolve_max_password_length<F>(get: &F, override_len: Option<usize>) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    let env_len = parse_password_length_env(
        "maximum password length",
        MAX_PASSWORD_LENGTH_ENV,
        get(MAX_PASSWORD_LENGTH_ENV).as_deref(),
    );
    let config_len = user_config_password_length(|limits| limits.max_password_length);
    resolve_password_length_from_tiers(
        "maximum password length",
        override_len,
        env_len,
        config_len,
        mmcp_auth::MAX_PASSWORD_LENGTH,
    )
}

/// Parse a raw password-length environment variable value, if any,
/// into a tier value. Logs and falls through (returns `None`) when
/// the variable is present but not a valid number, rather than
/// silently discarding it or defaulting it to zero.
fn parse_password_length_env(field: &str, var_name: &str, raw: Option<&str>) -> Option<usize> {
    let raw = raw?;
    match raw.parse::<usize>() {
        Ok(n) => Some(n),
        Err(err) => {
            tracing::warn!(
                env_value = %raw,
                error = %err,
                "{var_name} is not a valid number; ignoring and falling through to the next {field} tier"
            );
            None
        }
    }
}

/// Precedence resolution given each tier's already-fetched value:
/// `override_len` beats `env_len` beats `config_len` beats
/// `default_len`. A `Some(0)` at any tier counts as absent (falls
/// through), since a zero-byte password bound is never a legitimate
/// intent; whichever tier is the one actually rejected for this
/// reason is logged, naming `field` and that specific tier, before
/// falling through to the next one.
fn resolve_password_length_from_tiers(
    field: &str,
    override_len: Option<usize>,
    env_len: Option<usize>,
    config_len: Option<usize>,
    default_len: usize,
) -> usize {
    if let Some(n) = override_len {
        if n > 0 {
            return n;
        }
        tracing::warn!(
            "{field} override tier rejected: an explicit override of 0 is not a legitimate bound; falling through to the next {field} tier"
        );
    }
    if let Some(n) = env_len {
        if n > 0 {
            return n;
        }
        tracing::warn!(
            "{field} env tier rejected: a value of 0 is not a legitimate bound; falling through to the next {field} tier"
        );
    }
    if let Some(n) = config_len {
        if n > 0 {
            return n;
        }
        tracing::warn!(
            "{field} config tier rejected: a [limits] value of 0 is not a legitimate bound; falling through to the compiled-in default"
        );
    }
    default_len
}

/// Read a password-length field out of `~/.mmcp/config.toml`
/// `[limits]`, if present. `select` picks which of the two fields
/// (`min_password_length` / `max_password_length`) this call
/// resolves, so the min and max cascades share one read-and-log path
/// instead of duplicating it. Mirrors the read-only,
/// missing-file-or-section-means-`None` style
/// `mmcp_store::memory::user_config_max_auto_slug_length` already
/// uses for the same config file; never errors, since a broken or
/// absent user config must never fail server startup. A discovery or
/// parse failure is logged before falling through, rather than
/// discarded with no signal.
fn user_config_password_length(
    select: impl Fn(&mmcp_core::config::LimitsConfig) -> Option<usize>,
) -> Option<usize> {
    let home = match mmcp_store::MmcpHome::discover() {
        Ok(home) => home,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "failed to discover MmcpHome while resolving a password-length config tier; falling through to the next tier"
            );
            return None;
        }
    };
    let cfg = match home.load_user_config() {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "user config failed to parse while resolving a password-length config tier; falling through to the next tier"
            );
            return None;
        }
    };
    cfg.limits.as_ref().and_then(select)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a config from a `HashMap` so tests never touch
    /// `std::env`, which would race under cargo's default parallel
    /// test runner.
    fn from_map(entries: &[(&str, &str)]) -> ServerConfig {
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
    fn malformed_bind_falls_back_to_default_quietly() {
        let cfg = from_map(&[("MMCP_BIND", "not-a-socket-addr")]);
        assert_eq!(cfg.bind.to_string(), "127.0.0.1:8787");
    }

    #[test]
    fn valid_token_key_hex_is_parsed_verbatim() {
        // All-ones key is visibly different from any random fallback.
        let hex = "ff".repeat(32);
        let cfg = from_map(&[("MMCP_TOKEN_KEY_HEX", hex.as_str())]);
        assert_eq!(cfg.token_key, [0xFFu8; 32]);
    }

    #[test]
    fn invalid_token_key_hex_falls_back_to_random() {
        // Wrong length: fallback kicks in silently and still yields
        // 32 non-zero bytes. Two independent fallback calls must
        // also differ from each other, proving the CSPRNG generates
        // fresh randomness per call rather than a fixed or zeroed
        // buffer that would happen to be non-zero once.
        let cfg_a = from_map(&[("MMCP_TOKEN_KEY_HEX", "deadbeef")]);
        let cfg_b = from_map(&[("MMCP_TOKEN_KEY_HEX", "deadbeef")]);
        assert_eq!(cfg_a.token_key.len(), 32);
        assert_ne!(cfg_a.token_key, [0u8; 32]);
        assert_ne!(cfg_a.token_key, cfg_b.token_key);
    }

    #[test]
    fn invalid_hex_digit_falls_back_to_random() {
        // Right length but non-hex chars: parse_hex_key returns None.
        let bad = "z".repeat(64);
        let cfg = from_map(&[("MMCP_TOKEN_KEY_HEX", bad.as_str())]);
        assert_eq!(cfg.token_key.len(), 32);
        assert_ne!(cfg.token_key, [0u8; 32]);
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
        assert!(parse_hex_key("").is_none());
        assert!(parse_hex_key("ff").is_none());
        assert!(parse_hex_key(&"ff".repeat(31)).is_none());
        assert!(parse_hex_key(&"ff".repeat(33)).is_none());
    }

    #[test]
    fn parse_hex_key_rejects_non_hex_characters() {
        assert!(parse_hex_key(&"g".repeat(64)).is_none());
        // Mixed valid/invalid.
        let mut mixed = "f".repeat(63);
        mixed.push('z');
        assert!(parse_hex_key(&mixed).is_none());
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

    // ── Password-length tier cascade ───────────────────────────────

    #[test]
    fn resolve_password_length_from_tiers_prefers_explicit_override() {
        assert_eq!(
            resolve_password_length_from_tiers("x", Some(10), Some(20), Some(30), 40),
            10
        );
    }

    #[test]
    fn resolve_password_length_from_tiers_falls_back_env_then_config_then_default() {
        assert_eq!(
            resolve_password_length_from_tiers("x", None, Some(20), Some(30), 40),
            20
        );
        assert_eq!(
            resolve_password_length_from_tiers("x", None, None, Some(30), 40),
            30
        );
        assert_eq!(
            resolve_password_length_from_tiers("x", None, None, None, 40),
            40
        );
    }

    #[test]
    fn resolve_password_length_from_tiers_treats_zero_as_absent_at_every_tier() {
        assert_eq!(
            resolve_password_length_from_tiers("x", Some(0), Some(20), Some(30), 40),
            20
        );
        assert_eq!(
            resolve_password_length_from_tiers("x", Some(0), Some(0), Some(30), 40),
            30
        );
        assert_eq!(
            resolve_password_length_from_tiers("x", Some(0), Some(0), Some(0), 40),
            40
        );
    }

    /// Minimal `tracing::Subscriber` counting `WARN`-level events, so
    /// a rejected password-length tier can be asserted to actually
    /// log instead of silently discarding the bad value. Mirrors
    /// `mmcp_store::memory`'s `WarnCounter` test helper.
    struct WarnCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl tracing::Subscriber for WarnCounter {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            *metadata.level() == tracing::Level::WARN
        }
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    #[test]
    fn resolve_password_length_from_tiers_warns_only_for_the_tier_actually_reached() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        // The config tier is Some(0) in both calls below, but the
        // first call never reaches it (override wins), so it must
        // never warn; the second call falls through to it, so it
        // must warn exactly once.
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(
                resolve_password_length_from_tiers("x", Some(10), None, Some(0), 40),
                10
            );
        });
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the zero config tier was never consulted, so it must not warn"
        );

        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(
                resolve_password_length_from_tiers("x", None, None, Some(0), 40),
                40
            );
        });
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "falling through the zero config tier must log exactly one warning"
        );
    }

    #[test]
    fn parse_password_length_env_warns_on_malformed_value() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        let result = tracing::subscriber::with_default(subscriber, || {
            parse_password_length_env("x", "MMCP_X", Some("not-a-number"))
        });

        assert_eq!(
            result, None,
            "a malformed env var must be treated as absent, not fabricated into a number"
        );
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a malformed env var must log exactly one warning instead of being silently discarded"
        );
    }

    #[test]
    fn parse_password_length_env_accepts_a_valid_number() {
        assert_eq!(
            parse_password_length_env("x", "MMCP_X", Some("12")),
            Some(12)
        );
    }

    #[test]
    fn parse_password_length_env_treats_absent_as_none() {
        assert_eq!(parse_password_length_env("x", "MMCP_X", None), None);
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
            },
        );
        assert_eq!(cfg.min_password_length, 16);
        assert_eq!(cfg.max_password_length, 64);
    }

    #[test]
    fn from_source_with_overrides_prefers_env_over_a_zero_override() {
        let cfg = ServerConfig::from_source_with_overrides(
            |key| (key == MIN_PASSWORD_LENGTH_ENV).then(|| "12".to_string()),
            ServerConfigOverrides {
                min_password_length: Some(0),
                max_password_length: None,
            },
        );
        assert_eq!(cfg.min_password_length, 12);
    }
}
