//! OAuth flow token: the CSRF state and PKCE verifier of one pending authorization, sealed for a cookie.
//!
//! Bound to its own implicit assertion, a flow token never opens as a bearer token and the reverse.
//! Holding the pending flow in a token keeps the server from writing a record for an anonymous authorize request.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::defaults::OAUTH_FLOW_TOKEN_IMPLICIT_ASSERTION;
use crate::token::error::OauthFlowTokenError;
use crate::token::paseto_local::{self, LocalKey, V4_LOCAL_KEY_BYTES};

/// Claims of one pending OAuth authorization.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OauthFlowClaims {
    /// Slug of the provider the authorization was started against.
    pub provider: String,

    /// CSRF `state` value sent to the provider and echoed back at the callback.
    pub csrf_state: String,

    /// PKCE code verifier presented at the token exchange.
    pub pkce_verifier: String,

    /// Expiry instant, Unix seconds.
    pub expires_at: i64,
}

impl fmt::Debug for OauthFlowClaims {
    /// The CSRF state and PKCE verifier are secrets and never print.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OauthFlowClaims")
            .field("provider", &self.provider)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Seals and opens OAuth flow tokens under the server's token key.
pub struct OauthFlowTokenCodec {
    key: LocalKey,
}

impl OauthFlowTokenCodec {
    /// Build a codec from a pre-existing 32-byte key.
    #[must_use]
    pub fn from_key(raw: &[u8; V4_LOCAL_KEY_BYTES]) -> Self {
        Self {
            key: LocalKey::from_raw(raw),
        }
    }

    /// Seal `claims` into a token.
    pub fn seal(&self, claims: &OauthFlowClaims) -> Result<String, OauthFlowTokenError> {
        paseto_local::seal(&self.key, claims, Some(OAUTH_FLOW_TOKEN_IMPLICIT_ASSERTION))
            .map_err(|source| OauthFlowTokenError::Seal { source })
    }

    /// Open `token` and return its claims.
    ///
    /// Refuses a token whose expiry is at or before `now_secs`.
    /// Refuses a token issued for a provider other than `expected_provider`.
    pub fn open(
        &self,
        token: &str,
        expected_provider: &str,
        now_secs: i64,
    ) -> Result<OauthFlowClaims, OauthFlowTokenError> {
        let claims: OauthFlowClaims =
            paseto_local::open(&self.key, token, Some(OAUTH_FLOW_TOKEN_IMPLICIT_ASSERTION))
                .map_err(|source| OauthFlowTokenError::Open { source })?;
        if claims.expires_at <= now_secs {
            return Err(OauthFlowTokenError::Expired {
                expires_at: claims.expires_at,
                now: now_secs,
            });
        }
        if claims.provider != expected_provider {
            return Err(OauthFlowTokenError::ProviderMismatch {
                token_provider: claims.provider,
                callback_provider: expected_provider.to_owned(),
            });
        }
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::claims::SessionClaims;
    use crate::error::AuthError;
    use crate::token::{TokenIssuer, TokenVerifier};
    use uuid::Uuid;

    const NOW_SECS: i64 = 1_000;
    const LIFETIME_SECS: i64 = 600;

    fn test_key() -> [u8; V4_LOCAL_KEY_BYTES] {
        [9u8; V4_LOCAL_KEY_BYTES]
    }

    fn claims_for(provider: &str) -> OauthFlowClaims {
        OauthFlowClaims {
            provider: provider.to_owned(),
            csrf_state: "csrf-state-value".to_owned(),
            pkce_verifier: "pkce-verifier-value".to_owned(),
            expires_at: NOW_SECS + LIFETIME_SECS,
        }
    }

    #[test]
    fn oauth_flow_token_round_trips_and_rejects_expiry_tampering_and_provider_mismatch() {
        let codec = OauthFlowTokenCodec::from_key(&test_key());
        let claims = claims_for("github");
        let token = codec.seal(&claims).unwrap();

        assert_eq!(codec.open(&token, "github", NOW_SECS).unwrap(), claims);

        assert!(matches!(
            codec.open(&token, "github", claims.expires_at),
            Err(OauthFlowTokenError::Expired { .. })
        ));

        let mut tampered = token.clone().into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(tampered).unwrap();
        assert!(matches!(
            codec.open(&tampered, "github", NOW_SECS),
            Err(OauthFlowTokenError::Open { .. })
        ));

        assert!(matches!(
            codec.open(&token, "google", NOW_SECS),
            Err(OauthFlowTokenError::ProviderMismatch { .. })
        ));
    }

    #[test]
    fn oauth_flow_token_and_bearer_token_do_not_open_as_each_other() {
        let key = test_key();
        let codec = OauthFlowTokenCodec::from_key(&key);
        let issuer = TokenIssuer::from_key(&key);
        let verifier = TokenVerifier::from_key(&key);

        // A decryption refusal is `AuthError::Token`; a payload of the wrong shape would be `AuthError::Claims`.
        let flow_token = codec.seal(&claims_for("github")).unwrap();
        assert!(
            matches!(
                verifier.verify(&flow_token, NOW_SECS),
                Err(AuthError::Token(_))
            ),
            "a flow token must not decrypt as a bearer token"
        );

        let bearer_claims = SessionClaims::new_with_lifetime(
            Uuid::now_v7(),
            Uuid::now_v7(),
            NOW_SECS,
            LIFETIME_SECS,
        );
        let bearer_token = issuer.issue(&bearer_claims).unwrap();
        assert!(
            matches!(
                codec.open(&bearer_token, "github", NOW_SECS),
                Err(OauthFlowTokenError::Open {
                    source: AuthError::Token(_)
                })
            ),
            "a bearer token must not decrypt as a flow token"
        );
    }

    #[test]
    fn oauth_flow_token_does_not_open_under_another_key() {
        let token = OauthFlowTokenCodec::from_key(&test_key())
            .seal(&claims_for("github"))
            .unwrap();
        let mut other_key = test_key();
        other_key[0] ^= 0xFF;
        assert!(matches!(
            OauthFlowTokenCodec::from_key(&other_key).open(&token, "github", NOW_SECS),
            Err(OauthFlowTokenError::Open { .. })
        ));
    }

    #[test]
    fn oauth_flow_claims_debug_output_hides_the_secrets() {
        let rendered = format!("{:?}", claims_for("github"));
        assert!(rendered.contains("github"));
        assert!(!rendered.contains("csrf-state-value"));
        assert!(!rendered.contains("pkce-verifier-value"));
    }
}
