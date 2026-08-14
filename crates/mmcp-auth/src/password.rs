//! Argon2id password hashing, verification, and policy validation.

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use crate::error::AuthError;

/// Minimum accepted password length, in bytes.
/// NIST SP 800-63B sets 8 as the minimum a verifier must enforce;
/// no shorter project convention exists to defer to.
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// Maximum accepted password length, in bytes.
/// Argon2 has no practical length cap of its own, so an unbounded
/// password is a memory/CPU amplification vector at the hashing
/// step; 256 stays far above any real passphrase while remaining a
/// concrete, auditable bound, matching the maxima this project
/// already uses for other external string fields
/// (`mmcp_core::memory::limits`).
pub const MAX_PASSWORD_LENGTH: usize = 256;

/// Validate `plaintext` against the password policy: rejects an
/// empty or whitespace-only password (checked via `trim`, so a
/// password of spaces alone is rejected regardless of its raw
/// length) and enforces [`MIN_PASSWORD_LENGTH`] /
/// [`MAX_PASSWORD_LENGTH`].
///
/// Callers run this before [`hash_password`]: a rejected password
/// must never be hashed or persisted.
pub fn validate_password_policy(plaintext: &str) -> Result<(), AuthError> {
    if plaintext.trim().is_empty() {
        return Err(AuthError::PasswordTooShort {
            min: MIN_PASSWORD_LENGTH,
            actual: 0,
        });
    }
    let actual = plaintext.len();
    if actual < MIN_PASSWORD_LENGTH {
        return Err(AuthError::PasswordTooShort {
            min: MIN_PASSWORD_LENGTH,
            actual,
        });
    }
    if actual > MAX_PASSWORD_LENGTH {
        return Err(AuthError::PasswordTooLong {
            max: MAX_PASSWORD_LENGTH,
            actual,
        });
    }
    Ok(())
}

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

    #[test]
    fn validate_password_policy_rejects_empty() {
        let err = validate_password_policy("").unwrap_err();
        match err {
            AuthError::PasswordTooShort { min, actual } => {
                assert_eq!(min, MIN_PASSWORD_LENGTH);
                assert_eq!(actual, 0);
            }
            other => panic!("expected PasswordTooShort, got {other:?}"),
        }
    }

    #[test]
    fn validate_password_policy_rejects_whitespace_only() {
        // 8 raw bytes, but blank after trim: must still be rejected,
        // proving the check runs on the trimmed value first.
        let err = validate_password_policy("        ").unwrap_err();
        match err {
            AuthError::PasswordTooShort { min, actual } => {
                assert_eq!(min, MIN_PASSWORD_LENGTH);
                assert_eq!(actual, 0);
            }
            other => panic!("expected PasswordTooShort, got {other:?}"),
        }
    }

    #[test]
    fn validate_password_policy_rejects_too_short() {
        let err = validate_password_policy("short").unwrap_err();
        match err {
            AuthError::PasswordTooShort { min, actual } => {
                assert_eq!(min, MIN_PASSWORD_LENGTH);
                assert_eq!(actual, "short".len());
            }
            other => panic!("expected PasswordTooShort, got {other:?}"),
        }
    }

    #[test]
    fn validate_password_policy_rejects_too_long() {
        let too_long = "a".repeat(MAX_PASSWORD_LENGTH + 1);
        let err = validate_password_policy(&too_long).unwrap_err();
        match err {
            AuthError::PasswordTooLong { max, actual } => {
                assert_eq!(max, MAX_PASSWORD_LENGTH);
                assert_eq!(actual, MAX_PASSWORD_LENGTH + 1);
            }
            other => panic!("expected PasswordTooLong, got {other:?}"),
        }
    }

    #[test]
    fn validate_password_policy_accepts_at_bounds_and_valid() {
        assert!(validate_password_policy(&"a".repeat(MIN_PASSWORD_LENGTH)).is_ok());
        assert!(validate_password_policy(&"a".repeat(MAX_PASSWORD_LENGTH)).is_ok());
        assert!(validate_password_policy("correct horse battery staple").is_ok());
    }
}
