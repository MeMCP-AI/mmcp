//! Authentication and authorization primitives for mmcp.
//!
//! Exposes four cohesive pieces:
//!
//! - Argon2 password hashing through [`password`].
//! - PASETO v4 local session tokens through [`token`].
//! - Typed session claims through [`claims`].
//! - `axum-login` backend adapter through [`backend`].

pub mod backend;
pub mod claims;
mod defaults;
pub mod error;
pub mod password;
pub mod token;

pub use backend::{AuthSession, Credentials, MmcpAuthBackend, MmcpUser};
pub use claims::SessionClaims;
pub use defaults::{MAX_HANDLE_LENGTH, MIN_VIABLE_MAX_HANDLE_LENGTH};
pub use error::AuthError;
pub use password::{
    MAX_PASSWORD_LENGTH, MIN_PASSWORD_LENGTH, hash_password, validate_password_policy,
    verify_password,
};
pub use token::{TokenIssuer, TokenVerifier};
