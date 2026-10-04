//! Error type of the database-backed session store.

use mmcp_db::DbError;
use thiserror::Error;
use time::OffsetDateTime;
use time::error::ComponentRange;
use tower_sessions::session_store;

/// Failures of [`crate::session_store::DatabaseSessionStore`].
///
/// No variant carries a session id or its digest: the id is a bearer secret.
#[derive(Debug, Error)]
pub enum DatabaseSessionStoreError {
    /// A query against the `http_sessions` table failed.
    #[error("session store query failed")]
    Database(#[from] DbError),

    /// The session data map could not be serialized to JSON.
    #[error("session data could not be encoded")]
    Encode(#[source] serde_json::Error),

    /// The stored session data is not a valid JSON map.
    #[error("stored session data could not be decoded")]
    Decode(#[source] serde_json::Error),

    /// The expiry instant does not fit the stored epoch milliseconds.
    #[error("session expiry {expiry_date} is outside the representable millisecond range")]
    ExpiryOutOfRange { expiry_date: OffsetDateTime },

    /// The stored expiry milliseconds are not a representable instant.
    #[error("stored session expiry of {expires_at} milliseconds is not a representable instant")]
    StoredExpiryOutOfRange {
        expires_at: i64,
        #[source]
        source: ComponentRange,
    },

    /// Every fresh session id drawn by a `create` collided with an existing row.
    #[error("no free session id found after {attempts} attempts")]
    IdCollisionRetriesExhausted { attempts: u32 },
}

impl From<DatabaseSessionStoreError> for session_store::Error {
    /// The trait's error type carries a string, so only the top-level text crosses.
    /// The store logs the source chain.
    fn from(error: DatabaseSessionStoreError) -> Self {
        let text = error.to_string();
        match error {
            DatabaseSessionStoreError::Encode(_) => Self::Encode(text),
            DatabaseSessionStoreError::Decode(_) => Self::Decode(text),
            DatabaseSessionStoreError::Database(_)
            | DatabaseSessionStoreError::ExpiryOutOfRange { .. }
            | DatabaseSessionStoreError::StoredExpiryOutOfRange { .. }
            | DatabaseSessionStoreError::IdCollisionRetriesExhausted { .. } => Self::Backend(text),
        }
    }
}
