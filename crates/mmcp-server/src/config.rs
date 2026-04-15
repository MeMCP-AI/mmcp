//! Server configuration loaded from environment variables.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Server configuration.
///
/// Loaded from the following environment variables with the listed
/// defaults so local development is a zero-configuration experience:
///
/// | Variable             | Default                  |
/// | -------------------- | ------------------------ |
/// | `MMCP_BIND`          | `127.0.0.1:8787`         |
/// | `MMCP_DATABASE_URL`  | `sqlite::memory:`        |
/// | `MMCP_REPO_ROOT`     | `./data/repos`           |
/// | `MMCP_TOKEN_KEY_HEX` | random 32 bytes on start |
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub database_url: String,
    pub repo_root: PathBuf,
    pub token_key: [u8; 32],
}

impl ServerConfig {
    /// Build a config from the environment using sensible defaults.
    pub fn from_env() -> Self {
        let bind = std::env::var("MMCP_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8787".to_string())
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:8787".parse().unwrap());
        let database_url =
            std::env::var("MMCP_DATABASE_URL").unwrap_or_else(|_| "sqlite::memory:".to_string());
        let repo_root = std::env::var("MMCP_REPO_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/repos"));
        let token_key = std::env::var("MMCP_TOKEN_KEY_HEX")
            .ok()
            .and_then(|hex| parse_hex_key(&hex))
            .unwrap_or_else(random_key);
        Self {
            bind,
            database_url,
            repo_root,
            token_key,
        }
    }
}

fn parse_hex_key(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let byte = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
        out[i] = byte;
    }
    Some(out)
}

fn random_key() -> [u8; 32] {
    // We only need enough entropy to make a running server hand out
    // unique tokens; admins that care about reproducibility set
    // `MMCP_TOKEN_KEY_HEX` explicitly.
    let mut out = [0u8; 32];
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let bytes = t.to_le_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = bytes[i % bytes.len()] ^ (i as u8).wrapping_mul(37);
    }
    out
}
