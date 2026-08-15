//! Test-only helpers shared across the crate's own unit tests.
//!
//! `pub(crate)` (not test-module-private) so unit tests outside
//! `config` (`routes::sync`, `routes::mcp`) can reach it too, instead
//! of each keeping its own copy of the same tracing subscriber or
//! `ServerConfig` fixture, per the project's commonization rule.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::ServerConfig;

/// Loopback bind address used by every in-crate unit-test fixture.
/// Never actually bound: `ServerState::initialize` only parses it.
const TEST_BIND: &str = "127.0.0.1:0";

/// Minimal `ServerConfig` for constructing a real `ServerState`
/// directly in a unit test, without `tests/common` (a separate
/// integration-test crate, unreachable from an in-crate unit test).
/// Every field is a deterministic test default; only `repo_root` is
/// caller-supplied, since each test owns its own tempdir.
pub(crate) fn minimal_server_config(repo_root: PathBuf) -> ServerConfig {
    ServerConfig {
        bind: TEST_BIND
            .parse::<SocketAddr>()
            .expect("valid loopback addr"),
        database_url: "sqlite::memory:".to_string(),
        repo_root,
        token_key: [0u8; 32],
        oauth_providers: vec![],
        origin: "http://localhost:8787".to_string(),
        push_token: None,
        min_password_length: mmcp_auth::MIN_PASSWORD_LENGTH,
        max_password_length: mmcp_auth::MAX_PASSWORD_LENGTH,
        max_handle_length: mmcp_auth::MAX_HANDLE_LENGTH,
    }
}

/// Minimal `tracing::Subscriber` counting `WARN`-level events, so a
/// rejected config value can be asserted to actually log instead of
/// silently discarding it.
pub(crate) struct WarnCounter(pub(crate) Arc<AtomicUsize>);

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
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}
