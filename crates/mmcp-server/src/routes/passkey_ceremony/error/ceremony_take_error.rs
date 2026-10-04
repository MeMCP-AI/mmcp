//! Why taking the pending ceremony out of the session failed.

use thiserror::Error;
use tower_sessions::session;

use crate::session_store::DatabaseSessionStoreError;

/// A ceremony could not be taken out of the session store.
///
/// These are server faults, not refusals of the caller.
#[derive(Debug, Error)]
pub(crate) enum CeremonyTakeError {
    /// The request's session record could not be read.
    #[error("session record could not be read")]
    Session(#[from] session::Error),

    /// The session store could not remove the stored ceremony.
    #[error("stored ceremony could not be removed from the session store")]
    Store(#[from] DatabaseSessionStoreError),

    /// The stored ceremony does not decode as the ceremony kind the route expects.
    #[error("stored ceremony could not be decoded")]
    Malformed(#[source] serde_json::Error),
}
