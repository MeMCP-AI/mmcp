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

use crate::config::{PROJECT_MANIFEST, find_project_root, load as load_project_config};
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
    /// Opt into replacing an already-existing memory at this slug.
    /// `false` (default) makes the tool a strict CREATE — the wire
    /// name is `override` via serde rename; the Rust field uses a
    /// suffix to sidestep the reserved keyword. Callers almost
    /// never want this; prefer `edit_memory` for partial updates
    /// and reach for `override` only on deliberate replace-whole-
    /// file flows.
    #[serde(default, rename = "override")]
    pub override_: bool,
}

/// Argument shape for `edit_memory`.
///
/// Every mutator field is optional; the server reads the existing
/// memory file, applies the supplied deltas, re-renders, and
/// commits. `tags_add` / `tags_remove` compose cleanly under repeat
/// calls so callers don't have to fetch-merge-write the tag vector
/// themselves. All-None args still produce a commit — the edit
/// history stays explicit rather than collapsing no-op calls.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct EditMemoryArgs {
    /// Target group UUID.
    pub group: String,
    /// Memory slug.
    pub slug: String,
    /// Replace the markdown body verbatim. Absent leaves it
    /// untouched.
    #[serde(default)]
    pub body: Option<String>,
    /// Replace the human-readable title. Absent leaves it
    /// untouched.
    #[serde(default)]
    pub name: Option<String>,
    /// Replace the one-line description.
    #[serde(default)]
    pub description: Option<String>,
    /// Replace the memory kind.
    #[serde(default)]
    pub kind: Option<ToolMemoryKind>,
    /// Tags to insert into the existing set; duplicates are
    /// collapsed.
    #[serde(default)]
    pub tags_add: Vec<String>,
    /// Tags to strip from the existing set; missing tags are
    /// silently ignored.
    #[serde(default)]
    pub tags_remove: Vec<String>,
    /// Replace the mandatory flag. Absent leaves it untouched.
    #[serde(default)]
    pub mandatory: Option<bool>,
    /// Commit message override. Absent falls back to
    /// `"update memory {slug}"`.
    #[serde(default)]
    pub message: Option<String>,
}

/// Argument shape for `delete_memory`.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct DeleteMemoryArgs {
    /// Target group UUID.
    pub group: String,
    /// Memory slug to remove.
    pub slug: String,
    /// Commit message override. Absent falls back to
    /// `"delete memory {slug}"`.
    #[serde(default)]
    pub message: Option<String>,
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

/// Which memories `bootstrap_context` should return. Defaults to
/// `all` (union of mandatory and project). Callers pick `mandatory`
/// or `project` when they want to reload only one side without
/// re-paying the cost of the other.
#[derive(Debug, Default, Clone, Copy, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum BootstrapScope {
    /// Memories with `frontmatter.mandatory == true`, across every
    /// group the local mirror knows about.
    Mandatory,
    /// Memories inside the group whose UUID matches the project's
    /// `project_uuid` from `.mmcp.toml`.
    Project,
    /// Union of mandatory and project.
    #[default]
    All,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct BootstrapContextArgs {
    /// Which memories to include. Defaults to `all`.
    #[serde(default)]
    pub scope: Option<BootstrapScope>,
}

/// Action for `init_claude`. Matches the CLI's mutually-exclusive flag
/// set (override / append / convert).
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum InitClaudeAction {
    /// Replace CLAUDE.md with a fresh mmcp stub.
    Override,
    /// Insert or replace the mmcp-managed fence inside CLAUDE.md.
    Append,
    /// Split CLAUDE.md into typed memories, then replace with stub.
    Convert,
}

/// Pre-supplied answer to the dirty-file conflict question. Callers
/// who know they want to override / backup+override / cancel up front
/// set this on the first call; otherwise the tool returns a
/// `conflict_unresolved` error listing the required follow-up.
///
/// This is the serializable counterpart to the CLI's interactive
/// prompt. Future rmcp releases that expose `ElicitationRequest` can
/// replace the error-then-retry contract with a synchronous prompt —
/// the argument's shape stays the same.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum InitClaudeConflict {
    /// Overwrite the existing file without writing a .bak copy.
    Override,
    /// Write CLAUDE.md.bak, then overwrite.
    BackupOverride,
    /// Abort; do not touch CLAUDE.md.
    Cancel,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct InitClaudeArgs {
    /// Which action to apply.
    pub action: InitClaudeAction,
    /// Always write a .bak copy before modifying (true) or never (false).
    /// Omitted → back up iff file is dirty or untracked.
    #[serde(default)]
    pub backup: Option<bool>,
    /// Print the resolved plan and touch nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// Pre-resolved answer to the dirty/untracked conflict question.
    /// Required when `action` is append/convert/override against an
    /// untracked or dirty file; otherwise the tool returns an error
    /// naming the state so the caller can pick a resolution.
    #[serde(default)]
    pub on_conflict: Option<InitClaudeConflict>,
    /// Path to the CLAUDE.md file. Defaults to `./CLAUDE.md`.
    #[serde(default)]
    pub path: Option<String>,
}

/// Argument shape for `status`.
///
/// Empty today; the struct is kept so future flags (e.g. a `verbose`
/// switch that also reports per-memory HEAD commits) can land
/// without a schema break.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct StatusArgs {}

/// Shared argument shape for `sync_pull`, `sync_push`, and `sync`.
///
/// The `group` field is a forward-compatibility slot: today the
/// sync engine operates across the whole local mirror and the arg
/// is recorded as an advisory warning on the response instead of
/// scoping the operation. Kept as a struct so future flags can land
/// without breaking the tool schema.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct SyncToolArgs {
    /// Reserved. Today the engine pulls/pushes the whole mirror;
    /// any value passed here surfaces as a warning on the response.
    #[serde(default)]
    pub group: Option<String>,
}

