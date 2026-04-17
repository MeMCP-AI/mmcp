//! Background worker that runs every mmcp-store / mmcp-sync call off
//! the UI thread.
//!
//! The UI never awaits. Each frame it pushes zero or more
//! [`BackgroundTask`] values into the command channel and drains the
//! outcome channel via [`BackgroundHandle::try_recv`]. The worker
//! itself owns a long-lived `NativeBackend`, `GroupIndex`, and
//! optional `SyncEngine` bundle so repeat calls don't re-open the
//! bare repos or re-construct the sync client.

use std::sync::Arc;

use mmcp_core::config::ProjectConfig;
use mmcp_git::NativeBackend;
use mmcp_store::{GroupIndex, IndexResolver, MmcpHome, build_engine, config as project_config};
use mmcp_sync::{PendingQueue, SyncEngine};
use tokio::sync::mpsc;

use crate::error::GuiError;
use crate::io::memory_ops::{list_memory_slugs, read_memory_body};
use crate::io::sync_ops;
use crate::runtime::outcome::TaskOutcome;
use crate::runtime::task::BackgroundTask;
use crate::state::sync_status::SyncOp;

pub struct BackgroundHandle {
    tx: mpsc::UnboundedSender<BackgroundTask>,
    rx: mpsc::UnboundedReceiver<TaskOutcome>,
}

impl BackgroundHandle {
    /// Spawn the worker on `runtime` and return the handle the UI uses
    /// to talk to it. The worker initialises `MmcpHome` +
    /// `NativeBackend` + `GroupIndex` + (optional) `SyncEngine`
    /// eagerly, so a broken home surfaces as the first
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

struct SyncBundle {
    engine: SyncEngine,
    resolver: IndexResolver,
    queue: PendingQueue,
    server_url: String,
}

struct WorkerContext {
    backend: Arc<NativeBackend>,
    index: GroupIndex,
    sync: Option<SyncBundle>,
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

    let _ = outcome_tx.send(TaskOutcome::SyncAvailable {
        server_url: ctx.sync.as_ref().map(|s| s.server_url.clone()),
    });

    match refresh_groups(&ctx).await {
        Ok(groups) => {
            let _ = outcome_tx.send(TaskOutcome::GroupsRefreshed(groups));
        }
        Err(err) => {
            let _ = outcome_tx.send(TaskOutcome::Error(err.to_string()));
        }
    }

    while let Some(task) = task_rx.recv().await {
        let outcome = execute(&ctx, task).await;
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

    let sync = match load_project_sync().map_err(|e| GuiError::Other(e.to_string()))? {
        Some(config) => {
            let server_url = config.server_url.clone();
            let (engine, resolver, queue) =
                build_engine(Arc::clone(&backend), index.clone(), &server_url)
                    .map_err(|e| GuiError::Other(e.to_string()))?;
            Some(SyncBundle {
                engine,
                resolver,
                queue,
                server_url,
            })
        }
        None => None,
    };

    Ok(WorkerContext {
        backend,
        index,
        sync,
    })
}

/// Walk up from the current working directory looking for a
/// `.mmcp.toml` with a `[sync]` block. Returns `None` when either
/// the file is absent (GUI was launched outside a project) or the
/// project is not sync-enabled. Both cases mean "sync unavailable",
/// not an error — the GUI greys out the toolbar buttons.
fn load_project_sync() -> anyhow::Result<Option<mmcp_core::config::SyncConfig>> {
    let Ok(cwd) = std::env::current_dir() else {
        return Ok(None);
    };
    let Some(root) = project_config::find_project_root(&cwd) else {
        return Ok(None);
    };
    let cfg: ProjectConfig = project_config::load(&root)?;
    Ok(cfg.sync)
}

async fn refresh_groups(ctx: &WorkerContext) -> Result<Vec<mmcp_store::GroupEntry>, GuiError> {
    ctx.index.refresh().await?;
    Ok(ctx.index.list().await)
}

async fn execute(ctx: &WorkerContext, task: BackgroundTask) -> TaskOutcome {
    match task {
        BackgroundTask::RefreshGroups => match refresh_groups(ctx).await {
            Ok(groups) => TaskOutcome::GroupsRefreshed(groups),
            Err(err) => TaskOutcome::Error(err.to_string()),
        },
        BackgroundTask::LoadMemoryList { group_id } => match ctx.index.get(&group_id).await {
            None => TaskOutcome::Error(format!("group {group_id} is not in the local mirror")),
            Some(entry) => match list_memory_slugs(&ctx.backend, &entry.handle).await {
                Ok(slugs) => TaskOutcome::MemoryListLoaded { group_id, slugs },
                Err(err) => TaskOutcome::Error(err.to_string()),
            },
        },
        BackgroundTask::LoadMemory { group_id, slug } => match ctx.index.get(&group_id).await {
            None => TaskOutcome::Error(format!("group {group_id} is not in the local mirror")),
            Some(entry) => match read_memory_body(&ctx.backend, &entry.handle, &slug).await {
                Ok(memory) => TaskOutcome::MemoryLoaded {
                    group_id,
                    slug,
                    memory: Arc::new(memory),
                },
                Err(err) => TaskOutcome::Error(err.to_string()),
            },
        },
        BackgroundTask::SyncPull => match &ctx.sync {
            None => TaskOutcome::SyncFailed {
                op: SyncOp::Pull,
                message: "sync is not configured for this project".to_string(),
            },
            Some(bundle) => match sync_ops::pull(&bundle.engine, &bundle.resolver).await {
                Ok(report) => TaskOutcome::SyncPullCompleted {
                    updated: report.updated.len(),
                    new_groups: report.new_groups.len(),
                },
                Err(err) => TaskOutcome::SyncFailed {
                    op: SyncOp::Pull,
                    message: err.to_string(),
                },
            },
        },
        BackgroundTask::SyncPush => match &ctx.sync {
            None => TaskOutcome::SyncFailed {
                op: SyncOp::Push,
                message: "sync is not configured for this project".to_string(),
            },
            Some(bundle) => {
                match sync_ops::push(&bundle.engine, &bundle.queue, &bundle.resolver).await {
                    Ok(report) => TaskOutcome::SyncPushCompleted {
                        drained: report.drained.len(),
                    },
                    Err(err) => TaskOutcome::SyncFailed {
                        op: SyncOp::Push,
                        message: err.to_string(),
                    },
                }
            }
        },
    }
}
