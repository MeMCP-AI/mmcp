//! Authentication and authorization primitives for mmcp.
//!
//! Exposes five cohesive pieces, listed below.
//!
//! - Argon2 password hashing through [`password`].
//! - PASETO v4 local bearer tokens and OAuth flow tokens through [`token`].
//! - Typed session claims through [`claims`].
//! - `axum-login` backend adapter through [`backend`].
//! - The session auth hash a credential change invalidates through [`session_auth_hash`].

pub mod backend;
pub mod claims;
mod defaults;
pub mod error;
pub mod password;
pub mod session_auth_hash;
pub mod token;

pub use backend::{AuthSession, Credentials, MmcpAuthBackend, MmcpUser};
pub use claims::SessionClaims;
pub use defaults::{MAX_HANDLE_LENGTH, MIN_VIABLE_MAX_HANDLE_LENGTH};
pub use error::AuthError;
pub use password::{
    MAX_PASSWORD_LENGTH, MIN_PASSWORD_LENGTH, hash_password, validate_password_policy,
    verify_password,
};
pub use token::{
    OauthFlowClaims, OauthFlowOpenError, OauthFlowSealError, OauthFlowTokenCodec, TokenIssuer,
    TokenVerifier,
};
