//! [`OAuthProviderConfig`], configuration for a single OAuth provider.

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
