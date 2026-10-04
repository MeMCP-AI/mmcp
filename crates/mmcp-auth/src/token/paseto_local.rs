//! PASETO v4 local seal and open of one serializable payload.
//!
//! The payload travels as a JSON string in the custom claim `claims`.
//! The token's own `exp` claim is the library default; each payload carries and checks its own expiry.

use rusty_paseto::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::AuthError;

/// Size of the PASETO v4 local key, in bytes.
pub const V4_LOCAL_KEY_BYTES: usize = 32;

/// Name of the custom claim carrying the JSON payload.
const PAYLOAD_CLAIM_KEY: &str = "claims";

/// A PASETO v4 local symmetric key.
pub(super) struct LocalKey {
    key: PasetoSymmetricKey<V4, Local>,
}

impl LocalKey {
    pub(super) fn from_raw(raw: &[u8; V4_LOCAL_KEY_BYTES]) -> Self {
        Self {
            key: PasetoSymmetricKey::<V4, Local>::from(Key::from(raw)),
        }
    }
}

/// Seal `payload` under `key`, optionally bound to `implicit_assertion`.
///
/// A token only opens with the same assertion it was sealed with, which separates token purposes sharing one key.
pub(super) fn seal<T: Serialize>(
    key: &LocalKey,
    payload: &T,
    implicit_assertion: Option<&str>,
) -> Result<String, AuthError> {
    let payload_json =
        serde_json::to_string(payload).map_err(|e| AuthError::Claims(e.to_string()))?;
    let mut builder = PasetoBuilder::<V4, Local>::default();
    builder.set_claim(
        CustomClaim::try_from((PAYLOAD_CLAIM_KEY, payload_json))
            .map_err(|e| AuthError::Token(e.to_string()))?,
    );
    if let Some(assertion) = implicit_assertion {
        builder.set_implicit_assertion(ImplicitAssertion::from(assertion));
    }
    builder
        .build(&key.key)
        .map_err(|e| AuthError::Token(e.to_string()))
}

/// Open `token` under `key` and `implicit_assertion`, returning the payload.
///
/// The payload's own expiry is the caller's to check.
pub(super) fn open<T: DeserializeOwned>(
    key: &LocalKey,
    token: &str,
    implicit_assertion: Option<&str>,
) -> Result<T, AuthError> {
    let mut parser = PasetoParser::<V4, Local>::default();
    if let Some(assertion) = implicit_assertion {
        parser.set_implicit_assertion(ImplicitAssertion::from(assertion));
    }
    let value = parser
        .parse(token, &key.key)
        .map_err(|e| AuthError::Token(e.to_string()))?;
    let payload_json = value
        .get(PAYLOAD_CLAIM_KEY)
        .and_then(|v| v.as_str())
        .ok_or_else(|| AuthError::Claims("missing claims field".into()))?;
    serde_json::from_str(payload_json).map_err(|e| AuthError::Claims(e.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn test_key() -> LocalKey {
        LocalKey::from_raw(&[7u8; V4_LOCAL_KEY_BYTES])
    }

    #[test]
    fn seal_then_open_round_trips_a_payload_without_an_assertion() {
        let key = test_key();
        let token = seal(&key, &vec![1u8, 2, 3], None).unwrap();
        let opened: Vec<u8> = open(&key, &token, None).unwrap();
        assert_eq!(opened, vec![1, 2, 3]);
    }

    #[test]
    fn seal_then_open_round_trips_a_payload_under_the_same_assertion() {
        let key = test_key();
        let token = seal(&key, &"payload", Some("purpose-a")).unwrap();
        let opened: String = open(&key, &token, Some("purpose-a")).unwrap();
        assert_eq!(opened, "payload");
    }

    #[test]
    fn a_token_does_not_open_under_a_different_or_missing_assertion() {
        let key = test_key();
        let bound = seal(&key, &"payload", Some("purpose-a")).unwrap();
        let unbound = seal(&key, &"payload", None).unwrap();

        assert!(matches!(
            open::<String>(&key, &bound, Some("purpose-b")),
            Err(AuthError::Token(_))
        ));
        assert!(matches!(
            open::<String>(&key, &bound, None),
            Err(AuthError::Token(_))
        ));
        assert!(matches!(
            open::<String>(&key, &unbound, Some("purpose-a")),
            Err(AuthError::Token(_))
        ));
    }

    #[test]
    fn a_payload_of_the_wrong_shape_is_a_claims_error() {
        let key = test_key();
        let token = seal(&key, &"a string payload", None).unwrap();
        assert!(matches!(
            open::<Vec<u8>>(&key, &token, None),
            Err(AuthError::Claims(_))
        ));
    }
}
