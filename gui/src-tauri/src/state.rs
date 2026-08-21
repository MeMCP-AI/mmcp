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

use mmcp_core::config::{ProjectConfig, Remote, UserConfig};
use mmcp_git::NativeBackend;
use mmcp_store::{
    EffectiveRemotes, GroupIndex, IndexResolver, MmcpHome, ResolvedAuthor, build_engine,
    config as project_config, resolve_effective_remotes,
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

/// Handle to a live sync engine plus the resolved remote-set summary
/// the frontend shows in the status bar.
pub struct SyncBundle {
    pub engine: SyncEngine,
    pub resolver: IndexResolver,
    /// Human-readable label for the effective remote set. See
    /// [`EffectiveRemotes::summary_label`].
    pub remotes_summary: String,
    /// Base URL of the default remote when it is an `mmcp-server`
    /// transport; feeds the reachability probe. `None` when the
    /// default remote is `direct-git`, which has no manifest
    /// endpoint to poll.
    pub probe_url: Option<String>,
}

/// Status-relevant fields of a freshly rebuilt [`SyncBundle`],
/// returned by [`AppState::rebuild_sync`] so a caller refreshes the
/// status bar and respawns the reachability probe without re-reading
/// the `sync` lock itself.
pub struct SyncStatusSnapshot {
    pub remotes_summary: String,
    pub probe_url: Option<String>,
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
    /// Reason the last sync-resolution attempt (startup discovery, or
    /// a `rebuild_sync` call) failed, e.g. an ambiguous default
    /// remote across the merged user+project config.
    ///
    /// `None` while `sync` is either configured or genuinely
    /// unconfigured, which is not an error (no remotes declared
    /// anywhere).
    ///
    /// Read by `sync_status` so a broken config is visible from the
    /// status bar instead of just leaving `sync` at `None` with no
    /// explanation.
    pub sync_error: RwLock<Option<String>>,
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
        // A resolver failure (ambiguous default remote across the
        // merged config, an invalid remote name, ...) must degrade to
        // a usable app state rather than abort discovery entirely: an
        // `Err` here used to propagate through `?` and skip
        // `handle.manage(state)` altogether, leaving every
        // `State<AppState>`-dependent command dead, including the
        // ones the user would need to open Settings and fix the
        // config from inside a running app.
        let (sync, sync_error) =
            match build_sync(&backend, &index, &home, reference_point.as_deref()).await {
                Ok(sync) => (sync, None),
                Err(err) => {
                    tracing::error!(
                        error = %err,
                        "sync resolution failed at startup, degrading to sync-unconfigured"
                    );
                    (None, Some(err.to_string()))
                }
            };

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
            sync_error: RwLock::new(sync_error),
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
    /// new bundle's status snapshot (if any) so the caller can
    /// refresh the status bar and restart the probe loop.
    pub async fn rebuild_sync(
        &self,
        reference_point: Option<&Path>,
    ) -> GuiResult<Option<SyncStatusSnapshot>> {
        let home = MmcpHome::discover().map_err(GuiError::from)?;
        let fresh = build_sync(&self.backend, &self.index, &home, reference_point).await?;
        let snapshot = fresh.as_ref().map(|b| SyncStatusSnapshot {
            remotes_summary: b.remotes_summary.clone(),
            probe_url: b.probe_url.clone(),
        });
        *self.sync.write().await = fresh;
        // A successful rebuild supersedes whatever `sync_error`
        // startup discovery (or a previous rebuild) left behind.
        *self.sync_error.write().await = None;
        Ok(snapshot)
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
        && let Err(source) = std::fs::create_dir_all(repos_root)
    {
        return Err(GuiError::Io {
            path: repos_root.to_path_buf(),
            source,
        });
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
    .map_err(GuiError::from)?;

    debouncer
        .watcher()
        .watch(repos_root, RecursiveMode::Recursive)
        .map_err(GuiError::from)?;

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

async fn build_sync(
    backend: &Arc<NativeBackend>,
    index: &GroupIndex,
    home: &MmcpHome,
    reference_point: Option<&Path>,
) -> GuiResult<Option<SyncBundle>> {
    let Some(effective) = load_effective_remotes(home, reference_point)? else {
        return Ok(None);
    };
    let remotes_summary = effective.summary_label();
    let probe_url = default_probe_url(&effective);
    let (engine, resolver) = build_engine(Arc::clone(backend), index.clone(), &effective)
        .await
        .map_err(GuiError::from)?;
    Ok(Some(SyncBundle {
        engine,
        resolver,
        remotes_summary,
        probe_url,
    }))
}

/// Base URL to feed the reachability probe: the default remote's URL
/// when it is an `mmcp-server` transport, `None` when the effective
/// set is empty or its default remote is `direct-git` (no manifest
/// endpoint to poll).
fn default_probe_url(effective: &EffectiveRemotes) -> Option<String> {
    effective
        .default_remote()
        .and_then(|resolved| match &resolved.remote {
            Remote::MmcpServer { url, .. } => Some(url.clone()),
            Remote::DirectGit { .. } => None,
        })
}

/// Load the project config at `reference_point` (or cwd when unset)
/// plus `home`'s user config, and resolve the effective remote set
/// via `mmcp_store::resolve_effective_remotes`. `home` is
/// caller-supplied (never re-discovered here) so a test can point it
/// at a tempdir-rooted [`MmcpHome`] instead of the real `~/.mmcp`.
///
/// `Ok(None)` when no project config is found under the reference
/// point, or when the resolved effective set is empty: the GUI treats
/// a project with zero configured remotes the same as a project with
/// no config at all, sync stays unconfigured either way.
fn load_effective_remotes(
    home: &MmcpHome,
    reference_point: Option<&Path>,
) -> GuiResult<Option<EffectiveRemotes>> {
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
    let project_cfg: ProjectConfig = project_config::load(&root).map_err(GuiError::from)?;
    let user_cfg: UserConfig = home.load_user_config().map_err(GuiError::from)?;
    let effective = resolve_effective_remotes(&user_cfg, &project_cfg).map_err(GuiError::from)?;
    if effective.remotes.is_empty() {
        return Ok(None);
    }
    Ok(Some(effective))
}

/// Resolve the directory we use as the anchor for `.mmcp.toml`
/// discovery. Priority: `reference_point` from settings.json (if set
/// and it actually exists), otherwise process cwd.
///
/// `pub(crate)`: also called from `commands::config` so a config save
/// can find the currently active project to merge-validate a
/// candidate remote set against, without re-deriving this lookup.
pub(crate) fn resolve_reference_point(app: &AppHandle) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use mmcp_core::config::RemoteAuth;
    use mmcp_store::{RemoteLevel, ResolvedRemote};

    use super::*;

    fn mmcp_server_remote(name: &str, default: bool, level: RemoteLevel) -> ResolvedRemote {
        ResolvedRemote {
            remote: Remote::MmcpServer {
                name: name.to_string(),
                url: format!("https://{name}.example.com"),
                default,
                include_in_push_all: true,
            },
            level,
            direct_git_group: None,
        }
    }

    /// End-to-end proof that `load_effective_remotes` actually reads
    /// AND merges both config files: a real tempdir-rooted user
    /// config plus a real project `.mmcp.toml`, each declaring their
    /// own remote. Guards against a dead-fallback regression where
    /// `UserConfig.sync` is loaded but never actually consulted, so a
    /// user-level-only remote silently vanishes from the GUI's own
    /// load path.
    /// `default_probe_url`'s own tests below cover formatting on an already-merged `EffectiveRemotes`.
    /// `summary_label`'s formatting is owned by mmcp-store.
    /// This test covers the merge itself.
    #[tokio::test]
    async fn load_effective_remotes_merges_user_and_project_configs_with_project_default_winning() {
        let home_tmp = tempfile::TempDir::new().expect("home tempdir");
        let home = MmcpHome::from_root(home_tmp.path());
        let user_cfg = UserConfig {
            sync: mmcp_core::config::SyncConfig {
                remotes: vec![Remote::MmcpServer {
                    name: "u1".to_string(),
                    url: "https://u1.example.com".to_string(),
                    default: false,
                    include_in_push_all: true,
                }],
                ..mmcp_core::config::SyncConfig::default()
            },
            ..UserConfig::default()
        };
        home.save_user_config(&user_cfg).expect("save user config");

        let project_tmp = tempfile::TempDir::new().expect("project tempdir");
        let project_cfg = ProjectConfig {
            project_uuid: mmcp_core::id::ProjectUuid::new(),
            project_slug: None,
            sync: mmcp_core::config::SyncConfig {
                remotes: vec![Remote::MmcpServer {
                    name: "p1".to_string(),
                    url: "https://p1.example.com".to_string(),
                    default: true,
                    include_in_push_all: true,
                }],
                ..mmcp_core::config::SyncConfig::default()
            },
            project_remote_only: false,
            subscriptions: mmcp_core::config::SubscriptionsConfig::default(),
        };
        project_config::save(project_tmp.path(), &project_cfg).expect("save project config");

        let effective = load_effective_remotes(&home, Some(project_tmp.path()))
            .expect("load effective remotes")
            .expect("some effective remotes");

        let names: Vec<&str> = effective.remotes.iter().map(ResolvedRemote::name).collect();
        assert_eq!(names, vec!["u1", "p1"]);
        assert_eq!(
            effective.default_remote().map(ResolvedRemote::name),
            Some("p1")
        );
    }

    /// `SyncBundle::probe_url` must read the default remote's URL
    /// from a multi-remote set, not assume a single legacy
    /// `server_url`.
    #[test]
    fn default_probe_url_reads_the_default_remotes_url_from_a_multi_remote_set() {
        let effective = EffectiveRemotes {
            remotes: vec![
                mmcp_server_remote("u1", false, RemoteLevel::User),
                mmcp_server_remote("p1", true, RemoteLevel::Project),
            ],
            default_index: Some(1),
        };
        assert_eq!(
            default_probe_url(&effective).as_deref(),
            Some("https://p1.example.com")
        );
    }

    /// A `direct-git` default remote has no manifest endpoint: the
    /// probe URL must be `None`, not the git remote URL.
    #[test]
    fn default_probe_url_is_none_for_a_direct_git_default_remote() {
        let effective = EffectiveRemotes {
            remotes: vec![ResolvedRemote {
                remote: Remote::DirectGit {
                    name: "mirror".to_string(),
                    url: "ssh://git@example.com/mirror.git".to_string(),
                    auth: RemoteAuth::None,
                    group: Some("team-rust".to_string()),
                    default: true,
                    include_in_push_all: true,
                },
                level: RemoteLevel::Project,
                direct_git_group: Some("team-rust".to_string()),
            }],
            default_index: Some(0),
        };
        assert_eq!(default_probe_url(&effective), None);
    }

    #[test]
    fn default_probe_url_is_none_for_an_empty_effective_set() {
        let effective = EffectiveRemotes {
            remotes: vec![],
            default_index: None,
        };
        assert_eq!(default_probe_url(&effective), None);
    }

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
