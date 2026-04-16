//! `mmcp serve` implementation: MCP stdio server.
//!
//! Speaks real JSON-RPC 2.0 through the official `rmcp` crate.
//! Every exposed tool answers from real git content read via the
//! [`NativeBackend`] at `~/.mmcp/repos/`. No database is opened,
//! no placeholder responses are returned. Tools that need
//! per-session state (verification, compaction acknowledgement)
//! are deferred until Phase 5 of the implementation plan, when
//! the stdio server learns which session id it is serving.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use mmcp_core::id::GroupId;
use mmcp_core::memory::{MemoryFile, MemoryFrontmatter, MemoryKind};
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
use crate::state::{GroupEntry, GroupIndex, SessionStore, WatcherHandle, spawn_watcher};

/// Directory name under the user's home that holds mmcp state.
const MMCP_HOME_DIR: &str = ".mmcp";
/// Subdirectory holding bare group repositories.
const MMCP_REPOS_SUBDIR: &str = "repos";
/// Subdirectory holding per-session TOML state files.
const MMCP_SESSIONS_SUBDIR: &str = "sessions";
/// Subdirectory inside every group repo holding memory files.
const MEMORIES_PATH_PREFIX: &str = "memories";
/// File extension memory files use.
const MEMORY_EXTENSION: &str = ".md";

/// Run the MCP stdio server loop until the client disconnects.
pub async fn run() -> Result<()> {
    tracing::info!("mmcp stdio MCP server starting");
    let state = ClientState::initialize().await?;
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
    #[allow(dead_code)] // NOTE: consumed by session-scoped tools added in Phase 5.
    sessions: SessionStore,
    #[allow(dead_code)] // NOTE: held to keep the notify watcher alive for the process lifetime.
    watcher: WatcherHandle,
}

#[derive(Clone)]
struct ClientState(Arc<ClientStateInner>);

impl ClientState {
    async fn initialize() -> Result<Self> {
        let home = local_home()?;
        let mmcp_root = home.join(MMCP_HOME_DIR);
        let project_config_path = find_current_project_config();
        Self::initialize_at(mmcp_root, project_config_path).await
    }

    /// Initialize the client state rooted at an explicit directory.
    ///
    /// Used by `initialize()` above (rooted at the user's home) and
    /// by test helpers that want a tempdir-backed instance.
    async fn initialize_at(
        mmcp_root: PathBuf,
        project_config_path: Option<PathBuf>,
    ) -> Result<Self> {
        std::fs::create_dir_all(&mmcp_root)
            .with_context(|| format!("creating {}", mmcp_root.display()))?;

        let repos_root = mmcp_root.join(MMCP_REPOS_SUBDIR);
        let backend = Arc::new(
            NativeBackend::new(&repos_root)
                .with_context(|| format!("initializing repo root {}", repos_root.display()))?,
        );

        let sessions_root = mmcp_root.join(MMCP_SESSIONS_SUBDIR);
        let sessions = SessionStore::open(&sessions_root)
            .with_context(|| format!("opening session store at {}", sessions_root.display()))?;

        let groups = GroupIndex::build(repos_root.clone(), backend.clone())
            .await
            .with_context(|| format!("building group index at {}", repos_root.display()))?;

        let watcher = spawn_watcher(repos_root, project_config_path, groups.clone())
            .context("spawning filesystem watcher")?;

        Ok(Self(Arc::new(ClientStateInner {
            backend,
            groups,
            sessions,
            watcher,
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

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ImportMemoryArgs {
    /// Target group UUID.
    pub group: String,
    /// Memory slug.
    pub slug: String,
    /// Full markdown content with +++ TOML frontmatter.
    pub content: String,
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
        description = "Import a memory into a group. Content must be a complete markdown document with +++ TOML frontmatter including name, description, and kind fields. If a memory with the same slug already exists it is overwritten with a new commit."
    )]
    async fn import_memory(
        &self,
        #[allow(unused_variables)]
        Parameters(args): Parameters<ImportMemoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_group_id(&args.group)?;
        let entry = self
            .state
            .groups
            .get(&group_id)
            .await
            .ok_or_else(|| McpError::invalid_params("group not found", None))?;
        let result = crate::commands::import::import_memory(
            &self.state.backend,
            &entry.handle,
            &args.slug,
            &args.content,
            None,
        )
        .await
        .map_err(|e| McpError::internal_error(Cow::Owned(e.to_string()), None))?;
        Ok(ok_json(json!({
            "slug": result.slug,
            "commit_id": result.commit_id,
            "group": args.group,
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
                "mmcp memory server. Reads and writes memories directly from git repositories under ~/.mmcp/repos. Exposes list_memories, read_memory, list_versions, group_info, search_memories, and import_memory."
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
            MEMORIES_PATH_PREFIX,
            &Rev::Branch("main".to_string()),
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
            kind_to_string(file.frontmatter.kind).to_string(),
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
        None => Rev::Branch("main".to_string()),
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

fn memory_path(slug: &str) -> String {
    format!("{MEMORIES_PATH_PREFIX}/{slug}{MEMORY_EXTENSION}")
}

fn git_error(err: mmcp_git::GitError) -> McpError {
    McpError::internal_error(Cow::Owned(format!("git error: {err}")), None)
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

fn frontmatter_to_json(fm: &MemoryFrontmatter) -> serde_json::Value {
    json!({
        "name": fm.name,
        "description": fm.description,
        "kind": kind_to_string(fm.kind),
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
        let state = ClientState::initialize_at(tmp.path().join(".mmcp"), None)
            .await
            .expect("initialize_at");
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
                    branch: "main".to_string(),
                    author_name: "test".into(),
                    author_email: "test@example.com".into(),
                    message: format!("seed memory {memory_slug}"),
                    files: vec![(
                        format!("{MEMORIES_PATH_PREFIX}/{memory_slug}{MEMORY_EXTENSION}"),
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
