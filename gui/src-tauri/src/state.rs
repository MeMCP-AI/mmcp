//! Tauri-managed application state.
//!
//! Built once in `lib::run`'s setup closure and handed to every
//! command via `State<AppState>`. Holds everything that's expensive
//! to construct or has to outlive individual commands: the native
//! git backend, the group index, the optional sync bundle, and the
//! resolved commit author.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mmcp_core::config::ProjectConfig;
use mmcp_git::NativeBackend;
use mmcp_store::{
    GroupIndex, IndexResolver, MmcpHome, ResolvedAuthor, build_engine, config as project_config,
};
use mmcp_sync::SyncEngine;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{Debouncer, new_debouncer};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, RwLock};

use crate::commands::sync::MIRROR_CHANGED_EVENT;
use crate::error::{GuiError, GuiResult};

pub struct SyncBundle {
    pub engine: SyncEngine,
    pub resolver: IndexResolver,
    pub server_url: String,
}

/// Mutable runtime wrapper. The bundle is rebuilt whenever the
/// user changes the `reference_point` so pull/push/status all
/// read the *current* configured server, not the one that happened
/// to be set at startup.
pub struct AppState {
    pub backend: Arc<NativeBackend>,
    pub index: GroupIndex,
    pub sync: RwLock<Option<SyncBundle>>,
    /// Resolved commit author. Behind an RwLock so a
    /// `save_user_config` call can refresh the identity in place
    /// without restarting the app — commits issued after the edit
    /// carry the new name/email.
    pub author: RwLock<ResolvedAuthor>,
    /// Current reachability-probe task. `set_reference_point`
    /// aborts the old handle and spawns a fresh one against the new
    /// URL so probes never outlive the config they were started
    /// with.
    pub probe: Mutex<Option<JoinHandle<()>>>,
    /// Filesystem watcher on `~/.mmcp/repos`. Dropped with the
    /// AppState; the struct is kept alive for the lifetime of the
    /// app via `handle.manage`. The inner option is only None
    /// when discovery failed to spawn a watcher — the app keeps
    /// running with manual refresh still available.
    pub _watcher: Mutex<Option<Debouncer<RecommendedWatcher>>>,
}

impl AppState {
    pub async fn discover(app: &AppHandle) -> GuiResult<Self> {
        let home = MmcpHome::discover().map_err(GuiError::from)?;
        let repos_root = home.repos_root();
        let backend = Arc::new(NativeBackend::new(&repos_root).map_err(GuiError::from)?);
        let index = GroupIndex::build(repos_root.clone(), Arc::clone(&backend))
            .await
            .map_err(GuiError::from)?;
        let author = home.resolve_author();

        let reference_point = resolve_reference_point(app);
        let sync = build_sync(&backend, &index, reference_point.as_deref())?;

        let watcher = spawn_mirror_watcher(app, &repos_root).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "mirror watcher unavailable — manual refresh only");
            None
        });

        Ok(Self {
            backend,
            index,
            sync: RwLock::new(sync),
            author: RwLock::new(author),
            probe: Mutex::new(None),
            _watcher: Mutex::new(watcher),
        })
    }

    /// Refresh the cached commit author from disk. Called after
    /// `save_user_config` so edits to `~/.mmcp/config.toml`
    /// propagate without an app restart.
    pub async fn reload_author(&self) -> GuiResult<ResolvedAuthor> {
        let home = MmcpHome::discover().map_err(GuiError::from)?;
        let fresh = home.resolve_author();
        *self.author.write().await = fresh.clone();
        Ok(fresh)
    }

    /// Rebuild the sync bundle against a new reference point. Takes
    /// a write lock so in-flight pull/push finish first. Returns the
    /// new server URL (if any) so the caller can restart the probe
    /// loop.
    pub async fn rebuild_sync(
        &self,
        reference_point: Option<&Path>,
    ) -> GuiResult<Option<String>> {
        let fresh = build_sync(&self.backend, &self.index, reference_point)?;
        let url = fresh.as_ref().map(|b| b.server_url.clone());
        *self.sync.write().await = fresh;
        Ok(url)
    }
}

