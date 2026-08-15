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

impl OAuthProviderConfig {
    /// Build the GitHub OAuth provider config: the only provider
    /// wired up today. `client_id` / `client_secret` come from
    /// `MMCP_OAUTH_GITHUB_CLIENT_ID` / `MMCP_OAUTH_GITHUB_CLIENT_SECRET`;
    /// the endpoint URLs are GitHub's own fixed OAuth endpoints,
    /// defined once here (via [`OAuthProviderConfigBuilder`]) instead
    /// of restated as a bare struct literal at each construction site.
    pub fn github(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        OAuthProviderConfigBuilder::new("github")
            .client_id(client_id)
            .client_secret(client_secret)
            .auth_url("https://github.com/login/oauth/authorize")
            .token_url("https://github.com/login/oauth/access_token")
            .userinfo_url("https://api.github.com/user")
            .build()
    }
}

/// Builder for [`OAuthProviderConfig`].
///
/// Every field is semantically required (there is no sensible default
/// for a provider's client id, secret, or endpoint URLs), so the
/// builder exists to name each of the six fields at its call site
/// instead of repeating a bare six-field struct literal, per the
/// project's builder-pattern convention for 2-3+ field structs.
/// Chained-setter style, matching `TestServerConfigBuilder`
/// (`tests/common/mod.rs`).
pub struct OAuthProviderConfigBuilder {
    slug: String,
    client_id: String,
    client_secret: String,
    auth_url: String,
    token_url: String,
    userinfo_url: String,
}

impl OAuthProviderConfigBuilder {
    /// Starts a builder for the given provider slug. Every other
    /// field starts empty and must be set via its own setter before
    /// [`build`](Self::build).
    pub fn new(slug: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            client_id: String::new(),
            client_secret: String::new(),
            auth_url: String::new(),
            token_url: String::new(),
            userinfo_url: String::new(),
        }
    }

    /// Sets the OAuth client id issued by the provider.
    pub fn client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = client_id.into();
        self
    }

    /// Sets the OAuth client secret issued by the provider.
    pub fn client_secret(mut self, client_secret: impl Into<String>) -> Self {
        self.client_secret = client_secret.into();
        self
    }

    /// Sets the provider's authorization endpoint URL.
    pub fn auth_url(mut self, auth_url: impl Into<String>) -> Self {
        self.auth_url = auth_url.into();
        self
    }

    /// Sets the provider's token exchange endpoint URL.
    pub fn token_url(mut self, token_url: impl Into<String>) -> Self {
        self.token_url = token_url.into();
        self
    }

    /// Sets the provider's user-info endpoint URL.
    pub fn userinfo_url(mut self, userinfo_url: impl Into<String>) -> Self {
        self.userinfo_url = userinfo_url.into();
        self
    }

    /// Finishes the builder, returning the built [`OAuthProviderConfig`].
    pub fn build(self) -> OAuthProviderConfig {
        OAuthProviderConfig {
            slug: self.slug,
            client_id: self.client_id,
            client_secret: self.client_secret,
            auth_url: self.auth_url,
            token_url: self.token_url,
            userinfo_url: self.userinfo_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_sets_every_field() {
        let cfg = OAuthProviderConfigBuilder::new("acme")
            .client_id("id-123")
            .client_secret("secret-456")
            .auth_url("https://acme.example/authorize")
            .token_url("https://acme.example/token")
            .userinfo_url("https://acme.example/userinfo")
            .build();

        assert_eq!(cfg.slug, "acme");
        assert_eq!(cfg.client_id, "id-123");
        assert_eq!(cfg.client_secret, "secret-456");
        assert_eq!(cfg.auth_url, "https://acme.example/authorize");
        assert_eq!(cfg.token_url, "https://acme.example/token");
        assert_eq!(cfg.userinfo_url, "https://acme.example/userinfo");
    }

    #[test]
    fn github_constructor_fills_the_fixed_github_endpoints() {
        let cfg = OAuthProviderConfig::github("id-abc", "secret-xyz");

        assert_eq!(cfg.slug, "github");
        assert_eq!(cfg.client_id, "id-abc");
        assert_eq!(cfg.client_secret, "secret-xyz");
        assert_eq!(cfg.auth_url, "https://github.com/login/oauth/authorize");
        assert_eq!(cfg.token_url, "https://github.com/login/oauth/access_token");
        assert_eq!(cfg.userinfo_url, "https://api.github.com/user");
    }
}
