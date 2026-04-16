//! `mmcp serve` implementation: MCP stdio server.
//!
//! Speaks real JSON-RPC 2.0 through the official `rmcp` crate.
//! Every exposed tool answers from real git content read via the
//! [`NativeBackend`] at `~/.mmcp/repos/`. No database is opened,
//! no placeholder responses are returned. Tools that need
//! per-session state (verification, compaction acknowledgement)
//! are not yet exposed on the MCP router — the `SessionStore` is
//! wired through so they can land without touching initialization.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use mmcp_core::id::GroupId;
use mmcp_core::memory::{MemoryFile, MemoryFrontmatter};
use mmcp_git::{GitBackend, NativeBackend, Rev};
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

use crate::config::{PROJECT_MANIFEST, find_project_root};
use crate::home::MmcpHome;
use crate::state::{GroupEntry, GroupIndex, SessionStore, WatcherHandle, spawn_watcher};

use mmcp_core::conventions::{MEMORIES_DIR, MEMORY_EXTENSION, memory_path};

/// Run the MCP stdio server loop until the client disconnects.
pub async fn run(debug_mode: bool) -> Result<()> {
    if debug_mode {
        tracing::info!("mmcp stdio MCP server starting (debug tools enabled)");
    } else {
        tracing::info!("mmcp stdio MCP server starting");
    }
    let state = ClientState::initialize(debug_mode).await?;
    let server = McpServer::new(state);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// Everything the MCP server needs to answer tool calls from local
/// state. No database. Git and flat session files only.
struct ClientStateInner {
    backend: Arc<NativeBackend>,
    groups: GroupIndex,
    #[allow(dead_code)] // NOTE: consumed by session-scoped tools once they're wired onto the router.
    sessions: SessionStore,
    #[allow(dead_code)] // NOTE: held to keep the notify watcher alive for the process lifetime.
    watcher: WatcherHandle,
    /// Resolved commit author from user config cascade.
    author: crate::home::ResolvedAuthor,
    /// Debug mode flag. When true, raw git access tools are enabled.
    /// Can be toggled at runtime via the `debug_toggle` tool.
    debug: Arc<AtomicBool>,
}

#[derive(Clone)]
struct ClientState(Arc<ClientStateInner>);

impl ClientState {
    async fn initialize(debug: bool) -> Result<Self> {
        let home = MmcpHome::discover()?;
        let project_config_path = find_current_project_config();
        Self::initialize_from(home, project_config_path, debug).await
    }

    /// Initialize the client state from a resolved [`MmcpHome`].
    ///
    /// Used by `initialize()` above (discovers from env) and by
    /// test helpers that supply a tempdir-backed home.
    async fn initialize_from(
        home: MmcpHome,
        project_config_path: Option<PathBuf>,
        debug: bool,
    ) -> Result<Self> {
        std::fs::create_dir_all(home.root())
            .with_context(|| format!("creating {}", home.root().display()))?;

        let (backend, groups) = home.init_backend().await?;

        let sessions_root = home.sessions_root();
        let sessions = SessionStore::open(&sessions_root)
            .with_context(|| format!("opening session store at {}", sessions_root.display()))?;

        let watcher = spawn_watcher(home.repos_root(), project_config_path, groups.clone())
            .context("spawning filesystem watcher")?;

        let author = home.resolve_author();

        Ok(Self(Arc::new(ClientStateInner {
            backend,
            groups,
            sessions,
            watcher,
            author,
            debug: Arc::new(AtomicBool::new(debug)),
        })))
    }
}

impl std::ops::Deref for ClientState {
    type Target = ClientStateInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn find_current_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let root = find_project_root(&cwd)?;
    Some(root.join(PROJECT_MANIFEST))
}

/// MCP server exposing the stateless mmcp tools that can be served
/// purely from local git repos.
#[derive(Clone)]
struct McpServer {
    state: ClientState,
    // NOTE: `tool_router` is read through the `#[tool_handler]`
    // macro's generated plumbing, not from our own code.
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
struct ReadMemoryArgs {
    /// Group UUID that owns the memory.
    pub group: String,
    /// Memory slug (file name under `memories/` without the `.md` extension).
    pub slug: String,
    /// Optional branch name, tag name, or commit hex. Defaults to `main`.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ListVersionsArgs {
    /// Group UUID that owns the memory.
    pub group: String,
    /// Memory slug.
    pub slug: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct GroupInfoArgs {
    /// Group UUID to inspect.
    pub group: String,
}

/// Memory kind enum exposed to the MCP JSON schema so AI clients
/// see the valid variants in the tool definition, never free-form.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum ToolMemoryKind {
    Rule,
    Snapshot,
    Log,
    Reference,
    Scratch,
}

impl ToolMemoryKind {
    fn into_core(self) -> mmcp_core::memory::MemoryKind {
        match self {
            ToolMemoryKind::Rule => mmcp_core::memory::MemoryKind::Rule,
            ToolMemoryKind::Snapshot => mmcp_core::memory::MemoryKind::Snapshot,
            ToolMemoryKind::Log => mmcp_core::memory::MemoryKind::Log,
            ToolMemoryKind::Reference => mmcp_core::memory::MemoryKind::Reference,
            ToolMemoryKind::Scratch => mmcp_core::memory::MemoryKind::Scratch,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct WriteMemoryArgs {
    /// Target group UUID or slug.
    pub group: String,
    /// Memory slug (lowercase alphanumeric + hyphens).
    pub slug: String,
    /// Human-readable title.
    pub name: String,
    /// One-line summary for relevance inference.
    pub description: String,
    /// Memory kind.
    pub kind: ToolMemoryKind,
    /// Markdown body content (no frontmatter - the server builds it).
    pub body: String,
    /// Free-form classification tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether this memory must be read at least once per session.
    #[serde(default)]
    pub mandatory: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct SearchMemoriesArgs {
    /// Substring matched against memory slug and frontmatter name,
    /// case-insensitive.
    pub query: String,
    /// Optional maximum number of hits. Defaults to 50.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct CheckHealthArgs {
    /// Group UUID to check. If omitted, checks all groups.
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct DebugToggleArgs {
    /// Set to true to enable debug tools, false to disable.
    pub enabled: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct DebugReadFileArgs {
    /// Group UUID.
    pub group: String,
    /// File path inside the repo (e.g. "memories/my-mem.md" or ".mmcp.toml").
    pub path: String,
    /// Optional revision (branch, tag, or commit hex). Defaults to main.
    #[serde(default)]
    pub rev: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct DebugListTreeArgs {
    /// Group UUID.
    pub group: String,
    /// Path prefix to list under. Empty string for repo root.
    #[serde(default)]
    pub prefix: Option<String>,
    /// Optional revision. Defaults to main.
    #[serde(default)]
    pub rev: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct DebugGitLogArgs {
    /// Group UUID.
    pub group: String,
    /// Optional file path to filter history by. Defaults to .mmcp.toml.
    #[serde(default)]
    pub path: Option<String>,
    /// Max commits to return. Defaults to 20.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct DebugWriteFileArgs {
    /// Group UUID.
    pub group: String,
    /// File path inside the repo.
    pub path: String,
    /// File content as a string.
    pub content: String,
    /// Optional commit message.
    #[serde(default)]
    pub message: Option<String>,
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
        description = "List memories that live in the specified group. The group argument is the group UUID. Returns an empty list if the group is unknown or contains no memories."
    )]
    async fn list_memories(
        &self,
        Parameters(args): Parameters<ListMemoriesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_group_id(&args.group)?;
        let Some(entry) = self.state.groups.get(&group_id).await else {
            return Ok(ok_json(json!({ "memories": [] })));
        };
        let files = list_memory_files(&self.state.backend, &entry).await?;
        let mut memories = Vec::with_capacity(files.len());
        for slug in files {
            let descriptor = read_memory_descriptor(&self.state.backend, &entry, &slug, None)
                .await
                .map_err(git_error)?;
            memories.push(descriptor);
        }
        Ok(ok_json(json!({
            "group": entry.manifest.group_id,
            "memories": memories,
        })))
    }

    #[tool(
        description = "Read a memory by group and slug. Returns the TOML frontmatter and the Markdown body exactly as stored in git. Set `version` to a branch name, tag, or commit hex to read a specific revision; defaults to the latest `main`."
    )]
    async fn read_memory(
        &self,
        Parameters(args): Parameters<ReadMemoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_group_id(&args.group)?;
        let entry = self.state.groups.get(&group_id).await.ok_or_else(|| {
            McpError::invalid_params(
                "group not found in local mirror",
                Some(json!({ "group": group_id.to_string() })),
            )
        })?;
        let rev = parse_rev(args.version.as_deref());
        let path = memory_path(&args.slug);
        let bytes = self
            .state
            .backend
            .read_file(&entry.handle, &path, &rev)
            .await
            .map_err(|e| match e {
                mmcp_git::GitError::PathNotFound(p) => McpError::invalid_params(
                    "memory not found in group",
                    Some(json!({ "group": group_id.to_string(), "path": p })),
                ),
                mmcp_git::GitError::RevNotFound(r) => McpError::invalid_params(
                    "revision not found",
                    Some(json!({ "revision": r })),
                ),
                other => git_error(other),
            })?;
        let text = std::str::from_utf8(&bytes).map_err(|e| {
            McpError::internal_error(
                Cow::Owned(format!("memory file is not valid UTF-8: {e}")),
                None,
            )
        })?;
        let file = MemoryFile::parse(text).map_err(|e| {
            McpError::invalid_params(
                Cow::Owned(format!("memory frontmatter did not parse: {e}")),
                Some(json!({ "slug": args.slug })),
            )
        })?;
        Ok(ok_json(json!({
            "group": entry.manifest.group_id,
            "slug": args.slug,
            "version": rev_label(&rev),
            "frontmatter": frontmatter_to_json(&file.frontmatter),
            "body": file.body,
        })))
    }

    #[tool(
        description = "List the commit history of a single memory, most recent first. Each entry includes the commit id, author, message, and timestamp."
    )]
    async fn list_versions(
        &self,
        Parameters(args): Parameters<ListVersionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_group_id(&args.group)?;
        let entry = self.state.groups.get(&group_id).await.ok_or_else(|| {
            McpError::invalid_params(
                "group not found in local mirror",
                Some(json!({ "group": group_id.to_string() })),
            )
        })?;
        let path = memory_path(&args.slug);
        let history = self
            .state
            .backend
            .walk_history(&entry.handle, &path)
            .await
            .map_err(git_error)?;
        let versions: Vec<serde_json::Value> = history
            .into_iter()
            .map(|c| {
                json!({
                    "commit": c.id,
                    "subject": c.subject,
                    "message": c.message,
                    "author_name": c.author_name,
                    "author_email": c.author_email,
                    "timestamp": c.timestamp,
                })
            })
            .collect();
        Ok(ok_json(json!({
            "group": entry.manifest.group_id,
            "slug": args.slug,
            "versions": versions,
        })))
    }

    #[tool(
        description = "Return the manifest metadata for a group: slug, display name, owner kind and id, creation timestamp, and the number of memories currently stored in the group."
    )]
    async fn group_info(
        &self,
        Parameters(args): Parameters<GroupInfoArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_group_id(&args.group)?;
        let entry = self.state.groups.get(&group_id).await.ok_or_else(|| {
            McpError::invalid_params(
                "group not found in local mirror",
                Some(json!({ "group": group_id.to_string() })),
            )
        })?;
        let files = list_memory_files(&self.state.backend, &entry).await?;
        let owner = owner_hint_to_json(&entry.manifest.owner);
        Ok(ok_json(json!({
            "id": entry.manifest.group_id,
            "slug": entry.manifest.slug,
            "display_name": entry.manifest.display_name,
            "owner": owner,
            "schema_version": entry.manifest.schema_version,
            "created_at": entry.manifest.created_at,
            "memory_count": files.len(),
        })))
    }

    #[tool(
        description = "Case-insensitive substring search across every group in the local mirror. Matches against the memory slug and the frontmatter `name` field. Returns up to `limit` hits (default 50)."
    )]
    async fn search_memories(
        &self,
        Parameters(args): Parameters<SearchMemoriesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let needle = args.query.trim().to_lowercase();
        if needle.is_empty() {
            return Err(McpError::invalid_params("query must not be empty", None));
        }
        let limit = args.limit.unwrap_or(50).max(1) as usize;
        let mut hits: Vec<serde_json::Value> = Vec::new();
        for entry in self.state.groups.list().await {
            if hits.len() >= limit {
                break;
            }
            let slugs = list_memory_files(&self.state.backend, &entry).await?;
            for slug in slugs {
                if hits.len() >= limit {
                    break;
                }
                let matches_slug = slug.to_lowercase().contains(&needle);
                let descriptor = match read_memory_descriptor(
                    &self.state.backend,
                    &entry,
                    &slug,
                    None,
                )
                .await
                {
                    Ok(d) => d,
                    Err(err) => {
                        tracing::warn!(slug = %slug, error = %err, "search: descriptor read failed, skipping");
                        continue;
                    }
                };
                let matches_name = descriptor
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|n| n.to_lowercase().contains(&needle))
                    .unwrap_or(false);
                if matches_slug || matches_name {
                    hits.push(descriptor);
                }
            }
        }
        Ok(ok_json(json!({
            "hits": hits,
        })))
    }

    #[tool(
        description = "Write a memory into a group. All metadata fields (name, description, kind, tags, mandatory) are typed parameters - the server builds the frontmatter. If the slug already exists it is overwritten with a new commit."
    )]
    async fn write_memory(
        &self,
        Parameters(args): Parameters<WriteMemoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_group_id(&args.group)?;
        let entry = self
            .state
            .groups
            .get(&group_id)
            .await
            .ok_or_else(|| McpError::invalid_params("group not found", None))?;

        let kind = args.kind.into_core();

        use mmcp_core::memory::{FrontmatterFormat, MemoryFile, MemoryFrontmatter};
        let file = MemoryFile {
            frontmatter: MemoryFrontmatter {
                name: args.name,
                description: args.description,
                kind,
                mandatory: args.mandatory,
                version: None,
                tags: args.tags,
                bump_intent: None,
            },
            body: args.body,
            format: FrontmatterFormat::TomlPlus,
        };
        let rendered = file
            .to_string()
            .map_err(|e| McpError::internal_error(Cow::Owned(e.to_string()), None))?;

        let result = crate::commands::import::import_memory(
            &self.state.backend,
            &entry.handle,
            &args.slug,
            &rendered,
            None,
            &self.state.author,
        )
        .await
        .map_err(|e| McpError::internal_error(Cow::Owned(e.to_string()), None))?;
        Ok(ok_json(json!({
            "slug": result.slug,
            "commit_id": result.commit_id,
            "group": args.group,
        })))
    }

    #[tool(
        description = "Validate manifests and memory frontmatter for a group. Returns issues found: parse errors, missing required fields, empty bodies. Checks one group if group UUID given, all groups if omitted."
    )]
    async fn check_health(
        &self,
        Parameters(args): Parameters<CheckHealthArgs>,
    ) -> Result<CallToolResult, McpError> {
        let reports = if let Some(ref group_str) = args.group {
            let group_id = parse_group_id(group_str)?;
            let entry = self
                .state
                .groups
                .get(&group_id)
                .await
                .ok_or_else(|| McpError::invalid_params("group not found", None))?;
            vec![crate::commands::health::health_check_group(&self.state.backend, &entry).await]
        } else {
            crate::commands::health::health_check_all(&self.state.backend, &self.state.groups).await
        };
        let total_issues: usize = reports.iter().map(|r| r.issues.len()).sum();
        Ok(ok_json(json!({
            "groups": reports,
            "total_issues": total_issues,
            "healthy": total_issues == 0,
        })))
    }

    #[tool(
        description = "Deep diagnostic analysis of a group's memories. Everything check_health does plus: missing tags, empty bodies, naming drift, empty groups, UUID mismatches, created_at sanity, cross-group duplicate slugs, and structural hints. Severity levels: error, warning, info."
    )]
    async fn diagnose(
        &self,
        Parameters(args): Parameters<CheckHealthArgs>,
    ) -> Result<CallToolResult, McpError> {
        let diag = if let Some(ref group_str) = args.group {
            let group_id = parse_group_id(group_str)?;
            let entry = self
                .state
                .groups
                .get(&group_id)
                .await
                .ok_or_else(|| McpError::invalid_params("group not found", None))?;
            crate::commands::health::DiagReport {
                project_issues: Vec::new(),
                groups: vec![crate::commands::health::diagnose_group(&self.state.backend, &entry).await],
            }
        } else {
            crate::commands::health::diagnose_all(&self.state.backend, &self.state.groups).await
        };
        let all_issues = diag.groups.iter().flat_map(|r| &r.issues).chain(diag.project_issues.iter());
        let errors: usize = all_issues.clone().filter(|i| i.severity == "error").count();
        let warnings: usize = all_issues.clone().filter(|i| i.severity == "warning").count();
        let infos: usize = all_issues.filter(|i| i.severity == "info").count();
        Ok(ok_json(json!({
            "project_issues": diag.project_issues,
            "groups": diag.groups,
            "errors": errors,
            "warnings": warnings,
            "infos": infos,
            "healthy": errors == 0,
        })))
    }

    // ── Debug tools ─────────────────────────────────────────

    #[tool(
        description = "Enable or disable debug tools. Debug tools provide raw git access for troubleshooting. Pass enabled=true to activate, enabled=false to deactivate. Returns the new state."
    )]
    async fn debug_toggle(
        &self,
        Parameters(args): Parameters<DebugToggleArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.state.debug.store(args.enabled, Ordering::Relaxed);
        Ok(ok_json(json!({
            "debug": args.enabled,
            "message": if args.enabled { "debug tools enabled" } else { "debug tools disabled" },
        })))
    }

    #[tool(
        description = "Read any file at any path in a group's git repo. Requires debug mode. Use for inspecting raw repo state."
    )]
    async fn debug_read_file(
        &self,
        Parameters(args): Parameters<DebugReadFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.require_debug()?;
        let group_id = parse_group_id(&args.group)?;
        let entry = self
            .state
            .groups
            .get(&group_id)
            .await
            .ok_or_else(|| McpError::invalid_params("group not found", None))?;
        let rev = parse_rev(args.rev.as_deref());
        let bytes = self
            .state
            .backend
            .read_file(&entry.handle, &args.path, &rev)
            .await
            .map_err(git_error)?;
        let text = String::from_utf8_lossy(&bytes);
        Ok(ok_json(json!({
            "path": args.path,
            "rev": rev_label(&rev),
            "content": text,
            "size_bytes": bytes.len(),
        })))
    }

    #[tool(
        description = "List all files (blobs) under a path prefix in a group's git repo. Requires debug mode."
    )]
    async fn debug_list_tree(
        &self,
        Parameters(args): Parameters<DebugListTreeArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.require_debug()?;
        let group_id = parse_group_id(&args.group)?;
        let entry = self
            .state
            .groups
            .get(&group_id)
            .await
            .ok_or_else(|| McpError::invalid_params("group not found", None))?;
        let rev = parse_rev(args.rev.as_deref());
        let prefix = args.prefix.as_deref().unwrap_or("");
        let files = self
            .state
            .backend
            .list_tree(&entry.handle, prefix, &rev)
            .await
            .map_err(git_error)?;
        Ok(ok_json(json!({
            "prefix": prefix,
            "rev": rev_label(&rev),
            "files": files,
            "count": files.len(),
        })))
    }

    #[tool(
        description = "Show raw git commit history for the entire repo or a specific path. Requires debug mode."
    )]
    async fn debug_git_log(
        &self,
        Parameters(args): Parameters<DebugGitLogArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.require_debug()?;
        let group_id = parse_group_id(&args.group)?;
        let entry = self
            .state
            .groups
            .get(&group_id)
            .await
            .ok_or_else(|| McpError::invalid_params("group not found", None))?;
        let path = args.path.as_deref().unwrap_or(mmcp_core::manifest::MANIFEST_FILENAME);
        let history = self
            .state
            .backend
            .walk_history(&entry.handle, path)
            .await
            .map_err(git_error)?;
        let limit = args.limit.unwrap_or(20) as usize;
        let commits: Vec<_> = history
            .into_iter()
            .take(limit)
            .map(|c| {
                json!({
                    "id": c.id,
                    "subject": c.subject,
                    "author": c.author_name,
                    "timestamp": c.timestamp,
                })
            })
            .collect();
        Ok(ok_json(json!({
            "path": path,
            "commits": commits,
            "count": commits.len(),
        })))
    }

    #[tool(
        description = "Write any file at any path in a group's git repo. Requires debug mode. Use for low-level repairs."
    )]
    async fn debug_write_file(
        &self,
        Parameters(args): Parameters<DebugWriteFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.require_debug()?;
        let group_id = parse_group_id(&args.group)?;
        let entry = self
            .state
            .groups
            .get(&group_id)
            .await
            .ok_or_else(|| McpError::invalid_params("group not found", None))?;
        let commit_id = self
            .state
            .backend
            .write_commit(
                &entry.handle,
                mmcp_git::CommitSpec::mmcp_commit(
                    args.message.as_deref().unwrap_or("debug: write file"),
                    vec![(args.path.clone(), Some(args.content.into_bytes()))],
                    &self.state.author.name,
                    &self.state.author.email,
                ),
            )
            .await
            .map_err(git_error)?;
        Ok(ok_json(json!({
            "path": args.path,
            "commit_id": commit_id,
        })))
    }
}

