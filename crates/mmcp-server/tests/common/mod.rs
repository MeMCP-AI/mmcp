//! Test-only `ServerConfig` builder shared across mmcp-server's
//! integration test binaries.
//!
//! Each file under `tests/` compiles as an independent crate, so
//! this module is pulled in via `mod common;` (never a peer `use`)
//! in every file that needs it.

use std::path::PathBuf;

use mmcp_server::config::test_support::minimal_server_config;
use mmcp_server::config::{OAuthProviderConfig, ServerConfig};

/// Builder for the `ServerConfig` used across integration test
/// suites.
///
/// Every field starts at [`minimal_server_config`]'s fixed test
/// default: an ephemeral loopback bind, an in-memory SQLite database,
/// no OAuth providers, no push token, and the compiled-in
/// `mmcp_auth` password-length and handle-length defaults. Only
/// fields with a setter method below are settable through this
/// builder; `build()` returns the config with every other field at
/// these defaults.
pub struct TestServerConfigBuilder {
    config: ServerConfig,
}

impl TestServerConfigBuilder {
    /// Starts a builder with test defaults for the given repo root.
    ///
    /// `repo_root` is the one field every test genuinely supplies
    /// itself, since each test owns its own tempdir.
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            config: minimal_server_config(repo_root),
        }
    }

    /// Overrides the auth token signing key.
    #[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
    pub fn token_key(mut self, token_key: [u8; 32]) -> Self {
        self.config.token_key = token_key;
        self
    }

    /// Overrides the configured OAuth providers.
    #[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
    pub fn oauth_providers(mut self, oauth_providers: Vec<OAuthProviderConfig>) -> Self {
        self.config.oauth_providers = oauth_providers;
        self
    }

    /// Overrides the maximum accepted account handle length.
    #[allow(dead_code)] // Live in sibling test binaries; each tests/*.rs compiles common as its own crate.
    pub fn max_handle_length(mut self, max_handle_length: usize) -> Self {
        self.config.max_handle_length = max_handle_length;
        self
    }

    /// Finishes the builder, returning the built `ServerConfig`.
    pub fn build(self) -> ServerConfig {
        self.config
    }
}
