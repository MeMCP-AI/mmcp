//! Argon2id password hashing and verification.

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use crate::error::AuthError;

/// Hash `plaintext` with Argon2id using a random salt and default
/// parameters.
pub fn hash_password(plaintext: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    argon
        .hash_password(plaintext.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Password(e.to_string()))
}

/// Verify `plaintext` against a stored PHC-format hash.
///
/// Returns `Ok(())` if the password matches, and
/// [`AuthError::InvalidCredentials`] if it does not. Any parse or
/// backend failure is returned as [`AuthError::Password`].
pub fn verify_password(plaintext: &str, stored_hash: &str) -> Result<(), AuthError> {
    let parsed = PasswordHash::new(stored_hash).map_err(|e| AuthError::Password(e.to_string()))?;
    match Argon2::default().verify_password(plaintext.as_bytes(), &parsed) {
        Ok(()) => Ok(()),
        Err(argon2::password_hash::Error::Password) => Err(AuthError::InvalidCredentials),
        Err(e) => Err(AuthError::Password(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_round_trip() {
        let hash = hash_password("correct horse battery staple").unwrap();
        verify_password("correct horse battery staple", &hash).unwrap();
    }

    #[test]
    fn wrong_password_is_rejected() {
        let hash = hash_password("alpha").unwrap();
        let err = verify_password("beta", &hash).unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    #[test]
    fn each_hash_has_a_unique_salt() {
        let a = hash_password("same").unwrap();
        let b = hash_password("same").unwrap();
        assert_ne!(a, b);
    }
}
