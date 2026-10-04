//! PASETO v4 local bearer token issuer and verifier.
//!
//! We use local tokens (symmetric keyed authenticated encryption)
//! because mmcp always issues and verifies tokens on the same server
//! side. Public tokens (asymmetric) would only be necessary if an
//! external service had to verify tokens without holding the secret.

use crate::claims::SessionClaims;
use crate::error::AuthError;
use crate::token::paseto_local::{self, LocalKey, V4_LOCAL_KEY_BYTES};

/// Issues fresh PASETO v4 local tokens.
///
/// The issuer owns the key material. Callers that want to rotate
/// keys keep multiple issuers side by side and pick the current one
/// when issuing, while a pool of verifiers trusts all of them.
pub struct TokenIssuer {
    key: LocalKey,
}

/// Verifies PASETO v4 local tokens against a single key.
pub struct TokenVerifier {
    key: LocalKey,
}

impl TokenIssuer {
    /// Build an issuer from a pre-existing 32-byte key.
    #[must_use]
    pub fn from_key(raw: &[u8; V4_LOCAL_KEY_BYTES]) -> Self {
        Self {
            key: LocalKey::from_raw(raw),
        }
    }

    /// Issue a token carrying the provided claims.
    pub fn issue(&self, claims: &SessionClaims) -> Result<String, AuthError> {
        paseto_local::seal(&self.key, claims, None)
    }
}

impl TokenVerifier {
    /// Build a verifier from a pre-existing 32-byte key.
    #[must_use]
    pub fn from_key(raw: &[u8; V4_LOCAL_KEY_BYTES]) -> Self {
        Self {
            key: LocalKey::from_raw(raw),
        }
    }

    /// Verify a token and return the parsed claims. The wall clock
    /// `now_secs` is compared against the claims' `exp` field.
    pub fn verify(&self, token: &str, now_secs: i64) -> Result<SessionClaims, AuthError> {
        let claims: SessionClaims = paseto_local::open(&self.key, token, None)?;
        if claims.exp <= now_secs {
            return Err(AuthError::Expired);
        }
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use uuid::Uuid;

    fn test_key() -> [u8; V4_LOCAL_KEY_BYTES] {
        let mut key = [0u8; V4_LOCAL_KEY_BYTES];
        for (i, slot) in key.iter_mut().enumerate() {
            *slot = i as u8;
        }
        key
    }

    #[test]
    fn issue_and_verify_round_trip() {
        let key = test_key();
        let issuer = TokenIssuer::from_key(&key);
        let verifier = TokenVerifier::from_key(&key);

        let claims = SessionClaims::new_with_lifetime(Uuid::now_v7(), Uuid::now_v7(), 1_000, 600);
        let token = issuer.issue(&claims).unwrap();
        let verified = verifier.verify(&token, 1_100).unwrap();
        assert_eq!(verified.sub, claims.sub);
        assert_eq!(verified.jti, claims.jti);
    }

    #[test]
    fn expired_token_is_rejected() {
        let key = test_key();
        let issuer = TokenIssuer::from_key(&key);
        let verifier = TokenVerifier::from_key(&key);

        let claims = SessionClaims::new_with_lifetime(Uuid::now_v7(), Uuid::now_v7(), 1_000, 10);
        let token = issuer.issue(&claims).unwrap();
        let err = verifier.verify(&token, 2_000).unwrap_err();
        assert!(matches!(err, AuthError::Expired));
    }

    #[test]
    fn wrong_key_is_rejected() {
        let issuer = TokenIssuer::from_key(&test_key());
        let claims = SessionClaims::new_with_lifetime(Uuid::now_v7(), Uuid::now_v7(), 1_000, 600);
        let token = issuer.issue(&claims).unwrap();

        let mut bad = test_key();
        bad[0] ^= 0xFF;
        let verifier = TokenVerifier::from_key(&bad);
        let err = verifier.verify(&token, 1_100).unwrap_err();
        assert!(matches!(err, AuthError::Token(_)));
    }
}