/// Spawn a debounced recursive watcher on the mirror root. When
/// any file under a group repo changes, a `mirror:changed` event
/// fires with the group UUID (the first directory component under
/// repos_root). Events targeting files directly under repos_root
/// carry `group_id: null` so the frontend does a full group-list
/// refresh — picking up newly-cloned group repos after a pull.
fn spawn_mirror_watcher(
    app: &AppHandle,
    repos_root: &Path,
) -> GuiResult<Option<Debouncer<RecommendedWatcher>>> {
    // Mirror must exist before `notify` can watch it. `init_backend`
    // above normally creates it, but in a fresh home the directory
    // can briefly be absent — create it eagerly here so the watcher
    // always has something to observe.
    if !repos_root.exists() {
        if let Err(err) = std::fs::create_dir_all(repos_root) {
            return Err(GuiError::Other(format!(
                "create repos_root {}: {err}",
                repos_root.display()
            )));
        }
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<
        Vec<notify_debouncer_mini::DebouncedEvent>,
    >();

    let mut debouncer = new_debouncer(
        Duration::from_millis(500),
        move |res: notify_debouncer_mini::DebounceEventResult| {
            // Notify errors are best-effort — log and drop. The
            // watcher keeps running; the next change still fires.
            if let Ok(events) = res {
                let _ = tx.send(events);
            }
        },
    )
    .map_err(|e| GuiError::Other(format!("debouncer: {e}")))?;

    debouncer
        .watcher()
        .watch(repos_root, RecursiveMode::Recursive)
        .map_err(|e| GuiError::Other(format!("watch: {e}")))?;

    let handle = app.clone();
    let root = repos_root.to_path_buf();
    tauri::async_runtime::spawn(async move {
        while let Some(batch) = rx.recv().await {
            let mut groups: HashSet<String> = HashSet::new();
            let mut root_changed = false;
            for event in batch {
                // Strip the mirror root prefix. First non-empty
                // path component = group UUID (or `None` if the
                // event is at the root itself).
                let Ok(rel) = event.path.strip_prefix(&root) else {
                    continue;
                };
                let first = rel
                    .components()
                    .next()
                    .and_then(|c| c.as_os_str().to_str());
                match first {
                    Some(s) if !s.is_empty() => {
                        groups.insert(s.to_string());
                    }
                    _ => root_changed = true,
                }
            }
            if root_changed && groups.is_empty() {
                let _ = handle.emit(
                    MIRROR_CHANGED_EVENT,
                    serde_json::json!({ "group_id": null }),
                );
                continue;
            }
            for g in groups {
                let _ = handle.emit(
                    MIRROR_CHANGED_EVENT,
                    serde_json::json!({ "group_id": g }),
                );
            }
        }
    });

    Ok(Some(debouncer))
}

fn build_sync(
    backend: &Arc<NativeBackend>,
    index: &GroupIndex,
    reference_point: Option<&Path>,
) -> GuiResult<Option<SyncBundle>> {
    let Some(cfg) = load_project_sync(reference_point)? else {
        return Ok(None);
    };
    let server_url = cfg.server_url.clone();
    let (engine, resolver) =
        build_engine(Arc::clone(backend), index.clone(), &server_url).map_err(GuiError::from)?;
    Ok(Some(SyncBundle {
        engine,
        resolver,
        server_url,
    }))
}

/// Resolve the directory we use as the anchor for `.mmcp.toml`
/// discovery. Priority: `reference_point` from settings.json (if set
/// and it actually exists), otherwise process cwd.
fn resolve_reference_point(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    let path = dir.join("settings.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let raw = value.get("reference_point")?.as_str()?;
    if raw.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(raw);
    candidate.is_dir().then_some(candidate)
}

fn load_project_sync(
    reference_point: Option<&Path>,
) -> GuiResult<Option<mmcp_core::config::SyncConfig>> {
    let start: PathBuf = match reference_point {
        Some(p) => p.to_path_buf(),
        None => match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(_) => return Ok(None),
        },
    };
    let Some(root) = project_config::find_project_root(&start) else {
        return Ok(None);
    };
    let cfg: ProjectConfig = project_config::load(&root).map_err(GuiError::from)?;
    Ok(cfg.sync)
}