/// Argument shape for `init_project`.
///
/// All three fields are optional so the tool can serve three
/// distinct flows:
/// - **Fresh bootstrap**: `{slug: "team-rust"}` writes both config
///   and repo.
/// - **Config-only adoption**: `{slug: "team-rust", config_only: true}`
///   stamps `.mmcp.toml` for a server-side project that a
///   follow-up `sync_pull` will populate.
/// - **Slug-less retry**: `{}` when `.mmcp.toml` already stores a
///   `project_slug`, so the caller doesn't need to re-specify it.
///
/// Elicitation-based slug defaulting (FR-011) will plug in here
/// once rmcp exposes the client-side hook: the server will compute
/// a slugified project dir basename and ask the client to accept /
/// override it. Until then, `slug_required` is the pre-elicitation
/// graceful degradation.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct InitProjectArgs {
    /// Group slug (kebab-case, 1-128 chars, no leading/trailing or
    /// consecutive hyphens). Same contract as memory slugs. Absent
    /// is fine when `.mmcp.toml` already stores a `project_slug`;
    /// otherwise the tool returns `slug_required`.
    #[serde(default)]
    pub slug: Option<String>,

    /// When true, write `.mmcp.toml` only and skip bare-repo
    /// creation. Useful when adopting a project whose repo will
    /// land locally via the first `sync_pull`.
    #[serde(default)]
    pub config_only: bool,

    /// Adopt an explicit project UUID instead of minting a fresh
    /// v7. Rejected with `project_uuid_mismatch` if `.mmcp.toml`
    /// already stores a different UUID. Passed as the UUID's
    /// hyphenated string form; malformed values error with
    /// `invalid_project_uuid`.
    #[serde(default)]
    pub project_uuid: Option<String>,
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
        description = "CREATE a new memory in a group. All metadata fields (name, description, kind, tags, mandatory) are typed parameters — the server builds the frontmatter. Errors with code `memory_already_exists` when the slug is already on disk; use `edit_memory` to apply partial updates, `delete_memory` to remove, or pass `override: true` to deliberately replace the whole file (bulk-reset flows only — the default should almost always stay false)."
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
            args.override_,
        )
        .await
        .map_err(map_memory_error_to_mcp)?;
        Ok(ok_json(json!({
            "slug": result.slug,
            "commit_id": result.commit_id,
            "group": args.group,
            "replaced": args.override_,
        })))
    }

    #[tool(
        description = "Apply partial frontmatter / body deltas to an existing memory and record the result as a new commit. Every mutator field is optional: omit it to leave that slice of the memory untouched. `tags_add` / `tags_remove` compose additively so repeated calls dedupe correctly. Errors with code `memory_not_found` when the slug has no file in the target group; use `write_memory` to create fresh memories."
    )]
    async fn edit_memory(
        &self,
        Parameters(args): Parameters<EditMemoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_group_id(&args.group)?;
        let entry = self
            .state
            .groups
            .get(&group_id)
            .await
            .ok_or_else(|| McpError::invalid_params("group not found", None))?;

        let path = memory_path(&args.slug);
        let bytes = match self
            .state
            .backend
            .read_file(&entry.handle, &path, &Rev::head())
            .await
        {
            Ok(b) => b,
            Err(mmcp_git::GitError::PathNotFound(_)) => {
                return Err(map_memory_error_to_mcp(
                    crate::commands::import::ImportError::MemoryNotFound {
                        slug: args.slug.clone(),
                    },
                ));
            }
            Err(e) => return Err(git_error(e)),
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut file = mmcp_core::memory::MemoryFile::parse(&text).map_err(|e| {
            McpError::internal_error(
                Cow::Owned(format!("parsing existing memory: {e}")),
                None,
            )
        })?;

        // Apply deltas. Body replacement and frontmatter field
        // replacements are straightforward slot writes; tags_add /
        // tags_remove merge with the existing vector and dedupe so
        // repeated calls converge.
        if let Some(body) = args.body {
            file.body = body;
        }
        if let Some(name) = args.name {
            file.frontmatter.name = name;
        }
        if let Some(description) = args.description {
            file.frontmatter.description = description;
        }
        if let Some(kind) = args.kind {
            file.frontmatter.kind = kind.into_core();
        }
        if let Some(mandatory) = args.mandatory {
            file.frontmatter.mandatory = mandatory;
        }
        if !args.tags_add.is_empty() || !args.tags_remove.is_empty() {
            file.frontmatter.tags.extend(args.tags_add);
            file.frontmatter
                .tags
                .retain(|t| !args.tags_remove.contains(t));
            // Sort + dedup so the wire shape is deterministic; tag
            // order is not load-bearing for readers.
            file.frontmatter.tags.sort_unstable();
            file.frontmatter.tags.dedup();
        }

        let rendered = file
            .to_string()
            .map_err(|e| McpError::internal_error(Cow::Owned(e.to_string()), None))?;

        let commit_id = crate::commands::import::update_memory_file(
            &self.state.backend,
            &entry.handle,
            &args.slug,
            &rendered,
            &self.state.author,
            args.message.as_deref(),
        )
        .await
        .map_err(map_memory_error_to_mcp)?;

        Ok(ok_json(json!({
            "group": args.group,
            "slug": args.slug,
            "commit_id": commit_id,
        })))
    }

    #[tool(
        description = "Remove a memory from a group by committing a deletion on `main`. Errors with code `memory_not_found` when the slug has no file; no silent no-op. The commit is addressable through `list_versions` just like any other write, so the removal is auditable."
    )]
    async fn delete_memory(
        &self,
        Parameters(args): Parameters<DeleteMemoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_group_id(&args.group)?;
        let entry = self
            .state
            .groups
            .get(&group_id)
            .await
            .ok_or_else(|| McpError::invalid_params("group not found", None))?;

        let commit_id = crate::commands::import::delete_memory_file(
            &self.state.backend,
            &entry.handle,
            &args.slug,
            &self.state.author,
            args.message.as_deref(),
        )
        .await
        .map_err(map_memory_error_to_mcp)?;

        Ok(ok_json(json!({
            "group": args.group,
            "slug": args.slug,
            "commit_id": commit_id,
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

    #[tool(
        description = "Initialize the AI's context for this session. Returns the curated set of memories (mandatory, project-scoped, or both) with their bodies inline, plus advisory diagnostics about the project's CLAUDE.md state. Call at session start, after context compaction, before starting a new phase or task, and before/after each commit cycle. This tool never writes files — CLAUDE.md advice appears in `diagnostics` and must be acted on by calling `init_claude` explicitly."
    )]
    async fn bootstrap_context(
        &self,
        Parameters(args): Parameters<BootstrapContextArgs>,
    ) -> Result<CallToolResult, McpError> {
        let scope = args.scope.unwrap_or_default();

        // Resolve the project group (if any) from the cwd's .mmcp.toml.
        let project_root = std::env::current_dir()
            .ok()
            .and_then(|cwd| crate::config::find_project_root(&cwd));
        let project_uuid = project_root
            .as_ref()
            .and_then(|root| crate::config::load(root).ok())
            .map(|cfg| *cfg.project_uuid.as_uuid());

        let wants_project = matches!(scope, BootstrapScope::Project | BootstrapScope::All);
        let wants_mandatory = matches!(scope, BootstrapScope::Mandatory | BootstrapScope::All);

        // Walk every local group, collecting memories that match scope.
        let mut memories: Vec<serde_json::Value> = Vec::new();
        for entry in self.state.groups.list().await {
            let entry_uuid = *entry.manifest.group_id.as_uuid();
            let is_project = project_uuid == Some(entry_uuid);
            let slugs = list_memory_files(&self.state.backend, &entry)
                .await
                .unwrap_or_default();
            for slug in slugs {
                let path = mmcp_core::conventions::memory_path(&slug);
                let bytes = match self
                    .state
                    .backend
                    .read_file(&entry.handle, &path, &Rev::head())
                    .await
                {
                    Ok(b) => b,
                    Err(err) => {
                        tracing::warn!(
                            slug = %slug,
                            group = %entry_uuid,
                            error = %err,
                            "bootstrap_context: skipping unreadable memory"
                        );
                        continue;
                    }
                };
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    continue;
                };
                let Ok(file) = MemoryFile::parse(text) else {
                    continue;
                };
                let is_mandatory = file.frontmatter.mandatory;

                let include = (wants_mandatory && is_mandatory)
                    || (wants_project && is_project);
                if !include {
                    continue;
                }
                let reason = match (
                    wants_mandatory && is_mandatory,
                    wants_project && is_project,
                ) {
                    (true, true) => "mandatory,project",
                    (true, false) => "mandatory",
                    (false, true) => "project",
                    (false, false) => unreachable!("include guard above"),
                };
                memories.push(json!({
                    "group": entry_uuid,
                    "slug": slug,
                    "name": file.frontmatter.name,
                    "description": file.frontmatter.description,
                    "kind": file.frontmatter.kind.as_str(),
                    "tags": file.frontmatter.tags,
                    "mandatory": is_mandatory,
                    "reason": reason,
                    "body": file.body,
                }));
            }
        }

        // Advisory diagnostics about CLAUDE.md. These never block the
        // call and never write anything; the AI or operator decides.
        let diagnostics = build_claude_diagnostics(project_root.as_deref());

        Ok(ok_json(json!({
            "memories": memories,
            "project_root": project_root.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "project_uuid": project_uuid.map(|u| u.to_string()),
            "diagnostics": diagnostics,
        })))
    }

    #[tool(
        description = "Manage CLAUDE.md for the current project. Actions: `override` writes a fresh mmcp stub, `append` inserts or replaces the mmcp-managed fence block, `convert` splits existing CLAUDE.md into typed project memories and replaces the file with a stub. When the file is dirty or untracked and `on_conflict` is not set, the call errors with a structured `conflict_unresolved` payload naming the observed state so the caller can retry with a choice. Default backup policy writes `.bak` only when the file is dirty or untracked."
    )]
    async fn init_claude(
        &self,
        Parameters(args): Parameters<InitClaudeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let path = args
            .path
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("CLAUDE.md"));
        let state = crate::commands::claude::inspect(&path);

        // Convert/append need an existing file; override may write from
        // scratch. This mirrors the CLI's execute-time check but
        // catches the error before we even compute a backup.
        if matches!(
            args.action,
            InitClaudeAction::Append | InitClaudeAction::Convert
        ) && matches!(state, crate::commands::claude::FileState::Missing)
        {
            return Err(McpError::invalid_params(
                "cannot append/convert a missing CLAUDE.md; call with action=override first",
                Some(json!({ "state": state.as_wire_str() })),
            ));
        }

        // Resolve the conflict question (or demand an answer).
        let conflict = match (state.is_conflict(), args.on_conflict) {
            (false, _) => crate::commands::claude::ConflictChoice::NotApplicable,
            (true, Some(InitClaudeConflict::Override)) => {
                crate::commands::claude::ConflictChoice::Override
            }
            (true, Some(InitClaudeConflict::BackupOverride)) => {
                crate::commands::claude::ConflictChoice::BackupOverride
            }
            (true, Some(InitClaudeConflict::Cancel)) => {
                return Ok(ok_json(json!({
                    "action": action_wire(args.action),
                    "state_before": state.as_wire_str(),
                    "cancelled": true,
                    "wrote": null,
                    "memories_created": [],
                })));
            }
            (true, None) => {
                // Structured error: caller decides how to resolve and
                // re-invokes with `on_conflict` set. Future elicitation
                // support turns this into a prompt instead of an error.
                return Err(McpError::invalid_params(
                    "CLAUDE.md state requires an explicit conflict resolution",
                    Some(json!({
                        "code": "conflict_unresolved",
                        "state": state.as_wire_str(),
                        "choices": [
                            { "value": "override", "description": "overwrite without backup (lose local changes)" },
                            { "value": "backup_override", "description": "write .bak, then overwrite (recommended)" },
                            { "value": "cancel", "description": "abort; do not touch CLAUDE.md" }
                        ],
                        "retry_with": { "on_conflict": "backup_override" }
                    })),
                ));
            }
        };

        // Backup policy: explicit arg > default (dirty ⇒ backup) >
        // conflict-driven force (backup_override always backs up).
        let backup = match args.backup {
            Some(explicit) => explicit,
            None => state.is_conflict(),
        };
        let backup = matches!(
            conflict,
            crate::commands::claude::ConflictChoice::BackupOverride
        ) || backup;

        let action = match args.action {
            InitClaudeAction::Override => crate::commands::claude::Action::Override,
            InitClaudeAction::Append => crate::commands::claude::Action::Append,
            InitClaudeAction::Convert => crate::commands::claude::Action::Convert,
        };

        let cwd = std::env::current_dir()
            .map_err(|e| McpError::internal_error(Cow::Owned(format!("cwd: {e}")), None))?;
        let plan = crate::commands::claude::ClaudePlan {
            action,
            backup,
            dry_run: args.dry_run,
            path: path.clone(),
            state,
            cwd,
        };

        if plan.dry_run {
            return Ok(ok_json(json!({
                "action": action_wire(args.action),
                "state_before": state.as_wire_str(),
                "plan": {
                    "backup": plan.backup,
                    "path": plan.path.to_string_lossy(),
                },
                "wrote": null,
                "memories_created": [],
                "dry_run": true,
            })));
        }

        // Resolve the author and run the shared execute path.
        let home = crate::home::MmcpHome::discover()
            .map_err(|e| McpError::internal_error(Cow::Owned(e.to_string()), None))?;
        let author = home.resolve_author();
        let report = crate::commands::claude::execute(&plan, &home, &author)
            .await
            .map_err(|e| McpError::internal_error(Cow::Owned(e.to_string()), None))?;

        Ok(ok_json(json!({
            "action": action_wire(args.action),
            "state_before": report.state_before.as_wire_str(),
            "backup_path": report.backup_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "wrote": report.wrote.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "memories_created": report.memories_created.iter().map(|m| json!({
                "slug": m.slug,
                "commit": m.commit_id,
                "source_section": m.source_section,
            })).collect::<Vec<_>>(),
        })))
    }

    #[tool(
        description = "Pull updates from the configured mmcp sync server into the local mirror. Returns the groups whose local HEAD advanced plus any groups the server has that are not mirrored yet. Errors with code `sync_not_configured` when `.mmcp.toml` has no `[sync]` block, and code `sync_conflict` / `sync_remote` / `sync_transport` for engine-level failures."
    )]
    async fn sync_pull(
        &self,
        Parameters(args): Parameters<SyncToolArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (cfg, server_url) = self.require_sync_configured()?;
        let (engine, resolver, _queue) = crate::commands::sync::build_engine(
            self.state.backend.clone(),
            self.state.groups.clone(),
            &server_url,
        )
        .map_err(|e| {
            McpError::internal_error(format!("failed to build sync engine: {e}"), None)
        })?;
        let report = engine.pull(&resolver).await.map_err(map_sync_error_to_mcp)?;
        Ok(ok_json(json!({
            "updated": report.updated,
            "new_groups": report.new_groups,
            "project_uuid": cfg.project_uuid.to_string(),
            "server_url": server_url,
            "warnings": group_scope_warnings(args.group.as_deref()),
        })))
    }

    #[tool(
        description = "Push the local pending-edit queue to the configured mmcp sync server. Returns each drained edit with the server-assigned version and tag, plus whether the content plane (git push) actually shipped bytes. Errors with code `sync_not_configured` when `.mmcp.toml` has no `[sync]` block."
    )]
    async fn sync_push(
        &self,
        Parameters(args): Parameters<SyncToolArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (cfg, server_url) = self.require_sync_configured()?;
        let (engine, resolver, queue) = crate::commands::sync::build_engine(
            self.state.backend.clone(),
            self.state.groups.clone(),
            &server_url,
        )
        .map_err(|e| {
            McpError::internal_error(format!("failed to build sync engine: {e}"), None)
        })?;
        let report = engine
            .push(&queue, &resolver)
            .await
            .map_err(map_sync_error_to_mcp)?;
        Ok(ok_json(json!({
            "drained": report.drained.iter().map(|d| json!({
                "edit_id": d.edit_id.to_string(),
                "group_id": d.response.group_id.to_string(),
                "memory_id": d.response.memory_id.to_string(),
                "assigned_version": d.response.assigned_version,
                "tag": d.response.tag,
                "content_transferred": d.content_transferred,
            })).collect::<Vec<_>>(),
            "project_uuid": cfg.project_uuid.to_string(),
            "server_url": server_url,
            "warnings": group_scope_warnings(args.group.as_deref()),
        })))
    }

    #[tool(
        description = "Run a full sync (pull then push) against the configured mmcp server. Returns both report shapes nested under `pulled` and `pushed`. Same error codes as `sync_pull` / `sync_push`."
    )]
    async fn sync(
        &self,
        Parameters(args): Parameters<SyncToolArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (cfg, server_url) = self.require_sync_configured()?;
        let (engine, resolver, queue) = crate::commands::sync::build_engine(
            self.state.backend.clone(),
            self.state.groups.clone(),
            &server_url,
        )
        .map_err(|e| {
            McpError::internal_error(format!("failed to build sync engine: {e}"), None)
        })?;
        let report = engine
            .sync(&queue, &resolver)
            .await
            .map_err(map_sync_error_to_mcp)?;
        Ok(ok_json(json!({
            "pulled": {
                "updated": report.pulled.updated,
                "new_groups": report.pulled.new_groups,
            },
            "pushed": {
                "drained": report.pushed.drained.iter().map(|d| json!({
                    "edit_id": d.edit_id.to_string(),
                    "group_id": d.response.group_id.to_string(),
                    "memory_id": d.response.memory_id.to_string(),
                    "assigned_version": d.response.assigned_version,
                    "tag": d.response.tag,
                    "content_transferred": d.content_transferred,
                })).collect::<Vec<_>>(),
            },
            "project_uuid": cfg.project_uuid.to_string(),
            "server_url": server_url,
            "warnings": group_scope_warnings(args.group.as_deref()),
        })))
    }

    #[tool(
        description = "Return the local mmcp project state: discovered project root, configured sync server, and the mirrored groups with their memory counts. Pure-local — no network. Returns `project_configured: false` when no `.mmcp.toml` is in scope, so callers can distinguish 'not in a project' from transient errors."
    )]
    async fn status(
        &self,
        Parameters(_args): Parameters<StatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        let cwd = std::env::current_dir().map_err(|e| {
            McpError::internal_error(format!("cannot read working directory: {e}"), None)
        })?;

        // Collect group state first so callers inspecting a
        // machine-wide mirror from outside any project still see
        // what is mirrored.
        let entries = self.state.groups.list().await;
        let mut groups = Vec::with_capacity(entries.len());
        for entry in &entries {
            let files = list_memory_files(&self.state.backend, entry).await?;
            groups.push(json!({
                "slug": entry.manifest.slug,
                "uuid": entry.manifest.group_id.to_string(),
                "memory_count": files.len(),
            }));
        }

        Ok(ok_json(compose_status(&cwd, groups)?))
    }

    #[tool(
        description = "Bootstrap the project's `.mmcp.toml` and backing group repo. Idempotent and never-overwrite: a second call returns `created_config: false` / `created_repo: false` without rewriting either artifact. Errors with code `invalid_slug` when the slug does not satisfy the memory-slug contract, `slug_required` when no slug is available (arg missing and no `project_slug` in `.mmcp.toml`), `slug_mismatch` / `project_uuid_mismatch` when args disagree with an existing config, and `repo_without_config` if the bare repo exists but the config has been deleted."
    )]
    async fn init_project(
        &self,
        Parameters(args): Parameters<InitProjectArgs>,
    ) -> Result<CallToolResult, McpError> {
        let cwd = std::env::current_dir().map_err(|e| {
            McpError::internal_error(format!("cannot read working directory: {e}"), None)
        })?;
        let parsed_uuid = match args.project_uuid.as_deref() {
            Some(s) => Some(Uuid::parse_str(s).map_err(|e| {
                McpError::invalid_params(
                    format!("invalid project_uuid: {e}"),
                    Some(json!({
                        "code": "invalid_project_uuid",
                        "value": s,
                    })),
                )
            })?),
            None => None,
        };
        let opts = crate::commands::init::InitProjectOptions {
            slug: args.slug,
            config_only: args.config_only,
            project_uuid: parsed_uuid,
        };
        let report = crate::commands::init::create_project_group_from_state(
            &self.state.backend,
            &self.state.groups,
            &cwd,
            &opts,
        )
        .await
        .map_err(map_init_project_error_to_mcp)?;
        Ok(ok_json(json!({
            "project_uuid":   report.project_uuid.to_string(),
            "project_root":   report.project_root.to_string_lossy(),
            "group_id":       report.group_id.to_string(),
            "slug":           report.slug,
            "repo_path":      report.repo_path.as_ref().map(|p| p.to_string_lossy()),
            "created_config": report.created_config,
            "created_repo":   report.created_repo,
        })))
    }
}

