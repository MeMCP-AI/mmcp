//! Single-use removal of the pending ceremony from the caller's session.

use serde::de::DeserializeOwned;
use tower_sessions::Session;

use crate::routes::passkey_ceremony::PendingCeremony;
use crate::routes::passkey_ceremony::error::CeremonyTakeError;
use crate::session_store::DatabaseSessionStore;

/// Take the ceremony stored under `key` out of the session store.
///
/// The removal is written to the session store at once and atomically, whatever the response turns out to be.
/// Concurrent finishes on one session cookie receive the ceremony once between them.
/// Each of them loaded a session record that still holds it, but only one removal succeeds in the store.
/// A concurrent request that modifies the same session can still write its own loaded record back afterwards.
/// `None` means the session holds no such ceremony, which includes a request without a session.
pub(crate) async fn take_pending_ceremony<T: DeserializeOwned>(
    store: &DatabaseSessionStore,
    session: &Session,
    key: &str,
) -> Result<Option<PendingCeremony<T>>, CeremonyTakeError> {
    // The request's own copy loses the value too, so the save at the end of the request cannot write it back.
    session.remove_value(key).await?;
    let Some(id) = session.id() else {
        return Ok(None);
    };
    let Some(value) = store.take_value(&id, key).await? else {
        return Ok(None);
    };
    serde_json::from_value(value)
        .map(Some)
        .map_err(CeremonyTakeError::Malformed)
}
