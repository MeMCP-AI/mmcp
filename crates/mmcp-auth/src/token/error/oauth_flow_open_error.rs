//! Error type of opening an OAuth flow token.

use thiserror::Error;

use crate::error::AuthError;

/// Why an OAuth flow token was refused at the callback.
///
/// Neither the CSRF state nor the PKCE verifier appears in any variant.
#[derive(Debug, Error)]
pub enum OauthFlowOpenError {
    /// The token is malformed, tampered with, sealed under another key, or sealed for another purpose.
    #[error("failed to open the OAuth flow token")]
    Unreadable {
        #[source]
        source: AuthError,
    },

    /// The token opened but its expiry is at or before the check instant.
    #[error("OAuth flow token expired at {expires_at}, checked at {now}")]
    Expired {
        /// Expiry carried by the token, Unix seconds.
        expires_at: i64,
        /// Check instant, Unix seconds.
        now: i64,
    },

    /// The token opened but was issued for another provider than the callback's.
    #[error(
        "OAuth flow token was issued for provider '{token_provider}', not '{callback_provider}'"
    )]
    ProviderMismatch {
        token_provider: String,
        callback_provider: String,
    },
}
