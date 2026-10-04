//! Error type of sealing an OAuth flow token.

use thiserror::Error;

use crate::error::AuthError;

/// The claims of an OAuth flow could not be sealed into a token.
///
/// Neither the CSRF state nor the PKCE verifier appears in the error.
#[derive(Debug, Error)]
#[error("failed to seal the OAuth flow token")]
pub struct OauthFlowSealError {
    #[source]
    pub source: AuthError,
}
