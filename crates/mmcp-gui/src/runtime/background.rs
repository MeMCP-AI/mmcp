//! Background worker that runs every mmcp-store call off the UI thread.
//!
//! The UI never awaits. Each frame it pushes zero or more
//! [`BackgroundTask`] values into the command channel and drains the
//! outcome channel via [`BackgroundHandle::try_recv`]. The worker
//! itself owns a long-lived `NativeBackend` + `GroupIndex` so repeat
//! reads don't re-open the bare repos on every call.

use std::sync::Arc;

use mmcp_git::NativeBackend;
use mmcp_store::{GroupIndex, MmcpHome};
use tokio::sync::mpsc;

use crate::error::GuiError;
use crate::io::memory_ops::{list_memory_slugs, read_memory_body};
use crate::runtime::outcome::TaskOutcome;
use crate::runtime::task::BackgroundTask;

pub struct BackgroundHandle {
    tx: mpsc::UnboundedSender<BackgroundTask>,
    rx: mpsc::UnboundedReceiver<TaskOutcome>,
}

impl BackgroundHandle {
    /// Spawn the worker on `runtime` and return the handle the UI uses
    /// to talk to it. The worker initialises `MmcpHome` +
    /// `NativeBackend` + `GroupIndex` eagerly, so a broken home
    /// (missing `MMCP_HOME`, unreadable repos) surfaces as the first
    /// `TaskOutcome::Error` before any user action.
    pub fn spawn(runtime: &tokio::runtime::Runtime) -> Self {
        let (task_tx, task_rx) = mpsc::unbounded_channel::<BackgroundTask>();
        let (outcome_tx, outcome_rx) = mpsc::unbounded_channel::<TaskOutcome>();
        runtime.spawn(worker_loop(task_rx, outcome_tx));
        Self {
            tx: task_tx,
            rx: outcome_rx,
        }
    }

    pub fn send(&self, task: BackgroundTask) {
        if self.tx.send(task).is_err() {
            tracing::warn!("background worker dropped — task ignored");
        }
    }

    pub fn try_recv(&mut self) -> Option<TaskOutcome> {
        self.rx.try_recv().ok()
    }
}

struct WorkerContext {
    backend: Arc<NativeBackend>,
    index: GroupIndex,
}

async fn worker_loop(
    mut task_rx: mpsc::UnboundedReceiver<BackgroundTask>,
    outcome_tx: mpsc::UnboundedSender<TaskOutcome>,
) {
    let ctx = match init_context().await {
        Ok(c) => c,
        Err(err) => {
            let _ = outcome_tx.send(TaskOutcome::Error(err.to_string()));
            return;
        }
    };

    // Kick off an initial group refresh so the UI receives a populated
    // group list without the user having to press anything.
    match refresh_groups(&ctx).await {
        Ok(groups) => {
            let _ = outcome_tx.send(TaskOutcome::GroupsRefreshed(groups));
        }
        Err(err) => {
            let _ = outcome_tx.send(TaskOutcome::Error(err.to_string()));
        }
    }

    while let Some(task) = task_rx.recv().await {
        let outcome = match execute(&ctx, task).await {
            Ok(o) => o,
            Err(err) => TaskOutcome::Error(err.to_string()),
        };
        if outcome_tx.send(outcome).is_err() {
            break;
        }
    }
}

async fn init_context() -> Result<WorkerContext, GuiError> {
    let home = MmcpHome::discover().map_err(|e| GuiError::Other(e.to_string()))?;
    let repos_root = home.repos_root();
    let backend = Arc::new(NativeBackend::new(&repos_root)?);
    let index = GroupIndex::build(repos_root, Arc::clone(&backend)).await?;
    Ok(WorkerContext { backend, index })
}

async fn refresh_groups(ctx: &WorkerContext) -> Result<Vec<mmcp_store::GroupEntry>, GuiError> {
    ctx.index.refresh().await?;
    Ok(ctx.index.list().await)
}

async fn execute(ctx: &WorkerContext, task: BackgroundTask) -> Result<TaskOutcome, GuiError> {
    match task {
        BackgroundTask::RefreshGroups => {
            Ok(TaskOutcome::GroupsRefreshed(refresh_groups(ctx).await?))
        }
        BackgroundTask::LoadMemoryList { group_id } => {
            let Some(entry) = ctx.index.get(&group_id).await else {
                return Err(GuiError::Other(format!(
                    "group {group_id} is not in the local mirror"
                )));
            };
            let slugs = list_memory_slugs(&ctx.backend, &entry.handle).await?;
            Ok(TaskOutcome::MemoryListLoaded { group_id, slugs })
        }
        BackgroundTask::LoadMemory { group_id, slug } => {
            let Some(entry) = ctx.index.get(&group_id).await else {
                return Err(GuiError::Other(format!(
                    "group {group_id} is not in the local mirror"
                )));
            };
            let memory = read_memory_body(&ctx.backend, &entry.handle, &slug).await?;
            Ok(TaskOutcome::MemoryLoaded {
                group_id,
                slug,
                memory: Arc::new(memory),
            })
        }
    }
}
