//! `mmcp serve` implementation: MCP stdio server.
//!
//! Speaks real JSON-RPC 2.0 through the official `rmcp` crate.
//! Every exposed tool performs an actual operation against the
//! local mmcp state (the SQLite mirror under `~/.mmcp/local.db`
//! and the native git backend under `~/.mmcp/repos`). Tools that
//! cannot yet be served with the data available locally are not
//! exposed at all, so callers never see a placeholder response.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use jiff::Timestamp;
use mmcp_db::entities::group::OwnerKind;
use mmcp_db::entities::memory::MemoryKind;
use mmcp_db::repository::{group_repo, memory_repo};
use mmcp_db::{Database, connect};
use mmcp_git::NativeBackend;
use mmcp_session::SessionTracker;
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

/// Run the MCP stdio server loop until the client disconnects.
pub async fn run() -> Result<()> {
    tracing::info!("mmcp stdio MCP server starting");
    let state = ClientState::initialize().await?;
    let server = McpServer::new(state);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// Everything the MCP server needs to answer tool calls against
/// local state: the SQLite mirror and the bare-repo backend rooted
/// under `~/.mmcp`.
struct ClientStateInner {
    database: Database,
    #[allow(dead_code)] // NOTE: wired into read/write handlers in a follow-up.
    git: NativeBackend,
    #[allow(dead_code)] // NOTE: wired into verify/session-scoped handlers in a follow-up.
    sessions: SessionTracker,
}

#[derive(Clone)]
struct ClientState(Arc<ClientStateInner>);

impl ClientState {
    async fn initialize() -> Result<Self> {
        let home = local_home()?;
        let dir = home.join(".mmcp");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;

        let db_path = dir.join("local.db");
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let database = connect(&db_url)
            .await
            .with_context(|| format!("opening {}", db_path.display()))?;
        database.migrate().await.context("running local migrations")?;

        let repos_root = dir.join("repos");
        let git = NativeBackend::new(&repos_root)
            .with_context(|| format!("initializing repo root {}", repos_root.display()))?;

        let sessions = SessionTracker::new(database.connection().clone());

        Ok(Self(Arc::new(ClientStateInner {
            database,
            git,
            sessions,
        })))
    }
}

impl std::ops::Deref for ClientState {
    type Target = ClientStateInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn local_home() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }
    Err(anyhow!(
        "cannot determine home directory: set HOME or USERPROFILE"
    ))
}

/// MCP server exposing the mmcp tools that can be served purely
/// from local state.
#[derive(Clone)]
struct McpServer {
    state: ClientState,
    // NOTE: `tool_router` is read through the `#[tool_handler]`
    // macro's generated plumbing, not from our own code. The
    // dead-code warning is a framework quirk scoped to this field.
    #[allow(dead_code)]
    tool_router: ToolRouter<McpServer>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ListMemoriesArgs {
    /// Group UUID to list memories from.
    pub group: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ListVersionsArgs {
    /// Memory UUID to query.
    pub memory: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct GroupInfoArgs {
    /// Group UUID to inspect.
    pub group: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct SearchMemoriesArgs {
    /// Substring matched against memory slugs, case-insensitive.
    pub query: String,
    /// Optional maximum number of hits. Defaults to 50.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[tool_router]
impl McpServer {
    fn new(state: ClientState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List memories that live in the specified group (UUID). Returns an empty list if the group is unknown to the local mirror."
    )]
    async fn list_memories(
        &self,
        Parameters(args): Parameters<ListMemoriesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_uuid(&args.group, "group")?;
        let rows = memory_repo::list_in_group(self.state.database.connection(), group_id)
            .await
            .map_err(db_error)?;
        let descriptors: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "group": m.group_id,
                    "slug": m.slug,
                    "kind": kind_to_string(m.kind),
                    "mandatory": m.mandatory,
                    "latest_version": m.latest_version,
                    "updated_at": m.updated_at,
                })
            })
            .collect();
        Ok(ok_json(json!({ "memories": descriptors })))
    }

