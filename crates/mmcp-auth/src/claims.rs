//! Claims serialized inside a session token.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Typed claims carried inside a session token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionClaims {
    /// Subject user identifier.
    pub sub: Uuid,

    /// Unique token identifier. Lets the server revoke individual
    /// tokens without invalidating the whole user.
    pub jti: Uuid,

    /// Expiration time as Unix seconds.
    pub exp: i64,

    /// Issued-at time as Unix seconds.
    pub iat: i64,
}

impl SessionClaims {
    /// Convenience constructor computing the expiration from the
    /// current time plus a lifetime in seconds.
    #[must_use]
    pub fn new_with_lifetime(user: Uuid, token_id: Uuid, now: i64, lifetime_secs: i64) -> Self {
        Self {
            sub: user,
            jti: token_id,
            iat: now,
            exp: now + lifetime_secs,
        }
    }
}
