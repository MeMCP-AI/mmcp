//! Filesystem watcher task that keeps [`GroupIndex`] in sync with
//! disk changes and triggers reloads when the project's
//! `.mmcp/config.toml` is edited.
//!
//! Built on top of the `notify` crate. The `notify` API delivers
//! events on a blocking callback, so the watcher bridges them onto
//! an unbounded mpsc channel that a tokio task drains.

use std::path::PathBuf;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::state::error::StateError;
use crate::state::groups::GroupIndex;

/// Handle returned to the caller so the watcher task can be shut
/// down (or simply dropped when the client exits).
pub struct WatcherHandle {
    _task: JoinHandle<()>,
    _watcher: RecommendedWatcher,
}

/// Spawn a background task that watches `repos_root` non-recursively
/// and the optional `project_config_path` for changes, refreshing
/// `index` whenever something relevant changes.
///
/// Events are coalesced with a small debounce window so a flurry
/// of writes (e.g. a git push creating multiple pack files) only
/// triggers one rescan.
pub fn spawn_watcher(
    repos_root: PathBuf,
    project_config_path: Option<PathBuf>,
    index: GroupIndex,
) -> Result<WatcherHandle, StateError> {
    let (tx, mut rx) = mpsc::unbounded_channel::<WatcherMessage>();

    let forward_tx = tx.clone();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(
        move |res: notify::Result<notify::Event>| match res {
            Ok(_event) => {
                let _ = forward_tx.send(WatcherMessage::Event);
            }
            Err(err) => {
                let _ = forward_tx.send(WatcherMessage::Error(err.to_string()));
            }
        },
    )
    .map_err(|e| StateError::Io(format!("notify init: {e}")))?;

    watcher
        .watch(&repos_root, RecursiveMode::NonRecursive)
        .map_err(|e| StateError::Io(format!("watch {}: {e}", repos_root.display())))?;
    if let Some(path) = project_config_path.as_ref()
        && path.exists()
    {
        watcher
            .watch(path, RecursiveMode::NonRecursive)
            .map_err(|e| StateError::Io(format!("watch {}: {e}", path.display())))?;
    }

    let index_for_task = index.clone();
    let task = tokio::spawn(async move {
        loop {
            let first = match rx.recv().await {
                Some(m) => m,
                None => break,
            };
            let mut pending = matches!(first, WatcherMessage::Event);
            if let WatcherMessage::Error(err) = &first {
                tracing::warn!(error = %err, "notify watcher error");
            }
            // Debounce: drain any burst of events before rescanning.
            while let Ok(next) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                match next {
                    Some(WatcherMessage::Event) => {
                        pending = true;
                    }
                    Some(WatcherMessage::Error(err)) => {
                        tracing::warn!(error = %err, "notify watcher error");
                    }
                    None => break,
                }
            }
            if pending {
                if let Err(err) = index_for_task.refresh().await {
                    tracing::warn!(error = %err, "group index refresh failed");
                }
            }
        }
    });

    // Forget the intermediate sender so only the watcher owns one
    // and the task's rx closes once the watcher is dropped.
    drop(tx);

    Ok(WatcherHandle {
        _task: task,
        _watcher: watcher,
    })
}

/// Messages forwarded from the `notify` callback into the async
/// watcher task. The payload is dropped because the task only
/// needs to know that *something* changed, not what.
enum WatcherMessage {
    Event,
    Error(String),
}
