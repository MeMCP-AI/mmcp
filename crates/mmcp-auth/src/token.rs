//! PASETO v4 local token issuer and verifier.
//!
//! We use local tokens (symmetric keyed authenticated encryption)
//! because mmcp always issues and verifies tokens on the same server
//! side. Public tokens (asymmetric) would only be necessary if an
//! external service had to verify tokens without holding the secret.

use rusty_paseto::prelude::*;

use crate::claims::SessionClaims;
use crate::error::AuthError;

/// Size of the PASETO v4 local key, in bytes.
pub const V4_LOCAL_KEY_BYTES: usize = 32;

/// Issues fresh PASETO v4 local tokens.
///
/// The issuer owns the key material. Callers that want to rotate
/// keys keep multiple issuers side by side and pick the current one
/// when issuing, while a pool of verifiers trusts all of them.
pub struct TokenIssuer {
    key: PasetoSymmetricKey<V4, Local>,
}

/// Verifies PASETO v4 local tokens against a single key.
pub struct TokenVerifier {
    key: PasetoSymmetricKey<V4, Local>,
}

fn load_key(raw: &[u8; V4_LOCAL_KEY_BYTES]) -> PasetoSymmetricKey<V4, Local> {
    PasetoSymmetricKey::<V4, Local>::from(Key::from(raw))
}

impl TokenIssuer {
    /// Build an issuer from a pre-existing 32-byte key.
    #[must_use]
    pub fn from_key(raw: &[u8; V4_LOCAL_KEY_BYTES]) -> Self {
        Self { key: load_key(raw) }
    }

    /// Issue a token carrying the provided claims.
    pub fn issue(&self, claims: &SessionClaims) -> Result<String, AuthError> {
        let claims_json = serde_json::to_string(claims)
            .map_err(|e| AuthError::Claims(e.to_string()))?;
        let token = PasetoBuilder::<V4, Local>::default()
            .set_claim(
                CustomClaim::try_from(("claims", claims_json))
                    .map_err(|e| AuthError::Token(e.to_string()))?,
            )
            .build(&self.key)
            .map_err(|e| AuthError::Token(e.to_string()))?;
        Ok(token)
    }
}

impl TokenVerifier {
    /// Build a verifier from a pre-existing 32-byte key.
    #[must_use]
    pub fn from_key(raw: &[u8; V4_LOCAL_KEY_BYTES]) -> Self {
        Self { key: load_key(raw) }
    }

    /// Verify a token and return the parsed claims. The wall clock
    /// `now_secs` is compared against the claims' `exp` field.
    pub fn verify(&self, token: &str, now_secs: i64) -> Result<SessionClaims, AuthError> {
        let value = PasetoParser::<V4, Local>::default()
            .parse(token, &self.key)
            .map_err(|e| AuthError::Token(e.to_string()))?;

        let claims_str = value
            .get("claims")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthError::Claims("missing claims field".into()))?;
        let claims: SessionClaims = serde_json::from_str(claims_str)
            .map_err(|e| AuthError::Claims(e.to_string()))?;

        if claims.exp <= now_secs {
            return Err(AuthError::Expired);
        }
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
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
