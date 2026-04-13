//! End user account.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::id::UserId;

/// An mmcp end user account.
///
/// Holds only the identifying and display fields shared across the
/// system. Authentication state (password hashes, OAuth links, passkey
/// credentials) lives in `mmcp-auth` and `mmcp-db`, never in this
/// crate, because `mmcp-core` is I/O-free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    /// Stable identifier, generated at account creation.
    pub id: UserId,

    /// Unique handle used for login and URL paths. Case-insensitive on
    /// the server side; stored in the canonical lower-case form here.
    pub handle: String,

    /// Optional human display name.
    pub display_name: Option<String>,

    /// When the account was first created.
    pub created_at: Timestamp,
}
