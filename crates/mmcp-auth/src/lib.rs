//! Authentication and authorization primitives for mmcp.
//!
//! Exposes three cohesive pieces:
//!
//! - Argon2 password hashing through [`password`].
//! - PASETO v4 local session tokens through [`token`].
//! - Typed session claims through [`claims`].
//!
//! Middleware glue (axum-login, OAuth flows, passkey ceremonies)
//! lives in `mmcp-server` so this crate stays independent of any HTTP
//! framework and can be unit-tested in isolation.

pub mod claims;
pub mod error;
pub mod password;
pub mod token;

pub use claims::SessionClaims;
pub use error::AuthError;
pub use password::{hash_password, verify_password};
pub use token::{TokenIssuer, TokenVerifier};
