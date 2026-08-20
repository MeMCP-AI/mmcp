//! Environment variable names carrying sync credentials.
//!
//! Standalone constants file, kept out of [`super::sync_config`] even
//! though [`super::SyncConfig`] is their only reader: this crate's
//! module-organization convention (mirrored by e.g. `mmcp-sync`'s
//! `engine::defaults`) keeps a constant out of the file holding the
//! struct/logic that consumes it, even a small handful in an
//! otherwise-small module.

/// Environment variable carrying the control-plane bearer token for
/// `/sync/*` requests: a per-user PASETO session token issued by the
/// server's login flow and verified against `mmcp_auth::TokenVerifier`.
pub const SYNC_TOKEN_ENV: &str = "MMCP_SYNC_TOKEN";

/// Environment variable carrying the content-plane git push
/// credential, compared by the server's git smart-HTTP endpoint by
/// exact string equality against its own `MMCP_PUSH_TOKEN`. The two
/// env var names differ because they read in different processes
/// (this one client side, `MMCP_PUSH_TOKEN` server side); whoever
/// configures both sides sets them to the same value.
pub const SYNC_PUSH_TOKEN_ENV: &str = "MMCP_SYNC_PUSH_TOKEN";
