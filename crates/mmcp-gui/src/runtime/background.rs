//! Background worker that runs every mmcp-store / mmcp-sync call off
//! the UI thread.
//!
//! The UI never awaits. Each frame it pushes zero or more
//! [`BackgroundTask`] values into the command channel and drains the
//! outcome channel via [`BackgroundHandle::try_recv`]. The worker
//! itself owns a long-lived `NativeBackend`, `GroupIndex`, optional
//! `SyncEngine` bundle, and cached `ResolvedAuthor` so repeat calls
//! don't re-open the bare repos or re-resolve the commit author.

use std::sync::Arc;

use mmcp_core::config::ProjectConfig;
use mmcp_git::NativeBackend;
use mmcp_store::{
    GroupIndex, IndexResolver, MmcpHome, ResolvedAuthor, build_engine, config as project_config,
};
use mmcp_sync::{PendingQueue, SyncClient, SyncEngine, SyncError};
use tokio::sync::mpsc;
use tokio::time::{Duration, interval};

use crate::error::GuiError;
use crate::io::diagnostics_ops::run_diagnose;
use crate::io::memory_ops::{
    create_memory, delete_memory, list_memory_slugs, read_memory_body, update_memory,
};
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
    /// `NativeBackend` + `GroupIndex` + (optional) `SyncEngine` +
    /// `ResolvedAuthor` eagerly, so a broken home surfaces as the
    /// first `TaskOutcome::Error` before any user action.
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
    author: ResolvedAuthor,
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

    // Spawn the reachability probe when sync is configured, so the
    // toolbar can disable Pull/Push the moment the server becomes
    // unreachable. Probe client is separate from the main sync
    // engine's client so a long-running pull doesn't starve the
    // probe (or vice versa).
    if let Some(bundle) = ctx.sync.as_ref() {
        tokio::spawn(probe_loop(bundle.server_url.clone(), outcome_tx.clone()));
    }

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
    let author = home.resolve_author();

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
        author,
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

/// Interval between reachability probes. 15 s is short enough that a
/// dropped network is visible to the user in time for the next click
/// and long enough to keep the wire noise trivial. The first tick
/// fires immediately so the UI learns the initial state without a
/// user-visible delay.
const PROBE_INTERVAL: Duration = Duration::from_secs(15);

async fn probe_loop(server_url: String, tx: mpsc::UnboundedSender<TaskOutcome>) {
    let client = match SyncClient::new(&server_url) {
        Ok(c) => c,
        Err(err) => {
            let _ = tx.send(TaskOutcome::HealthChanged {
                online: false,
                reason: Some(err.to_string()),
            });
            return;
        }
    };
    let mut ticker = interval(PROBE_INTERVAL);
    loop {
        ticker.tick().await;
        let outcome = match client.get_manifest().await {
            Ok(_) => TaskOutcome::HealthChanged {
                online: true,
                reason: None,
            },
            // Only transport failures mean "offline". A Remote error
            // (401 / 403 / 500) means the server responded — we are
            // online, auth or server-side state just isn't happy.
            // The user clicks Pull / Push and gets a clear toast
            // instead of a silently-disabled button.
            Err(SyncError::Transport(msg)) => TaskOutcome::HealthChanged {
                online: false,
                reason: Some(msg),
            },
            Err(_) => TaskOutcome::HealthChanged {
                online: true,
                reason: None,
            },
        };
        if tx.send(outcome).is_err() {
            break;
        }
    }
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
        BackgroundTask::RunDiagnose => {
            let report = run_diagnose(&ctx.backend, &ctx.index).await;
            TaskOutcome::DiagnoseCompleted(report)
        }
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
        BackgroundTask::CreateMemory {
            group_id,
            slug,
            memory,
        } => match ctx.index.get(&group_id).await {
            None => TaskOutcome::Error(format!("group {group_id} is not in the local mirror")),
            Some(entry) => {
                match create_memory(&ctx.backend, &entry.handle, &slug, &memory, &ctx.author).await
                {
                    Ok(_commit_id) => TaskOutcome::MemoryCreated { group_id, slug },
                    Err(err) => TaskOutcome::Error(err.to_string()),
                }
            }
        },
        BackgroundTask::UpdateMemory {
            group_id,
            slug,
            memory,
        } => match ctx.index.get(&group_id).await {
            None => TaskOutcome::Error(format!("group {group_id} is not in the local mirror")),
            Some(entry) => {
                match update_memory(&ctx.backend, &entry.handle, &slug, &memory, &ctx.author).await
                {
                    Ok(_commit_id) => TaskOutcome::MemoryUpdated { group_id, slug },
                    Err(err) => TaskOutcome::Error(err.to_string()),
                }
            }
        },
        BackgroundTask::DeleteMemory { group_id, slug } => match ctx.index.get(&group_id).await {
            None => TaskOutcome::Error(format!("group {group_id} is not in the local mirror")),
            Some(entry) => {
                match delete_memory(&ctx.backend, &entry.handle, &slug, &ctx.author).await {
                    Ok(_commit_id) => TaskOutcome::MemoryDeleted { group_id, slug },
                    Err(err) => TaskOutcome::Error(err.to_string()),
                }
            }
        },
    }
}