/// Compose the `status` tool response from a cwd + a pre-built
/// groups list.
///
/// Split out of [`McpServer::status`] so tests can feed a deterministic
/// cwd without mutating process state. The caller gathers the groups
/// (which needs access to the async backend) and this function
/// handles the synchronous project-root + sync-config lookup.
fn compose_status(
    cwd: &std::path::Path,
    groups: Vec<serde_json::Value>,
) -> Result<serde_json::Value, McpError> {
    let Some(root) = find_project_root(cwd) else {
        return Ok(json!({
            "project_configured": false,
            "groups": groups,
        }));
    };
    let cfg = load_project_config(&root).map_err(|e| {
        McpError::invalid_params(
            format!("failed to load project config: {e}"),
            Some(json!({ "code": "project_config_load_failed" })),
        )
    })?;
    let sync = match cfg.sync.as_ref() {
        Some(s) => json!({ "configured": true, "server_url": s.server_url }),
        None => json!({ "configured": false }),
    };
    Ok(json!({
        "project_configured": true,
        "project_root": root.to_string_lossy(),
        "project_uuid": cfg.project_uuid.to_string(),
        "sync": sync,
        "groups": groups,
    }))
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

    /// Discover the current project and enforce that `[sync]` is
    /// present in its `.mmcp.toml`. Thin wrapper over
    /// [`resolve_sync_config`] that pulls the current working
    /// directory from the process. Kept on the server so tool
    /// methods stay short; the pure logic lives in the free
    /// function so unit tests can drive it with a tempdir-rooted
    /// path.
    fn require_sync_configured(
        &self,
    ) -> Result<(mmcp_core::config::ProjectConfig, String), McpError> {
        let cwd = std::env::current_dir().map_err(|e| {
            McpError::internal_error(format!("cannot read working directory: {e}"), None)
        })?;
        resolve_sync_config(&cwd)
    }
}

