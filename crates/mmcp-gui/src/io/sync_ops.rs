//! Thin typed wrappers over `mmcp_sync::SyncEngine`.
//!
//! The engine's own errors are `SyncError`; we flatten them into
//! `GuiError::Other` with the rendered message because the GUI's
//! toast / status-bar paths only need the displayable string. A
//! future phase that wants to branch on `sync_not_configured` /
//! `sync_conflict` / `sync_transport` will thread the typed variant
//! through instead.

use mmcp_store::IndexResolver;
use mmcp_sync::{PendingQueue, PullReport, PushReport, SyncEngine};

use crate::error::GuiError;

pub async fn pull(engine: &SyncEngine, resolver: &IndexResolver) -> Result<PullReport, GuiError> {
    engine
        .pull(resolver)
        .await
        .map_err(|e| GuiError::Other(e.to_string()))
}

pub async fn push(
    engine: &SyncEngine,
    queue: &PendingQueue,
    resolver: &IndexResolver,
) -> Result<PushReport, GuiError> {
    engine
        .push(queue, resolver)
        .await
        .map_err(|e| GuiError::Other(e.to_string()))
}
