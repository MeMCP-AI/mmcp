//! Tauri-managed application state.
//!
//! Built once in `lib::run`'s setup closure and handed to every
//! command via `State<AppState>`. Holds everything that's expensive
//! to construct or has to outlive individual commands: the native
//! git backend, the group index, the optional sync bundle, and the
//! resolved commit author.

use std::sync::Arc;

use mmcp_core::config::ProjectConfig;
use mmcp_git::NativeBackend;
use mmcp_store::{
    GroupIndex, IndexResolver, MmcpHome, ResolvedAuthor, build_engine, config as project_config,
};
use mmcp_sync::{PendingQueue, SyncEngine};
use tokio::sync::Mutex;

use crate::error::{GuiError, GuiResult};

pub struct SyncBundle {
    pub engine: SyncEngine,
    pub resolver: IndexResolver,
    pub queue: Mutex<PendingQueue>,
    pub server_url: String,
}

pub struct AppState {
    pub backend: Arc<NativeBackend>,
    pub index: GroupIndex,
    pub sync: Option<SyncBundle>,
    pub author: ResolvedAuthor,
}

impl AppState {
    pub async fn discover() -> GuiResult<Self> {
        let home = MmcpHome::discover().map_err(GuiError::from)?;
        let repos_root = home.repos_root();
        let backend = Arc::new(NativeBackend::new(&repos_root).map_err(GuiError::from)?);
        let index = GroupIndex::build(repos_root, Arc::clone(&backend))
            .await
            .map_err(GuiError::from)?;
        let author = home.resolve_author();

        let sync = match load_project_sync()? {
            Some(cfg) => {
                let server_url = cfg.server_url.clone();
                let (engine, resolver, queue) =
                    build_engine(Arc::clone(&backend), index.clone(), &server_url)
                        .map_err(GuiError::from)?;
                Some(SyncBundle {
                    engine,
                    resolver,
                    queue: Mutex::new(queue),
                    server_url,
                })
            }
            None => None,
        };

        Ok(Self {
            backend,
            index,
            sync,
            author,
        })
    }
}

fn load_project_sync() -> GuiResult<Option<mmcp_core::config::SyncConfig>> {
    let Ok(cwd) = std::env::current_dir() else {
        return Ok(None);
    };
    let Some(root) = project_config::find_project_root(&cwd) else {
        return Ok(None);
    };
    let cfg: ProjectConfig = project_config::load(&root).map_err(GuiError::from)?;
    Ok(cfg.sync)
}
