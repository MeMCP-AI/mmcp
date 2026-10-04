//! The session auth hash: the digest `axum-login` compares on every request.
//!
//! A session stores the digest taken at login, and a request whose user digests differently signs out.
//! The digest covers the user's credential epoch and password hash, so a credential change signs the user out elsewhere.
//!
//! The password hash stays in the input.
//! A password hash rewritten without a password change, such as a rehash under new parameters, therefore signs the user out as well.
//! That is the fail-safe direction and the accepted residual.

use sha2::{Digest, Sha256};

use crate::defaults::SESSION_AUTH_HASH_DOMAIN;

/// Digest of the credential state a session was created under.
///
/// The input is the domain tag, the epoch as 8 big-endian bytes, one presence byte for the password hash, then its bytes.
/// The presence byte keeps a user without a password apart from a user whose password hash is empty.
/// The epoch has a fixed width, so no two distinct inputs share an encoding.
/// Only the digest reaches a persisted session, never the password hash.
pub fn session_auth_hash(credential_epoch: i64, password_hash: Option<&str>) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SESSION_AUTH_HASH_DOMAIN.as_bytes());
    digest.update(credential_epoch.to_be_bytes());
    match password_hash {
        Some(hash) => {
            digest.update([1u8]);
            digest.update(hash.as_bytes());
        }
        None => digest.update([0u8]),
    }
    digest.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHC_STRING: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2g";
    const CHANGED_PHC_STRING: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$b3RoZXJoYXNo";

    #[test]
    fn passwordless_hash_changes_when_epoch_changes() {
        assert_ne!(
            session_auth_hash(0, None),
            session_auth_hash(1, None),
            "a credential change of a passwordless user must sign its other sessions out"
        );
    }

    #[test]
    fn hash_changes_when_password_hash_changes_at_one_epoch() {
        assert_ne!(
            session_auth_hash(3, Some(PHC_STRING)),
            session_auth_hash(3, Some(CHANGED_PHC_STRING)),
        );
    }

    #[test]
    fn hash_changes_when_epoch_changes_at_one_password_hash() {
        assert_ne!(
            session_auth_hash(3, Some(PHC_STRING)),
            session_auth_hash(4, Some(PHC_STRING)),
        );
    }

    #[test]
    fn hash_is_stable_for_unchanged_credentials() {
        assert_eq!(
            session_auth_hash(7, Some(PHC_STRING)),
            session_auth_hash(7, Some(PHC_STRING))
        );
        assert_eq!(session_auth_hash(7, None), session_auth_hash(7, None));
    }

    #[test]
    fn passwordless_and_password_inputs_never_produce_one_hash() {
        assert_ne!(session_auth_hash(0, None), session_auth_hash(0, Some("")));
        assert_ne!(
            session_auth_hash(0, None),
            session_auth_hash(0, Some("no-password"))
        );
    }
}