    #[tool(
        description = "List the published version history of a memory by UUID, oldest first."
    )]
    async fn list_versions(
        &self,
        Parameters(args): Parameters<ListVersionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let memory_id = parse_uuid(&args.memory, "memory")?;
        let rows = memory_repo::list_versions(self.state.database.connection(), memory_id)
            .await
            .map_err(db_error)?;
        let versions: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|v| {
                json!({
                    "version": v.version,
                    "commit": v.commit,
                    "author": v.author_id,
                    "published_at": v.published_at,
                    "summary": v.summary,
                })
            })
            .collect();
        Ok(ok_json(json!({
            "memory": memory_id,
            "versions": versions,
        })))
    }

    #[tool(
        description = "Return metadata about a group: slug, owner, display name, and the number of memories currently known to the local mirror."
    )]
    async fn group_info(
        &self,
        Parameters(args): Parameters<GroupInfoArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_uuid(&args.group, "group")?;
        let conn = self.state.database.connection();
        let group = group_repo::find_by_id(conn, group_id)
            .await
            .map_err(db_error)?
            .ok_or_else(|| {
                McpError::invalid_params(
                    "group not found in local mirror",
                    Some(json!({ "group": group_id.to_string() })),
                )
            })?;
        let memories = memory_repo::list_in_group(conn, group_id)
            .await
            .map_err(db_error)?;
        let owner = match group.owner_kind {
            OwnerKind::User => format!("user:{}", group.owner_id),
            OwnerKind::Org => format!("org:{}", group.owner_id),
        };
        Ok(ok_json(json!({
            "id": group.id,
            "slug": group.slug,
            "owner": owner,
            "display_name": group.display_name,
            "memory_count": memories.len(),
            "created_at": group.created_at,
        })))
    }

    #[tool(
        description = "Substring search against memory slugs in the local mirror. Matching is case-insensitive."
    )]
    async fn search_memories(
        &self,
        Parameters(args): Parameters<SearchMemoriesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let needle = args.query.to_lowercase();
        if needle.is_empty() {
            return Err(McpError::invalid_params("query must not be empty", None));
        }
        let limit = args.limit.unwrap_or(50).max(1) as u64;
        let rows = memory_repo::search_by_slug(
            self.state.database.connection(),
            &needle,
            limit,
        )
        .await
        .map_err(db_error)?;

        let hits: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "group": m.group_id,
                    "slug": m.slug,
                    "kind": kind_to_string(m.kind),
                    "mandatory": m.mandatory,
                    "latest_version": m.latest_version,
                })
            })
            .collect();
        let timestamp = Timestamp::now().as_millisecond();
        Ok(ok_json(json!({
            "hits": hits,
            "queried_at": timestamp,
        })))
    }
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "mmcp memory server (local-mirror mode). Exposes list_memories, list_versions, group_info, and search_memories backed by the SQLite mirror under ~/.mmcp/local.db. Tools requiring sync with a remote server or git content reads are not yet exposed in this build."
                    .to_string(),
            )
    }
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, McpError> {
    Uuid::parse_str(value).map_err(|_| {
        McpError::invalid_params(
            Cow::Owned(format!("{field} is not a valid UUID")),
            Some(json!({ field: value })),
        )
    })
}

fn db_error(err: mmcp_db::DbError) -> McpError {
    McpError::internal_error(
        Cow::Owned(format!("database error: {err}")),
        None,
    )
}

fn ok_json(value: serde_json::Value) -> CallToolResult {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    CallToolResult::success(vec![Content::text(Cow::Owned(text))])
}

fn kind_to_string(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Rule => "rule",
        MemoryKind::Snapshot => "snapshot",
        MemoryKind::Log => "log",
        MemoryKind::Reference => "reference",
        MemoryKind::Scratch => "scratch",
    }
}
