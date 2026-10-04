//! Server configuration loaded from environment variables.

mod cascade;
mod database_url_redaction;
mod defaults;
mod env_value;
mod error;
mod oauth_provider;
mod overrides;
mod server_config;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use defaults::{
    MAX_HANDLE_LENGTH_ENV, MAX_PASSWORD_LENGTH_ENV, MIN_PASSWORD_LENGTH_ENV, WEBUI_DIR_ENV,
};
pub use error::ConfigError;
pub use oauth_provider::OAuthProviderConfig;
pub use overrides::ServerConfigOverrides;
pub use server_config::{ServerConfig, generate_secret_hex};
