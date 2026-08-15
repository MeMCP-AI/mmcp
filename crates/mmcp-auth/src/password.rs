//! Argon2id password hashing, verification, and policy validation.

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use crate::error::AuthError;

/// Compiled-in default minimum accepted password length, in bytes.
/// NIST SP 800-63B sets 8 as the minimum a verifier must enforce;
/// no shorter project convention exists to defer to.
///
/// This is the LOWEST-precedence tier only: callers resolve the
/// effective bound through the config cascade
/// (`mmcp_server::config::ServerConfig::from_source_with_overrides`)
/// and pass it into [`validate_password_policy`] as `min_len`; the
/// function itself never reads this constant directly.
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// Compiled-in default maximum accepted password length, in bytes.
/// Argon2 has no practical length cap of its own, so an unbounded
/// password is a memory/CPU amplification vector at the hashing
/// step; 256 stays far above any real passphrase while remaining a
/// concrete, auditable bound, matching the maxima this project
/// already uses for other external string fields
/// (`mmcp_core::memory::limits`).
///
/// This is the LOWEST-precedence tier only; see
/// [`MIN_PASSWORD_LENGTH`]'s doc comment for the resolution cascade.
pub const MAX_PASSWORD_LENGTH: usize = 256;

/// Validate `plaintext` against the password policy: rejects a truly
/// empty password ([`AuthError::PasswordBlank`], zero bytes), then
/// enforces the caller-supplied `min_len` / `max_len` bounds, in
/// bytes, on every other byte sequence with no distinction by
/// content, whitespace included.
///
/// `min_len` and `max_len` are resolved by the caller through the
/// config cascade (env/config-file/CLI-override, falling back to
/// [`MIN_PASSWORD_LENGTH`] / [`MAX_PASSWORD_LENGTH`]); this function
/// only enforces whatever it is given.
///
/// Callers run this before [`hash_password`]: a rejected password
/// must never be hashed or persisted.
pub fn validate_password_policy(
    plaintext: &str,
    min_len: usize,
    max_len: usize,
) -> Result<(), AuthError> {
    if plaintext.is_empty() {
        return Err(AuthError::PasswordBlank);
    }
    let actual = plaintext.len();
    if actual < min_len {
        return Err(AuthError::PasswordTooShort {
            min: min_len,
            actual,
        });
    }
    if actual > max_len {
        return Err(AuthError::PasswordTooLong {
            max: max_len,
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
        let err =
            validate_password_policy("", MIN_PASSWORD_LENGTH, MAX_PASSWORD_LENGTH).unwrap_err();
        assert!(matches!(err, AuthError::PasswordBlank));
    }

    #[test]
    fn validate_password_policy_accepts_whitespace_only_at_min_length() {
        // A password of only spaces, at exactly MIN_PASSWORD_LENGTH,
        // is accepted like any other content: whitespace bytes carry
        // no special rejection.
        assert!(
            validate_password_policy(
                &" ".repeat(MIN_PASSWORD_LENGTH),
                MIN_PASSWORD_LENGTH,
                MAX_PASSWORD_LENGTH
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_password_policy_rejects_whitespace_only_below_min_length_as_too_short() {
        // A whitespace-only password shorter than min_len is rejected
        // by the ordinary length check, not by any whitespace-specific
        // path.
        let err =
            validate_password_policy("   ", MIN_PASSWORD_LENGTH, MAX_PASSWORD_LENGTH).unwrap_err();
        match err {
            AuthError::PasswordTooShort { min, actual } => {
                assert_eq!(min, MIN_PASSWORD_LENGTH);
                assert_eq!(actual, 3);
            }
            other => panic!("expected PasswordTooShort, got {other:?}"),
        }
    }

    #[test]
    fn validate_password_policy_rejects_too_short() {
        let err = validate_password_policy("short", MIN_PASSWORD_LENGTH, MAX_PASSWORD_LENGTH)
            .unwrap_err();
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
        let err = validate_password_policy(&too_long, MIN_PASSWORD_LENGTH, MAX_PASSWORD_LENGTH)
            .unwrap_err();
        match err {
            AuthError::PasswordTooLong { max, actual } => {
                assert_eq!(max, MAX_PASSWORD_LENGTH);
                assert_eq!(actual, MAX_PASSWORD_LENGTH + 1);
            }
            other => panic!("expected PasswordTooLong, got {other:?}"),
        }
    }

    #[test]
    fn validate_password_policy_honors_caller_supplied_bounds_over_the_defaults() {
        // A caller-resolved bound narrower than the compiled-in
        // default constants must be what's actually enforced,
        // proving the function reads its parameters, not the
        // module-level constants, for the policy check itself.
        let err = validate_password_policy("short", 10, MAX_PASSWORD_LENGTH).unwrap_err();
        match err {
            AuthError::PasswordTooShort { min, actual } => {
                assert_eq!(min, 10);
                assert_eq!(actual, "short".len());
            }
            other => panic!("expected PasswordTooShort, got {other:?}"),
        }
        assert!(validate_password_policy("short", 3, MAX_PASSWORD_LENGTH).is_ok());
    }

    #[test]
    fn validate_password_policy_accepts_at_bounds_and_valid() {
        assert!(
            validate_password_policy(
                &"a".repeat(MIN_PASSWORD_LENGTH),
                MIN_PASSWORD_LENGTH,
                MAX_PASSWORD_LENGTH
            )
            .is_ok()
        );
        assert!(
            validate_password_policy(
                &"a".repeat(MAX_PASSWORD_LENGTH),
                MIN_PASSWORD_LENGTH,
                MAX_PASSWORD_LENGTH
            )
            .is_ok()
        );
        assert!(
            validate_password_policy(
                "correct horse battery staple",
                MIN_PASSWORD_LENGTH,
                MAX_PASSWORD_LENGTH
            )
            .is_ok()
        );
    }
}
