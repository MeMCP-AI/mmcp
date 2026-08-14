//! Server configuration loaded from environment variables.

use std::net::SocketAddr;
use std::path::PathBuf;

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
    /// defaults. Thin wrapper over [`from_source`] that reads each
    /// variable via `std::env::var`.
    pub fn from_env() -> Self {
        Self::from_source(|key| std::env::var(key).ok())
    }

    /// Build a config by pulling each variable from an injectable
    /// source. Exposed separately from [`from_env`] so tests can feed
    /// a deterministic map without mutating the process environment.
    pub fn from_source<F>(get: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let bind: SocketAddr = get("MMCP_BIND")
            .unwrap_or_else(|| "127.0.0.1:8787".to_string())
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:8787".parse().unwrap());
        let database_url =
            get("MMCP_DATABASE_URL").unwrap_or_else(|| "sqlite::memory:".to_string());
        let repo_root = get("MMCP_REPO_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data/repos"));
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

        Self {
            bind,
            database_url,
            repo_root,
            token_key,
            oauth_providers,
            origin,
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
    // guessable tokens. Admins set `MMCP_TOKEN_KEY_HEX` explicitly
    // when they need a stable key across restarts.
    let mut out = [0u8; 32];
    getrandom::fill(&mut out).expect("OS CSPRNG unavailable; set MMCP_TOKEN_KEY_HEX explicitly");
    out
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
}