/// Resolve the project at `cwd` (walking parent dirs) and return its
/// [`ProjectConfig`] plus the configured `[sync].server_url`.
///
/// Factored out of [`McpServer::require_sync_configured`] so tests can
/// feed a deterministic path without touching process-wide
/// `current_dir`. The three error branches are stable wire contracts:
/// `project_not_found`, `project_config_load_failed`, and
/// `sync_not_configured`.
fn resolve_sync_config(
    cwd: &std::path::Path,
) -> Result<(mmcp_core::config::ProjectConfig, String), McpError> {
    let root = find_project_root(cwd).ok_or_else(|| {
        McpError::invalid_params(
            "no mmcp project found in current directory or any parent",
            Some(json!({ "code": "project_not_found" })),
        )
    })?;
    let cfg = load_project_config(&root).map_err(|e| {
        McpError::invalid_params(
            format!("failed to load project config: {e}"),
            Some(json!({ "code": "project_config_load_failed" })),
        )
    })?;
    let Some(sync) = cfg.sync.as_ref() else {
        return Err(McpError::invalid_params(
            "project has no [sync] block; cannot sync against a remote",
            Some(json!({
                "code": "sync_not_configured",
                "project_uuid": cfg.project_uuid.to_string(),
                "retry_hint": "add [sync] server_url = \"http://...\" to .mmcp.toml"
            })),
        ));
    };
    let server_url = sync.server_url.clone();
    Ok((cfg, server_url))
}

/// Map a [`mmcp_sync::SyncError`] to an [`McpError`] that carries a
/// structured `code` payload. Callers (AI or test code) can branch
/// on the code string instead of parsing the human message.
fn map_sync_error_to_mcp(err: mmcp_sync::SyncError) -> McpError {
    use mmcp_sync::SyncError;
    let message = err.to_string();
    let payload = match &err {
        SyncError::Conflict {
            memory,
            local_commit,
            remote_commit,
        } => json!({
            "code": "sync_conflict",
            "memory": memory.to_string(),
            "local_commit": local_commit,
            "remote_commit": remote_commit,
        }),
        SyncError::Remote { status, message } => json!({
            "code": "sync_remote",
            "status": status,
            "message": message,
        }),
        SyncError::Transport(detail) => json!({
            "code": "sync_transport",
            "detail": detail,
        }),
        SyncError::NotFound(detail) => json!({
            "code": "sync_not_found",
            "detail": detail,
        }),
        SyncError::Git(g) => json!({
            "code": "sync_git",
            "detail": g.to_string(),
        }),
        SyncError::InvalidVersion(v) => json!({
            "code": "sync_invalid_version",
            "detail": v.to_string(),
        }),
    };
    McpError::invalid_params(message, Some(payload))
}

/// Map a [`commands::init::InitProjectError`] to an [`McpError`]
/// with a structured `code` payload so AI callers can branch on
/// state instead of parsing the error string.
fn map_init_project_error_to_mcp(err: crate::commands::init::InitProjectError) -> McpError {
    use crate::commands::init::InitProjectError;
    let message = err.to_string();
    let payload = match &err {
        InitProjectError::InvalidSlug { slug } => json!({
            "code": "invalid_slug",
            "slug": slug,
        }),
        InitProjectError::ConfigLoadFailed(detail) => json!({
            "code": "project_config_load_failed",
            "detail": detail,
        }),
        InitProjectError::ConfigWriteFailed(detail) => json!({
            "code": "project_config_write_failed",
            "detail": detail,
        }),
        InitProjectError::ProjectUuidMismatch { expected, got } => json!({
            "code": "project_uuid_mismatch",
            "expected": expected.to_string(),
            "got": got.to_string(),
        }),
        InitProjectError::SlugMismatch { expected, got } => json!({
            "code": "slug_mismatch",
            "expected": expected,
            "got": got,
        }),
        InitProjectError::SlugRequired => json!({
            "code": "slug_required",
            "retry_hint": "pass a `slug` argument, or store `project_slug` in .mmcp.toml",
        }),
        InitProjectError::RepoWithoutConfig { uuid, path } => json!({
            "code": "repo_without_config",
            "uuid": uuid.to_string(),
            "path": path.to_string_lossy(),
        }),
        InitProjectError::GitBackend(detail) => json!({
            "code": "git_backend",
            "detail": detail,
        }),
        InitProjectError::IndexRefreshFailed(detail) => json!({
            "code": "index_refresh_failed",
            "detail": detail,
        }),
    };
    McpError::invalid_params(message, Some(payload))
}

