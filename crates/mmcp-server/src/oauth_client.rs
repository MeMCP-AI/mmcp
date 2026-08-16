//! Per-provider `oauth2` client construction.
//!
//! [`crate::state::ServerState::initialize`] builds one [`OauthClient`]
//! per configured [`OAuthProviderConfig`], replacing the hand-rolled
//! authorize-URL `format!`/`urlencoding::encode` and the hand-rolled
//! `reqwest::Client::new().post(&cfg.token_url).form(&[...])` token
//! exchange that used to live in `crate::routes::auth`. Everything
//! else in that module (session storage of the CSRF state and PKCE
//! verifier, the `OAuthStateRejection` cause taxonomy, the provider
//! registry, and the GitHub-specific userinfo fetch) stays hand-rolled:
//! `oauth2` has no opinion on any of it.

use anyhow::{Context, Result};
use oauth2::basic::BasicClient;
use oauth2::{AuthUrl, ClientId, ClientSecret, EndpointNotSet, EndpointSet, RedirectUrl, TokenUrl};

use crate::config::OAuthProviderConfig;

/// A [`BasicClient`] with both the authorize and token endpoints set
/// (`EndpointSet` in those two typestate slots): the shape every
/// provider's client reaches once [`build_oauth_client`] finishes
/// configuring it. Device-authorization, introspection, and
/// revocation stay `EndpointNotSet`; mmcp only ever drives the
/// Authorization Code flow.
pub type OauthClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

/// Build the OAuth2 client for one provider from its static config
/// plus this server's own origin, which determines the redirect URL
/// (`{origin}/auth/oauth/{slug}/callback`, matching the path
/// `crate::routes::auth::router` mounts the callback handler on).
///
/// Setting the redirect URL here, once, means neither
/// `oauth_authorize` nor `oauth_callback` has to rebuild it per
/// request: `oauth2` attaches it to both the authorize URL and the
/// token exchange automatically once it is set on the client.
pub fn build_oauth_client(cfg: &OAuthProviderConfig, origin: &str) -> Result<OauthClient> {
    let redirect_url = format!("{origin}/auth/oauth/{}/callback", cfg.slug);
    Ok(BasicClient::new(ClientId::new(cfg.client_id.clone()))
        .set_client_secret(ClientSecret::new(cfg.client_secret.clone()))
        // oauth2 defaults to `AuthType::BasicAuth` (client_id/secret
        // in an `Authorization: Basic` header) once a client secret
        // is set. The pre-migration hand-rolled exchange sent both
        // as form-body fields instead; `RequestBody` here keeps that
        // exact, already-proven-working wire format for every
        // configured provider rather than switching credential
        // transport as a side effect of this migration.
        .set_auth_type(oauth2::AuthType::RequestBody)
        .set_auth_uri(
            AuthUrl::new(cfg.auth_url.clone())
                .with_context(|| format!("provider '{}': invalid OAuth authorize URL", cfg.slug))?,
        )
        .set_token_uri(
            TokenUrl::new(cfg.token_url.clone())
                .with_context(|| format!("provider '{}': invalid OAuth token URL", cfg.slug))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_url)
                .with_context(|| format!("provider '{}': invalid OAuth redirect URL", cfg.slug))?,
        ))
}

/// Build the dedicated async HTTP client
/// [`crate::routes::auth::oauth_callback`] passes to
/// [`oauth2::CodeTokenRequest::request_async`] for the token
/// exchange.
///
/// `oauth2`'s `reqwest` feature implements its `AsyncHttpClient`
/// trait only for ITS OWN transitively pulled `reqwest` major
/// version (re-exported as [`oauth2::reqwest`]), which is a different
/// type from this workspace's own `reqwest` dependency (used
/// everywhere else in this crate, including this same handler's
/// GitHub-specific userinfo fetch): the two cannot be swapped for
/// each other, so the token exchange gets this client and every
/// other HTTP call in the crate keeps using the workspace's own.
///
/// `redirect(Policy::none())`: the token endpoint is not expected to
/// redirect, and blindly following one on a POST that carries the
/// client secret and authorization code would replay both to
/// whatever the response's `Location` pointed at.
pub fn build_oauth_exchange_http_client() -> oauth2::reqwest::Client {
    oauth2::reqwest::Client::builder()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .build()
        .expect(
            "reqwest::Client::builder().build() with no I/O and no proxy/TLS override only \
             fails on a broken builder configuration, which this call never supplies; \
             `reqwest::Client::new()` used elsewhere in this crate carries the same never-fails \
             assumption internally",
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OAuthProviderConfig;

    #[test]
    fn build_oauth_client_sets_every_endpoint_from_the_provider_config() {
        let cfg = OAuthProviderConfig::github("client-abc", "secret-xyz");
        let client = build_oauth_client(&cfg, "https://mmcp.example").expect("valid urls");

        assert_eq!(
            client.auth_uri().url().as_str(),
            "https://github.com/login/oauth/authorize"
        );
        assert_eq!(
            client.token_uri().url().as_str(),
            "https://github.com/login/oauth/access_token"
        );
    }

    #[test]
    fn build_oauth_client_rejects_a_malformed_endpoint_url() {
        let mut cfg = OAuthProviderConfig::github("client-abc", "secret-xyz");
        cfg.auth_url = "not a url".to_string();

        assert!(build_oauth_client(&cfg, "https://mmcp.example").is_err());
    }
}
