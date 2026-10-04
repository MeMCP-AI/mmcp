//! Server configuration loaded from environment variables.

mod auth_rate_limit;
mod auth_rate_limit_overrides;
mod cascade;
mod cors_origins;
mod defaults;
mod env_value;
mod error;
mod ipv6_network_tier_config;
mod oauth_provider;
mod overrides;
mod server_config;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod trusted_proxies;

pub use auth_rate_limit::AuthRateLimitConfig;
pub use auth_rate_limit_overrides::AuthRateLimitOverrides;
pub(crate) use defaults::IPV6_ADDRESS_BITS;
pub use defaults::{
    AUTH_RATE_LIMIT_BURST_ENV, AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH_ENV,
    AUTH_RATE_LIMIT_IPV6_NETWORK_BURST_ENV, AUTH_RATE_LIMIT_IPV6_NETWORK_PER_MINUTE_ENV,
    AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH_ENV, AUTH_RATE_LIMIT_PER_MINUTE_ENV,
    CORS_ALLOWED_ORIGINS_ENV, GITHUB_OAUTH_CLIENT_ID_ENV, GITHUB_OAUTH_CLIENT_SECRET_ENV,
    MAX_HANDLE_LENGTH_ENV, MAX_PASSWORD_LENGTH_ENV, MIN_PASSWORD_LENGTH_ENV, TRUSTED_PROXIES_ENV,
    WEBUI_DIR_ENV,
};
pub use error::ConfigError;
pub use ipv6_network_tier_config::Ipv6NetworkTierConfig;
pub use oauth_provider::OAuthProviderConfig;
pub use overrides::ServerConfigOverrides;
pub use server_config::{ServerConfig, generate_secret_hex};
pub use trusted_proxies::TrustedProxies;
