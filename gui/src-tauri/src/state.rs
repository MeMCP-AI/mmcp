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
use uuid::Uuid;

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
    pub async fn rebuild_sync(&self, reference_point: Option<&Path>) -> GuiResult<Option<String>> {
        let fresh = build_sync(&self.backend, &self.index, reference_point)?;
        let url = fresh.as_ref().map(|b| b.server_url.clone());
        *self.sync.write().await = fresh;
        Ok(url)
    }
}

/// Where one filesystem-watch event, relative to `repos_root`, points.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WatchTarget {
    /// Inside a specific group repository, carrying its bare UUID.
    Group(String),
    /// At the mirror root itself, or under an entry that doesn't
    /// look like a real group repository.
    Root,
}

/// Classify `rel`, a watch-event path stripped of the `repos_root` prefix.
///
/// A mirrored group lives at `<repos_root>/<uuid>.git/` (see `mmcp_store::groups::scan_repos_root`).
/// The emitted id is the bare UUID, matching `GroupEntryDto.group_id`.
/// Anything else at top level classifies as [`WatchTarget::Root`], never as a bogus group id.
fn classify_watch_path(rel: &Path) -> WatchTarget {
    let Some(first) = rel.components().next().and_then(|c| c.as_os_str().to_str()) else {
        return WatchTarget::Root;
    };
    let Some(stem) = first.strip_suffix(".git") else {
        return WatchTarget::Root;
    };
    match Uuid::parse_str(stem) {
        Ok(uuid) => WatchTarget::Group(uuid.to_string()),
        Err(_) => WatchTarget::Root,
    }
}

/// Spawn a debounced recursive watcher on the mirror root.
/// A `mirror:changed` event fires with the group's bare UUID (see [`classify_watch_path`]).
/// Events directly under `repos_root` carry `group_id: null`, triggering a full group-list refresh.
/// This picks up newly-cloned group repos after a pull.
fn spawn_mirror_watcher(
    app: &AppHandle,
    repos_root: &Path,
) -> GuiResult<Option<Debouncer<RecommendedWatcher>>> {
    // Mirror must exist before `notify` can watch it. `init_backend`
    // above normally creates it, but in a fresh home the directory
    // can briefly be absent — create it eagerly here so the watcher
    // always has something to observe.
    if !repos_root.exists()
        && let Err(err) = std::fs::create_dir_all(repos_root)
    {
        return Err(GuiError::Other(format!(
            "create repos_root {}: {err}",
            repos_root.display()
        )));
    }

    let (tx, mut rx) =
        tokio::sync::mpsc::unbounded_channel::<Vec<notify_debouncer_mini::DebouncedEvent>>();

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
                let Ok(rel) = event.path.strip_prefix(&root) else {
                    continue;
                };
                match classify_watch_path(rel) {
                    WatchTarget::Group(id) => {
                        groups.insert(id);
                    }
                    WatchTarget::Root => root_changed = true,
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
                let _ = handle.emit(MIRROR_CHANGED_EVENT, serde_json::json!({ "group_id": g }));
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
    let token = cfg.resolve_token();
    let (engine, resolver) = build_engine(
        Arc::clone(backend),
        index.clone(),
        &server_url,
        token.as_deref(),
    )
    .map_err(GuiError::from)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A path under `<uuid>.git/` classifies as `Group` with the bare UUID, never the directory name.
    #[test]
    fn group_directory_path_yields_a_bare_uuid_parseable_group_id() {
        let rel = Path::new("019d955d-4cce-77f2-a0b3-0b79ed394612.git/objects/pack/x");
        let target = classify_watch_path(rel);
        let WatchTarget::Group(id) = target else {
            panic!("expected WatchTarget::Group, got {target:?}");
        };
        assert!(
            Uuid::parse_str(&id).is_ok(),
            "group_id {id} must parse as a UUID"
        );
        assert!(
            !id.ends_with(".git"),
            "group_id {id} must not carry the .git suffix"
        );
        assert_eq!(id, "019d955d-4cce-77f2-a0b3-0b79ed394612");
    }

    #[test]
    fn a_file_directly_under_repos_root_is_a_root_change() {
        assert_eq!(
            classify_watch_path(Path::new("some-file")),
            WatchTarget::Root
        );
    }

    #[test]
    fn a_dot_git_directory_that_is_not_a_uuid_falls_back_to_root() {
        assert_eq!(
            classify_watch_path(Path::new("not-a-uuid.git/HEAD")),
            WatchTarget::Root
        );
    }

    #[test]
    fn a_uuid_directory_missing_the_git_suffix_falls_back_to_root() {
        assert_eq!(
            classify_watch_path(Path::new("019d955d-4cce-77f2-a0b3-0b79ed394612/HEAD")),
            WatchTarget::Root
        );
    }
}
