//! Tauri-managed application state.
//!
//! Built once in `lib::run`'s setup closure and handed to every
//! command via `State<AppState>`. Holds everything that's expensive
//! to construct or has to outlive individual commands: the native
//! git backend, the group index, the optional sync bundle, and the
//! resolved commit author.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mmcp_core::config::ProjectConfig;
use mmcp_git::NativeBackend;
use mmcp_store::{
    GroupIndex, IndexResolver, MmcpHome, ResolvedAuthor, build_engine, config as project_config,
};
use mmcp_sync::{PendingQueue, SyncEngine};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Manager};
use tokio::sync::{Mutex, RwLock};

use crate::error::{GuiError, GuiResult};

pub struct SyncBundle {
    pub engine: SyncEngine,
    pub resolver: IndexResolver,
    pub queue: Mutex<PendingQueue>,
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
    pub author: ResolvedAuthor,
    /// Current reachability-probe task. `set_reference_point`
    /// aborts the old handle and spawns a fresh one against the new
    /// URL so probes never outlive the config they were started
    /// with.
    pub probe: Mutex<Option<JoinHandle<()>>>,
}

impl AppState {
    pub async fn discover(app: &AppHandle) -> GuiResult<Self> {
        let home = MmcpHome::discover().map_err(GuiError::from)?;
        let repos_root = home.repos_root();
        let backend = Arc::new(NativeBackend::new(&repos_root).map_err(GuiError::from)?);
        let index = GroupIndex::build(repos_root, Arc::clone(&backend))
            .await
            .map_err(GuiError::from)?;
        let author = home.resolve_author();

        let reference_point = resolve_reference_point(app);
        let sync = build_sync(&backend, &index, reference_point.as_deref())?;

        Ok(Self {
            backend,
            index,
            sync: RwLock::new(sync),
            author,
            probe: Mutex::new(None),
        })
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

fn build_sync(
    backend: &Arc<NativeBackend>,
    index: &GroupIndex,
    reference_point: Option<&Path>,
) -> GuiResult<Option<SyncBundle>> {
    let Some(cfg) = load_project_sync(reference_point)? else {
        return Ok(None);
    };
    let server_url = cfg.server_url.clone();
    let (engine, resolver, queue) =
        build_engine(Arc::clone(backend), index.clone(), &server_url).map_err(GuiError::from)?;
    Ok(Some(SyncBundle {
        engine,
        resolver,
        queue: Mutex::new(queue),
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