/// Map an [`ImportError`] coming from the memory CRUD primitives
/// onto an [`McpError`] with a structured `code` payload. The four
/// wire codes (`memory_not_found`, `memory_already_exists`,
/// `invalid_slug`, `memory_render_failed`) are stable wire contracts
/// the `edit_memory`, `delete_memory`, and tightened `write_memory`
/// tools all share.
fn map_memory_error_to_mcp(err: crate::commands::import::ImportError) -> McpError {
    use crate::commands::import::ImportError;
    let message = err.to_string();
    let payload = match &err {
        ImportError::MemoryNotFound { slug } => json!({
            "code": "memory_not_found",
            "slug": slug,
        }),
        ImportError::MemoryAlreadyExists { slug } => json!({
            "code": "memory_already_exists",
            "slug": slug,
            "retry_hint": "use edit_memory to update in place, delete_memory to remove, or pass override: true to replace",
        }),
        ImportError::InvalidSlug(slug) => json!({
            "code": "invalid_slug",
            "slug": slug,
        }),
        ImportError::Render(detail) => json!({
            "code": "memory_render_failed",
            "detail": detail,
        }),
        ImportError::Parse(detail) => json!({
            "code": "memory_parse_failed",
            "detail": detail.to_string(),
        }),
        ImportError::Git(detail) => json!({
            "code": "git_backend",
            "detail": detail.to_string(),
        }),
        ImportError::MissingFrontmatter => json!({
            "code": "memory_missing_frontmatter",
        }),
        ImportError::UnknownKind(kind) => json!({
            "code": "memory_unknown_kind",
            "kind": kind,
        }),
        ImportError::GroupNotFound(group) => json!({
            "code": "group_not_found",
            "group": group,
        }),
    };
    McpError::invalid_params(message, Some(payload))
}

/// Emit a one-element advisory warning list when the caller passed a
/// `group` argument that the engine cannot honor yet. Empty list
/// when nothing was passed so the field stays stable (`[]`) on every
/// successful response.
fn group_scope_warnings(group: Option<&str>) -> Vec<String> {
    match group {
        Some(_) => vec!["group scoping not yet implemented; operated on the whole mirror".into()],
        None => Vec::new(),
    }
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(SESSION_INSTRUCTIONS.to_string())
    }
}

/// Session-start protocol delivered to every MCP client on handshake.
///
/// This text is the authoritative reading order — CLAUDE.md points at
/// it rather than duplicating it. When the checkpoint list or tool
/// usage changes, update this constant; no other surface repeats the
/// protocol.
const SESSION_INSTRUCTIONS: &str = concat!(
    "mmcp memory server — the project's single source of truth for coding rules, ",
    "conventions, and project notes. Memories live in git repositories under ",
    "~/.mmcp/repos and are surfaced through typed MCP tools; never hand-edit TOML.\n\n",
    "## Session-start protocol (MANDATORY)\n\n",
    "Call `bootstrap_context` at the start of every session and again at EACH of ",
    "the following checkpoints. These are not suggestions; skipping any of them ",
    "leaves you working against stale rules.\n\n",
    "- Session start, before any other tool call or file write.\n",
    "- After ANY context compaction. Compaction summaries are NOT authoritative; ",
    "the memories are. Never trust a compaction report.\n",
    "- Before starting a new phase or task.\n",
    "- Before a commit cycle (git conventions may have shipped updates).\n",
    "- After a commit cycle (re-align before picking up the next step).\n",
    "- Any time a rule is corrected, added, or discussed — the memory may have ",
    "been updated; re-read it.\n\n",
    "`bootstrap_context` returns every mandatory and project-scoped memory with ",
    "its body inline in a single round trip. Call with no args for `scope=all`; ",
    "pass `scope=mandatory` or `scope=project` to reload one side.\n\n",
    "## On-demand lookups\n\n",
    "Outside the mandatory set, use `search_memories(query)` for cross-group ",
    "substring matches, `list_memories(group)` to enumerate a group, and ",
    "`read_memory(group, slug[, version])` for a specific entry (optionally at ",
    "a branch, tag, or commit hex). `group_info(group)` returns manifest ",
    "metadata. `list_versions(group, slug)` walks the memory's commit history.\n\n",
    "## Authoring and maintenance\n\n",
    "The memory CRUD surface is three separate tools; pick the right one:\n",
    "- `write_memory(group, slug, name, description, kind, body[, tags, mandatory])` ",
    "CREATES a new memory. Errors with `memory_already_exists` on collision — ",
    "the tool will NOT overwrite silently. Pass `override: true` only when you ",
    "genuinely mean replace-the-whole-file (bulk-reset flows); almost never.\n",
    "- `edit_memory(group, slug[, body, name, description, kind, tags_add, ",
    "tags_remove, mandatory, message])` applies partial deltas to an existing ",
    "memory. Omit a field to leave it untouched. `tags_add` / `tags_remove` ",
    "compose additively with dedup. Use this to tweak a rule, NOT `write_memory`.\n",
    "- `delete_memory(group, slug[, message])` removes a memory by commit. ",
    "The removal is auditable through `list_versions`; double-delete errors ",
    "with `memory_not_found`.\n\n",
    "The server builds all frontmatter from typed parameters — you never ",
    "construct fence blocks by hand. Use `check_health` for surface validation ",
    "(manifest readable, memories parse), `diagnose` for deep structural checks ",
    "(missing fields, empty bodies, cross-group slug collisions, config gaps).\n\n",
    "## CLAUDE.md management\n\n",
    "Never hand-edit CLAUDE.md to add rules. If `bootstrap_context` emits a ",
    "`claude_md_missing` / `claude_md_unmanaged` / `claude_md_stale` diagnostic, ",
    "act on it by calling `init_claude` explicitly; otherwise leave the file ",
    "alone. Rules live in memories, not in CLAUDE.md.\n\n",
    "## Debug tools\n\n",
    "`debug_toggle` / `debug_read_file` / `debug_write_file` / `debug_list_tree` ",
    "/ `debug_git_log` provide raw git access for troubleshooting. They require ",
    "`debug_toggle(enabled=true)` to be active and should stay off outside of ",
    "repair scenarios."
);

/// List every `memories/<slug>.md` blob in the group's repo at the
/// current `HEAD` and return the slugs without the `.md` extension.
async fn list_memory_files(
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> Result<Vec<String>, McpError> {
    let files = backend
        .list_tree(
            &entry.handle,
            MEMORIES_DIR,
            &Rev::head(),
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
        // Default: resolve via HEAD so repos whose default branch is
        // not `main` still return the latest content.
        None => Rev::head(),
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
        Rev::Head => "HEAD".to_string(),
    }
}

fn git_error(err: mmcp_git::GitError) -> McpError {
    McpError::internal_error(Cow::Owned(format!("git error: {err}")), None)
}

/// Map `InitClaudeAction` onto the wire string used in tool responses.
fn action_wire(action: InitClaudeAction) -> &'static str {
    match action {
        InitClaudeAction::Override => "override",
        InitClaudeAction::Append => "append",
        InitClaudeAction::Convert => "convert",
    }
}

/// Current version of the mmcp-managed block embedded in CLAUDE.md.
/// Bumping this value lets `init_claude` detect stale blocks and lets
/// `bootstrap_context` emit a `claude_md_stale` diagnostic when a
/// project carries an older fence.
const CLAUDE_MD_BLOCK_VERSION: &str = "v1";