impl McpServer {
    fn require_debug(&self) -> Result<(), McpError> {
        if self.state.debug.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(McpError::invalid_params(
                "debug tools are disabled; call debug_toggle(enabled=true) first",
                None,
            ))
        }
    }
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "mmcp memory server. Reads and writes memories directly from git repositories under ~/.mmcp/repos. Exposes list_memories, read_memory, list_versions, group_info, search_memories, and write_memory. All metadata is typed - the server builds frontmatter from structured parameters."
                    .to_string(),
            )
    }
}

/// List every `memories/<slug>.md` blob in the group's repo at
/// `main` and return the slugs without the `.md` extension.
async fn list_memory_files(
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> Result<Vec<String>, McpError> {
    let files = backend
        .list_tree(
            &entry.handle,
            MEMORIES_DIR,
            &Rev::main(),
        )
        .await
        .map_err(git_error)?;
    Ok(files
        .into_iter()
        .filter_map(|name| name.strip_suffix(MEMORY_EXTENSION).map(str::to_string))
        .collect())
}

/// Read one memory and return a compact descriptor including the
/// slug, the parsed frontmatter fields, and a short summary.
async fn read_memory_descriptor(
    backend: &NativeBackend,
    entry: &GroupEntry,
    slug: &str,
    version: Option<&str>,
) -> Result<serde_json::Value, mmcp_git::GitError> {
    let rev = parse_rev(version);
    let path = memory_path(slug);
    let bytes = backend.read_file(&entry.handle, &path, &rev).await?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (name, description, kind, mandatory, version_str, tags) = match MemoryFile::parse(&text) {
        Ok(file) => (
            Some(file.frontmatter.name),
            Some(file.frontmatter.description),
            file.frontmatter.kind.as_str().to_string(),
            file.frontmatter.mandatory,
            file.frontmatter.version.map(|v| v.to_string()),
            file.frontmatter.tags,
        ),
        Err(_) => (None, None, "rule".to_string(), false, None, Vec::new()),
    };
    Ok(json!({
        "group": entry.manifest.group_id,
        "slug": slug,
        "name": name,
        "description": description,
        "kind": kind,
        "mandatory": mandatory,
        "latest_version": version_str,
        "tags": tags,
    }))
}

fn parse_group_id(value: &str) -> Result<GroupId, McpError> {
    let uuid = Uuid::parse_str(value).map_err(|_| {
        McpError::invalid_params(
            "group is not a valid UUID",
            Some(json!({ "group": value })),
        )
    })?;
    Ok(GroupId::from_uuid(uuid))
}

fn parse_rev(value: Option<&str>) -> Rev {
    match value {
        None => Rev::main(),
        Some(v) => {
            // Heuristic: 40-char hex string -> commit, otherwise branch.
            if v.len() == 40 && v.chars().all(|c| c.is_ascii_hexdigit()) {
                Rev::Commit(v.to_string())
            } else {
                Rev::Branch(v.to_string())
            }
        }
    }
}

fn rev_label(rev: &Rev) -> String {
    match rev {
        Rev::Branch(b) => format!("branch:{b}"),
        Rev::Tag(t) => format!("tag:{t}"),
        Rev::Commit(c) => format!("commit:{c}"),
    }
}

fn git_error(err: mmcp_git::GitError) -> McpError {
    McpError::internal_error(Cow::Owned(format!("git error: {err}")), None)
}

fn frontmatter_to_json(fm: &MemoryFrontmatter) -> serde_json::Value {
    json!({
        "name": fm.name,
        "description": fm.description,
        "kind": fm.kind.as_str(),
        "mandatory": fm.mandatory,
        "version": fm.version.as_ref().map(|v| v.to_string()),
        "tags": fm.tags,
        "bump_intent": fm.bump_intent,
    })
}

fn owner_hint_to_json(owner: &mmcp_core::manifest::GroupOwnerHint) -> serde_json::Value {
    use mmcp_core::manifest::GroupOwnerHint;
    match owner {
        GroupOwnerHint::User(id) => json!({ "kind": "user", "id": id }),
        GroupOwnerHint::Org(id) => json!({ "kind": "org", "id": id }),
    }
}

fn ok_json(value: serde_json::Value) -> CallToolResult {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    CallToolResult::success(vec![Content::text(Cow::Owned(text))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmcp_core::id::GroupId;
    use mmcp_core::manifest::GroupManifest;
    use mmcp_git::{CommitSpec, GitBackend};
    use tempfile::TempDir;

    /// Build a `ClientState` rooted inside a fresh tempdir so the
    /// test never touches the real user home.
    async fn test_state() -> (ClientState, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let home = crate::home::MmcpHome::from_root(tmp.path().join("mmcp-home"));
        let state = ClientState::initialize_from(home, None, false)
            .await
            .expect("initialize_from");
        (state, tmp)
    }

    /// Seed a real group repository in the client's repos root with
    /// a manifest (via `create_group_repo`) and one memory file
    /// committed on `main`.
    async fn seed_group_with_memory(
        state: &ClientState,
        slug: &str,
        memory_slug: &str,
        memory_body: &str,
    ) -> GroupId {
        let owner = Uuid::now_v7();
        let group_id = GroupId::new();
        let manifest = GroupManifest::new_user_owned(group_id, slug, owner);
        let handle = state
            .backend
            .create_group_repo(&manifest)
            .await
            .expect("create group repo");
        state
            .backend
            .write_commit(
                &handle,
                CommitSpec {
                    branch: mmcp_core::conventions::MAIN_BRANCH.to_string(),
                    author_name: "test".into(),
                    author_email: "test@example.com".into(),
                    message: format!("seed memory {memory_slug}"),
                    files: vec![(
                        memory_path(memory_slug),
                        Some(memory_body.as_bytes().to_vec()),
                    )],
                },
            )
            .await
            .expect("write commit");
        state.groups.refresh().await.expect("refresh");
        group_id
    }

    fn parse_ok_json(result: CallToolResult) -> serde_json::Value {
        assert!(!result.content.is_empty(), "tool result has no content");
        let text = result.content[0]
            .as_text()
            .expect("text content")
            .text
            .clone();
        serde_json::from_str(&text).expect("json parse")
    }

    const SAMPLE_MEMORY: &str = "+++\nname = \"Sample\"\ndescription = \"A sample memory\"\nkind = \"rule\"\nmandatory = false\ntags = [\"sample\"]\n+++\n# Sample\nBody text.\n";

    #[tokio::test]
    async fn list_memories_returns_real_entries_from_git() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);

        let res = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: group.to_string(),
            }))
            .await
            .expect("list_memories");
        let parsed = parse_ok_json(res);
        let memories = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .expect("memories array");
        assert_eq!(memories.len(), 1);
        let first = &memories[0];
        assert_eq!(first.get("slug").and_then(|v| v.as_str()), Some("rules"));
        assert_eq!(
            first.get("name").and_then(|v| v.as_str()),
            Some("Sample"),
            "frontmatter name should be parsed"
        );
    }

    #[tokio::test]
    async fn read_memory_returns_frontmatter_and_body() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);

        let res = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: "rules".into(),
                version: None,
            }))
            .await
            .expect("read_memory");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed
                .get("frontmatter")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("Sample")
        );
        let body = parsed
            .get("body")
            .and_then(|v| v.as_str())
            .expect("body string");
        assert!(body.contains("# Sample"));
        assert!(body.contains("Body text."));
    }

    #[tokio::test]
    async fn read_memory_rejects_unknown_slug() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);

        let err = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: "missing".into(),
                version: None,
            }))
            .await
            .expect_err("should be an error");
        assert!(
            err.message.contains("memory not found"),
            "expected memory-not-found error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn group_info_reports_manifest_and_memory_count() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);

        let res = server
            .group_info(Parameters(GroupInfoArgs {
                group: group.to_string(),
            }))
            .await
            .expect("group_info");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("slug").and_then(|v| v.as_str()),
            Some("team-rust")
        );
        assert_eq!(
            parsed.get("memory_count").and_then(|v| v.as_u64()),
            Some(1)
        );
        let owner_kind = parsed
            .get("owner")
            .and_then(|v| v.get("kind"))
            .and_then(|v| v.as_str());
        assert_eq!(owner_kind, Some("user"));
    }

    #[tokio::test]
    async fn search_memories_matches_slug_substring() {
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "team-rust", "coding-rules", SAMPLE_MEMORY).await;
        seed_group_with_memory(&state, "team-python", "style-guide", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);

        let res = server
            .search_memories(Parameters(SearchMemoriesArgs {
                query: "coding".into(),
                limit: None,
            }))
            .await
            .expect("search_memories");
        let parsed = parse_ok_json(res);
        let hits = parsed
            .get("hits")
            .and_then(|v| v.as_array())
            .expect("hits array");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].get("slug").and_then(|v| v.as_str()),
            Some("coding-rules")
        );
    }

    #[tokio::test]
    async fn list_versions_returns_commit_history_for_memory() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);

        let res = server
            .list_versions(Parameters(ListVersionsArgs {
                group: group.to_string(),
                slug: "rules".into(),
            }))
            .await
            .expect("list_versions");
        let parsed = parse_ok_json(res);
        let versions = parsed
            .get("versions")
            .and_then(|v| v.as_array())
            .expect("versions array");
        assert_eq!(versions.len(), 1);
        let commit = versions[0]
            .get("commit")
            .and_then(|v| v.as_str())
            .expect("commit string");
        assert_eq!(commit.len(), 40, "commit should be a 40-char hex id");
    }

    #[tokio::test]
    async fn unknown_group_id_returns_empty_list_or_invalid_params() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state);
        let unknown = Uuid::now_v7();

        let res = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: unknown.to_string(),
            }))
            .await
            .expect("list_memories");
        let parsed = parse_ok_json(res);
        let memories = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .expect("memories array");
        assert!(memories.is_empty());

        let err = server
            .group_info(Parameters(GroupInfoArgs {
                group: unknown.to_string(),
            }))
            .await
            .expect_err("group_info on unknown id should error");
        assert!(err.message.contains("group not found"));
    }
}