/// Compute advisory diagnostics about the project's CLAUDE.md state.
///
/// Read-only: the function inspects the file on disk but never writes
/// anything. `bootstrap_context` emits these so the AI can decide to
/// call `init_claude`. An empty vector means either no project root
/// was resolved (nothing to diagnose) or the file is already healthy.
fn build_claude_diagnostics(project_root: Option<&std::path::Path>) -> Vec<serde_json::Value> {
    let Some(root) = project_root else {
        return Vec::new();
    };
    let claude_md = root.join("CLAUDE.md");
    if !claude_md.exists() {
        return vec![json!({
            "severity": "warning",
            "code": "claude_md_missing",
            "message": "CLAUDE.md is missing at the project root. Running `init_claude` (action=override) bootstraps it with the mmcp pointer template so future sessions see the checkpoint protocol.",
            "suggested_tool": "init_claude",
            "suggested_args": { "action": "override" },
        })];
    }
    let Ok(body) = std::fs::read_to_string(&claude_md) else {
        return Vec::new();
    };
    let begin_marker = format!("<!-- mmcp:begin {CLAUDE_MD_BLOCK_VERSION} -->");
    if body.contains(&begin_marker) {
        return Vec::new();
    }
    // Older-version fence present? Flag as stale so init_claude can upgrade.
    if body.contains("<!-- mmcp:begin ") {
        return vec![json!({
            "severity": "info",
            "code": "claude_md_stale",
            "message": format!(
                "CLAUDE.md carries an older mmcp block; current version is {CLAUDE_MD_BLOCK_VERSION}. Re-run `init_claude` (action=append) to upgrade the fenced region in place."
            ),
            "suggested_tool": "init_claude",
            "suggested_args": { "action": "append" },
        })];
    }
    // No fence at all — file is unmanaged.
    vec![json!({
        "severity": "warning",
        "code": "claude_md_unmanaged",
        "message": "CLAUDE.md has no mmcp-managed block. Run `init_claude` (action=append) to insert the session-start protocol without touching user-authored content, or (action=convert) to split existing rule content into typed memories and replace the file with a stub.",
        "suggested_tool": "init_claude",
        "suggested_args": { "action": "append" },
    })]
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

    const MANDATORY_MEMORY: &str = "+++\nname = \"Mandatory Rule\"\ndescription = \"A rule that must be read every session\"\nkind = \"rule\"\nmandatory = true\ntags = [\"global\",\"rule\"]\n+++\n\nAlways follow this rule.\n";

    const OPTIONAL_MEMORY: &str = "+++\nname = \"Optional Note\"\ndescription = \"Nice to read but not required\"\nkind = \"reference\"\nmandatory = false\ntags = [\"reference\"]\n+++\n\nSome background.\n";

    #[tokio::test]
    async fn bootstrap_context_mandatory_scope_returns_only_mandatory_with_body() {
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "globals", "mandatory-rule", MANDATORY_MEMORY).await;
        seed_group_with_memory(&state, "globals2", "optional-note", OPTIONAL_MEMORY).await;
        let server = McpServer::new(state);

        let res = server
            .bootstrap_context(Parameters(BootstrapContextArgs {
                scope: Some(BootstrapScope::Mandatory),
            }))
            .await
            .expect("bootstrap_context");
        let parsed = parse_ok_json(res);
        let memories = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .expect("memories array");
        assert_eq!(memories.len(), 1, "only the mandatory memory should qualify");
        let m = &memories[0];
        assert_eq!(m.get("slug").and_then(|v| v.as_str()), Some("mandatory-rule"));
        assert_eq!(m.get("mandatory").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(m.get("reason").and_then(|v| v.as_str()), Some("mandatory"));
        let body = m
            .get("body")
            .and_then(|v| v.as_str())
            .expect("body inline");
        assert!(body.contains("Always follow this rule."));
    }

    #[tokio::test]
    async fn bootstrap_context_all_scope_includes_mandatory_when_no_project_configured() {
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "globals", "rule-one", MANDATORY_MEMORY).await;
        seed_group_with_memory(&state, "globals", "optional", OPTIONAL_MEMORY).await;
        let server = McpServer::new(state);

        // No project config reachable from cwd → `All` collapses to mandatory-only.
        let res = server
            .bootstrap_context(Parameters(BootstrapContextArgs { scope: None }))
            .await
            .expect("bootstrap_context");
        let parsed = parse_ok_json(res);
        let memories = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .expect("memories array");
        let mandatory_count = memories
            .iter()
            .filter(|m| m.get("mandatory").and_then(|v| v.as_bool()) == Some(true))
            .count();
        assert!(
            mandatory_count >= 1,
            "mandatory memories must flow through All scope; saw: {memories:?}"
        );
    }

    #[test]
    fn build_claude_diagnostics_flags_missing_file() {
        let tmp = TempDir::new().expect("tempdir");
        let diagnostics = build_claude_diagnostics(Some(tmp.path()));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0]
                .get("code")
                .and_then(|v| v.as_str()),
            Some("claude_md_missing")
        );
    }

    #[test]
    fn build_claude_diagnostics_flags_unmanaged_file() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(
            tmp.path().join("CLAUDE.md"),
            "# Legacy\n\nHand-authored without any mmcp fence.\n",
        )
        .expect("write claude");
        let diagnostics = build_claude_diagnostics(Some(tmp.path()));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0]
                .get("code")
                .and_then(|v| v.as_str()),
            Some("claude_md_unmanaged")
        );
    }

    #[test]
    fn build_claude_diagnostics_is_silent_when_fence_matches_current_version() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(
            tmp.path().join("CLAUDE.md"),
            format!("# Managed\n\n<!-- mmcp:begin {CLAUDE_MD_BLOCK_VERSION} -->\n...\n<!-- mmcp:end {CLAUDE_MD_BLOCK_VERSION} -->\n"),
        )
        .expect("write claude");
        let diagnostics = build_claude_diagnostics(Some(tmp.path()));
        assert!(diagnostics.is_empty(), "current-version fence should produce no diagnostics");
    }

    #[test]
    fn build_claude_diagnostics_is_silent_without_project_root() {
        assert!(build_claude_diagnostics(None).is_empty());
    }

    #[tokio::test]
    async fn init_claude_dry_run_override_against_missing_file_reports_plan() {
        let (state, tmp) = test_state().await;
        let server = McpServer::new(state);
        let target = tmp.path().join("CLAUDE.md");

        let res = server
            .init_claude(Parameters(InitClaudeArgs {
                action: InitClaudeAction::Override,
                backup: None,
                dry_run: true,
                on_conflict: None,
                path: Some(target.to_string_lossy().into_owned()),
            }))
            .await
            .expect("init_claude dry_run");
        let parsed = parse_ok_json(res);
        assert_eq!(parsed.get("action").and_then(|v| v.as_str()), Some("override"));
        assert_eq!(parsed.get("state_before").and_then(|v| v.as_str()), Some("missing"));
        assert_eq!(parsed.get("dry_run").and_then(|v| v.as_bool()), Some(true));
        assert!(parsed.get("wrote").map(|v| v.is_null()).unwrap_or(false));
        assert!(!target.exists(), "dry run must not write the file");
    }

    #[tokio::test]
    async fn init_claude_refuses_dirty_file_without_on_conflict_with_structured_error() {
        let (state, tmp) = test_state().await;
        let server = McpServer::new(state);
        let target = tmp.path().join("CLAUDE.md");
        std::fs::write(&target, "# existing\n").expect("write fixture");

        let err = server
            .init_claude(Parameters(InitClaudeArgs {
                action: InitClaudeAction::Override,
                backup: None,
                dry_run: false,
                on_conflict: None,
                path: Some(target.to_string_lossy().into_owned()),
            }))
            .await
            .expect_err("must surface conflict");
        // Error data carries the structured code the client uses to
        // decide how to retry; payload shape matches the documented
        // contract.
        let payload = err.data.as_ref().expect("error data");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("conflict_unresolved")
        );
        assert!(payload.get("choices").is_some(), "choices list must be present");
    }

    #[tokio::test]
    async fn init_claude_writes_stub_when_file_is_missing() {
        let (state, tmp) = test_state().await;
        let server = McpServer::new(state);
        let target = tmp.path().join("CLAUDE.md");

        let res = server
            .init_claude(Parameters(InitClaudeArgs {
                action: InitClaudeAction::Override,
                backup: None,
                dry_run: false,
                on_conflict: None,
                path: Some(target.to_string_lossy().into_owned()),
            }))
            .await
            .expect("init_claude override");
        let parsed = parse_ok_json(res);
        assert_eq!(parsed.get("action").and_then(|v| v.as_str()), Some("override"));
        assert!(target.exists(), "stub must have been written");
        let body = std::fs::read_to_string(&target).expect("read stub");
        assert!(body.contains("mmcp is mandatory"));
        assert!(body.contains("bootstrap_context"));
    }

    // ── sync tool helpers (FR-014) ────────────────────────────────────

    /// Write a deterministic `.mmcp.toml` at `path`.
    fn write_project_config(root: &std::path::Path, body: &str) {
        std::fs::write(root.join(PROJECT_MANIFEST), body).expect("write .mmcp.toml");
    }

    #[test]
    fn resolve_sync_config_errors_when_cwd_has_no_project() {
        let tmp = TempDir::new().expect("tempdir");
        let err =
            resolve_sync_config(tmp.path()).expect_err("tempdir should not host a mmcp project");
        let payload = err.data.as_ref().expect("error data");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("project_not_found")
        );
    }

    #[test]
    fn resolve_sync_config_errors_when_sync_block_is_missing() {
        let tmp = TempDir::new().expect("tempdir");
        write_project_config(
            tmp.path(),
            "project_uuid = \"018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91\"\n",
        );
        let err = resolve_sync_config(tmp.path()).expect_err("must reject missing [sync]");
        let payload = err.data.as_ref().expect("error data");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("sync_not_configured")
        );
        assert_eq!(
            payload.get("project_uuid").and_then(|v| v.as_str()),
            Some("018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"),
            "payload must echo the project uuid so the caller can disambiguate multi-project sessions",
        );
        assert!(payload.get("retry_hint").is_some(), "retry_hint must be present");
    }

    #[test]
    fn resolve_sync_config_returns_server_url_on_happy_path() {
        let tmp = TempDir::new().expect("tempdir");
        write_project_config(
            tmp.path(),
            "project_uuid = \"018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91\"\n\n[sync]\nserver_url = \"http://localhost:8787\"\n",
        );
        let (cfg, server_url) =
            resolve_sync_config(tmp.path()).expect("happy path should resolve");
        assert_eq!(server_url, "http://localhost:8787");
        assert_eq!(
            cfg.project_uuid.to_string(),
            "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"
        );
    }

    #[test]
    fn group_scope_warnings_is_empty_when_no_group_requested() {
        assert!(group_scope_warnings(None).is_empty());
    }

    #[test]
    fn group_scope_warnings_emits_note_when_group_is_passed() {
        let warnings = group_scope_warnings(Some("team-rust"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("group scoping not yet implemented"));
    }

    #[test]
    fn map_sync_error_preserves_conflict_payload() {
        use mmcp_sync::SyncError;
        let mem = Uuid::nil();
        let err = SyncError::Conflict {
            memory: mem,
            local_commit: "aaaa".into(),
            remote_commit: "bbbb".into(),
        };
        let mapped = map_sync_error_to_mcp(err);
        let payload = mapped.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("sync_conflict")
        );
        assert_eq!(
            payload.get("local_commit").and_then(|v| v.as_str()),
            Some("aaaa")
        );
        assert_eq!(
            payload.get("remote_commit").and_then(|v| v.as_str()),
            Some("bbbb")
        );
    }

    #[test]
    fn map_sync_error_preserves_remote_status_and_message() {
        use mmcp_sync::SyncError;
        let err = SyncError::Remote {
            status: 503,
            message: "backend down".into(),
        };
        let mapped = map_sync_error_to_mcp(err);
        let payload = mapped.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("sync_remote")
        );
        assert_eq!(payload.get("status").and_then(|v| v.as_i64()), Some(503));
    }

    // ── status tool (FR-015) ──────────────────────────────────────────

    #[test]
    fn compose_status_flags_project_missing_when_outside_any_mmcp_directory() {
        let tmp = TempDir::new().expect("tempdir");
        let res = compose_status(tmp.path(), vec![]).expect("compose_status");
        assert_eq!(
            res.get("project_configured").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(
            res.get("project_root").is_none(),
            "no project root should be surfaced when there is no project"
        );
    }

    #[test]
    fn compose_status_reports_sync_not_configured_when_block_missing() {
        let tmp = TempDir::new().expect("tempdir");
        write_project_config(
            tmp.path(),
            "project_uuid = \"018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91\"\n",
        );
        let res = compose_status(tmp.path(), vec![]).expect("compose_status");
        assert_eq!(
            res.get("project_configured").and_then(|v| v.as_bool()),
            Some(true)
        );
        let sync = res.get("sync").expect("sync object");
        assert_eq!(
            sync.get("configured").and_then(|v| v.as_bool()),
            Some(false),
            "sync.configured must be false when no [sync] block is present",
        );
        assert!(
            sync.get("server_url").is_none(),
            "server_url must be absent when sync is not configured",
        );
    }

    #[test]
    fn compose_status_surfaces_server_url_and_passes_groups_through() {
        let tmp = TempDir::new().expect("tempdir");
        write_project_config(
            tmp.path(),
            "project_uuid = \"018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91\"\n\n[sync]\nserver_url = \"http://localhost:8787\"\n",
        );
        let groups = vec![json!({
            "slug": "team-rust",
            "uuid": "018f7c3e-0000-0000-0000-000000000001",
            "memory_count": 3,
        })];
        let res = compose_status(tmp.path(), groups.clone()).expect("compose_status");
        assert_eq!(
            res.get("project_uuid").and_then(|v| v.as_str()),
            Some("018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91")
        );
        let sync = res.get("sync").expect("sync object");
        assert_eq!(
            sync.get("configured").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            sync.get("server_url").and_then(|v| v.as_str()),
            Some("http://localhost:8787")
        );
        assert_eq!(
            res.get("groups").cloned().unwrap_or_default(),
            json!(groups),
            "groups should pass through compose_status unchanged",
        );
    }

    #[tokio::test]
    async fn status_tool_lists_groups_with_memory_counts() {
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);
        let res = server
            .status(Parameters(StatusArgs::default()))
            .await
            .expect("status");
        let parsed = parse_ok_json(res);
        let groups = parsed
            .get("groups")
            .and_then(|v| v.as_array())
            .expect("groups array");
        let group = groups
            .iter()
            .find(|g| g.get("slug").and_then(|s| s.as_str()) == Some("team-rust"))
            .expect("seeded group should appear");
        assert_eq!(
            group.get("memory_count").and_then(|v| v.as_u64()),
            Some(1),
            "memory_count must match the number of seeded memories",
        );
    }

    // ── init_project tool (FR-003) ────────────────────────────────────

    #[test]
    fn map_init_project_error_surfaces_slug_required_with_retry_hint() {
        let err = map_init_project_error_to_mcp(
            crate::commands::init::InitProjectError::SlugRequired,
        );
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("slug_required")
        );
        assert!(
            payload.get("retry_hint").is_some(),
            "retry_hint must be present so the AI knows what to do next",
        );
    }

    #[test]
    fn map_init_project_error_surfaces_invalid_slug_with_slug_echo() {
        let err = map_init_project_error_to_mcp(
            crate::commands::init::InitProjectError::InvalidSlug {
                slug: "BAD SLUG".to_string(),
            },
        );
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_slug")
        );
        assert_eq!(
            payload.get("slug").and_then(|v| v.as_str()),
            Some("BAD SLUG"),
            "caller's original slug must be echoed so UIs can highlight it",
        );
    }

    #[test]
    fn map_init_project_error_surfaces_slug_mismatch_with_both_sides() {
        let err = map_init_project_error_to_mcp(
            crate::commands::init::InitProjectError::SlugMismatch {
                expected: "stored".to_string(),
                got: "passed".to_string(),
            },
        );
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("slug_mismatch")
        );
        assert_eq!(
            payload.get("expected").and_then(|v| v.as_str()),
            Some("stored")
        );
        assert_eq!(
            payload.get("got").and_then(|v| v.as_str()),
            Some("passed")
        );
    }

    /// End-to-end exercise of `create_project_group_from_state` with
    /// the new options shape. Writes both config and repo from
    /// scratch on the first call.
    #[tokio::test]
    async fn init_project_helper_creates_repo_and_refreshes_index() {
        let (state, tmp) = test_state().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("create project root");

        let opts = crate::commands::init::InitProjectOptions {
            slug: Some("team-rust".to_string()),
            ..Default::default()
        };
        let report = crate::commands::init::create_project_group_from_state(
            &state.backend,
            &state.groups,
            &project_root,
            &opts,
        )
        .await
        .expect("create_project_group_from_state");

        assert!(report.created_config, "config must be written on first call");
        assert!(report.created_repo, "repo must be written on first call");
        assert_eq!(report.slug, "team-rust");
        assert!(
            state
                .groups
                .get(&GroupId::from_uuid(*report.project_uuid.as_uuid()))
                .await
                .is_some(),
            "newly created group must be visible via GroupIndex",
        );
    }

    #[tokio::test]
    async fn init_project_helper_respects_config_only() {
        let (state, tmp) = test_state().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("create project root");

        let opts = crate::commands::init::InitProjectOptions {
            slug: Some("team-rust".to_string()),
            config_only: true,
            ..Default::default()
        };
        let report = crate::commands::init::create_project_group_from_state(
            &state.backend,
            &state.groups,
            &project_root,
            &opts,
        )
        .await
        .expect("config-only bootstrap");

        assert!(report.created_config);
        assert!(
            !report.created_repo,
            "config_only must never create the bare repo",
        );
        assert!(report.repo_path.is_none());
        assert!(project_root.join(".mmcp.toml").exists());
    }

    #[tokio::test]
    async fn init_project_helper_returns_slug_required_when_no_source_available() {
        // Seed a config without a `project_slug`, then call without
        // a slug arg. The helper has no TTY to prompt, so it must
        // surface `slug_required`.
        let (state, tmp) = test_state().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("create project root");
        let cfg = mmcp_core::config::ProjectConfig {
            project_uuid: mmcp_core::id::ProjectUuid::new(),
            project_slug: None,
            sync: None,
            groups: Default::default(),
            languages: Default::default(),
        };
        crate::config::save(&project_root, &cfg).expect("seed config");

        let err = crate::commands::init::create_project_group_from_state(
            &state.backend,
            &state.groups,
            &project_root,
            &crate::commands::init::InitProjectOptions::default(),
        )
        .await
        .expect_err("must require slug");
        assert!(matches!(
            err,
            crate::commands::init::InitProjectError::SlugRequired
        ));
    }

    // ── write_memory (FR-018 tightening) ──────────────────────────

    fn write_memory_args(group: &GroupId, slug: &str, override_: bool) -> WriteMemoryArgs {
        WriteMemoryArgs {
            group: group.to_string(),
            slug: slug.to_string(),
            name: "Draft".into(),
            description: "Short desc".into(),
            kind: ToolMemoryKind::Rule,
            body: "# Draft\nBody.".into(),
            tags: Vec::new(),
            mandatory: false,
            override_,
        }
    }

    #[tokio::test]
    async fn write_memory_creates_fresh_slug_without_override() {
        let (state, _tmp) = test_state().await;
        // Seed one memory so the group repo exists; the write
        // targets a different slug.
        let group = seed_group_with_memory(&state, "rules", "existing", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);
        let res = server
            .write_memory(Parameters(write_memory_args(&group, "fresh", false)))
            .await
            .expect("create");
        let parsed = parse_ok_json(res);
        assert_eq!(parsed.get("slug").and_then(|v| v.as_str()), Some("fresh"));
        assert_eq!(parsed.get("replaced").and_then(|v| v.as_bool()), Some(false));
    }

    #[tokio::test]
    async fn write_memory_rejects_existing_slug_by_default() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "taken", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);
        let err = server
            .write_memory(Parameters(write_memory_args(&group, "taken", false)))
            .await
            .expect_err("must refuse collision");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("memory_already_exists")
        );
        assert_eq!(
            payload.get("slug").and_then(|v| v.as_str()),
            Some("taken")
        );
    }

    #[tokio::test]
    async fn write_memory_accepts_existing_slug_when_override_is_true() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "replaced", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);
        let res = server
            .write_memory(Parameters(write_memory_args(&group, "replaced", true)))
            .await
            .expect("override replaces");
        let parsed = parse_ok_json(res);
        assert_eq!(parsed.get("replaced").and_then(|v| v.as_bool()), Some(true));
    }

    // ── edit_memory (FR-016) ──────────────────────────────────────

    #[tokio::test]
    async fn edit_memory_replaces_body_leaving_frontmatter_intact() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "first", SAMPLE_MEMORY).await;
        let server = McpServer::new(state.clone());
        let res = server
            .edit_memory(Parameters(EditMemoryArgs {
                group: group.to_string(),
                slug: "first".into(),
                body: Some("# Edited\nNew body.".into()),
                ..Default::default()
            }))
            .await
            .expect("edit");
        let parsed = parse_ok_json(res);
        assert_eq!(parsed.get("slug").and_then(|v| v.as_str()), Some("first"));

        // Reload and confirm body changed, frontmatter preserved.
        let read = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: "first".into(),
                version: None,
            }))
            .await
            .expect("read");
        let parsed = parse_ok_json(read);
        assert!(
            parsed
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .contains("New body"),
            "body must reflect the edit; got: {parsed:?}",
        );
        let fm = parsed.get("frontmatter").expect("frontmatter object");
        assert_eq!(fm.get("name").and_then(|v| v.as_str()), Some("Sample"));
    }

    #[tokio::test]
    async fn edit_memory_tag_operators_compose_with_dedup() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "taggy", SAMPLE_MEMORY).await;
        let server = McpServer::new(state.clone());
        server
            .edit_memory(Parameters(EditMemoryArgs {
                group: group.to_string(),
                slug: "taggy".into(),
                tags_add: vec!["alpha".into(), "beta".into(), "sample".into()],
                tags_remove: vec!["sample".into()],
                ..Default::default()
            }))
            .await
            .expect("edit");
        let read = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: "taggy".into(),
                version: None,
            }))
            .await
            .expect("read");
        let parsed = parse_ok_json(read);
        let tags: Vec<String> = parsed
            .get("frontmatter")
            .and_then(|v| v.get("tags"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        assert!(tags.contains(&"alpha".to_string()));
        assert!(tags.contains(&"beta".to_string()));
        assert!(
            !tags.contains(&"sample".to_string()),
            "sample tag should be removed by tags_remove; got {tags:?}",
        );
        // Sort + dedup means the wire vec is monotonic; duplicate
        // ("alpha" inserted twice would collapse).
    }

    #[tokio::test]
    async fn edit_memory_returns_memory_not_found_when_slug_absent() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "existing", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);
        let err = server
            .edit_memory(Parameters(EditMemoryArgs {
                group: group.to_string(),
                slug: "ghost".into(),
                body: Some("n/a".into()),
                ..Default::default()
            }))
            .await
            .expect_err("must surface not-found");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("memory_not_found")
        );
        assert_eq!(
            payload.get("slug").and_then(|v| v.as_str()),
            Some("ghost")
        );
    }

    // ── delete_memory (FR-017) ────────────────────────────────────

    #[tokio::test]
    async fn delete_memory_removes_slug_from_listing() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "doomed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);
        server
            .delete_memory(Parameters(DeleteMemoryArgs {
                group: group.to_string(),
                slug: "doomed".into(),
                message: None,
            }))
            .await
            .expect("delete");
        let list = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: group.to_string(),
            }))
            .await
            .expect("list");
        let parsed = parse_ok_json(list);
        let slugs: Vec<String> = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("slug").and_then(|s| s.as_str().map(str::to_string)))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !slugs.contains(&"doomed".to_string()),
            "listing should exclude the deleted slug; got {slugs:?}",
        );
    }

    #[tokio::test]
    async fn delete_memory_errors_when_slug_absent() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "present", SAMPLE_MEMORY).await;
        let server = McpServer::new(state);
        let err = server
            .delete_memory(Parameters(DeleteMemoryArgs {
                group: group.to_string(),
                slug: "never-existed".into(),
                message: None,
            }))
            .await
            .expect_err("must surface not-found");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("memory_not_found")
        );
    }

    #[tokio::test]
    async fn init_project_helper_is_idempotent() {
        let (state, tmp) = test_state().await;
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("create project root");

        let opts = crate::commands::init::InitProjectOptions {
            slug: Some("team-rust".to_string()),
            ..Default::default()
        };
        let first = crate::commands::init::create_project_group_from_state(
            &state.backend,
            &state.groups,
            &project_root,
            &opts,
        )
        .await
        .expect("first");
        assert!(first.created_config);
        assert!(first.created_repo);

        let second = crate::commands::init::create_project_group_from_state(
            &state.backend,
            &state.groups,
            &project_root,
            &opts,
        )
        .await
        .expect("second");
        assert!(
            !second.created_config && !second.created_repo,
            "second call must not rewrite anything",
        );
    }
}
