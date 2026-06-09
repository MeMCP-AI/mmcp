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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use mmcp_core::id::GroupId;
use mmcp_core::memory::{MemoryFile, MemoryFrontmatter};
use mmcp_git::{GitBackend, NativeBackend, Rev};
use rmcp::{
    ErrorData as McpError, Peer, RoleServer, ServerHandler, ServiceExt, elicit_safe,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars::JsonSchema,
    service::ElicitationError,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::notes::{
    dangling_ref_notes_for, findings_to_notes, id_validation_to_notes, malformed_frontmatter_notes,
};
use crate::state::{WatcherHandle, spawn_watcher};
use mmcp_store::config::{PROJECT_MANIFEST, find_project_root, load as load_project_config};
use mmcp_store::diagnostics::{
    DiagReport, diagnose_all, diagnose_group, health_check_all, health_check_group,
};
use mmcp_store::groups::{GroupEntry, GroupIndex};
use mmcp_store::home::{MmcpHome, ResolvedAuthor};
use mmcp_store::memory::ImportError;
use mmcp_store::sessions::SessionStore;


/// Run the MCP stdio server loop until the client disconnects.
pub async fn run(debug_mode: bool, serve_mode: ServeMode) -> Result<()> {
    if debug_mode {
        tracing::info!(
            "mmcp stdio MCP server starting (debug tools enabled, mode={})",
            serve_mode.as_label(),
        );
    } else {
        tracing::info!(
            "mmcp stdio MCP server starting (mode={})",
            serve_mode.as_label(),
        );
    }
    let state = ClientState::initialize(debug_mode).await?;
    let server = McpServer::new(state, serve_mode);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// Everything the MCP server needs to answer tool calls from local
/// state. No database. Git and flat session files only.
struct ClientStateInner {
    backend: Arc<NativeBackend>,
    groups: GroupIndex,
    #[allow(dead_code)]
    // NOTE: consumed by session-scoped tools once they're wired onto the router.
    sessions: SessionStore,
    #[allow(dead_code)] // NOTE: held to keep the notify watcher alive for the process lifetime.
    watcher: WatcherHandle,
    /// Resolved commit author from user config cascade.
    author: ResolvedAuthor,
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

/// Restrict the registered tool surface to a subset of the FR-029
/// annotation matrix, mirroring Serena's read-only / edit / full
/// posture. The check happens once at `McpServer::new` time so a
/// disabled tool is not announced through `tools/list` at all — a
/// stronger guarantee than the advisory-only annotation hints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ServeMode {
    /// Only tools whose `read_only_hint == Some(true)`.
    Readonly,
    /// Read-only tools plus mutators that are not flagged
    /// `destructive_hint = Some(true)`. Additive writes
    /// (`write_memory`, `import_memory`, `add_feature`, …) and
    /// non-destructive sync (`sync_fetch`, `sync_push`) stay
    /// available; deletes, rewrites, and replay-style sync
    /// (`sync_pull`, `sync`) are filtered out.
    Edit,
    /// Every registered tool. Default.
    #[default]
    Full,
}

impl ServeMode {
    /// Lower-case label round-tripped to clap, the `status` tool
    /// response, and the FR-031 catalogue. Picked to match the clap
    /// `ValueEnum` derived names so the CLI surface and the wire
    /// surface read identically.
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Readonly => "readonly",
            Self::Edit => "edit",
            Self::Full => "full",
        }
    }

    /// Decide whether `tool` is allowed under this mode using only
    /// its FR-029 annotations. Tools that lack annotations
    /// (which the FR-29 conformance test forbids on the production
    /// surface) get the conservative answer `false` for narrower
    /// modes — better to drop a tool than expose it under a stricter
    /// label than its annotations promise.
    fn allows(self, tool: &rmcp::model::Tool) -> bool {
        match self {
            Self::Full => true,
            other => {
                let ann = match tool.annotations.as_ref() {
                    Some(a) => a,
                    None => return false,
                };
                let read_only = ann.read_only_hint.unwrap_or(false);
                let destructive = ann.destructive_hint.unwrap_or(false);
                match other {
                    Self::Readonly => read_only,
                    Self::Edit => read_only || !destructive,
                    Self::Full => unreachable!(),
                }
            }
        }
    }
}

/// MCP server exposing the stateless mmcp tools that can be served
/// purely from local git repos.
#[derive(Clone)]
struct McpServer {
    state: ClientState,
    /// Active filter from the `--mode` flag. Stored so `status`
    /// and `get_info` can echo the running posture; the actual
    /// filtering happens once during `new`.
    mode: ServeMode,
    // NOTE: `tool_router` is read through the `#[tool_handler]`
    // macro's generated plumbing, not from our own code.
    #[allow(dead_code)]
    tool_router: ToolRouter<McpServer>,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct ListMemoriesArgs {
    /// Group UUID to list memories from.
    pub group: String,
    /// Optional FR-41 path prefix filter. When set, only memories
    /// whose slug starts with `<path_prefix>/` (or equals it) are
    /// returned. The prefix itself is matched literally — no
    /// wildcards or regexes.
    #[serde(default)]
    pub path_prefix: Option<String>,
    /// FR-41: when `false`, only memories whose slug has exactly
    /// one segment beyond `path_prefix` (or one segment total when
    /// no prefix is set) are returned. Defaults to `true` so the
    /// pre-FR-41 default of "every memory in the group" is
    /// preserved.
    #[serde(default)]
    pub recursive: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ReadMemoryArgs {
    /// Group UUID that owns the memory.
    pub group: String,
    /// Memory slug (directory under `memories/`). Optional when
    /// `id` is supplied; if both are present the server verifies
    /// that the memory at `memories/<slug>/<id>.md` has matching
    /// frontmatter.
    #[serde(default)]
    pub slug: Option<String>,
    /// Canonical UUID of the memory (FR-028). Optional when `slug`
    /// is supplied; required when multiple memories share a slug.
    #[serde(default)]
    pub id: Option<String>,
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

/// Wire-form [`mmcp_core::memory::MemoryRef`] input shared by every
/// tool that accepts typed cross-references (`write_memory` /
/// `edit_memory` / `add_feature` / `update_feature`). Plain-string
/// fields so schemars generates the obvious JSON object; the tool
/// methods funnel every entry through
/// [`mmcp_store::parse_memory_refs`] for UUID / commit-sha
/// validation.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct MemoryRefArg {
    /// UUID of the referenced memory (post-FR-028 primary key).
    pub target: String,
    /// 40-character lowercase hex commit sha pinning the reference
    /// to a specific revision of the target.
    pub commit: String,
}

impl MemoryRefArg {
    fn into_store_input(self) -> mmcp_store::MemoryRefInput {
        mmcp_store::MemoryRefInput {
            target: self.target,
            commit: self.commit,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct WriteMemoryArgs {
    /// Target group UUID or slug.
    pub group: String,
    /// Memory slug (lowercase alphanumeric + hyphens). Duplicate
    /// slugs are allowed post-FR-028; the server distinguishes
    /// memories by `id` inside the shared slug directory.
    pub slug: String,
    /// Canonical UUID to stamp into frontmatter and the on-disk
    /// path (FR-028). Leave absent to mint a fresh UUIDv7; supply
    /// an explicit id to pin an existing memory or to collide
    /// deliberately with `override: true`.
    #[serde(default)]
    pub id: Option<String>,
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
    /// Typed cross-references to other memories, pinned at a
    /// specific commit each. Empty list is the common case; the
    /// server stores absent as "no refs".
    #[serde(default)]
    pub refs: Vec<MemoryRefArg>,
    /// FR-38 provenance UUID. When set, this memory was filed by
    /// an agent acting on behalf of the named owner (group UUID
    /// for federated workflows; memory UUID when chaining a
    /// promoted copy). Absent means the owning group authored the
    /// memory itself. The string is parsed as a UUID; bad input
    /// errors with `code: invalid_source`.
    #[serde(default)]
    pub source: Option<String>,
    /// Opt into replacing an already-existing memory at this slug.
    /// `false` (default) makes the tool a strict CREATE — the wire
    /// name is `override` via serde rename; the Rust field uses a
    /// suffix to sidestep the reserved keyword. Callers almost
    /// never want this; prefer `edit_memory` for partial updates
    /// and reach for `override` only on deliberate replace-whole-
    /// file flows.
    #[serde(default, rename = "override")]
    pub override_: bool,
    /// FR-28 / D4: bypass the filename-vs-frontmatter id mismatch
    /// rejection on a `ByFilename` write. Defaults to `false` so
    /// drift is caught loudly; set `true` only when the caller has
    /// confirmed they intend to write a new payload at the same
    /// filename UUID even though the frontmatter id disagrees.
    #[serde(default)]
    pub force: bool,
}

/// Wire-form source format accepted by the `import_memory` tool.
///
/// `asciidoc` is accepted as an alias for `adoc` so callers can use
/// either common spelling; both route through the `acdc` bridge that
/// already backs the CLI `mmcp import` path.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum ToolImportSourceFormat {
    Markdown,
    #[serde(alias = "asciidoc")]
    Adoc,
}

impl ToolImportSourceFormat {
    fn is_adoc(self) -> bool {
        matches!(self, ToolImportSourceFormat::Adoc)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ImportMemoryArgs {
    /// Target group UUID or slug.
    pub group: String,

    /// Memory slug the imported file will land under. Duplicate
    /// slugs are legal post-FR-028; each import mints a fresh
    /// UUIDv7 when the source carries no `id` in its frontmatter.
    pub slug: String,

    /// Raw source document. May already carry a `+++` / `---` /
    /// `---json` frontmatter fence (in which case the `name` /
    /// `description` / `kind` args are ignored), or be a bare body
    /// that gets frontmatter stamped from those args. Mixing a
    /// fence with synth args is harmless - the fence wins.
    pub source: String,

    /// Source format. Defaults to `markdown` when absent;
    /// `adoc` / `asciidoc` routes through the AsciiDoc bridge before
    /// the normal import pipeline runs. The converted markdown is
    /// what lands on disk, so downstream tooling never sees the
    /// original format.
    #[serde(default)]
    pub format: Option<ToolImportSourceFormat>,

    /// Human-readable title. Required alongside `description` and
    /// `kind` when the source has no embedded frontmatter; ignored
    /// when the source already carries a fence.
    #[serde(default)]
    pub name: Option<String>,

    /// One-line summary. See `name` for the together-or-not-at-all
    /// rule against embedded frontmatter.
    #[serde(default)]
    pub description: Option<String>,

    /// Memory kind. See `name` for the together-or-not-at-all rule.
    #[serde(default)]
    pub kind: Option<ToolMemoryKind>,

    /// Replace an existing memory whose id collides with the one
    /// embedded in the source's frontmatter. Fresh imports (no
    /// pinned id) always create a new sibling, so this only
    /// matters for pinned-id flows.
    #[serde(default, rename = "override")]
    pub override_: bool,
    /// FR-28 / D4: bypass the filename-vs-frontmatter id mismatch
    /// rejection. Reserved for parity with the other write tools;
    /// `import_memory` mints / pins ids in lockstep with the
    /// filename, so the flag is a no-op on the happy path.
    #[serde(default)]
    pub force: bool,
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
    /// Memory slug (directory). Optional when `id` is supplied.
    #[serde(default)]
    pub slug: Option<String>,
    /// Canonical UUID of the memory (FR-028).
    #[serde(default)]
    pub id: Option<String>,
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
    /// Typed refs to add or replace. Dedupe is by target UUID —
    /// entries whose `target` already appears in the memory's
    /// existing refs are replaced in place (so the new commit pin
    /// wins); new targets are appended.
    #[serde(default)]
    pub refs_add: Vec<MemoryRefArg>,
    /// UUIDs to strip from the existing refs list. Commit sha is
    /// not part of the match so callers do not need to remember
    /// which revision a ref was pinned to.
    #[serde(default)]
    pub refs_remove: Vec<String>,
    /// Replace the mandatory flag. Absent leaves it untouched.
    #[serde(default)]
    pub mandatory: Option<bool>,
    /// Commit message override. Absent falls back to
    /// `"update memory {slug}"`.
    #[serde(default)]
    pub message: Option<String>,
    /// FR-28 / D4: bypass the filename-vs-frontmatter id mismatch
    /// rejection on a `ByFilename` write. Defaults to `false`.
    #[serde(default)]
    pub force: bool,
}

/// Argument shape for `delete_memory`.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct DeleteMemoryArgs {
    /// Target group UUID.
    pub group: String,
    /// Memory slug to remove. Optional when `id` is supplied.
    #[serde(default)]
    pub slug: Option<String>,
    /// Canonical UUID of the memory (FR-028).
    #[serde(default)]
    pub id: Option<String>,
    /// Commit message override. Absent falls back to
    /// `"delete memory {slug}"`.
    #[serde(default)]
    pub message: Option<String>,
    /// FR-28 / D4: present for parity with the other write tools;
    /// `delete_memory` does not render new bytes, so the flag is a
    /// no-op on the happy path.
    #[serde(default)]
    pub force: bool,
}

/// Args for `read_memory_body_sections` (FR-026).
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct ReadMemoryBodySectionsArgs {
    /// Target group UUID.
    pub group: String,
    /// Memory slug to inspect. Optional when `id` is supplied.
    #[serde(default)]
    pub slug: Option<String>,
    /// Canonical UUID of the memory (FR-028).
    #[serde(default)]
    pub id: Option<String>,
}

/// Args for `edit_memory_body` (FR-026).
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct EditMemoryBodyArgs {
    /// Target group UUID.
    pub group: String,
    /// Memory slug to mutate. Optional when `id` is supplied.
    #[serde(default)]
    pub slug: Option<String>,
    /// Canonical UUID of the memory (FR-028).
    #[serde(default)]
    pub id: Option<String>,
    /// Ordered list of body edits. Each op is a tagged union
    /// whose `op` field names the variant. See the
    /// [`ToolMemoryEditOp`] enum for the per-variant fields.
    #[serde(default)]
    pub ops: Vec<ToolMemoryEditOp>,
    /// Optional override for the git commit message.
    #[serde(default)]
    pub message: Option<String>,
    /// FR-28 / D4: bypass the filename-vs-frontmatter id mismatch
    /// rejection on a `ByFilename` write. Defaults to `false`.
    #[serde(default)]
    pub force: bool,
}

/// Tool-layer mirror of `mmcp_store::MemoryEditOp`. The store
/// enum deliberately does not depend on `rmcp::schemars` so the
/// store crate stays consumer-agnostic; this mirror carries the
/// `JsonSchema` derive the MCP tool schema needs and converts
/// into the store type before `apply_ops` runs.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "op", rename_all = "snake_case")]
enum ToolMemoryEditOp {
    UpsertSection {
        path: String,
        level: u8,
        heading: String,
        body: String,
    },
    DeleteSection {
        path: String,
    },
    InsertSectionBefore {
        anchor_path: String,
        level: u8,
        heading: String,
        body: String,
    },
    InsertSectionAfter {
        anchor_path: String,
        level: u8,
        heading: String,
        body: String,
    },
    MoveSectionBefore {
        target_path: String,
        anchor_path: String,
    },
    MoveSectionAfter {
        target_path: String,
        anchor_path: String,
    },
    ReplaceSectionBody {
        path: String,
        body: String,
    },
    InsertAtLine {
        line: u32,
        content: String,
    },
    ReplaceLines {
        start: u32,
        end: u32,
        content: String,
    },
    DeleteLines {
        start: u32,
        end: u32,
    },
}

impl From<ToolMemoryEditOp> for mmcp_store::MemoryEditOp {
    fn from(op: ToolMemoryEditOp) -> Self {
        match op {
            ToolMemoryEditOp::UpsertSection {
                path,
                level,
                heading,
                body,
            } => mmcp_store::MemoryEditOp::UpsertSection {
                path,
                level,
                heading,
                body,
            },
            ToolMemoryEditOp::DeleteSection { path } => {
                mmcp_store::MemoryEditOp::DeleteSection { path }
            }
            ToolMemoryEditOp::InsertSectionBefore {
                anchor_path,
                level,
                heading,
                body,
            } => mmcp_store::MemoryEditOp::InsertSectionBefore {
                anchor_path,
                level,
                heading,
                body,
            },
            ToolMemoryEditOp::InsertSectionAfter {
                anchor_path,
                level,
                heading,
                body,
            } => mmcp_store::MemoryEditOp::InsertSectionAfter {
                anchor_path,
                level,
                heading,
                body,
            },
            ToolMemoryEditOp::MoveSectionBefore {
                target_path,
                anchor_path,
            } => mmcp_store::MemoryEditOp::MoveSectionBefore {
                target_path,
                anchor_path,
            },
            ToolMemoryEditOp::MoveSectionAfter {
                target_path,
                anchor_path,
            } => mmcp_store::MemoryEditOp::MoveSectionAfter {
                target_path,
                anchor_path,
            },
            ToolMemoryEditOp::ReplaceSectionBody { path, body } => {
                mmcp_store::MemoryEditOp::ReplaceSectionBody { path, body }
            }
            ToolMemoryEditOp::InsertAtLine { line, content } => {
                mmcp_store::MemoryEditOp::InsertAtLine { line, content }
            }
            ToolMemoryEditOp::ReplaceLines {
                start,
                end,
                content,
            } => mmcp_store::MemoryEditOp::ReplaceLines {
                start,
                end,
                content,
            },
            ToolMemoryEditOp::DeleteLines { start, end } => {
                mmcp_store::MemoryEditOp::DeleteLines { start, end }
            }
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct SearchMemoriesArgs {
    /// Single substring matched against memory slug and frontmatter
    /// `name`, case-insensitive. Mutually exclusive with `queries`;
    /// passing both errors with `code: invalid_search_args`.
    #[serde(default)]
    pub query: Option<String>,
    /// Multi-substring form of `query`. Each entry is matched
    /// independently; results are deduped by memory UUID so a memory
    /// hit by N queries appears exactly once. Each hit carries a
    /// `matched_queries` list naming every supplied entry that
    /// matched it. Mutually exclusive with `query`.
    #[serde(default)]
    pub queries: Option<Vec<String>>,
    /// Optional maximum number of hits. Defaults to 50. Caps total
    /// deduped hits across all queries — not per query.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Optional group filter (UUID or slug). When set, only
    /// memories inside the matching group are considered. Absent
    /// preserves whole-mirror search; the read-only surface is
    /// lower risk than write ops so this stays opt-in rather than
    /// required.
    #[serde(default)]
    pub group: Option<String>,
    /// Optional scope filter. When set, only memories whose owning
    /// group carries the matching `GroupScope` are considered.
    /// Composes with `group`: if both are provided, the group must
    /// also satisfy the scope.
    #[serde(default)]
    pub scope: Option<ToolGroupScope>,
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

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct BootstrapContextArgs {
    /// Target project group (UUID or slug). When set, resolves
    /// against the local mirror without touching the filesystem;
    /// the response carries no `project_root` and no
    /// subscription resolution (no `.mmcp.toml` is loaded). Use
    /// `path` instead when you want subscriptions honored. FR-44.
    #[serde(default)]
    pub project: Option<String>,

    /// Project root override. When set, the server walks this
    /// path for `.mmcp.toml` instead of the process cwd. Lets
    /// MCP harnesses launched outside the project dir bootstrap
    /// against an explicit root, and lets parallel tests avoid
    /// racing on the process-global cwd.
    #[serde(default)]
    pub path: Option<String>,
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
struct StatusArgs {
    /// Target project group (UUID or slug). When omitted, the
    /// server falls back to walking `cwd` for a `.mmcp.toml`.
    /// When present, the response returns the minimal shape
    /// `{project_configured, project_uuid, groups}` — filesystem-
    /// anchored fields (`project_root`, `sync`) are only emitted
    /// on the cwd-walk branch since an explicit selector does
    /// not guarantee a local filesystem root. FR-44.
    #[serde(default)]
    pub project: Option<String>,
}

/// Argument shape for `list_groups`. Takes no parameters today;
/// a future `owner_scope` filter would land here without breaking
/// the wire contract since the field would default-serde in.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct ListGroupsArgs {}

#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct DescribeToolsArgs {}

// ── Elicitation payload shapes (FR-011) ──────────────────────────
//
// Each struct defines the JSON schema the server sends in the
// elicitation request; the client renders a matching form and
// returns the filled payload. `elicit_safe!` registers the type
// as eligible for the peer.elicit() API.
//
// The string fields use a `choice` discriminator rather than a
// typed Rust enum because the elicitation schema subset supports
// string + enum constraints cleanly but can refuse nested
// `oneOf`-style enums depending on the client implementation; a
// plain string keeps the wire shape portable across every rmcp
// client that speaks elicitation.

/// Response shape for the CLAUDE.md conflict-resolution prompt.
///
/// Mirrors the pre-elicitation `conflict_unresolved` structured
/// error: the three legal choices are `override` (overwrite with no
/// backup), `backup_override` (write `.bak`, then overwrite), and
/// `cancel` (abort). Anything else round-trips back as a generic
/// bad-response error.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ClaudeConflictPrompt {
    /// One of `override`, `backup_override`, `cancel`. The
    /// elicitation client displays these as radio options.
    choice: String,
}
elicit_safe!(ClaudeConflictPrompt);

/// Response shape for the protected-group confirmation prompt.
///
/// A single boolean so the client renders a "confirm the write
/// into `<group>`" checkbox. Absent / false → abort the write with
/// `protected_write_cancelled`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ProtectedWriteConfirm {
    /// `true` → proceed with the write. Any other value aborts.
    confirmed: bool,
}
elicit_safe!(ProtectedWriteConfirm);

/// Shared argument shape for `sync_pull`, `sync_push`, and `sync`.
///
/// Exactly one selector governs which groups the operation touches
/// and is required on every call:
///
/// - `group` - a UUID or slug identifying a single group.
/// - `scope` - every locally-known group whose manifest carries
///   the given `GroupScope`.
/// - `all` - explicit fanout across the whole mirror.
///
/// Empty or multi-selector calls surface as `selector_required` /
/// `selector_conflict` errors so AI clients cannot silently trigger
/// a whole-mirror write.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct SyncToolArgs {
    /// Target a single group by UUID or slug. Mutually exclusive
    /// with `scope` and `all`.
    #[serde(default)]
    pub group: Option<String>,

    /// Target every locally-known group whose manifest carries the
    /// chosen `GroupScope`. Mutually exclusive with `group` and
    /// `all`. Wire-form mirrors the serde rename: `"global"`,
    /// `"shared"`, `"project"`.
    #[serde(default)]
    pub scope: Option<ToolGroupScope>,

    /// Explicit opt-in to fan out across the whole local mirror.
    /// This is the only way to reproduce pre-scoping behaviour
    /// once the breaking flip lands. Mutually exclusive with
    /// `group` and `scope`.
    #[serde(default)]
    pub all: Option<bool>,
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

/// Wire-form mirror of [`mmcp_core::manifest::GroupScope`].
///
/// Kept as a separate enum so the JSON schema exported by `rmcp` is
/// owned by this crate; the mmcp-core definition stays serde-only
/// and unaware of schemars. `rename_all = "snake_case"` matches
/// [`GroupScope`]'s own serde rename, so the wire strings are
/// identical (`"global"`, `"shared"`, `"project"`).
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum ToolGroupScope {
    Global,
    Shared,
    Project,
}

impl ToolGroupScope {
    fn into_core(self) -> mmcp_core::manifest::GroupScope {
        use mmcp_core::manifest::GroupScope;
        match self {
            ToolGroupScope::Global => GroupScope::Global,
            ToolGroupScope::Shared => GroupScope::Shared,
            ToolGroupScope::Project => GroupScope::Project,
        }
    }
}

/// Render a [`GroupScope`] as its wire string. Matches the enum's
/// serde `rename_all = "snake_case"` so response and request
/// vocabularies stay in lockstep.
fn group_scope_wire(scope: mmcp_core::manifest::GroupScope) -> &'static str {
    use mmcp_core::manifest::GroupScope;
    match scope {
        GroupScope::Global => "global",
        GroupScope::Shared => "shared",
        GroupScope::Project => "project",
    }
}

/// Argument shape for `create_group`.
///
/// Bootstraps a standalone group repository under `~/.mmcp/repos/`
/// without touching any `.mmcp.toml` on the filesystem. Use this for
/// `global` rule sets or `shared` language/team bundles; use
/// `init_project` when the group IS a specific project's memory
/// store.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct CreateGroupArgs {
    /// Group slug (kebab-case, 1-128 chars, no leading/trailing or
    /// consecutive hyphens). Must be unique across the local mirror;
    /// a collision errors with `slug_already_exists` carrying the
    /// pre-existing `group_id`.
    pub slug: String,

    /// Optional human-readable name surfaced in listings.
    #[serde(default)]
    pub display_name: Option<String>,

    /// Cross-project reach of the new group. Defaults to `shared`,
    /// which is the right answer for groups consumed by multiple
    /// projects that opt in. Pick `global` only for install-wide
    /// rule sets that should surface in every session; `project` is
    /// accepted for completeness but rarely useful outside
    /// `init_project`.
    #[serde(default)]
    pub scope: Option<ToolGroupScope>,

    /// When true, the manifest's `protected` flag is set so every
    /// subsequent mutation goes through the FR-019 confirmation
    /// guard.
    #[serde(default)]
    pub protected: bool,
}

// ── Feature-request tool arg shapes (FR-007) ─────────────────────
//
// Each FR tool auto-resolves the project group from the server
// process's cwd (same discovery as `status`/`bootstrap_context`),
// so none of these structs carry a `group` field — writing into
// anything other than the project's own FR backlog goes through
// `write_memory`.

/// Args for `add_feature`.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct AddFeatureArgs {
    /// Target project group (UUID or slug). When omitted, the
    /// server falls back to walking `cwd` for a `.mmcp.toml` and
    /// using whichever project it finds. FR-44.
    #[serde(default)]
    pub project: Option<String>,

    /// Stable slug for the FR. Auto-minted from the title when
    /// omitted; when present must satisfy the memory-slug contract.
    #[serde(default)]
    pub slug: Option<String>,

    /// Human-readable title shown in listings. Required unless a
    /// slug is supplied explicitly.
    #[serde(default)]
    pub title: String,

    /// One-line summary, used by listings and relevance inference.
    #[serde(default)]
    pub description: String,

    /// Full FR body as freeform markdown. Convention: `## Need`
    /// first, optional `## Resolution` / `## Non-goals` sections
    /// after. Not parsed by the tool — preserved verbatim.
    #[serde(default)]
    pub body: String,

    /// Initial status. Defaults to `open` when absent. Wire form is
    /// the snake_case enum: `open | resolved | blocked | deferred | duplicate`.
    #[serde(default)]
    pub status: Option<String>,

    /// Slugs of FRs this one depends on.
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Slugs of FRs whose resolution is gated on this one.
    #[serde(default)]
    pub blocks: Vec<String>,

    /// Typed cross-references attached to the new FR. Every entry
    /// is a `{target, commit}` pair pinning the referenced memory
    /// to a specific revision. Absent is the common case.
    #[serde(default)]
    pub refs: Vec<MemoryRefArg>,

    /// Slug (or UUID) of an existing FR in the same project group
    /// to supersede. When present, the server runs the two-commit
    /// supersede flow: commit A writes this new FR with its refs
    /// auto-populated to include the target; commit B re-writes
    /// the target FR with `status = superseded` and a typed
    /// `superseded_by` back-link pointing at commit A. Errors:
    /// `supersedes_unknown`, `supersedes_invalid_status`,
    /// `supersedes_cross_group_unsupported`.
    #[serde(default)]
    pub supersedes: Option<String>,

    /// FR-38 provenance UUID. When set, this FR was filed by an
    /// agent acting on behalf of the named owner — group UUID for
    /// federated workflows where one project files an FR against
    /// another, or memory UUID when the FR was promoted from an
    /// existing reference memory. Absent means the project group
    /// authored the FR directly. Errors with `code: invalid_source`
    /// when not parseable as a UUID.
    #[serde(default)]
    pub source: Option<String>,

    /// Optional override for the git commit message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Args for `read_feature`.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct ReadFeatureArgs {
    /// Target project group (UUID or slug). When omitted, the
    /// server falls back to walking `cwd` for a `.mmcp.toml`. FR-44.
    #[serde(default)]
    pub project: Option<String>,

    /// Slug of the FR to read.
    pub slug: String,

    /// Branch name, tag, or 40-char commit hex. Defaults to the
    /// group's `main` when absent.
    #[serde(default)]
    pub version: Option<String>,
}

/// Args for `update_feature`.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct UpdateFeatureArgs {
    /// Target project group (UUID or slug). When omitted, the
    /// server falls back to walking `cwd` for a `.mmcp.toml`. FR-44.
    #[serde(default)]
    pub project: Option<String>,

    /// Slug of the FR to mutate.
    pub slug: String,

    /// New title; omit to leave unchanged.
    #[serde(default)]
    pub title: Option<String>,

    /// New description; omit to leave unchanged.
    #[serde(default)]
    pub description: Option<String>,

    /// Replacement body; omit to leave unchanged.
    #[serde(default)]
    pub body: Option<String>,

    /// New status; omit to leave unchanged. Wire form matches
    /// `AddFeatureArgs::status`.
    #[serde(default)]
    pub status: Option<String>,

    /// Replacement `depends_on` list; omit to leave unchanged.
    /// Pass `[]` to clear.
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,

    /// Replacement `blocks` list; omit to leave unchanged.
    /// Pass `[]` to clear.
    #[serde(default)]
    pub blocks: Option<Vec<String>>,

    /// Typed refs to add or replace. Dedupe is by target UUID;
    /// entries whose target already appears replace in place
    /// (add-side commit pin wins on collision).
    #[serde(default)]
    pub refs_add: Vec<MemoryRefArg>,

    /// UUIDs to strip from the existing refs list. Commit sha is
    /// not part of the match.
    #[serde(default)]
    pub refs_remove: Vec<String>,

    /// Typed `superseded_by` back-link retry path for when commit
    /// B of a two-commit `supersedes` flow half-landed. Callers
    /// pass the new FR's `{target, commit}`; the server flips
    /// `status` to `superseded` and writes the link. Setting this
    /// with any other `status` surfaces the
    /// `SupersedeInvariantError` via `invalid_memory_ref`.
    #[serde(default)]
    pub superseded_by: Option<MemoryRefArg>,

    /// Optional override for the git commit message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Args for `delete_feature`.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct DeleteFeatureArgs {
    /// Target project group (UUID or slug). When omitted, the
    /// server falls back to walking `cwd` for a `.mmcp.toml`. FR-44.
    #[serde(default)]
    pub project: Option<String>,

    /// Slug of the FR to delete.
    pub slug: String,

    /// Optional override for the git commit message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Args for `move_memory` (FR-41).
///
/// Atomically rewrites a memory's slug path in a single commit.
/// The memory id stays stable across the move, so cross-refs in
/// other memories remain valid. In-group only — cross-group
/// transfer is FR-36 territory and will extend this tool with an
/// optional `target_group` later.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct MoveMemoryArgs {
    /// Target group UUID.
    pub group: String,
    /// Source slug path. Optional when `id` is supplied; if both
    /// are present they must address the same memory.
    #[serde(default)]
    pub slug: Option<String>,
    /// Canonical UUID of the memory (FR-028).
    #[serde(default)]
    pub id: Option<String>,
    /// New slug path. May be a single segment (`feedback`) or a
    /// `/`-joined multi-segment path (`feedback/git/commit-phase`)
    /// up to [`mmcp_store::MAX_SLUG_SEGMENTS`] segments.
    pub new_slug: String,
    /// Optional override for the git commit message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Args for `rename_feature` (FR-027).
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct RenameFeatureArgs {
    /// Target project group (UUID or slug). When omitted, the
    /// server falls back to walking `cwd` for a `.mmcp.toml`. FR-44.
    #[serde(default)]
    pub project: Option<String>,

    /// Current slug directory. Every memory under
    /// `memories/<old_slug>/` moves in one atomic commit.
    pub old_slug: String,

    /// Target slug directory. UUIDs stay stable across the move
    /// so cross-refs in other features continue to resolve.
    pub new_slug: String,

    /// Optional override for the git commit message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Args for `list_features`.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[schemars(crate = "rmcp::schemars")]
struct ListFeaturesArgs {
    /// Target project group (UUID or slug). When omitted, the
    /// server falls back to walking `cwd` for a `.mmcp.toml`. FR-44.
    #[serde(default)]
    pub project: Option<String>,

    /// Restrict to FRs with this status. Wire form matches
    /// `AddFeatureArgs::status`. Explicit selector wins over the
    /// `all` flag — an operator asking for `resolved` FRs always
    /// sees them even when the default hide is on.
    #[serde(default)]
    pub status: Option<String>,

    /// When `true`, include FRs whose status is not `open`.
    /// Defaults to `false` — the tool returns only `open` FRs
    /// unless `status` selects a different variant or `all` is set.
    /// FR-024.
    #[serde(default)]
    pub all: Option<bool>,
}

/// Arguments for the `export_archive` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ExportArchiveArgs {
    /// Groups to export (UUID or slug). Repeatable. Mutually
    /// exclusive with `all`.
    #[serde(default)]
    pub group: Vec<String>,
    /// Export every group in the local mirror.
    #[serde(default)]
    pub all: bool,
    /// Destination archive path on the server's filesystem.
    pub output: String,
    /// gzip-compress the tar stream.
    #[serde(default)]
    pub gzip: bool,
}

/// Arguments for the `import_archive` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ImportArchiveArgs {
    /// Path to the archive file on the server's filesystem.
    pub input: String,
    /// Remap every memory into this existing group (UUID or slug)
    /// instead of recreating the archived groups.
    #[serde(default)]
    pub into: Option<String>,
    /// Replace colliding memories instead of reporting a conflict.
    #[serde(default)]
    pub overwrite: bool,
    /// Mint fresh UUIDs for every imported memory (fork / copy).
    #[serde(default)]
    pub new_ids: bool,
}

#[tool_router]
impl McpServer {
    fn new(state: ClientState, mode: ServeMode) -> Self {
        let mut tool_router = Self::tool_router();
        if !matches!(mode, ServeMode::Full) {
            // ToolRouter's `map` is `pub`; filtering at construction
            // time means dropped tools never appear on `tools/list`,
            // closing the gap between the advisory FR-029 hints and
            // hard registration-level enforcement.
            tool_router.map.retain(|_, route| mode.allows(&route.attr));
        }
        // FR-49 / FR-50 / FR-45: patch icons, meta, and the shared
        // output schema on the live router so `tools/list` surfaces
        // the same fields `describe_tools` returns. The static
        // `_tool_attr()` helpers do not carry these (rmcp builds
        // them at macro-expansion time), so the canonical patch
        // lives here and in `registered_tool_attrs()`.
        let output_schema = shared_output_schema();
        for (name, route) in tool_router.map.iter_mut() {
            let name_str = name.as_ref();
            route.attr.icons =
                Some(icons_for_category(tool_icon_category(name_str)));
            route.attr.meta = meta_for_tool(name_str);
            route.attr.output_schema = Some(output_schema.clone());
        }
        Self {
            state,
            mode,
            tool_router,
        }
    }

    #[tool(
        description = "Enumerate every group the local mirror holds. Returns `{groups: [{slug, uuid, memory_count, protected, is_project}]}` — cheap manifest-only walk, no memory bodies. `is_project` is true for the group whose UUID matches the current cwd's `.mmcp.toml`; false for every other group including cases where no project is in scope. Pure-local, no network.",
        annotations(
            title = "List mirrored groups",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn list_groups(
        &self,
        Parameters(_args): Parameters<ListGroupsArgs>,
    ) -> Result<CallToolResult, McpError> {
        // Resolve the project uuid up front so every group row can
        // flag whether it matches without re-reading the config per
        // iteration. Missing / unreadable config → `None`, which
        // just means no row will be flagged `is_project: true`.
        let project_uuid = std::env::current_dir()
            .ok()
            .and_then(|cwd| find_project_root(&cwd))
            .and_then(|root| load_project_config(&root).ok())
            .map(|cfg| *cfg.project_uuid.as_uuid());

        let entries = self.state.groups.list().await;
        let mut groups = Vec::with_capacity(entries.len());
        for entry in entries {
            let files = list_memory_files(&self.state.backend, &entry).await?;
            let uuid = entry.manifest.group_id;
            groups.push(json!({
                "slug":         entry.manifest.slug,
                "uuid":         uuid.to_string(),
                "memory_count": files.len(),
                "protected":    entry.manifest.protected,
                "is_project":   project_uuid == Some(*uuid.as_uuid()),
            }));
        }
        Ok(ok_json(json!({
            "groups": groups,
            "count":  groups.len(),
        })))
    }

    #[tool(
        description = "List memories that live in the specified group. The group argument is the group UUID. Returns `{group, memories, mirrored: bool}` — `mirrored: false` signals the group UUID is unknown to the local mirror (distinct from a mirrored-but-empty group, which returns `mirrored: true` with `memories: []`). FR-41: pass `path_prefix` to restrict to a slug subtree (literal prefix, no wildcards), and `recursive: false` to surface only the immediate children at that prefix level.",
        annotations(
            title = "List memories in a group",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn list_memories(
        &self,
        Parameters(args): Parameters<ListMemoriesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let group_id = parse_group_id(&args.group)?;
        // `mirrored: false` distinguishes "group UUID is unknown to
        // the local mirror" from "group exists locally but is empty"
        // (which returns `mirrored: true` with `memories: []`).
        // Before the signal landed both states collapsed to the same
        // wire shape and callers silently skipped mandatory re-reads
        // when the project group happened not to be pulled yet.
        let Some(entry) = self.state.groups.get(&group_id).await else {
            return Ok(ok_json(json!({
                "group":    args.group,
                "memories": Vec::<serde_json::Value>::new(),
                "mirrored": false,
            })));
        };
        let files = list_memory_files(&self.state.backend, &entry).await?;
        let recursive = args.recursive.unwrap_or(true);
        let prefix = args.path_prefix.as_deref().map(|p| p.trim_end_matches('/'));
        let mut memories = Vec::with_capacity(files.len());
        for file in files {
            if !slug_matches_filter(&file.slug, prefix, recursive) {
                continue;
            }
            let descriptor = read_memory_descriptor(
                &self.state.backend,
                &entry,
                &file.path,
                &file.slug,
                None,
            )
            .await
            .map_err(git_error)?;
            memories.push(descriptor);
        }
        Ok(ok_json(json!({
            "group":    entry.manifest.group_id,
            "memories": memories,
            "mirrored": true,
        })))
    }

    #[tool(
        description = "Read a memory by group and slug. Returns the TOML frontmatter and the Markdown body exactly as stored in git. Set `version` to a branch name, tag, or commit hex to read a specific revision; defaults to the latest `main`.",
        annotations(
            title = "Read a memory",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn read_memory(
        &self,
        Parameters(args): Parameters<ReadMemoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (entry, resolved) = self
            .resolve_memory_address(&args.group, args.slug.as_deref(), args.id.as_deref())
            .await?;
        let rev = parse_rev(args.version.as_deref());
        let bytes = self
            .state
            .backend
            .read_file(&entry.handle, &resolved.path, &rev)
            .await
            .map_err(|e| match e {
                mmcp_git::GitError::PathNotFound(p) => McpError::invalid_params(
                    "memory not found in group",
                    Some(json!({ "group": entry.manifest.group_id.to_string(), "path": p })),
                ),
                mmcp_git::GitError::RevNotFound(r) => {
                    McpError::invalid_params("revision not found", Some(json!({ "revision": r })))
                }
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
                Some(json!({ "slug": resolved.slug })),
            )
        })?;
        // FR-45 `malformed_frontmatter` populator: the parser
        // accepted the file (hard errors already returned above)
        // but some soft integrity signals are worth surfacing so
        // callers know to reconcile. Shape matches what
        // `mcp:diagnose` flags, but returned through the notes
        // channel per-read.
        let notes = malformed_frontmatter_notes(&resolved.slug, resolved.id, &file);
        Ok(ok_json_with_notes(
            json!({
                "group": entry.manifest.group_id,
                "slug": resolved.slug,
                "id": resolved.id.to_string(),
                "version": rev_label(&rev),
                "frontmatter": frontmatter_to_json(&file.frontmatter),
                "body": file.body,
            }),
            notes,
        ))
    }

    #[tool(
        description = "List the commit history of a single memory, most recent first. Each entry includes the commit id, author, message, and timestamp.",
        annotations(
            title = "List memory versions",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn list_versions(
        &self,
        Parameters(args): Parameters<ListVersionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let entry = self.resolve_group_entry(&args.group).await?;
        let resolved = mmcp_store::resolve_memory(
            &self.state.backend,
            &entry.handle,
            Some(&args.slug),
            None,
        )
        .await
        .map_err(map_memory_error_to_mcp)?;
        let history = self
            .state
            .backend
            .walk_history(&entry.handle, &resolved.path)
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
            "slug": resolved.slug,
            "id": resolved.id.to_string(),
            "versions": versions,
        })))
    }

    #[tool(
        description = "Return the manifest metadata for a group: slug, display name, owner kind and id, creation timestamp, and the number of memories currently stored in the group.",
        annotations(
            title = "Inspect a group manifest",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        description = "Case-insensitive substring search across the local mirror. Matches against the memory slug and the frontmatter `name` field. Pass `query: String` for the single-substring form (legacy shape, hits returned bare) OR `queries: Vec<String>` for multi-substring novelty checks (each hit wraps the descriptor under `memory` and carries a `matched_queries` array; results dedupe by memory UUID across the whole query set). Passing both errors with `invalid_search_args`. Optional `group` (UUID or slug) and `scope` (`global`/`shared`/`project`) filter the search set; absent means whole-mirror search, which stays the default because the tool is read-only. Returns up to `limit` hits (default 50, total cap across all queries).",
        annotations(
            title = "Search memories",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn search_memories(
        &self,
        Parameters(args): Parameters<SearchMemoriesArgs>,
    ) -> Result<CallToolResult, McpError> {
        // FR-43: accept either `query` (legacy single-string form,
        // unchanged shape) or `queries` (Vec<String>, deduped output
        // with `matched_queries` per hit). Passing both is rejected
        // up front so callers cannot half-fall-back to the legacy
        // shape mid-flight.
        let (needles, originals, multi_mode) = match (args.query, args.queries) {
            (Some(_), Some(_)) => {
                return Err(McpError::invalid_params(
                    "pass exactly one of `query` or `queries`",
                    Some(json!({ "code": "invalid_search_args" })),
                ));
            }
            (None, None) => {
                return Err(McpError::invalid_params(
                    "either `query` or `queries` must be set",
                    Some(json!({ "code": "invalid_search_args" })),
                ));
            }
            (Some(q), None) => {
                let needle = q.trim().to_lowercase();
                if needle.is_empty() {
                    return Err(McpError::invalid_params("query must not be empty", None));
                }
                (vec![needle], vec![q], false)
            }
            (None, Some(qs)) => {
                if qs.is_empty() {
                    return Err(McpError::invalid_params(
                        "queries must contain at least one entry",
                        Some(json!({ "code": "invalid_search_args" })),
                    ));
                }
                let mut needles = Vec::with_capacity(qs.len());
                for q in &qs {
                    let n = q.trim().to_lowercase();
                    if n.is_empty() {
                        return Err(McpError::invalid_params(
                            "queries entries must not be empty",
                            Some(json!({ "code": "invalid_search_args" })),
                        ));
                    }
                    needles.push(n);
                }
                (needles, qs, true)
            }
        };
        let limit = args.limit.unwrap_or(50).max(1) as usize;
        // Resolve the optional group filter once up front so the
        // per-entry loop is a straight UUID compare, mirroring how
        // sync's resolve_sync_filter collapses slug lookups.
        let group_filter = match args.group.as_deref() {
            Some(query) => Some(
                mmcp_store::resolve_group(&self.state.groups, query)
                    .await
                    .map_err(|e| {
                        McpError::invalid_params(
                            e.to_string(),
                            Some(json!({
                                "code": "unknown_group",
                                "query": query,
                            })),
                        )
                    })?
                    .manifest
                    .group_id,
            ),
            None => None,
        };
        let scope_filter = args.scope.map(ToolGroupScope::into_core);
        let mut hits: Vec<serde_json::Value> = Vec::new();
        for entry in self.state.groups.list().await {
            if hits.len() >= limit {
                break;
            }
            if let Some(target) = group_filter {
                if entry.manifest.group_id != target {
                    continue;
                }
            }
            if let Some(target) = scope_filter {
                if entry.manifest.scope != target {
                    continue;
                }
            }
            let files = list_memory_files(&self.state.backend, &entry).await?;
            for file in files {
                if hits.len() >= limit {
                    break;
                }
                let slug_lower = file.slug.to_lowercase();
                let descriptor = match read_memory_descriptor(
                    &self.state.backend,
                    &entry,
                    &file.path,
                    &file.slug,
                    None,
                )
                .await
                {
                    Ok(d) => d,
                    Err(err) => {
                        tracing::warn!(slug = %file.slug, error = %err, "search: descriptor read failed, skipping");
                        continue;
                    }
                };
                let name_lower = descriptor
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::to_lowercase)
                    .unwrap_or_default();
                let mut matched: Vec<String> = Vec::new();
                for (needle, original) in needles.iter().zip(originals.iter()) {
                    if slug_lower.contains(needle) || name_lower.contains(needle) {
                        matched.push(original.clone());
                    }
                }
                if matched.is_empty() {
                    continue;
                }
                if multi_mode {
                    hits.push(json!({
                        "memory": descriptor,
                        "matched_queries": matched,
                    }));
                } else {
                    hits.push(descriptor);
                }
            }
        }
        Ok(ok_json(json!({
            "hits": hits,
        })))
    }

    #[tool(
        description = "CREATE a new memory in a group. All metadata fields (name, description, kind, tags, mandatory) are typed parameters — the server builds the frontmatter. Errors with code `memory_already_exists` when the slug is already on disk; use `edit_memory` to apply partial updates, `delete_memory` to remove, or pass `override: true` to deliberately replace the whole file (bulk-reset flows only — the default should almost always stay false).",
        annotations(
            title = "Create memory",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    async fn write_memory(
        &self,
        Parameters(args): Parameters<WriteMemoryArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let entry = self.resolve_group_entry(&args.group).await?;
        // Even a fresh CREATE on a protected group needs user-
        // visible confirmation: adding an unauthorized rule to
        // `global` has the same blast radius as editing one.
        confirm_protected_write(
            &peer,
            &entry,
            &args.slug,
            if args.override_ { "override" } else { "create" },
        )
        .await?;
        self.write_memory_unguarded(args).await
    }

    /// Peer-less test entry point: re-resolves the entry and
    /// commits the write WITHOUT firing the elicitation guard.
    /// The public `write_memory` tool calls this after
    /// `confirm_protected_write` returns `Ok`; tests call it
    /// directly to cover memory-layer behaviour without
    /// constructing a mock `Peer`.
    async fn write_memory_unguarded(
        &self,
        args: WriteMemoryArgs,
    ) -> Result<CallToolResult, McpError> {
        let entry = self.resolve_group_entry(&args.group).await?;

        let kind = args.kind.into_core();

        // FR-028: every new memory gets a UUIDv7 primary key.
        // Callers can pin an explicit id (for migrations or to
        // collide under `override: true`); otherwise we mint one.
        let supplied_id = parse_optional_uuid(args.id.as_deref())?;
        let id = supplied_id.unwrap_or_else(Uuid::now_v7);

        let refs = parse_wire_refs(args.refs, "refs")?;
        // FR-38: round-trip the optional source UUID into the
        // frontmatter. Bad input fails fast with `invalid_source`
        // so callers see the contract — group UUID for federated
        // workflows, memory UUID for chained promotions; either
        // way it's a parseable UUID.
        let source = parse_optional_source(args.source.as_deref())?;
        use mmcp_core::memory::{FrontmatterFormat, MemoryFile, MemoryFrontmatter};
        let file = MemoryFile {
            frontmatter: MemoryFrontmatter::new(args.name, args.description, kind)
                .with_id(id)
                .with_mandatory(args.mandatory)
                .with_tags(args.tags)
                .with_refs(refs)
                .with_source(source),
            body: args.body,
            format: FrontmatterFormat::TomlPlus,
        };
        let rendered = file
            .to_string()
            .map_err(|e| McpError::internal_error(Cow::Owned(e.to_string()), None))?;

        // FR-39 v2: group-level create chain — Shared Process +
        // Exclusive Group(g). The kind-partitioned scope is gone;
        // every create in the group serialises on Group(g) so the
        // shared ticket counter and slug-uniqueness invariant
        // both run against a stable view.
        let _lock_guards = mmcp_store::lock::acquire_chain(&mmcp_store::lock::create_chain(
            *entry.manifest.group_id.as_uuid(),
        ))
        .await;
        let _ = kind;

        // FR-28 / D4: write_memory mints / pins `id` and stamps it
        // into frontmatter on the lines above, so filename and
        // frontmatter agree by construction. Address by filename
        // (the caller specified the slug+id pair) and let the
        // store-side `validate_id_mismatch` pick up future drift.
        let (commit_id, validation) = mmcp_store::write_memory_by_id(
            &self.state.backend,
            &entry.handle,
            &args.slug,
            id,
            &rendered,
            &self.state.author,
            args.override_,
            mmcp_store::AddressingMode::ByFilename,
            args.force,
            None,
        )
        .await
        .map_err(map_memory_error_to_mcp)?;

        // FR-45 `deprecated_arg_form`: `override: true` rewrites
        // the whole file and is almost never the right call.
        // `mcp:edit_memory` targets specific fields and keeps git
        // history cleaner. Surface a soft nudge in the notes
        // channel so callers can migrate.
        let mut notes = Vec::new();
        if args.override_ {
            notes.push(
                mmcp_proto::Note::warn(
                    "deprecated_arg_form",
                    "override: true rewrites the whole file; prefer edit_memory for partial updates",
                )
                .with_context(json!({
                    "tool": "write_memory",
                    "arg": "override",
                    "alternative": "edit_memory",
                })),
            );
        }
        notes.extend(id_validation_to_notes(&validation, &args.slug));
        Ok(ok_json_with_notes(
            json!({
                "slug": args.slug,
                "id": id.to_string(),
                "commit_id": commit_id,
                "group": args.group,
                "replaced": args.override_,
            }),
            notes,
        ))
    }

    #[tool(
        description = "Import a memory from a source document (markdown with a `+++` / `---` / `---json` frontmatter fence, or a raw body plus `name` + `description` + `kind` synth fields). Set `format` to `adoc` / `asciidoc` to route through the AsciiDoc bridge before import; the stored memory always lands as markdown at `memories/<slug>/<uuid>.md`. For typed-args creation with no source parsing use `write_memory`; for partial edits use `edit_memory`. Errors include `invalid_slug`, `synth_frontmatter_partial`, `missing_frontmatter`, `memory_already_exists`, and `adoc_parse_failed` / `adoc_render_failed`.",
        annotations(
            title = "Import memory from source document",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    async fn import_memory(
        &self,
        Parameters(args): Parameters<ImportMemoryArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let entry = self.resolve_group_entry(&args.group).await?;
        confirm_protected_write(&peer, &entry, &args.slug, "import").await?;
        self.import_memory_unguarded(args).await
    }

    /// Peer-less test entry point matching `write_memory_unguarded`.
    /// The public `import_memory` tool delegates here after the
    /// protected-group guard returns `Ok`; tests call it directly
    /// to cover the conversion + synth paths without constructing
    /// a mock `Peer`.
    async fn import_memory_unguarded(
        &self,
        args: ImportMemoryArgs,
    ) -> Result<CallToolResult, McpError> {
        let entry = self.resolve_group_entry(&args.group).await?;

        // FR-28 / D4: `force` is part of the unified write-tool
        // wire surface but `import_memory` mints / pins ids in
        // lockstep with the filename, so the flag has nothing to
        // bypass on the happy path. Bind to underscore so the
        // wire arg stays visible to clients.
        let _force = args.force;

        // Synth fields are together-or-not-at-all. Partial sets
        // surface as `synth_frontmatter_partial` so AI callers can
        // repair the arg shape instead of getting a cryptic import
        // failure deeper in the pipeline.
        let synth = match (args.name, args.description, args.kind) {
            (Some(name), Some(description), Some(kind)) => Some(mmcp_store::SynthFrontmatter {
                name,
                description,
                kind: kind.into_core(),
            }),
            (None, None, None) => None,
            _ => {
                return Err(McpError::invalid_params(
                    Cow::Borrowed(
                        "name, description, and kind must all be provided together or all omitted",
                    ),
                    Some(json!({ "code": "synth_frontmatter_partial" })),
                ));
            }
        };

        let body = if args.format.is_some_and(ToolImportSourceFormat::is_adoc) {
            mmcp_store::convert_adoc_to_markdown(&args.source)
                .map_err(map_adoc_convert_error_to_mcp)?
        } else {
            args.source
        };

        let result = mmcp_store::import_memory(
            &self.state.backend,
            &entry.handle,
            &args.slug,
            &body,
            synth,
            &self.state.author,
            args.override_,
        )
        .await
        .map_err(map_memory_error_to_mcp)?;

        Ok(ok_json(json!({
            "slug":      result.slug,
            "id":        result.id.to_string(),
            "commit_id": result.commit_id,
            "group":     args.group,
        })))
    }

    #[tool(
        description = "Export one or more groups to a portable mmcp archive (tar; optionally gzip) at the `output` path on the server's filesystem. The batch counterpart to `import_archive`. Select groups by `group` (UUID or slug, repeatable) or `all: true`. Memories are copied verbatim at HEAD so UUIDs, slugs, kinds, tags, and feature/issue numbers round-trip. Errors: `invalid_selector`, `no_groups_selected`, `archive_write_failed`.",
        annotations(
            title = "Export groups to an archive",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true,
        )
    )]
    async fn export_archive(
        &self,
        Parameters(args): Parameters<ExportArchiveArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.all && !args.group.is_empty() {
            return Err(McpError::invalid_params(
                Cow::Borrowed("`all` cannot be combined with `group`"),
                Some(json!({ "code": "invalid_selector" })),
            ));
        }
        let selected = if args.all {
            self.state.groups.list().await
        } else if !args.group.is_empty() {
            let mut out = Vec::with_capacity(args.group.len());
            for group in &args.group {
                out.push(self.resolve_group_any(group).await?);
            }
            out
        } else {
            return Err(McpError::invalid_params(
                Cow::Borrowed("specify `group` (repeatable) or `all: true`"),
                Some(json!({ "code": "no_groups_selected" })),
            ));
        };
        if selected.is_empty() {
            return Err(McpError::invalid_params(
                Cow::Borrowed("no groups to export"),
                Some(json!({ "code": "no_groups_selected" })),
            ));
        }
        let file = std::fs::File::create(&args.output).map_err(|e| {
            McpError::internal_error(
                Cow::Owned(format!("creating archive {}: {e}", args.output)),
                Some(json!({ "code": "archive_write_failed" })),
            )
        })?;
        let manifest = mmcp_store::export_archive(
            &self.state.backend,
            &selected,
            &mmcp_store::ExportOptions {
                gzip: args.gzip,
                ..Default::default()
            },
            file,
        )
        .await
        .map_err(map_archive_error_to_mcp)?;
        Ok(ok_json(json!({
            "output": args.output,
            "format_version": manifest.format_version,
            "total_memories": manifest.total_memory_count(),
            "groups": manifest
                .groups
                .iter()
                .map(|g| json!({
                    "group_id": g.group_id.to_string(),
                    "slug": g.slug,
                    "memory_count": g.memory_count,
                }))
                .collect::<Vec<_>>(),
        })))
    }

    #[tool(
        description = "Import a portable mmcp archive (tar; gzip auto-detected) from the `input` path on the server's filesystem, recreating its groups by uuid and replaying each memory through the same primitive `import_memory` uses. `into` remaps every memory into one existing group; `overwrite` replaces colliding memories instead of reporting them; `new_ids` mints fresh UUIDs (fork / copy). Protected target groups prompt for confirmation before writing. Errors: `archive_read_failed`, `unsupported_archive_format`, `group_not_found` (bad `into`), `malformed_archive`, `protected_write_cancelled`.",
        annotations(
            title = "Import an archive into the store",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true,
        )
    )]
    async fn import_archive(
        &self,
        Parameters(args): Parameters<ImportArchiveArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let bytes = std::fs::read(&args.input).map_err(|e| {
            McpError::invalid_params(
                Cow::Owned(format!("reading archive {}: {e}", args.input)),
                Some(json!({ "code": "archive_read_failed" })),
            )
        })?;
        let manifest = mmcp_store::inspect_archive(&bytes).map_err(map_archive_error_to_mcp)?;

        let into_group = match &args.into {
            Some(query) => {
                let entry = self.resolve_group_any(query).await?;
                Some(GroupId::from_uuid(entry.handle.group_id))
            }
            None => None,
        };

        // Confirm every existing protected target group before any
        // write; recreated groups are authorised by the import intent.
        self.confirm_archive_protected(&peer, &manifest, into_group)
            .await?;

        let options = mmcp_store::ImportArchiveOptions {
            into_group,
            overwrite: args.overwrite,
            new_ids: args.new_ids,
            allow_protected: true,
            ..Default::default()
        };
        let report = mmcp_store::import_archive(
            &self.state.backend,
            &self.state.groups,
            &self.state.author,
            &bytes,
            &options,
        )
        .await
        .map_err(map_archive_error_to_mcp)?;

        Ok(ok_json(json!({
            "input": args.input,
            "groups": report
                .groups
                .iter()
                .map(|g| json!({
                    "source_group_id": g.source_group_id.to_string(),
                    "target_group_id": g.target_group_id.to_string(),
                    "slug": g.slug,
                    "created_group": g.created_group,
                    "created": g.created,
                    "overwritten": g.overwritten,
                    "skipped": g.skipped,
                    "conflicts": g
                        .conflicts
                        .iter()
                        .map(|c| json!({ "slug": c.slug, "id": c.id.to_string() }))
                        .collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
        })))
    }

    #[tool(
        description = "Apply partial frontmatter / body deltas to an existing memory and record the result as a new commit. Every mutator field is optional: omit it to leave that slice of the memory untouched. `tags_add` / `tags_remove` compose additively so repeated calls dedupe correctly. Errors with code `memory_not_found` when the slug has no file in the target group; use `write_memory` to create fresh memories.",
        annotations(
            title = "Edit memory (partial update)",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    async fn edit_memory(
        &self,
        Parameters(args): Parameters<EditMemoryArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let entry = self.resolve_group_entry(&args.group).await?;
        let slug_for_guard = memory_label_for_guard(args.slug.as_deref(), args.id.as_deref());
        confirm_protected_write(&peer, &entry, &slug_for_guard, "edit").await?;
        self.edit_memory_unguarded(args).await
    }

    /// Peer-less test entry point for `edit_memory`. Mirrors
    /// `write_memory_unguarded`.
    async fn edit_memory_unguarded(
        &self,
        args: EditMemoryArgs,
    ) -> Result<CallToolResult, McpError> {
        let (entry, resolved) = self
            .resolve_memory_address(&args.group, args.slug.as_deref(), args.id.as_deref())
            .await?;

        let bytes = self
            .state
            .backend
            .read_file(&entry.handle, &resolved.path, &Rev::head())
            .await
            .map_err(git_error)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut file = mmcp_core::memory::MemoryFile::parse(&text).map_err(|e| {
            McpError::internal_error(Cow::Owned(format!("parsing existing memory: {e}")), None)
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

        // Compose refs: remove-side first (by target UUID,
        // ignoring commit), add-side second (dedupe by target so
        // the add-side commit pin wins on collision with an
        // existing entry).
        if !args.refs_remove.is_empty() {
            let removed: Vec<Uuid> = args
                .refs_remove
                .iter()
                .map(|raw| {
                    Uuid::parse_str(raw).map_err(|_| {
                        McpError::invalid_params(
                            format!("refs_remove entry '{raw}' is not a valid UUID"),
                            Some(json!({
                                "code": "invalid_memory_ref",
                                "field": "refs_remove",
                                "detail": format!("'{raw}' is not a valid UUID"),
                            })),
                        )
                    })
                })
                .collect::<Result<_, _>>()?;
            file.frontmatter
                .refs
                .retain(|r| !removed.contains(&r.target));
        }
        if !args.refs_add.is_empty() {
            let to_add = parse_wire_refs(args.refs_add, "refs_add")?;
            for new in to_add {
                file.frontmatter.refs.retain(|r| r.target != new.target);
                file.frontmatter.refs.push(new);
            }
        }

        let rendered = file
            .to_string()
            .map_err(|e| McpError::internal_error(Cow::Owned(e.to_string()), None))?;

        // FR-39 v2: memory-modify chain — Shared on every ancestor
        // and Exclusive on the per-memory leaf. Concurrent edits to
        // *different* memories under the same group don't contend;
        // a coarsening rename (Exclusive Group) waits for the
        // Shared Group ancestor to drop.
        let _lock_guards = mmcp_store::lock::acquire_chain(&mmcp_store::lock::memory_chain(
            *entry.manifest.group_id.as_uuid(),
            resolved.id,
            mmcp_store::lock::LockMode::Exclusive,
        ))
        .await;

        let commit_message = args
            .message
            .unwrap_or_else(|| format!("update memory {}", resolved.slug));
        let (commit_id, validation) = mmcp_store::write_file_at_path(
            &self.state.backend,
            &entry.handle,
            &resolved.path,
            &rendered,
            &self.state.author,
            resolved.addressing_mode,
            args.force,
            Some(&commit_message),
        )
        .await
        .map_err(map_memory_error_to_mcp)?;

        let notes = id_validation_to_notes(&validation, &resolved.slug);
        Ok(ok_json_with_notes(
            json!({
                "group": args.group,
                "slug": resolved.slug,
                "id": resolved.id.to_string(),
                "commit_id": commit_id,
            }),
            notes,
        ))
    }

    #[tool(
        description = "FR-41: atomically move a memory to a new slug path inside the same group. The memory id stays stable across the move, so cross-references in other memories keep resolving. The new slug may be a single segment (`feedback`) or a `/`-joined multi-segment path (`feedback/git/commit-phase`). Same-slug moves short-circuit as no-ops. Refuses to overwrite an existing memory at the destination with the same id; pick a different target or delete the existing entry first.",
        annotations(
            title = "Move memory to a new slug path",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn move_memory(
        &self,
        Parameters(args): Parameters<MoveMemoryArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let entry = self.resolve_group_entry(&args.group).await?;
        let slug_for_guard = memory_label_for_guard(args.slug.as_deref(), args.id.as_deref());
        confirm_protected_write(&peer, &entry, &slug_for_guard, "move").await?;
        self.move_memory_unguarded(args).await
    }

    /// Peer-less test entry point for `move_memory`. Mirrors
    /// `edit_memory_unguarded`.
    async fn move_memory_unguarded(
        &self,
        args: MoveMemoryArgs,
    ) -> Result<CallToolResult, McpError> {
        let id_opt = match args.id.as_deref() {
            Some(raw) => Some(Uuid::parse_str(raw).map_err(|e| {
                McpError::invalid_params(
                    Cow::Owned(format!("`id` is not a valid UUID: {e}")),
                    Some(json!({ "code": "invalid_uuid", "id": raw })),
                )
            })?),
            None => None,
        };
        if args.slug.is_none() && id_opt.is_none() {
            return Err(McpError::invalid_params(
                "move_memory requires at least one of `slug` or `id`",
                Some(json!({ "code": "missing_address" })),
            ));
        }
        let entry = self.resolve_group_entry(&args.group).await?;
        // FR-39 v2: a slug-rewrite move spans the source and target
        // slug directories, so we need the same coarsening lock the
        // feature rename takes — Exclusive Group blocks every
        // narrower in-flight memory edit and every new one.
        let _lock_guards = mmcp_store::lock::acquire_chain(
            &mmcp_store::lock::coarsen_group_chain(*entry.manifest.group_id.as_uuid()),
        )
        .await;
        let outcome = mmcp_store::move_memory_path(
            &self.state.backend,
            &entry.handle,
            args.slug.as_deref(),
            id_opt,
            &args.new_slug,
            &self.state.author,
            args.message.as_deref(),
        )
        .await
        .map_err(map_memory_error_to_mcp)?;
        Ok(ok_json(json!({
            "group":     args.group,
            "id":        outcome.id.to_string(),
            "old_slug":  outcome.old_slug,
            "new_slug":  outcome.new_slug,
            "old_path":  outcome.old_path,
            "new_path":  outcome.new_path,
            "commit_id": outcome.commit_id,
        })))
    }

    #[tool(
        description = "Remove a memory from a group by committing a deletion on `main`. Errors with code `memory_not_found` when the slug has no file; no silent no-op. The commit is addressable through `list_versions` just like any other write, so the removal is auditable.",
        annotations(
            title = "Delete memory",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn delete_memory(
        &self,
        Parameters(args): Parameters<DeleteMemoryArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let entry = self.resolve_group_entry(&args.group).await?;
        let slug_for_guard = memory_label_for_guard(args.slug.as_deref(), args.id.as_deref());
        confirm_protected_write(&peer, &entry, &slug_for_guard, "delete").await?;
        self.delete_memory_unguarded(args).await
    }

    /// Peer-less test entry point for `delete_memory`.
    async fn delete_memory_unguarded(
        &self,
        args: DeleteMemoryArgs,
    ) -> Result<CallToolResult, McpError> {
        // FR-28 / D4: `force` is part of the unified write-tool
        // wire surface but `delete_memory` does not render new
        // bytes to validate, so the flag has nothing to bypass.
        // Bind to underscore so the wire arg stays visible.
        let _force = args.force;
        let (entry, resolved) = self
            .resolve_memory_address(&args.group, args.slug.as_deref(), args.id.as_deref())
            .await?;

        // Memory-modify chain — Shared on every ancestor, Exclusive
        // on the per-memory leaf. Concurrent deletes of *different*
        // memories proceed in parallel; a coarsening rename
        // (Exclusive Group) waits for the Shared Group ancestor.
        let _lock_guards = mmcp_store::lock::acquire_chain(&mmcp_store::lock::memory_chain(
            *entry.manifest.group_id.as_uuid(),
            resolved.id,
            mmcp_store::lock::LockMode::Exclusive,
        ))
        .await;

        let commit_message = args
            .message
            .unwrap_or_else(|| format!("delete memory {}", resolved.slug));
        let commit_id = mmcp_store::delete_file_at_path(
            &self.state.backend,
            &entry.handle,
            &resolved.path,
            &self.state.author,
            Some(&commit_message),
        )
        .await
        .map_err(map_memory_error_to_mcp)?;

        Ok(ok_json(json!({
            "group": args.group,
            "slug": resolved.slug,
            "id": resolved.id.to_string(),
            "commit_id": commit_id,
        })))
    }

    #[tool(
        description = "Return the section tree of a memory's markdown body (FR-026). Every heading gets a stable dot-separated path id (slugified heading trail with `-2`, `-3` disambiguators for duplicate siblings) plus its level, raw heading text, and half-open line range. Callers discover addressable nodes here before issuing `edit_memory_body` ops. A synthetic `preamble` section covers content before the first heading so even headingless bodies return one entry.",
        annotations(
            title = "Read memory body sections",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn read_memory_body_sections(
        &self,
        Parameters(args): Parameters<ReadMemoryBodySectionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.read_memory_body_sections_inner(args).await
    }

    /// Peer-less core of `read_memory_body_sections`. Tests call
    /// this directly to avoid constructing a mock `Peer`.
    async fn read_memory_body_sections_inner(
        &self,
        args: ReadMemoryBodySectionsArgs,
    ) -> Result<CallToolResult, McpError> {
        let (entry, resolved) = self
            .resolve_memory_address(&args.group, args.slug.as_deref(), args.id.as_deref())
            .await?;
        let bytes = self
            .state
            .backend
            .read_file(&entry.handle, &resolved.path, &Rev::head())
            .await
            .map_err(git_error)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let file = mmcp_core::memory::MemoryFile::parse(&text).map_err(|e| {
            McpError::internal_error(Cow::Owned(format!("parsing existing memory: {e}")), None)
        })?;
        let sections = mmcp_core::memory::parse_sections(&file.body).map_err(|e| {
            McpError::internal_error(Cow::Owned(format!("parsing body: {e}")), None)
        })?;
        let as_json: Vec<_> = sections
            .iter()
            .map(|s| {
                json!({
                    "path": s.path,
                    "level": s.level,
                    "heading": s.heading,
                    "line_start": s.line_start,
                    "line_end": s.line_end,
                })
            })
            .collect();
        Ok(ok_json(json!({
            "group": args.group,
            "slug": resolved.slug,
            "id": resolved.id.to_string(),
            "sections": as_json,
            "count": sections.len(),
        })))
    }

    #[tool(
        description = "Apply an ordered list of section-level or line-level edits to a memory's markdown body and commit the result (FR-026). Section ops address a whole section (heading + nested children) by the dot-path id returned from `read_memory_body_sections`. Line ops are escape hatches for non-heading content. Ops run transactionally: the first error aborts the batch. Structured error codes: `section_not_found`, `move_would_loop`, `level_out_of_range`, `invalid_line_range`, `line_past_eof`, `body_parse_failed`. The protected-group guard from FR-019 / FR-011 still gates this path.",
        annotations(
            title = "Edit memory body (semantic ops)",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    async fn edit_memory_body(
        &self,
        Parameters(args): Parameters<EditMemoryBodyArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let entry = self.resolve_group_entry(&args.group).await?;
        let slug_for_guard = memory_label_for_guard(args.slug.as_deref(), args.id.as_deref());
        confirm_protected_write(&peer, &entry, &slug_for_guard, "edit_body").await?;
        self.edit_memory_body_unguarded(args).await
    }

    /// Peer-less core of `edit_memory_body`. Tests call this
    /// directly so the pre-elicitation write path is still
    /// exercised without a mock `Peer`.
    async fn edit_memory_body_unguarded(
        &self,
        args: EditMemoryBodyArgs,
    ) -> Result<CallToolResult, McpError> {
        let (entry, resolved) = self
            .resolve_memory_address(&args.group, args.slug.as_deref(), args.id.as_deref())
            .await?;

        let bytes = self
            .state
            .backend
            .read_file(&entry.handle, &resolved.path, &Rev::head())
            .await
            .map_err(git_error)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut file = mmcp_core::memory::MemoryFile::parse(&text).map_err(|e| {
            McpError::internal_error(Cow::Owned(format!("parsing existing memory: {e}")), None)
        })?;

        let store_ops: Vec<mmcp_store::MemoryEditOp> =
            args.ops.into_iter().map(Into::into).collect();
        let new_body =
            mmcp_store::apply_ops(&file.body, &store_ops).map_err(map_memory_edit_error_to_mcp)?;
        file.body = new_body;
        let rendered = file
            .to_string()
            .map_err(|e| McpError::internal_error(Cow::Owned(e.to_string()), None))?;

        // FR-39 v2: same memory-modify chain as `edit_memory`.
        let _lock_guards = mmcp_store::lock::acquire_chain(&mmcp_store::lock::memory_chain(
            *entry.manifest.group_id.as_uuid(),
            resolved.id,
            mmcp_store::lock::LockMode::Exclusive,
        ))
        .await;

        let commit_message = args
            .message
            .unwrap_or_else(|| format!("update memory {}", resolved.slug));
        let (commit_id, validation) = mmcp_store::write_file_at_path(
            &self.state.backend,
            &entry.handle,
            &resolved.path,
            &rendered,
            &self.state.author,
            resolved.addressing_mode,
            args.force,
            Some(&commit_message),
        )
        .await
        .map_err(map_memory_error_to_mcp)?;

        // Re-parse the freshly-written body so callers see the
        // post-edit section tree without a second round trip.
        let sections = mmcp_core::memory::parse_sections(&file.body).map_err(|e| {
            McpError::internal_error(Cow::Owned(format!("parsing body: {e}")), None)
        })?;
        let sections_json: Vec<_> = sections
            .iter()
            .map(|s| {
                json!({
                    "path": s.path,
                    "level": s.level,
                    "heading": s.heading,
                })
            })
            .collect();

        let notes = id_validation_to_notes(&validation, &resolved.slug);
        Ok(ok_json_with_notes(
            json!({
                "group": args.group,
                "slug": resolved.slug,
                "id": resolved.id.to_string(),
                "commit_id": commit_id,
                "sections": sections_json,
            }),
            notes,
        ))
    }

    #[tool(
        description = "Validate manifests and memory frontmatter for a group. Returns one entry per group (manifest ok?, memory count) and lifts every finding onto the FR-45 notes channel: parse errors, missing required fields, empty bodies, and similar surface issues all appear as `notes` with stable codes. Checks one group if group UUID given, all groups if omitted.",
        annotations(
            title = "Check group health",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
            vec![health_check_group(&self.state.backend, &entry).await]
        } else {
            health_check_all(&self.state.backend, &self.state.groups).await
        };
        // FR-45: every finding becomes a note; wire response keeps
        // only the per-group structural summary.
        let mut notes: Vec<mmcp_proto::Note> = Vec::new();
        let mut groups_body: Vec<serde_json::Value> = Vec::with_capacity(reports.len());
        for report in &reports {
            notes.extend(findings_to_notes(&report.findings));
            groups_body.push(json!({
                "group_id": report.group_id,
                "slug": report.slug,
                "manifest_ok": report.manifest_ok,
                "memory_count": report.memory_count,
            }));
        }
        let healthy = notes.is_empty();
        Ok(ok_json_with_notes(
            json!({
                "groups": groups_body,
                "healthy": healthy,
            }),
            notes,
        ))
    }

    #[tool(
        description = "Deep diagnostic analysis of a group's memories. Everything check_health does plus: missing tags, empty bodies, naming drift, empty groups, UUID mismatches, created_at sanity, cross-group duplicate slugs, cross-ref integrity, supersede-chain reciprocity, and structural hints. Per-group structural summary lives in `groups`; every finding — plus project-level (user / sync / config) signals — rides the FR-45 notes channel with stable codes.",
        annotations(
            title = "Deep diagnose group",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
            DiagReport {
                project_findings: Vec::new(),
                groups: vec![diagnose_group(&self.state.backend, &entry).await],
            }
        } else {
            diagnose_all(&self.state.backend, &self.state.groups).await
        };
        // FR-45: collapse project_findings and every group's findings
        // onto the notes channel. Wire body keeps only the
        // per-group structural summary.
        let mut notes: Vec<mmcp_proto::Note> = findings_to_notes(&diag.project_findings);
        let mut groups_body: Vec<serde_json::Value> = Vec::with_capacity(diag.groups.len());
        for report in &diag.groups {
            notes.extend(findings_to_notes(&report.findings));
            groups_body.push(json!({
                "group_id": report.group_id,
                "slug": report.slug,
                "manifest_ok": report.manifest_ok,
                "memory_count": report.memory_count,
            }));
        }
        // FR-34: surface any registered tool whose `annotations` slot
        // is `None`. The FR-29 conformance test catches this in CI
        // but operators running a stale build still want a runtime
        // hint — `mmcp diagnose` (CLI) / `diagnose` (MCP) is the
        // single place every install can probe.
        notes.extend(collect_missing_annotation_notes(&registered_tool_attrs()));
        let healthy = !notes
            .iter()
            .any(|n| n.level == mmcp_proto::NoteLevel::Error);
        Ok(ok_json_with_notes(
            json!({
                "groups": groups_body,
                "healthy": healthy,
            }),
            notes,
        ))
    }

    // ── Debug tools ─────────────────────────────────────────

    #[tool(
        description = "Enable or disable debug tools. Debug tools provide raw git access for troubleshooting. Pass enabled=true to activate, enabled=false to deactivate. Returns the new state.",
        annotations(
            title = "Debug: toggle raw-access gate",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        description = "Read any file at any path in a group's git repo. Requires debug mode. Use for inspecting raw repo state.",
        annotations(
            title = "Debug: raw file read",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        description = "List all files (blobs) under a path prefix in a group's git repo. Requires debug mode.",
        annotations(
            title = "Debug: raw tree walk",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        description = "Show raw git commit history for the entire repo or a specific path. Requires debug mode.",
        annotations(
            title = "Debug: raw git log",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
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
        let path = args
            .path
            .as_deref()
            .unwrap_or(mmcp_core::manifest::MANIFEST_FILENAME);
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
        description = "Write any file at any path in a group's git repo. Requires debug mode. Use for low-level repairs. Protected groups are gated the same as `edit_memory` / `delete_memory`: the write errors with `protected_requires_elicitation` so a raw debug path can't silently poke at shared rules.",
        annotations(
            title = "Debug: raw file write",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    async fn debug_write_file(
        &self,
        Parameters(args): Parameters<DebugWriteFileArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.require_debug()?;
        let group_id = parse_group_id(&args.group)?;
        let entry = self
            .state
            .groups
            .get(&group_id)
            .await
            .ok_or_else(|| McpError::invalid_params("group not found", None))?;
        // The protection guard lives here too so a debug-mode
        // operator can't accidentally sidestep the typed CRUD
        // surface to mutate a global rule. `args.path` stands in
        // for a memory slug — we surface it on the error payload
        // so callers still see which path tripped the guard.
        confirm_protected_write(&peer, &entry, &args.path, "debug_write").await?;
        self.debug_write_file_unguarded(args).await
    }

    /// Peer-less test entry point for `debug_write_file`. Keeps
    /// the `require_debug` gate so debug-mode semantics are still
    /// exercised without constructing a mock `Peer`.
    async fn debug_write_file_unguarded(
        &self,
        args: DebugWriteFileArgs,
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
        description = "CALL FIRST IN EVERY SESSION, before answering the user or invoking any other tool. Initializes the AI's context. Returns the session protocol as `instructions` plus a metadata manifest of the mandatory and project-scoped memories the caller should plan to read. Memory BODIES are not inlined; fetch each with `read_memory(group, slug|id)` as needed. Call at session start, after context compaction, before starting a new phase or task, and before/after each commit cycle. This tool never writes files - CLAUDE.md advice appears in `diagnostics` and must be acted on by calling `init_claude` explicitly.",
        annotations(
            title = "Bootstrap session context",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn bootstrap_context(
        &self,
        Parameters(args): Parameters<BootstrapContextArgs>,
    ) -> Result<CallToolResult, McpError> {
        // FR-44: explicit selector wins; `path` walks an explicit
        // root for `.mmcp.toml`; otherwise walk cwd. The selector
        // branch leaves `project_cfg` as `None` because a UUID
        // carries no filesystem guarantees — Shared-group adoption
        // and subscriptions resolution therefore only fire on the
        // path / cwd branches.
        let (project_uuid, project_cfg, project_root) = match args.project.as_deref() {
            Some(query) => {
                let entry = mmcp_store::memory::resolve_group(&self.state.groups, query)
                    .await
                    .map_err(|_| {
                        McpError::invalid_params(
                            format!("project selector '{query}' does not resolve to a mirrored group"),
                            Some(json!({
                                "code": "unknown_project",
                                "query": query,
                            })),
                        )
                    })?;
                (Some(*entry.manifest.group_id.as_uuid()), None, None)
            }
            None => {
                let starting_dir: Option<std::path::PathBuf> = match args.path.as_deref() {
                    Some(p) => Some(std::path::PathBuf::from(p)),
                    None => std::env::current_dir().ok(),
                };
                let project_root =
                    starting_dir.and_then(|dir| find_project_root(&dir));
                let project_cfg = project_root
                    .as_ref()
                    .and_then(|root| load_project_config(root).ok());
                let project_uuid = project_cfg.as_ref().map(|cfg| *cfg.project_uuid.as_uuid());
                (project_uuid, project_cfg, project_root)
            }
        };

        // FR-025: which Shared-scoped groups does this project pull
        // into scope? The same adoption test that gates mandatory
        // memory visibility is also the predicate for "fully
        // subscribed" groups under the new subscriptions engine.
        let entries = self.state.groups.list().await;
        let adopted_shared: std::collections::HashSet<Uuid> = match project_cfg.as_ref() {
            None => std::collections::HashSet::new(),
            Some(cfg) => entries
                .iter()
                .filter(|entry| entry.manifest.scope == mmcp_core::manifest::GroupScope::Shared)
                .filter(|entry| is_group_adopted(&entry.manifest.slug, cfg))
                .map(|entry| *entry.manifest.group_id.as_uuid())
                .collect(),
        };

        // Build `groups_in_scope`: every group the AI is allowed to
        // enumerate via `list_memories`. Project group (if any),
        // Global, plus every adopted Shared group.
        let mut groups_in_scope: Vec<serde_json::Value> = Vec::new();
        for entry in &entries {
            let entry_uuid = *entry.manifest.group_id.as_uuid();
            let in_scope = match entry.manifest.scope {
                mmcp_core::manifest::GroupScope::Global => true,
                mmcp_core::manifest::GroupScope::Shared => adopted_shared.contains(&entry_uuid),
                mmcp_core::manifest::GroupScope::Project => project_uuid == Some(entry_uuid),
            };
            if !in_scope {
                continue;
            }
            groups_in_scope.push(json!({
                "uuid": entry_uuid.to_string(),
                "slug": entry.manifest.slug,
                "scope": match entry.manifest.scope {
                    mmcp_core::manifest::GroupScope::Global => "global",
                    mmcp_core::manifest::GroupScope::Shared => "shared",
                    mmcp_core::manifest::GroupScope::Project => "project",
                },
            }));
        }

        // Resolve subscribed reads from the four axes. Returns
        // (group_uuid, slug) addresses only — no metadata.
        let subscribed_reads = match project_cfg.as_ref() {
            None => Vec::new(),
            Some(cfg) => {
                resolve_subscribed_reads(
                    &self.state.backend,
                    &entries,
                    cfg,
                    &adopted_shared,
                    project_uuid,
                )
                .await
            }
        };

        let subscriptions_summary = match project_cfg.as_ref() {
            Some(cfg) => json!({
                "tags": cfg.subscriptions.tags,
                "memories": cfg.subscriptions.memories,
                "groups": cfg.subscriptions.groups,
                "languages": cfg.subscriptions.languages,
            }),
            None => json!({
                "tags": [],
                "memories": [],
                "groups": [],
                "languages": [],
            }),
        };

        // FR-45: advisory CLAUDE.md signals flow through the
        // standard notes channel; no bespoke `diagnostics` field.
        // `init_claude` is still the only remediation — the note
        // context points callers at it.
        let notes = claude_md_notes(project_root.as_deref());

        Ok(ok_json_with_notes(
            json!({
                "instructions": SESSION_INSTRUCTIONS,
                "next_action": {
                    "imperative_mandatory": "For each entry in `groups_in_scope`, call list_memories(group=<uuid>) and read every memory whose `mandatory == true`. The bodies are NOT in this response — read_memory(group, slug) fetches each one.",
                    "imperative_optional": "Inspect the same `list_memories` results for non-mandatory entries that match this task. Use subscribe(kind='memory'|'tag'|'group'|'language', value=...) to pin the ones relevant to this project; subscribed entries appear in `subscribed_reads` next bootstrap.",
                    "groups_in_scope": groups_in_scope,
                    "subscribed_reads": subscribed_reads,
                    "subscriptions_summary": subscriptions_summary,
                },
                "project_root": project_root.as_ref().map(|p| p.to_string_lossy().into_owned()),
                "project_uuid": project_uuid.map(|u| u.to_string()),
            }),
            notes,
        ))
    }

    #[tool(
        description = "Manage CLAUDE.md for the current project. Actions: `override` writes a fresh mmcp stub, `append` inserts or replaces the mmcp-managed fence block, `convert` splits existing CLAUDE.md into typed project memories and replaces the file with a stub. When the file is dirty or untracked and `on_conflict` is not set, the call errors with a structured `conflict_unresolved` payload naming the observed state so the caller can retry with a choice. Default backup policy writes `.bak` only when the file is dirty or untracked.",
        annotations(
            title = "Manage CLAUDE.md fence",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn init_claude(
        &self,
        Parameters(args): Parameters<InitClaudeArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // FR-011: when the caller did not pre-supply `on_conflict`
        // and the file is in a conflict state, prompt via MCP
        // elicitation. Resolved choice is stamped back onto `args`
        // before the unguarded body runs, so the rest of the logic
        // is unchanged regardless of whether the choice came from
        // `on_conflict` or from the elicitation response.
        let path = args
            .path
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("CLAUDE.md"));
        let state = crate::commands::claude::inspect(&path);
        let args = if state.is_conflict() && args.on_conflict.is_none() {
            let choice = elicit_claude_conflict_choice(&peer, state).await?;
            let wire = match choice {
                crate::commands::claude::ConflictChoice::Override => InitClaudeConflict::Override,
                crate::commands::claude::ConflictChoice::BackupOverride => {
                    InitClaudeConflict::BackupOverride
                }
                crate::commands::claude::ConflictChoice::Cancel => InitClaudeConflict::Cancel,
                crate::commands::claude::ConflictChoice::NotApplicable => {
                    // Unreachable: we only enter this branch when
                    // `state.is_conflict()` is true.
                    unreachable!("conflict state guarded by is_conflict()")
                }
            };
            InitClaudeArgs {
                on_conflict: Some(wire),
                ..args
            }
        } else {
            args
        };
        self.init_claude_unguarded(args).await
    }

    /// Peer-less test entry point for `init_claude`. Preserves the
    /// legacy `conflict_unresolved` structured error when
    /// `on_conflict` is absent on a dirty / untracked file, so
    /// existing tests keep covering that wire shape.
    async fn init_claude_unguarded(
        &self,
        args: InitClaudeArgs,
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
                // Legacy structured-error path. `init_claude`
                // (the public tool) normally converts this into an
                // elicitation round-trip first; this branch only
                // fires for callers that bypass the tool wrapper
                // (tests, future CLI consumers, pre-elicitation
                // retry flows).
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
        let home = MmcpHome::discover()
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
        description = "Add an entry to the project's `[subscriptions]` table in `.mmcp/config.toml`. `kind` selects the axis (tag / memory / group / language) and `value` is the target. Memory targets must be `<group_uuid>:<slug>` and resolve to an existing memory; group targets must resolve to a mirrored group. Tags and languages skip validation. Idempotent: subscribing twice is a no-op. Returns `changed=false` when the entry was already present.",
        annotations(
            title = "Subscribe to a tag / memory / group / language",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn subscribe(
        &self,
        Parameters(args): Parameters<crate::commands::subscribe::SubscribeMcpArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.apply_subscription_mcp(args, crate::commands::subscribe::SubscriptionAction::Subscribe)
            .await
    }

    #[tool(
        description = "Remove an entry from the project's `[subscriptions]` table in `.mmcp/config.toml`. `kind` and `value` mirror `subscribe`. Idempotent: unsubscribing from a value the project never subscribed to is a no-op. Returns `changed=false` in that case.",
        annotations(
            title = "Unsubscribe from a tag / memory / group / language",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn unsubscribe(
        &self,
        Parameters(args): Parameters<crate::commands::subscribe::SubscribeMcpArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.apply_subscription_mcp(
            args,
            crate::commands::subscribe::SubscriptionAction::Unsubscribe,
        )
        .await
    }

    #[tool(
        description = "Read each in-scope group's remote HEAD into a local remote-tracking ref without advancing the group's `main` branch. Git-symmetric with `fetch`: use this to inspect what `sync_pull` would fast-forward before committing to it. Errors with code `sync_not_configured` when `.mmcp.toml` has no `[sync]` block, and the usual `selector_required` / `selector_conflict` / `unknown_group` for arg validation.",
        annotations(
            title = "Fetch remote-tracking refs",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true,
        )
    )]
    async fn sync_fetch(
        &self,
        Parameters(args): Parameters<SyncToolArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (cfg, server_url) = self.require_sync_configured()?;
        let filter = resolve_sync_filter(&args, &self.state.groups).await?;
        let (engine, resolver) = mmcp_store::sync::build_engine(
            self.state.backend.clone(),
            self.state.groups.clone(),
            &server_url,
        )
        .map_err(|e| McpError::internal_error(format!("failed to build sync engine: {e}"), None))?;
        let report = engine
            .fetch(filter, &resolver, &resolver)
            .await
            .map_err(map_sync_error_to_mcp)?;
        Ok(ok_json(json!({
            "groups": report.groups.iter().map(|g| json!({
                "group_id": g.group_id.to_string(),
                "slug": g.slug,
                "remote_head": g.remote_head,
                "ref_updated": g.ref_updated,
            })).collect::<Vec<_>>(),
            "new_groups": report.new_groups,
            "project_uuid": cfg.project_uuid.to_string(),
            "server_url": server_url,
        })))
    }

    #[tool(
        description = "Pull updates from the configured mmcp sync server into the local mirror. Returns the groups whose local HEAD advanced plus any groups the server has that are not mirrored yet. Errors with code `sync_not_configured` when `.mmcp.toml` has no `[sync]` block, and code `sync_conflict` / `sync_remote` / `sync_transport` for engine-level failures.",
        annotations(
            title = "Pull from sync server",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true,
        )
    )]
    async fn sync_pull(
        &self,
        Parameters(args): Parameters<SyncToolArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (cfg, server_url) = self.require_sync_configured()?;
        let filter = resolve_sync_filter(&args, &self.state.groups).await?;
        let (engine, resolver) = mmcp_store::sync::build_engine(
            self.state.backend.clone(),
            self.state.groups.clone(),
            &server_url,
        )
        .map_err(|e| McpError::internal_error(format!("failed to build sync engine: {e}"), None))?;
        let report = engine
            .pull(filter, &resolver, &resolver)
            .await
            .map_err(map_sync_error_to_mcp)?;
        Ok(ok_json(json!({
            "updated": report.updated,
            "new_groups": report.new_groups,
            "project_uuid": cfg.project_uuid.to_string(),
            "server_url": server_url,
        })))
    }

    #[tool(
        description = "Push the local pending-edit queue to the configured mmcp sync server. Returns each drained edit with the server-assigned version and tag, plus whether the content plane (git push) actually shipped bytes. Errors with code `sync_not_configured` when `.mmcp.toml` has no `[sync]` block.",
        annotations(
            title = "Push to sync server",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true,
        )
    )]
    async fn sync_push(
        &self,
        Parameters(args): Parameters<SyncToolArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (cfg, server_url) = self.require_sync_configured()?;
        let filter = resolve_sync_filter(&args, &self.state.groups).await?;
        let (engine, resolver) = mmcp_store::sync::build_engine(
            self.state.backend.clone(),
            self.state.groups.clone(),
            &server_url,
        )
        .map_err(|e| McpError::internal_error(format!("failed to build sync engine: {e}"), None))?;
        let report = engine
            .push(filter, &resolver, &resolver)
            .await
            .map_err(map_sync_error_to_mcp)?;
        // FR-45 `sync_partial_failure` populator: per-group
        // content_transferred=false means the control plane
        // accepted the push but the git content plane did not
        // actually ship bytes (transport error, server rejected,
        // network blip, etc.). Shared helper keeps the CLI and
        // MCP surfaces emitting identical codes and contexts.
        let notes = crate::notes::sync_push_partial_failure_notes(&report, &server_url);
        Ok(ok_json_with_notes(
            json!({
                "pushed": report.pushed.iter().map(|p| json!({
                    "group_id": p.group_id.to_string(),
                    "content_transferred": p.content_transferred,
                })).collect::<Vec<_>>(),
                "project_uuid": cfg.project_uuid.to_string(),
                "server_url": server_url,
            }),
            notes,
        ))
    }

    #[tool(
        description = "Run a full sync (pull then push) against the configured mmcp server. Returns both report shapes nested under `pulled` and `pushed`. Same error codes as `sync_pull` / `sync_push`.",
        annotations(
            title = "Full sync (pull + push)",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = true,
        )
    )]
    async fn sync(
        &self,
        Parameters(args): Parameters<SyncToolArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (cfg, server_url) = self.require_sync_configured()?;
        let filter = resolve_sync_filter(&args, &self.state.groups).await?;
        let (engine, resolver) = mmcp_store::sync::build_engine(
            self.state.backend.clone(),
            self.state.groups.clone(),
            &server_url,
        )
        .map_err(|e| McpError::internal_error(format!("failed to build sync engine: {e}"), None))?;
        let report = engine
            .sync(filter, &resolver, &resolver)
            .await
            .map_err(map_sync_error_to_mcp)?;
        Ok(ok_json(json!({
            "pulled": {
                "updated": report.pulled.updated,
                "new_groups": report.pulled.new_groups,
            },
            "pushed": {
                "pushed": report.pushed.pushed.iter().map(|p| json!({
                    "group_id": p.group_id.to_string(),
                    "content_transferred": p.content_transferred,
                })).collect::<Vec<_>>(),
            },
            "project_uuid": cfg.project_uuid.to_string(),
            "server_url": server_url,
        })))
    }

    #[tool(
        description = "Return the local mmcp project state: discovered project root, configured sync server, and the mirrored groups with their memory counts. Pure-local — no network. Returns `project_configured: false` when no `.mmcp.toml` is in scope, so callers can distinguish 'not in a project' from transient errors.",
        annotations(
            title = "Local mirror status",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn status(
        &self,
        Parameters(args): Parameters<StatusArgs>,
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

        // FR-44: explicit selector returns the minimal
        // filesystem-free shape. Cwd walk keeps the full shape
        // with project_root + sync fields.
        if let Some(query) = args.project.as_deref() {
            let entry = mmcp_store::memory::resolve_group(&self.state.groups, query)
                .await
                .map_err(|_| {
                    McpError::invalid_params(
                        format!("project selector '{query}' does not resolve to a mirrored group"),
                        Some(json!({
                            "code": "unknown_project",
                            "query": query,
                        })),
                    )
                })?;
            return Ok(ok_json(json!({
                "project_configured": true,
                "project_uuid": entry.manifest.group_id.to_string(),
                "project_slug": entry.manifest.slug,
                "groups": groups,
                "mode": self.mode.as_label(),
            })));
        }

        let mut payload = compose_status(&cwd, groups)?;
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert(
                "mode".to_string(),
                serde_json::Value::from(self.mode.as_label()),
            );
        }
        Ok(ok_json(payload))
    }

    #[tool(
        description = "Return the annotated tool surface in one read-only call. Each entry carries name, description, title, plus the four MCP annotation hints (read_only, destructive, idempotent, open_world). Use this when a harness needs a deterministic catalogue of safe-tool subsets without parsing per-client `tools/list` quirks. Pure-local introspection; no group, no I/O.",
        annotations(
            title = "Describe registered MCP tools",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn describe_tools(
        &self,
        Parameters(_args): Parameters<DescribeToolsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let tools: Vec<serde_json::Value> = Self::registered_tool_attrs()
            .into_iter()
            .map(|tool| {
                let ann = tool.annotations.as_ref();
                // FR-32: surface per-arg risk hints alongside the
                // tool-level annotations so a harness that trusts
                // `destructive_hint = false` for write_memory still
                // sees that `override: true` carries its own risk.
                let arg_risk_hints = arg_risk_hints_for(tool.name.as_ref());
                json!({
                    "name":            tool.name,
                    "description":     tool.description,
                    "title":           ann.and_then(|a| a.title.clone()),
                    "read_only":       ann.and_then(|a| a.read_only_hint),
                    "destructive":     ann.and_then(|a| a.destructive_hint),
                    "idempotent":      ann.and_then(|a| a.idempotent_hint),
                    "open_world":      ann.and_then(|a| a.open_world_hint),
                    "arg_risk_hints":  arg_risk_hints,
                })
            })
            .collect();
        Ok(ok_json(json!({
            "count": tools.len(),
            "tools": tools,
        })))
    }

    #[tool(
        description = "Bootstrap the project's `.mmcp.toml` and backing group repo. Idempotent and never-overwrite: a second call returns `created_config: false` / `created_repo: false` without rewriting either artifact. Errors with code `invalid_slug` when the slug does not satisfy the memory-slug contract, `slug_required` when no slug is available (arg missing and no `project_slug` in `.mmcp.toml`), `slug_mismatch` / `project_uuid_mismatch` when args disagree with an existing config, and `repo_without_config` if the bare repo exists but the config has been deleted.",
        annotations(
            title = "Initialize mmcp project",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn init_project(
        &self,
        Parameters(args): Parameters<InitProjectArgs>,
    ) -> Result<CallToolResult, McpError> {
        // FR-39 v2: process-coarsening write — Exclusive Process
        // serialises every group-creating call across the whole
        // mirror so two concurrent `init_project` calls cannot
        // race on slug uniqueness or repo bootstrap.
        let _lock_guards =
            mmcp_store::lock::acquire_chain(&mmcp_store::lock::coarsen_process_chain()).await;

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

    #[tool(
        description = "Create a standalone group under `~/.mmcp/repos/`. Does not touch `.mmcp.toml`; use `init_project` for project-backed groups. Scope defaults to `shared`. Errors: `invalid_slug`, `slug_already_exists` (with existing `group_id`).",
        annotations(
            title = "Create group",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    async fn create_group(
        &self,
        Parameters(args): Parameters<CreateGroupArgs>,
    ) -> Result<CallToolResult, McpError> {
        // FR-39 v2: process-coarsening write. Same rationale as
        // `init_project` — slug uniqueness and repo bootstrap
        // need to be globally serialised.
        let _lock_guards =
            mmcp_store::lock::acquire_chain(&mmcp_store::lock::coarsen_process_chain()).await;

        let scope = args
            .scope
            .map(ToolGroupScope::into_core)
            .unwrap_or(mmcp_core::manifest::GroupScope::Shared);
        let opts = crate::commands::group::CreateGroupOptions {
            slug: args.slug,
            display_name: args.display_name,
            scope,
            protected: args.protected,
        };
        let report = crate::commands::group::create_standalone_group(
            &self.state.backend,
            &self.state.groups,
            &opts,
        )
        .await
        .map_err(map_create_group_error_to_mcp)?;
        Ok(ok_json(json!({
            "group_id":     report.group_id.to_string(),
            "slug":         report.slug,
            "scope":        group_scope_wire(report.scope),
            "display_name": report.display_name,
            "protected":    report.protected,
            "repo_path":    report.repo_path.to_string_lossy(),
        })))
    }

    // ── Feature-request tools (FR-007) ───────────────────────────
    //
    // All five auto-resolve the project group from the server's
    // cwd via `mmcp_store::features::resolve_project_group`. FR
    // tools intentionally refuse to fall through to a no-op when
    // the project context is missing: the caller gets a structured
    // `project_not_found` payload and can decide whether to offer
    // `init_project` or ask the user to `cd` into the repo.

    #[tool(
        description = "File a new feature request in the current project's group. Slug is auto-minted from the title when omitted. Status defaults to `open`; supply one of `open | resolved | blocked | deferred | duplicate` to override. Errors with code `project_not_found` when no `.mmcp.toml` is on any ancestor of the server's cwd, `invalid_slug` when the supplied or derived slug fails validation, and `memory_already_exists` when the slug collides with an existing memory in the project group.",
        annotations(
            title = "Add feature request",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    async fn add_feature(
        &self,
        Parameters(args): Parameters<AddFeatureArgs>,
    ) -> Result<CallToolResult, McpError> {
        let cwd = current_dir_for_mcp()?;
        let (entry, _root) = mmcp_store::features::resolve_project_group_with_selector(
            &self.state.groups,
            args.project.as_deref(),
            &cwd,
        )
            .await
            .map_err(map_feature_error_to_mcp)?;
        let status = parse_status_arg(args.status.as_deref())?.unwrap_or_default();
        let depends_on =
            mmcp_store::parse_cross_refs(&args.depends_on, "depends_on").map_err(map_xref_error_to_mcp)?;
        let blocks = mmcp_store::parse_cross_refs(&args.blocks, "blocks")
            .map_err(map_xref_error_to_mcp)?;
        let refs = parse_wire_refs(args.refs, "refs")?;
        let source = parse_optional_source(args.source.as_deref())?;
        let spec = mmcp_store::features::AddSpec {
            slug: args.slug,
            title: args.title,
            description: args.description,
            body: args.body,
            status,
            depends_on,
            blocks,
            refs,
            supersedes: args.supersedes,
            source,
            message: args.message,
            // FR-37: `number` is server-assigned only, never
            // accepted from the wire. Leaving default None lets
            // `add_feature` auto-assign `max + 1` under lock.
            ..mmcp_store::features::AddSpec::default()
        };
        let record = mmcp_store::features::add_feature(
            &self.state.backend,
            &entry,
            spec,
            &self.state.author,
        )
        .await
        .map_err(map_feature_error_to_mcp)?;
        Ok(ok_json(feature_record_to_json(&entry, &record)))
    }

    #[tool(
        description = "Read a feature request by slug from the current project's group. Returns the full FR record (title, description, body, status, depends_on, blocks, commit_id). Set `version` to a branch, tag, or 40-char commit hex to read a specific revision. Errors with `not_a_feature` when the slug resolves to a memory whose kind is not `fr`.",
        annotations(
            title = "Read a feature request",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn read_feature(
        &self,
        Parameters(args): Parameters<ReadFeatureArgs>,
    ) -> Result<CallToolResult, McpError> {
        let cwd = current_dir_for_mcp()?;
        let (entry, _root) = mmcp_store::features::resolve_project_group_with_selector(
            &self.state.groups,
            args.project.as_deref(),
            &cwd,
        )
            .await
            .map_err(map_feature_error_to_mcp)?;
        let record = mmcp_store::features::read_feature(
            &self.state.backend,
            &entry,
            &args.slug,
            args.version.as_deref(),
        )
        .await
        .map_err(map_feature_error_to_mcp)?;
        // FR-45 `dangling_ref` populator: walk this feature's
        // depends_on / blocks / superseded_by targets against the
        // group's memory index and flag any UUID that does not
        // resolve locally.
        let notes = dangling_ref_notes_for(
            &self.state.backend,
            &entry,
            &record.slug,
            &record.depends_on,
            &record.blocks,
            record.superseded_by.as_ref(),
        )
        .await;
        Ok(ok_json_with_notes(feature_record_to_json(&entry, &record), notes))
    }

    #[tool(
        description = "Apply partial updates to an existing feature request and commit the result. Every mutator is optional — omit to leave untouched. `depends_on` and `blocks` are full-list replacements; pass `[]` to clear, omit to preserve. `status` takes the wire form of the status enum. Errors with `memory_not_found` when the slug has no FR, `not_a_feature` when the slug is a non-FR memory, and `invalid_feature_status` when `status` is not one of the five variants.",
        annotations(
            title = "Update feature request",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false,
        )
    )]
    async fn update_feature(
        &self,
        Parameters(args): Parameters<UpdateFeatureArgs>,
    ) -> Result<CallToolResult, McpError> {
        let cwd = current_dir_for_mcp()?;
        let (entry, _root) = mmcp_store::features::resolve_project_group_with_selector(
            &self.state.groups,
            args.project.as_deref(),
            &cwd,
        )
            .await
            .map_err(map_feature_error_to_mcp)?;
        let status = match args.status.as_deref() {
            Some(raw) => Some(parse_status_arg(Some(raw))?.unwrap_or_default()),
            None => None,
        };
        let depends_on = args
            .depends_on
            .as_deref()
            .map(|v| mmcp_store::parse_cross_refs(v, "depends_on"))
            .transpose()
            .map_err(map_xref_error_to_mcp)?;
        let blocks = args
            .blocks
            .as_deref()
            .map(|v| mmcp_store::parse_cross_refs(v, "blocks"))
            .transpose()
            .map_err(map_xref_error_to_mcp)?;
        let refs_add = if args.refs_add.is_empty() {
            None
        } else {
            Some(parse_wire_refs(args.refs_add, "refs_add")?)
        };
        let refs_remove = if args.refs_remove.is_empty() {
            None
        } else {
            let parsed: Result<Vec<Uuid>, _> = args
                .refs_remove
                .iter()
                .map(|raw| {
                    Uuid::parse_str(raw).map_err(|_| {
                        McpError::invalid_params(
                            format!("refs_remove entry '{raw}' is not a valid UUID"),
                            Some(json!({
                                "code": "invalid_memory_ref",
                                "field": "refs_remove",
                                "detail": format!("'{raw}' is not a valid UUID"),
                            })),
                        )
                    })
                })
                .collect();
            Some(parsed?)
        };
        let superseded_by = match args.superseded_by {
            None => None,
            Some(arg) => Some(
                parse_wire_refs(vec![arg], "superseded_by")?
                    .into_iter()
                    .next()
                    .expect("parse_wire_refs returns one entry per input"),
            ),
        };
        let spec = mmcp_store::features::UpdateSpec {
            title: args.title,
            description: args.description,
            body: args.body,
            status,
            depends_on,
            blocks,
            refs_add,
            refs_remove,
            superseded_by,
            message: args.message,
        };
        let record = mmcp_store::features::update_feature(
            &self.state.backend,
            &entry,
            &args.slug,
            spec,
            &self.state.author,
        )
        .await
        .map_err(map_feature_error_to_mcp)?;
        Ok(ok_json(feature_record_to_json(&entry, &record)))
    }

    #[tool(
        description = "Delete a feature request by slug. The deletion is committed on the group's main branch so the FR is recoverable via `list_versions`. Refuses with `not_a_feature` when the slug points at a non-FR memory so the FR tools never drop unrelated memories.",
        annotations(
            title = "Delete feature request",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn delete_feature(
        &self,
        Parameters(args): Parameters<DeleteFeatureArgs>,
    ) -> Result<CallToolResult, McpError> {
        let cwd = current_dir_for_mcp()?;
        let (entry, _root) = mmcp_store::features::resolve_project_group_with_selector(
            &self.state.groups,
            args.project.as_deref(),
            &cwd,
        )
            .await
            .map_err(map_feature_error_to_mcp)?;
        let commit_id = mmcp_store::features::delete_feature(
            &self.state.backend,
            &entry,
            &args.slug,
            &self.state.author,
            args.message.as_deref(),
        )
        .await
        .map_err(map_feature_error_to_mcp)?;
        Ok(ok_json(json!({
            "group":     entry.manifest.group_id.to_string(),
            "slug":      args.slug,
            "commit_id": commit_id,
        })))
    }

    #[tool(
        description = "Rename every feature memory under `old_slug` to `new_slug` in a single atomic commit (FR-027). UUIDs stay stable across the rename so cross-references in other features keep resolving without further rewrites. Duplicate slugs (FR-028) move as a batch — every entry under `memories/<old_slug>/` lands under `memories/<new_slug>/`. Errors with `memory_not_found` when no memory lives at `old_slug` and with `not_a_feature` when the source is a non-FR memory.",
        annotations(
            title = "Rename feature request",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn rename_feature(
        &self,
        Parameters(args): Parameters<RenameFeatureArgs>,
    ) -> Result<CallToolResult, McpError> {
        let cwd = current_dir_for_mcp()?;
        let (entry, _root) = mmcp_store::features::resolve_project_group_with_selector(
            &self.state.groups,
            args.project.as_deref(),
            &cwd,
        )
            .await
            .map_err(map_feature_error_to_mcp)?;
        let records = mmcp_store::rename_feature(
            &self.state.backend,
            &entry,
            &args.old_slug,
            &args.new_slug,
            &self.state.author,
            args.message.as_deref(),
        )
        .await
        .map_err(map_feature_error_to_mcp)?;
        let features: Vec<_> = records
            .iter()
            .map(|record| feature_record_to_json(&entry, record))
            .collect();
        Ok(ok_json(json!({
            "group":      entry.manifest.group_id.to_string(),
            "old_slug":   args.old_slug,
            "new_slug":   args.new_slug,
            "renamed":    features.len(),
            "features":   features,
        })))
    }

    #[tool(
        description = "List feature requests in the current project's group. By default hides every FR whose status is terminal-ish: `resolved`, `duplicate`, `superseded`. Open, blocked, and deferred FRs stay visible so the default listing reads as 'what still needs work'. Pass `all: true` to include every status, or `status: <variant>` to pin a specific lifecycle state (explicit `status` wins over the `all` flag). Non-FR memories in the same group are skipped so the listing stays FR-shaped. Memories whose frontmatter fails to parse are quietly omitted; use `diagnose` to surface those.",
        annotations(
            title = "List feature requests",
            read_only_hint = true,
            idempotent_hint = true,
            open_world_hint = false,
        )
    )]
    async fn list_features(
        &self,
        Parameters(args): Parameters<ListFeaturesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let cwd = current_dir_for_mcp()?;
        let (entry, _root) = mmcp_store::features::resolve_project_group_with_selector(
            &self.state.groups,
            args.project.as_deref(),
            &cwd,
        )
            .await
            .map_err(map_feature_error_to_mcp)?;
        let status = parse_status_arg(args.status.as_deref())?;
        let show_all = args.all.unwrap_or(false);
        // FR-048: list-style surfaces return body-free summaries.
        // Bodies fly back through `read_feature` only, keeping the
        // response well under the MCP client token cap on populated
        // FR groups.
        let summaries = mmcp_store::features::list_feature_summaries(
            &self.state.backend,
            &entry,
            status,
            show_all,
        )
        .await
        .map_err(map_feature_error_to_mcp)?;
        // FR-45 `dangling_ref`: aggregate dangling-ref notes
        // across every record in the listing so callers see a
        // single pane of reference-integrity warnings alongside
        // the listing itself.
        let mut notes = Vec::new();
        for summary in &summaries {
            notes.extend(
                dangling_ref_notes_for(
                    &self.state.backend,
                    &entry,
                    &summary.slug,
                    &summary.depends_on,
                    &summary.blocks,
                    summary.superseded_by.as_ref(),
                )
                .await,
            );
        }
        let features: Vec<_> = summaries
            .iter()
            .map(|summary| feature_summary_to_json(&entry, summary))
            .collect();
        Ok(ok_json_with_notes(
            json!({
                "group":    entry.manifest.group_id.to_string(),
                "features": features,
                "count":    summaries.len(),
            }),
            notes,
        ))
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

    /// Canonical list of every registered MCP tool's static
    /// `Tool` descriptor.
    ///
    /// Single source of truth for `describe_tools` (FR-31), the
    /// `mmcp tools` CLI (FR-33), and the `diagnose` annotation-
    /// coverage check (FR-34). The FR-29 conformance test keeps its
    /// own hard-coded matrix so a new tool appearing here without a
    /// matching matrix entry still trips the test, preserving the
    /// double-entry safeguard.
    fn registered_tool_attrs() -> Vec<rmcp::model::Tool> {
        vec![
            // Read-only tools.
            Self::list_groups_tool_attr(),
            Self::list_memories_tool_attr(),
            Self::read_memory_tool_attr(),
            Self::list_versions_tool_attr(),
            Self::group_info_tool_attr(),
            Self::search_memories_tool_attr(),
            Self::read_memory_body_sections_tool_attr(),
            Self::check_health_tool_attr(),
            Self::diagnose_tool_attr(),
            Self::debug_read_file_tool_attr(),
            Self::debug_list_tree_tool_attr(),
            Self::debug_git_log_tool_attr(),
            Self::bootstrap_context_tool_attr(),
            Self::status_tool_attr(),
            Self::read_feature_tool_attr(),
            Self::list_features_tool_attr(),
            Self::describe_tools_tool_attr(),
            // Local mutators (open_world = false).
            Self::write_memory_tool_attr(),
            Self::import_memory_tool_attr(),
            Self::edit_memory_tool_attr(),
            Self::edit_memory_body_tool_attr(),
            Self::move_memory_tool_attr(),
            Self::debug_write_file_tool_attr(),
            Self::update_feature_tool_attr(),
            Self::delete_memory_tool_attr(),
            Self::init_claude_tool_attr(),
            Self::delete_feature_tool_attr(),
            Self::debug_toggle_tool_attr(),
            Self::init_project_tool_attr(),
            Self::rename_feature_tool_attr(),
            Self::subscribe_tool_attr(),
            Self::unsubscribe_tool_attr(),
            Self::create_group_tool_attr(),
            Self::add_feature_tool_attr(),
            // Archive tools (open_world = true).
            Self::export_archive_tool_attr(),
            Self::import_archive_tool_attr(),
            // Sync tools (open_world = true).
            Self::sync_fetch_tool_attr(),
            Self::sync_push_tool_attr(),
            Self::sync_pull_tool_attr(),
            Self::sync_tool_attr(),
        ]
    }

}

/// Module-level pub(crate) accessor for the canonical tool list.
///
/// `McpServer::registered_tool_attrs()` is the same data; this free
/// function is the form `commands::tools` reaches for since the
/// CLI subcommand never instantiates an `McpServer`.
///
/// FR-49: every entry is decorated with category-derived icons so
/// `describe_tools`, the `mmcp tools` CLI, and (via the live
/// `tool_router` in `McpServer::new`) `tools/list` all surface the
/// same per-tool glyph without 37 separate `icons = ...` macro
/// arguments at every `#[tool]` site.
pub(crate) fn registered_tool_attrs() -> Vec<rmcp::model::Tool> {
    let mut tools = McpServer::registered_tool_attrs();
    for tool in &mut tools {
        tool.icons = Some(icons_for_category(tool_icon_category(tool.name.as_ref())));
        // FR-50: meta lands on the same patching seam as icons so
        // describe_tools and the CLI surface match the live router.
        tool.meta = meta_for_tool(tool.name.as_ref());
        // FR-45: every tool gets a permissive object output schema
        // so clients can validate. Per-tool typed schemas defer to
        // a follow-up FR.
        tool.output_schema = Some(shared_output_schema());
    }
    tools
}

/// FR-49: per-tool category that drives icon selection. Hand-
/// curated by tool name; an unmapped tool falls through to
/// `Mutate` so the FR-29 conformance test surfaces the omission
/// rather than shipping a generic glyph that misleads operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolIconCategory {
    /// Read-only tools that walk the local mirror without writing.
    Read,
    /// Local mutators — additive or destructive writes against the
    /// mirror, the sessions store, or `.mmcp.toml`.
    Mutate,
    /// FR-007 feature-request tools (`*_feature`).
    Feature,
    /// `debug_*` raw-git escape hatches.
    Debug,
    /// `sync_*` tools that contact the remote server.
    Sync,
}

fn tool_icon_category(name: &str) -> ToolIconCategory {
    match name {
        "list_groups"
        | "list_memories"
        | "read_memory"
        | "list_versions"
        | "group_info"
        | "search_memories"
        | "read_memory_body_sections"
        | "check_health"
        | "diagnose"
        | "bootstrap_context"
        | "status"
        | "describe_tools" => ToolIconCategory::Read,
        "read_feature"
        | "list_features"
        | "add_feature"
        | "update_feature"
        | "delete_feature"
        | "rename_feature" => ToolIconCategory::Feature,
        "debug_read_file"
        | "debug_list_tree"
        | "debug_git_log"
        | "debug_write_file"
        | "debug_toggle" => ToolIconCategory::Debug,
        "sync_fetch" | "sync_push" | "sync_pull" | "sync" => ToolIconCategory::Sync,
        // Archive export reads the store to produce an artifact;
        // import writes the store from one.
        "export_archive" => ToolIconCategory::Read,
        "import_archive" => ToolIconCategory::Mutate,
        // Default arm: every remaining live tool is a local
        // mutator. New tools that drift outside the buckets above
        // surface as `Mutate` until the curator updates this match;
        // `tool_icons_match_categories` test covers the live set.
        _ => ToolIconCategory::Mutate,
    }
}

fn icons_for_category(cat: ToolIconCategory) -> Vec<rmcp::model::Icon> {
    let src = match cat {
        ToolIconCategory::Read => READ_ICON_SRC,
        ToolIconCategory::Mutate => MUTATE_ICON_SRC,
        ToolIconCategory::Feature => FEATURE_ICON_SRC,
        ToolIconCategory::Debug => DEBUG_ICON_SRC,
        ToolIconCategory::Sync => SYNC_ICON_SRC,
    };
    vec![rmcp::model::Icon::new(src).with_mime_type("image/svg+xml")]
}

// FR-49: tiny inline-SVG data URIs so the icon ships with the
// binary instead of relying on an external CDN. Each glyph is a
// single emoji rendered as text inside a 16x16 viewBox; clients
// with icon-capable UIs render the emoji at any size.
const READ_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F4D6}</text></svg>";
const MUTATE_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{270F}\u{FE0F}</text></svg>";
const FEATURE_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F6A9}</text></svg>";
const DEBUG_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F41B}</text></svg>";
const SYNC_ICON_SRC: &str = "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='14' font-size='14'>\u{1F504}</text></svg>";

/// FR-50: build the per-tool `_meta` map carrying mmcp-specific
/// advisory hints that complement the FR-29 `ToolAnnotations` bits.
/// Returns `None` for tools that need none of the bits so the wire
/// shape stays absent rather than `{}` for unrelated tools.
///
/// Every key is namespaced under `mmcp.` per the FR spec. Today's
/// vocabulary:
///
/// - `mmcp.requires_project` — tool errors without a discovered
///   `.mmcp.toml` (every FR tool plus `subscribe` / `unsubscribe`).
/// - `mmcp.requires_sync` — tool errors without a configured
///   `[sync]` block in `.mmcp.toml` (every `sync_*` tool).
/// - `mmcp.debug_gated` — tool refuses unless `debug_toggle(true)`
///   has been called this session (every `debug_*` tool).
/// - `mmcp.protected_group_gated` — tool fires the FR-019
///   `confirm_protected_write` elicitation when targeting a
///   protected group (write / edit / delete / debug_write_file /
///   init_claude).
/// - `mmcp.network` — tool reaches outside the local mirror.
///   Today only the `sync_*` tools set this, mirroring
///   `open_world_hint` but kept distinct so future open-world
///   tools that don't sync (e.g. a future fetch-from-URL)
///   classify cleanly.
fn meta_for_tool(name: &str) -> Option<rmcp::model::Meta> {
    let mut keys: Vec<(&'static str, bool)> = Vec::new();

    // Tools that auto-resolve a project from cwd and error out
    // when no `.mmcp.toml` is in scope. The `project` selector
    // arg lets callers point at a specific group, but the bit
    // still flags "needs project context" for harness pre-flight.
    if matches!(
        name,
        "read_feature"
            | "list_features"
            | "add_feature"
            | "update_feature"
            | "delete_feature"
            | "rename_feature"
            | "subscribe"
            | "unsubscribe"
    ) {
        keys.push(("mmcp.requires_project", true));
    }

    // Sync tools need a configured `[sync] server_url` and they
    // touch the network. Two bits flag both axes so harnesses
    // targeting offline-only mirrors can mask them out.
    if matches!(name, "sync_fetch" | "sync_push" | "sync_pull" | "sync") {
        keys.push(("mmcp.requires_sync", true));
        keys.push(("mmcp.network", true));
    }

    if matches!(
        name,
        "debug_read_file"
            | "debug_list_tree"
            | "debug_git_log"
            | "debug_write_file"
            | "debug_toggle"
    ) {
        keys.push(("mmcp.debug_gated", true));
    }

    if matches!(
        name,
        "write_memory"
            | "edit_memory"
            | "edit_memory_body"
            | "move_memory"
            | "delete_memory"
            | "debug_write_file"
            | "init_claude"
            | "import_archive"
    ) {
        keys.push(("mmcp.protected_group_gated", true));
    }

    if keys.is_empty() {
        return None;
    }
    let mut meta = rmcp::model::Meta::new();
    for (k, v) in keys {
        meta.0.insert(k.to_string(), serde_json::Value::Bool(v));
    }
    Some(meta)
}

/// FR-45: every registered tool receives a permissive object
/// `output_schema` so MCP clients can validate that responses are
/// JSON objects (with optional `notes` channel) and surface the
/// shape in autocomplete UIs. Per-tool typed schemas — the FR's
/// stretch goal — are deferred to a follow-up: replacing the
/// `json!({...})` payloads with typed structs deriving `JsonSchema`
/// is a 37-tool refactor of its own that doesn't compose cleanly
/// inside this metadata-sweep streak.
///
/// Cached behind a `OnceLock` so the same `Arc<JsonObject>` reaches
/// every tool. Cheap to clone; cheaper than rebuilding the map per
/// tool on every `tools/list` round-trip.
fn shared_output_schema() -> std::sync::Arc<rmcp::model::JsonObject> {
    use std::sync::OnceLock;
    static SCHEMA: OnceLock<std::sync::Arc<rmcp::model::JsonObject>> = OnceLock::new();
    SCHEMA
        .get_or_init(|| {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("object".to_string()),
            );
            obj.insert(
                "additionalProperties".to_string(),
                serde_json::Value::Bool(true),
            );
            // Surface the shared `notes` field shape so harnesses
            // know to look there for FR-45 dangling-ref / parse-
            // warning notes; absent on tools that never emit any.
            let mut props = serde_json::Map::new();
            let mut notes_schema = serde_json::Map::new();
            notes_schema
                .insert("type".to_string(), serde_json::Value::String("array".to_string()));
            notes_schema.insert(
                "description".to_string(),
                serde_json::Value::String(
                    "FR-45 notes channel. Optional warnings emitted alongside the \
                     tool's primary payload."
                        .to_string(),
                ),
            );
            props.insert("notes".to_string(), serde_json::Value::Object(notes_schema));
            obj.insert("properties".to_string(), serde_json::Value::Object(props));
            obj.insert(
                "description".to_string(),
                serde_json::Value::String(
                    "Tool response. Permissive object shape — per-tool typed schemas \
                     land in a follow-up FR."
                        .to_string(),
                ),
            );
            std::sync::Arc::new(obj)
        })
        .clone()
}

/// FR-32 per-argument risk hint. Each entry names a specific arg
/// (and the value that activates the risk) so harnesses can prompt
/// even when the tool itself is not flagged destructive at the
/// FR-29 level. Serialised into `describe_tools` and the
/// `mmcp tools` CLI.
///
/// Today only boolean-true triggers are modelled — the existing
/// risky args (`override`, `force`) are all flag-shaped. Enum or
/// numeric value triggers can extend the `risk_when` field later
/// without breaking the wire shape.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ArgRiskHint {
    /// Name of the argument as it appears in the tool's input
    /// schema.
    pub arg: &'static str,
    /// Value condition that makes the arg risky. Today always
    /// `"true"` since every existing risky arg is boolean.
    pub risk_when: &'static str,
    /// Stable code matching the tool-level `destructive_hint`
    /// vocabulary so harnesses can re-use the same prompt text.
    pub kind: &'static str,
    /// Human-readable one-line explanation. Suitable for direct
    /// display in a confirmation prompt.
    pub reason: &'static str,
}

/// FR-32 curated hint registry. Tool name → risky-arg entries.
///
/// Entries are hand-maintained — there is no derive macro that
/// inspects the args struct. The trade-off is honest: most tool
/// args are not risk-bearing, so the registry stays short, and the
/// FR-31 `describe_tools` consumer wants explicit reasons that a
/// macro could not generate.
pub(crate) fn arg_risk_hints_for(tool_name: &str) -> &'static [ArgRiskHint] {
    match tool_name {
        "write_memory" => &[
            ArgRiskHint {
                arg: "override",
                risk_when: "true",
                kind: "destructive",
                reason: "override: true overwrites the existing file silently; prefer edit_memory for partial updates",
            },
            ArgRiskHint {
                arg: "force",
                risk_when: "true",
                kind: "destructive",
                reason: "force: true bypasses the FR-28 filename/frontmatter id-mismatch guard",
            },
        ],
        "import_memory" => &[
            ArgRiskHint {
                arg: "override",
                risk_when: "true",
                kind: "destructive",
                reason: "override: true replaces the colliding-id memory in place",
            },
        ],
        "edit_memory" => &[
            ArgRiskHint {
                arg: "force",
                risk_when: "true",
                kind: "destructive",
                reason: "force: true bypasses the FR-28 filename/frontmatter id-mismatch guard on a ByFilename write",
            },
        ],
        "edit_memory_body" => &[
            ArgRiskHint {
                arg: "force",
                risk_when: "true",
                kind: "destructive",
                reason: "force: true bypasses the FR-28 filename/frontmatter id-mismatch guard",
            },
        ],
        "import_archive" => &[
            ArgRiskHint {
                arg: "overwrite",
                risk_when: "true",
                kind: "destructive",
                reason: "overwrite: true replaces colliding memories in place instead of reporting a conflict",
            },
        ],
        _ => &[],
    }
}

/// FR-34 helper: emit one `missing_tool_annotations` warn note per
/// tool whose `annotations` slot is `None`. Extracted so the
/// diagnose body stays a single fan-out and so unit tests can
/// exercise the loop against synthetic tools without standing up a
/// full server.
fn collect_missing_annotation_notes(tools: &[rmcp::model::Tool]) -> Vec<mmcp_proto::Note> {
    tools
        .iter()
        .filter(|tool| tool.annotations.is_none())
        .map(|tool| {
            mmcp_proto::Note::warn(
                "missing_tool_annotations",
                format!("MCP tool '{}' has no annotations", tool.name),
            )
            .with_context(json!({ "tool": tool.name.to_string() }))
        })
        .collect()
}

impl McpServer {
    /// Parse the wire `group` string into a `GroupId` and look up
    /// the entry from the local mirror. Every memory-addressing
    /// tool starts with the same two lines; keeping them here
    /// makes adding new tools a one-line preamble instead of a
    /// copy-pasted ten-line chain.
    async fn resolve_group_entry(&self, group: &str) -> Result<GroupEntry, McpError> {
        let group_id = parse_group_id(group)?;
        self.state.groups.get(&group_id).await.ok_or_else(|| {
            McpError::invalid_params(
                "group not found in local mirror",
                Some(json!({ "group": group_id.to_string() })),
            )
        })
    }

    /// Resolve a group by UUID *or* slug. Unlike `resolve_group_entry`
    /// (UUID-only), the archive tools accept either so operators can
    /// name groups the way they do everywhere else on the CLI.
    async fn resolve_group_any(&self, group: &str) -> Result<GroupEntry, McpError> {
        mmcp_store::resolve_group(&self.state.groups, group)
            .await
            .map_err(map_memory_error_to_mcp)
    }

    /// Fire the protected-group confirmation for every existing
    /// protected group an archive import would write into. New groups
    /// recreated from the archive carry no local protection to confirm.
    async fn confirm_archive_protected(
        &self,
        peer: &Peer<RoleServer>,
        manifest: &mmcp_store::ArchiveManifest,
        into_group: Option<GroupId>,
    ) -> Result<(), McpError> {
        let targets: Vec<GroupEntry> = if let Some(group_id) = into_group {
            self.state.groups.get(&group_id).await.into_iter().collect()
        } else {
            let mut out = Vec::new();
            for group_meta in &manifest.groups {
                if let Ok(entry) =
                    mmcp_store::resolve_group(&self.state.groups, &group_meta.group_id.to_string())
                        .await
                {
                    out.push(entry);
                }
            }
            out
        };
        for entry in &targets {
            if entry.manifest.protected {
                confirm_protected_write(peer, entry, "<archive>", "archive-import").await?;
            }
        }
        Ok(())
    }

    /// Shared body for `subscribe` and `unsubscribe`. Resolves the
    /// project root, validates the target against the local mirror,
    /// applies the in-memory mutation, and persists the config when
    /// anything actually changed. The two MCP tool methods are
    /// one-liners around this helper so the audit-trail commit
    /// boundaries (subscribe vs unsubscribe) stay distinct without
    /// duplicating the body.
    async fn apply_subscription_mcp(
        &self,
        args: crate::commands::subscribe::SubscribeMcpArgs,
        action: crate::commands::subscribe::SubscriptionAction,
    ) -> Result<CallToolResult, McpError> {
        use crate::commands::subscribe::{
            apply_subscription, resolve_project_root, validate_subscription_target,
        };

        let cwd = current_dir_for_mcp()?;
        let explicit = args.path.as_deref().map(Path::new);
        let project_root = resolve_project_root(explicit, Some(&cwd))
            .map_err(map_subscribe_error_to_mcp)?;

        validate_subscription_target(
            &self.state.backend,
            &self.state.groups,
            args.kind,
            &args.value,
        )
        .await
        .map_err(map_subscribe_error_to_mcp)?;

        let mut cfg = load_project_config(&project_root).map_err(|e| {
            McpError::internal_error(format!("loading project config: {e}"), None)
        })?;
        let changed = apply_subscription(&mut cfg, args.kind, &args.value, action);
        if changed {
            mmcp_store::config::save(&project_root, &cfg).map_err(|e| {
                McpError::internal_error(format!("saving project config: {e}"), None)
            })?;
        }
        Ok(ok_json(json!({
            "kind": args.kind.as_str(),
            "value": args.value,
            "action": action.as_str(),
            "changed": changed,
            "project_root": project_root.to_string_lossy().into_owned(),
            "project_uuid": cfg.project_uuid.to_string(),
        })))
    }

    /// Resolve the group entry **and** a specific memory within it
    /// in one call. Every read / edit / delete / body tool runs
    /// this exact chain post-FR-028: parse the group, look up the
    /// entry, parse the optional UUID, then call `resolve_memory`.
    /// Centralising it here keeps tool bodies to their actual
    /// per-tool logic.
    async fn resolve_memory_address(
        &self,
        group: &str,
        slug: Option<&str>,
        id: Option<&str>,
    ) -> Result<(GroupEntry, mmcp_store::ResolvedMemory), McpError> {
        let entry = self.resolve_group_entry(group).await?;
        let id = parse_optional_uuid(id)?;
        let resolved =
            mmcp_store::resolve_memory(&self.state.backend, &entry.handle, slug, id)
                .await
                .map_err(map_memory_error_to_mcp)?;
        Ok((entry, resolved))
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
        SyncError::PullDiverged {
            group,
            local,
            target,
        } => json!({
            "code": "pull_diverged",
            "group": group.to_string(),
            "local": local,
            "target": target,
        }),
        SyncError::PushDiverged { group, stderr } => json!({
            "code": "push_diverged",
            "group": group.to_string(),
            "stderr": stderr,
        }),
    };
    McpError::invalid_params(message, Some(payload))
}

/// Resolve a [`SyncToolArgs`] into a [`mmcp_sync::SyncFilter`].
///
/// Structured error payloads follow the same code convention as
/// the other tool error mappers so AI clients branch on state
/// instead of parsing prose. Empty selector falls back to
/// `SyncFilter::All` for this commit; the follow-up commit turns
/// that fallback into a `selector_required` error so CLI and MCP
/// reject bare calls simultaneously.
async fn resolve_sync_filter(
    args: &SyncToolArgs,
    groups: &mmcp_store::GroupIndex,
) -> Result<mmcp_sync::SyncFilter, McpError> {
    let all_flag = args.all.unwrap_or(false);
    let provided: Vec<&str> = [
        args.group.is_some().then_some("group"),
        args.scope.is_some().then_some("scope"),
        all_flag.then_some("all"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if provided.len() > 1 {
        return Err(McpError::invalid_params(
            "sync selectors are mutually exclusive; pass exactly one of `group`, `scope`, `all`",
            Some(json!({
                "code": "selector_conflict",
                "provided": provided,
            })),
        ));
    }
    if all_flag {
        return Ok(mmcp_sync::SyncFilter::All);
    }
    if let Some(scope) = args.scope {
        return Ok(mmcp_sync::SyncFilter::Scope(scope.into_core()));
    }
    if let Some(query) = args.group.as_deref() {
        let entry = mmcp_store::resolve_group(groups, query).await.map_err(|e| {
            McpError::invalid_params(
                e.to_string(),
                Some(json!({
                    "code": "unknown_group",
                    "query": query,
                })),
            )
        })?;
        return Ok(mmcp_sync::SyncFilter::Group(
            *entry.manifest.group_id.as_uuid(),
        ));
    }
    Err(McpError::invalid_params(
        "sync selector required: pass exactly one of `group`, `scope`, or `all`",
        Some(json!({
            "code": "selector_required",
            "accepted": ["group", "scope", "all"],
        })),
    ))
}

/// Read the server process's current working directory, mapping
/// `io::Error` onto `McpError::internal_error` so every FR tool
/// surfaces the failure identically. Factored out because five
/// tools share it and an inline expression would drift between
/// variants.
/// Resolve `bootstrap_context.next_action.subscribed_reads` from the
/// four subscription axes:
///
/// - `groups` + `languages` → every memory address from the named
///   group surfaces (mandatory + non-mandatory). Project group and
///   Global are also treated as "fully subscribed" so the AI sees
///   all their addresses without paying per-group `list_memories`.
/// - `memories` → literal `<group_uuid>:<slug>` pins.
/// - `tags` → scan every group the local mirror knows about (not
///   only in-scope ones — the whole point of tag pins is to reach
///   memories from groups the project hasn't fully adopted) and
///   include any non-mandatory memory whose tags overlap.
///
/// Returns `(group_uuid, slug)` JSON entries with no metadata.
/// Deduplicated across axes so a tag-pinned memory in a fully
/// subscribed group only appears once.
pub(crate) async fn resolve_subscribed_reads(
    backend: &NativeBackend,
    entries: &[GroupEntry],
    cfg: &mmcp_core::config::ProjectConfig,
    adopted_shared: &std::collections::HashSet<Uuid>,
    project_uuid: Option<Uuid>,
) -> Vec<serde_json::Value> {
    let mut seen: std::collections::HashSet<(Uuid, String)> = std::collections::HashSet::new();
    let mut out: Vec<serde_json::Value> = Vec::new();

    let want_tags: std::collections::HashSet<String> =
        cfg.subscriptions.tags.iter().cloned().collect();
    let want_memories: std::collections::HashSet<String> =
        cfg.subscriptions.memories.iter().cloned().collect();

    let push_addr = |seen: &mut std::collections::HashSet<(Uuid, String)>,
                     out: &mut Vec<serde_json::Value>,
                     group: Uuid,
                     slug: &str| {
        if seen.insert((group, slug.to_string())) {
            out.push(json!({
                "group": group.to_string(),
                "slug": slug,
            }));
        }
    };

    for entry in entries {
        let entry_uuid = *entry.manifest.group_id.as_uuid();
        let fully_subscribed = match entry.manifest.scope {
            mmcp_core::manifest::GroupScope::Global => true,
            mmcp_core::manifest::GroupScope::Project => project_uuid == Some(entry_uuid),
            mmcp_core::manifest::GroupScope::Shared => adopted_shared.contains(&entry_uuid),
        };

        // Memory pins target a specific (group, slug); we always
        // need to walk every group's file list so the resolver
        // surfaces pins from groups that aren't fully subscribed.
        let need_listing = fully_subscribed || !want_tags.is_empty() || !want_memories.is_empty();
        if !need_listing {
            continue;
        }

        let files = match list_memory_files(backend, entry).await {
            Ok(f) => f,
            Err(_) => continue,
        };

        for file_ref in files {
            let pin_key = format!("{}:{}", entry_uuid, file_ref.slug);
            let pinned_individually = want_memories.contains(&pin_key);

            if fully_subscribed {
                push_addr(&mut seen, &mut out, entry_uuid, &file_ref.slug);
                continue;
            }
            if pinned_individually {
                push_addr(&mut seen, &mut out, entry_uuid, &file_ref.slug);
                continue;
            }
            if !want_tags.is_empty() {
                // Tag matching needs to peek at the frontmatter.
                let bytes = match backend
                    .read_file(&entry.handle, &file_ref.path, &Rev::head())
                    .await
                {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    continue;
                };
                let Ok(file) = MemoryFile::parse(text) else {
                    continue;
                };
                if file.frontmatter.mandatory {
                    // Tag-based subscription is intentionally about
                    // non-mandatory memories — mandatory entries
                    // already surface through `list_memories` on
                    // every in-scope group.
                    continue;
                }
                if file.frontmatter.tags.iter().any(|t| want_tags.contains(t)) {
                    push_addr(&mut seen, &mut out, entry_uuid, &file_ref.slug);
                }
            }
        }
    }

    out
}

/// FR-025: does `cfg` adopt the given group slug? Checks both the
/// explicit `subscriptions.groups` list and the `lang/<name>` mapping
/// implied by `subscriptions.languages`. Bare string equality for
/// now — namespace-aware resolution is a follow-up when the
/// adoption format stabilises.
pub(crate) fn is_group_adopted(slug: &str, cfg: &mmcp_core::config::ProjectConfig) -> bool {
    cfg.subscriptions.groups.iter().any(|s| s == slug)
        || cfg
            .subscriptions
            .languages
            .iter()
            .any(|lang| slug == format!("lang/{lang}"))
}

fn current_dir_for_mcp() -> Result<std::path::PathBuf, McpError> {
    std::env::current_dir()
        .map_err(|e| McpError::internal_error(format!("cannot read working directory: {e}"), None))
}

/// Parse the wire form of [`FeatureStatus`] from an optional string
/// argument. `None` → `Ok(None)`; a known variant → `Ok(Some(...))`;
/// an unknown variant → structured `invalid_feature_status` error.
fn parse_status_arg(
    raw: Option<&str>,
) -> Result<Option<mmcp_core::memory::FeatureStatus>, McpError> {
    let Some(s) = raw else {
        return Ok(None);
    };
    mmcp_core::memory::FeatureStatus::parse(s)
        .map(Some)
        .map_err(|err| {
            McpError::invalid_params(
                err.to_string(),
                Some(json!({
                    "code":  "invalid_feature_status",
                    "input": err.input,
                    "allowed": mmcp_core::memory::FeatureStatus::all()
                        .iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                })),
            )
        })
}

/// Serialize a [`FeatureRecord`] to the JSON shape returned by the
/// FR tools. Includes the group id so callers can cross-reference
/// without a second `status` call.
fn feature_record_to_json(
    entry: &GroupEntry,
    record: &mmcp_store::features::FeatureRecord,
) -> serde_json::Value {
    json!({
        "group":         entry.manifest.group_id.to_string(),
        "slug":          record.slug,
        "title":         record.title,
        "description":   record.description,
        "body":          record.body,
        "status":        record.status.as_str(),
        "number":        record.number,
        "depends_on":    record.depends_on,
        "blocks":        record.blocks,
        "superseded_by": record.superseded_by.as_ref().map(memory_ref_to_json),
        "commit_id":     record.commit_id,
    })
}

/// Serialize a [`FeatureSummary`] to the body-free JSON shape used
/// by `list_features`. Same fields as `feature_record_to_json` minus
/// `body` — listings stay metadata-only so populated FR groups
/// don't blow past the MCP client token cap.
fn feature_summary_to_json(
    entry: &GroupEntry,
    summary: &mmcp_store::features::FeatureSummary,
) -> serde_json::Value {
    json!({
        "group":         entry.manifest.group_id.to_string(),
        "slug":          summary.slug,
        "title":         summary.title,
        "description":   summary.description,
        "status":        summary.status.as_str(),
        "number":        summary.number,
        "depends_on":    summary.depends_on,
        "blocks":        summary.blocks,
        "superseded_by": summary.superseded_by.as_ref().map(memory_ref_to_json),
        "commit_id":     summary.commit_id,
    })
}

/// Render a [`mmcp_core::memory::MemoryRef`] onto the MCP wire shape
/// as a plain `{ target, commit }` object. Used by every response
/// that surfaces typed refs so the shape stays identical across
/// `read_memory`, `read_feature`, and the feature CRUD returns.
fn memory_ref_to_json(r: &mmcp_core::memory::MemoryRef) -> serde_json::Value {
    json!({
        "target": r.target.to_string(),
        "commit": r.commit,
    })
}

/// Parse a wire-form `Vec<MemoryRefArg>` (raw from the tool args)
/// into the `Vec<MemoryRef>` the store layer expects. Funnels
/// through [`mmcp_store::parse_memory_refs`] so UUID +
/// commit-sha validation lives in one place; the MCP boundary
/// maps the resulting error to `invalid_memory_ref`.
fn parse_wire_refs(
    raw: Vec<MemoryRefArg>,
    field: &'static str,
) -> Result<Vec<mmcp_core::memory::MemoryRef>, McpError> {
    let inputs: Vec<_> = raw.into_iter().map(MemoryRefArg::into_store_input).collect();
    mmcp_store::parse_memory_refs(&inputs, field).map_err(map_xref_error_to_mcp)
}

/// Map a [`mmcp_store::XrefError`] onto an [`McpError`] for tool
/// surfaces that take cross-reference input directly (the parsers
/// in `mmcp_core::memory::xrefs` return `XrefError`, not
/// `FeatureError`, so they need their own mapper without going
/// through the feature-error envelope).
fn map_xref_error_to_mcp(err: mmcp_store::XrefError) -> McpError {
    let message = err.to_string();
    match err {
        mmcp_store::XrefError::InvalidCrossRef { field, value } => McpError::invalid_params(
            message.clone(),
            Some(json!({
                "code": "invalid_feature_cross_reference",
                "field": field,
                "value": value,
            })),
        ),
        mmcp_store::XrefError::InvalidMemoryRef { field, detail } => McpError::invalid_params(
            message.clone(),
            Some(json!({
                "code": "invalid_memory_ref",
                "field": field,
                "detail": detail,
            })),
        ),
        // XrefError is `#[non_exhaustive]`; a future variant
        // surfaces here as a generic invalid_params rather than
        // a panic.
        _ => McpError::invalid_params(message, Some(json!({ "code": "invalid_xref" }))),
    }
}

/// Map a [`features::FeatureError`] onto an [`McpError`] with a
/// structured `code` payload so AI callers branch on state rather
/// than parsing strings. Covers the FR-specific cases first, then
/// delegates to `map_memory_error_to_mcp` for the wrapped memory
/// CRUD failures so the wire contract stays identical between FR
/// tools and `write_memory` / `edit_memory` / `delete_memory`.
fn map_feature_error_to_mcp(err: mmcp_store::features::FeatureError) -> McpError {
    use mmcp_store::features::FeatureError;
    let message = err.to_string();
    match err {
        FeatureError::NotAFeature { slug, kind } => McpError::invalid_params(
            message,
            Some(json!({
                "code": "not_a_feature",
                "slug": slug,
                "kind": kind,
            })),
        ),
        FeatureError::TitleRequired => {
            McpError::invalid_params(message, Some(json!({ "code": "feature_title_required" })))
        }
        FeatureError::ProjectNotFound => McpError::invalid_params(
            message,
            Some(json!({
                "code": "project_not_found",
                "retry_hint": "run `init_project` or cd into a directory that contains a `.mmcp.toml`",
            })),
        ),
        FeatureError::ProjectGroupMissing { project_uuid } => McpError::invalid_params(
            message,
            Some(json!({
                "code": "project_group_missing",
                "project_uuid": project_uuid,
                "retry_hint": "run `sync_pull` to fetch the project group, or `init_project` to create it locally",
            })),
        ),
        FeatureError::ProjectConfigBroken { path, detail } => McpError::invalid_params(
            message,
            Some(json!({
                "code": "project_config_broken",
                "path": path,
                "detail": detail,
            })),
        ),
        FeatureError::Xref(xref) => map_xref_error_to_mcp(xref),
        FeatureError::SupersedesUnknown { query } => McpError::invalid_params(
            message,
            Some(json!({
                "code": "supersedes_unknown",
                "query": query,
            })),
        ),
        FeatureError::SupersedesInvalidStatus {
            slug,
            status,
            existing_link,
        } => McpError::invalid_params(
            message,
            Some(json!({
                "code": "supersedes_invalid_status",
                "slug": slug,
                "status": status.as_str(),
                "existing_link": existing_link.map(|r| json!({
                    "target": r.target.to_string(),
                    "commit": r.commit,
                })),
            })),
        ),
        FeatureError::SupersedesCrossGroupUnsupported { query } => McpError::invalid_params(
            message,
            Some(json!({
                "code": "supersedes_cross_group_unsupported",
                "query": query,
            })),
        ),
        FeatureError::UnknownProject { query } => McpError::invalid_params(
            message,
            Some(json!({
                "code": "unknown_project",
                "query": query,
            })),
        ),
        FeatureError::Memory(inner) => map_memory_error_to_mcp(inner),
    }
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

/// Map an [`mmcp_store::AdocConvertError`] to an [`McpError`] with
/// a structured `code` payload. Used by the `import_memory` tool
/// when the caller requested adoc source handling and the `acdc`
/// bridge refused to produce markdown.
/// Map a [`crate::commands::subscribe::SubscribeError`] to an
/// [`McpError`] with a structured `code` payload.
fn map_subscribe_error_to_mcp(
    err: crate::commands::subscribe::SubscribeError,
) -> McpError {
    use crate::commands::subscribe::SubscribeError;
    let message = err.to_string();
    let payload = match &err {
        SubscribeError::NotInProject => json!({ "code": "not_in_project" }),
        SubscribeError::MalformedMemoryValue(value) => json!({
            "code": "malformed_memory_value",
            "value": value,
        }),
        SubscribeError::InvalidGroupUuid(value) => json!({
            "code": "invalid_group_uuid",
            "value": value,
        }),
        SubscribeError::UnknownGroup(group) => json!({
            "code": "unknown_group",
            "group": group,
        }),
        SubscribeError::UnknownMemory { group, slug } => json!({
            "code": "unknown_memory",
            "group": group,
            "slug": slug,
        }),
    };
    McpError::invalid_params(message, Some(payload))
}

fn map_adoc_convert_error_to_mcp(err: mmcp_store::AdocConvertError) -> McpError {
    use mmcp_store::AdocConvertError;
    let message = err.to_string();
    let code = match &err {
        AdocConvertError::Parse(_) => "adoc_parse_failed",
        AdocConvertError::Render(_) => "adoc_render_failed",
        AdocConvertError::Utf8(_) => "adoc_output_not_utf8",
    };
    McpError::invalid_params(message, Some(json!({ "code": code })))
}

/// Map a [`commands::group::CreateGroupError`] to an [`McpError`]
/// with a structured `code` payload so AI callers can branch on
/// state instead of parsing the error string.
fn map_create_group_error_to_mcp(err: crate::commands::group::CreateGroupError) -> McpError {
    use crate::commands::group::CreateGroupError;
    let message = err.to_string();
    let payload = match &err {
        CreateGroupError::InvalidSlug { slug } => json!({
            "code": "invalid_slug",
            "slug": slug,
        }),
        CreateGroupError::SlugAlreadyExists {
            slug,
            existing_group_id,
        } => json!({
            "code": "slug_already_exists",
            "slug": slug,
            "existing_group_id": existing_group_id.to_string(),
        }),
        CreateGroupError::GitBackend(detail) => json!({
            "code": "git_backend",
            "detail": detail,
        }),
        CreateGroupError::IndexRefreshFailed(detail) => json!({
            "code": "index_refresh_failed",
            "detail": detail,
        }),
    };
    McpError::invalid_params(message, Some(payload))
}

/// Guard the three MCP mutation paths against accidental writes
/// into a group that carries `GroupManifest.protected = true`.
///
/// Today the guard hard-errors with a structured
/// `protected_requires_elicitation` payload — no bool-arg bypass on
/// purpose, since the point of protection is a user-visible
/// confirmation, not a flag the AI can flip. Once rmcp exposes
/// `ElicitationRequest` (FR-011), this helper will instead fire an
/// elicitation with the group / slug / action in the request shape
/// and only proceed on a positive answer. The wire contract stays
/// stable across that migration because callers that can't
/// elicit will still see the same error `code`.
fn ensure_not_protected(entry: &GroupEntry, slug: &str, action: &str) -> Result<(), McpError> {
    if !entry.manifest.protected {
        return Ok(());
    }
    Err(McpError::invalid_params(
        format!(
            "group `{group}` is protected; {action} requires user confirmation",
            group = entry.manifest.slug,
            action = action,
        ),
        Some(json!({
            "code": "protected_requires_elicitation",
            "group_slug": entry.manifest.slug,
            "group_id": entry.manifest.group_id.to_string(),
            "slug": slug,
            "action": action,
            "retry_hint": "run the operation via CLI (mmcp import --override / direct edit with operator intent) or wait for an elicitation-capable MCP client",
        })),
    ))
}

/// FR-011: route protected-group mutations through an elicitation
/// prompt when the client supports it; fall back to the
/// `protected_requires_elicitation` structured error otherwise.
///
/// Returns `Ok(())` only when the client explicitly confirms the
/// write. Declines, cancels, or any non-`true` `confirmed` field
/// map to `protected_write_cancelled` so the caller can show a
/// user-facing abort message. Transport / timeout errors bubble
/// up as `internal_error` so retries are allowed.
async fn confirm_protected_write(
    peer: &Peer<RoleServer>,
    entry: &GroupEntry,
    slug: &str,
    action: &str,
) -> Result<(), McpError> {
    if !entry.manifest.protected {
        return Ok(());
    }
    let message = format!(
        "Confirm writing into protected group `{group}` (action: {action}, slug: {slug})",
        group = entry.manifest.slug,
    );
    match peer.elicit::<ProtectedWriteConfirm>(message.clone()).await {
        Ok(Some(ProtectedWriteConfirm { confirmed: true })) => Ok(()),
        Ok(Some(ProtectedWriteConfirm { confirmed: false })) | Ok(None) => {
            Err(McpError::invalid_params(
                "write into protected group cancelled",
                Some(json!({
                    "code": "protected_write_cancelled",
                    "group_slug": entry.manifest.slug,
                    "group_id": entry.manifest.group_id.to_string(),
                    "slug": slug,
                    "action": action,
                })),
            ))
        }
        Err(ElicitationError::UserDeclined) | Err(ElicitationError::UserCancelled) => {
            Err(McpError::invalid_params(
                "write into protected group cancelled",
                Some(json!({
                    "code": "protected_write_cancelled",
                    "group_slug": entry.manifest.slug,
                    "group_id": entry.manifest.group_id.to_string(),
                    "slug": slug,
                    "action": action,
                })),
            ))
        }
        Err(ElicitationError::CapabilityNotSupported) => {
            // Pre-elicitation fallback — the existing structured
            // error shape that CLI operators already know how to
            // round-trip through `mmcp import`.
            ensure_not_protected(entry, slug, action)
        }
        Err(other) => Err(McpError::internal_error(
            format!("elicitation failed: {other}"),
            Some(json!({ "code": "protected_elicitation_failed" })),
        )),
    }
}

/// FR-011: prompt the operator to choose how to resolve a
/// CLAUDE.md conflict via an elicitation request. Returns the
/// resolved `ConflictChoice` on success; pre-elicitation clients
/// fall back to the legacy `conflict_unresolved` structured error
/// so `on_conflict`-retry flows keep working.
async fn elicit_claude_conflict_choice(
    peer: &Peer<RoleServer>,
    state: crate::commands::claude::FileState,
) -> Result<crate::commands::claude::ConflictChoice, McpError> {
    let message = format!(
        "CLAUDE.md is {state}. Choose how to proceed: `override` (overwrite without backup), `backup_override` (write .bak, then overwrite), or `cancel`.",
        state = state.as_wire_str(),
    );
    match peer.elicit::<ClaudeConflictPrompt>(message).await {
        Ok(Some(ClaudeConflictPrompt { choice })) => match choice.as_str() {
            "override" => Ok(crate::commands::claude::ConflictChoice::Override),
            "backup_override" => Ok(crate::commands::claude::ConflictChoice::BackupOverride),
            "cancel" => Ok(crate::commands::claude::ConflictChoice::Cancel),
            other => Err(McpError::invalid_params(
                format!("unknown conflict choice `{other}`"),
                Some(json!({
                    "code": "conflict_invalid_choice",
                    "got": other,
                    "allowed": ["override", "backup_override", "cancel"],
                })),
            )),
        },
        // No content on an accepted response, or an explicit cancel
        // / decline, all map to the same "don't touch the file"
        // outcome so the downstream logic stays single-branch.
        Ok(None) | Err(ElicitationError::UserDeclined) | Err(ElicitationError::UserCancelled) => {
            Ok(crate::commands::claude::ConflictChoice::Cancel)
        }
        Err(ElicitationError::CapabilityNotSupported) => Err(McpError::invalid_params(
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
        )),
        Err(other) => Err(McpError::internal_error(
            format!("elicitation failed: {other}"),
            Some(json!({ "code": "conflict_elicitation_failed" })),
        )),
    }
}

/// Map an [`ImportError`] coming from the memory CRUD primitives
/// onto an [`McpError`] with a structured `code` payload. The four
/// wire codes (`memory_not_found`, `memory_already_exists`,
/// `invalid_slug`, `memory_render_failed`) are stable wire contracts
/// the `edit_memory`, `delete_memory`, and tightened `write_memory`
/// tools all share.
/// Map a store-layer `ArchiveError` onto a typed MCP error with a
/// stable `code` payload so harnesses can branch on the failure mode.
fn map_archive_error_to_mcp(err: mmcp_store::ArchiveError) -> McpError {
    use mmcp_store::ArchiveError;
    let message = err.to_string();
    match err {
        // Per-memory replay failures carry their own structured codes
        // (invalid_slug, memory_already_exists, ...); keep them rather
        // than flattening into an opaque `archive_error`.
        ArchiveError::Import(inner) => map_memory_error_to_mcp(inner),
        ArchiveError::UnsupportedFormatVersion { found, supported } => McpError::invalid_params(
            message,
            Some(json!({
                "code": "unsupported_archive_format",
                "found": found,
                "supported": supported,
            })),
        ),
        ArchiveError::MissingManifest => {
            McpError::invalid_params(message, Some(json!({ "code": "missing_archive_manifest" })))
        }
        ArchiveError::GroupManifestMissing { group_id } => McpError::invalid_params(
            message,
            Some(json!({ "code": "group_manifest_missing", "group_id": group_id.to_string() })),
        ),
        ArchiveError::GroupManifestParse { group_id, .. } => McpError::invalid_params(
            message,
            Some(json!({ "code": "group_manifest_parse", "group_id": group_id.to_string() })),
        ),
        ArchiveError::ProtectedGroup { group_id, slug } => McpError::invalid_params(
            message,
            Some(json!({
                "code": "protected_group",
                "group_id": group_id.to_string(),
                "slug": slug,
            })),
        ),
        ArchiveError::IntoGroupNotFound(group) => McpError::invalid_params(
            message,
            Some(json!({ "code": "into_group_not_found", "group": group })),
        ),
        ArchiveError::ManifestParse(_) => {
            McpError::invalid_params(message, Some(json!({ "code": "malformed_archive" })))
        }
        ArchiveError::Malformed { .. } => {
            McpError::invalid_params(message, Some(json!({ "code": "malformed_archive" })))
        }
        ArchiveError::NotUtf8 { .. } => {
            McpError::invalid_params(message, Some(json!({ "code": "archive_not_utf8" })))
        }
        _ => McpError::internal_error(message, Some(json!({ "code": "archive_error" }))),
    }
}

fn map_memory_error_to_mcp(err: ImportError) -> McpError {
    let message = err.to_string();
    let payload = match &err {
        ImportError::MemoryNotFound { slug, id } => json!({
            "code": "memory_not_found",
            "slug": slug,
            "id": id.map(|u| u.to_string()),
        }),
        ImportError::MemoryAlreadyExists { slug } => json!({
            "code": "memory_already_exists",
            "slug": slug,
            "retry_hint": "use edit_memory to update in place, delete_memory to remove, or pass override: true to replace",
        }),
        ImportError::MemoryAmbiguous { slug, candidates } => json!({
            "code": "memory_ambiguous",
            "slug": slug,
            "candidates": candidates.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
            "retry_hint": "pass an explicit id to disambiguate",
        }),
        ImportError::MemoryIdMismatch { slug, expected, got } => json!({
            "code": "memory_id_mismatch",
            "slug": slug,
            "expected": expected.to_string(),
            "got": got.to_string(),
        }),
        ImportError::ResolveArgsMissing => json!({
            "code": "resolve_args_missing",
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
        ImportError::IdMismatchOnFilenameWrite {
            path,
            filename,
            frontmatter,
        } => json!({
            "code": "id_mismatch_on_filename_write",
            "path": path,
            "filename": filename.to_string(),
            "frontmatter": frontmatter.to_string(),
            "retry_hint": "pass force: true to override (filename UUID stays; the rejected frontmatter id is the new source of truth)",
        }),
    };
    McpError::invalid_params(message, Some(payload))
}

/// FR-026: map the section-applier's typed errors onto stable
/// MCP wire codes. Keeps the tool body terse and every error
/// path consistent across the `edit_memory_body` surface.
fn map_memory_edit_error_to_mcp(err: mmcp_store::MemoryEditError) -> McpError {
    use mmcp_store::MemoryEditError as E;
    let message = err.to_string();
    let payload = match &err {
        E::SectionNotFound { path } => json!({
            "code": "section_not_found",
            "path": path,
        }),
        E::MoveWouldLoop { target, anchor } => json!({
            "code": "move_would_loop",
            "target": target,
            "anchor": anchor,
        }),
        E::LevelOutOfRange { level } => json!({
            "code": "level_out_of_range",
            "level": level,
        }),
        E::InvalidLineRange {
            start,
            end,
            line_count,
        } => json!({
            "code": "invalid_line_range",
            "start": start,
            "end": end,
            "line_count": line_count,
        }),
        E::LinePastEof { line, line_count } => json!({
            "code": "line_past_eof",
            "line": line,
            "line_count": line_count,
        }),
        E::Parse(inner) => json!({
            "code": "body_parse_failed",
            "detail": inner.to_string(),
        }),
    };
    McpError::invalid_params(message, Some(payload))
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let mut instructions = SESSION_INSTRUCTIONS.to_string();
        if !matches!(self.mode, ServeMode::Full) {
            // Filtered modes drop tools at registration time, so an
            // assistant that calls a missing tool gets a generic
            // "method not found"; the suffix tells it up front
            // which surface is reachable on this connection.
            instructions.push_str(&format!(
                "\n\n## Active mode: {label}\n\nThis server was started with `--mode {label}`. \
                 Mutating tools may be unavailable; call `describe_tools` for the live catalogue.",
                label = self.mode.as_label(),
            ));
        }
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(instructions)
    }
}

/// Session-start protocol delivered to every MCP client on handshake.
///
/// This text is the authoritative reading order — CLAUDE.md points at
/// it rather than duplicating it. When the checkpoint list or tool
/// usage changes, update this constant; no other surface repeats the
/// protocol.
const SESSION_INSTRUCTIONS: &str = concat!(
    "REQUIRED FIRST ACTION: call `bootstrap_context` before answering the user ",
    "or invoking any other tool. The rules for this project live in mmcp ",
    "memories, NOT in this prompt and NOT in CLAUDE.md. Skipping this step ",
    "means working against stale rules. This directive runs on cold boot, ",
    "before any manifest has been fetched.\n\n",
    "mmcp memory server — the project's single source of truth for coding rules, ",
    "conventions, and project notes. Memories live in git repositories under ",
    "~/.mmcp/repos and are surfaced through typed MCP tools; never hand-edit TOML.\n\n",
    "## STOP — bootstrap returns instructions, not rules\n\n",
    "`bootstrap_context` carries NO memory metadata and NO bodies. It returns the ",
    "groups you are allowed to enumerate (`groups_in_scope`), the addresses your ",
    "project has subscribed to (`subscribed_reads`), and your current ",
    "subscriptions (`subscriptions_summary`). To find rules, call ",
    "`list_memories(group)` for every entry in `groups_in_scope`, decide which ",
    "entries are relevant — mandatory rules ALWAYS, plus context-relevant ",
    "non-mandatory ones — and call `read_memory(group, slug)` for each. Do not ",
    "write code, do not commit, do not answer the user's task until every ",
    "mandatory entry's BODY has been fetched.\n\n",
    "## Subscriptions: opt into non-mandatory memories per project\n\n",
    "Non-mandatory rules from in-scope groups are visible through ",
    "`list_memories` but the AI typically should not read them all. Instead, ",
    "when you find a non-mandatory memory that is relevant to this project, ",
    "subscribe to it once via `subscribe(kind=..., value=...)`. Subscribed ",
    "entries surface in `subscribed_reads` on the next bootstrap, so future ",
    "sessions skip the discovery step. Four axes:\n",
    "- `kind=tag`: any non-mandatory memory whose tags overlap surfaces.\n",
    "- `kind=memory`: a specific `<group_uuid>:<slug>` pin.\n",
    "- `kind=group`: every memory in the named group, even if the group ",
    "is otherwise out of scope (`mmcp` Shared groups).\n",
    "- `kind=language`: every memory in the matching `lang/<name>` group.\n",
    "Use `unsubscribe(kind=..., value=...)` to remove a pin.\n\n",
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

/// Enumerate every memory in the group at `HEAD`, returning one
/// entry per on-disk file. Delegates to the shared
/// `mmcp_store::list_all_memory_files` walker so this tool
/// surface, the GUI, and the diagnostics layer all agree on how
/// the two-level FR-028 layout + legacy flat fallback are
/// enumerated. Duplicate slugs surface as multiple entries with
/// distinct UUIDs — perfect for `list_memories` / search, where
/// each memory is its own row.
async fn list_memory_files(
    backend: &NativeBackend,
    entry: &GroupEntry,
) -> Result<Vec<mmcp_store::MemoryFileRef>, McpError> {
    mmcp_store::list_all_memory_files(backend, &entry.handle, &Rev::head())
        .await
        .map_err(git_error)
}

/// FR-41 path filter for `list_memories`. Returns `true` when
/// `slug` (the full slash-joined memory slug) belongs in a
/// listing constrained to `prefix` and the recursion mode.
///
/// `depth` is measured from the *anchor* — the prefix when one
/// is set, or the implicit `memories/` root when not. The anchor
/// itself sits at depth 0; a top-level slug like `feedback` is
/// depth 1 from the root, and one level below a prefix is depth
/// 1 from the prefix.
///
/// - `prefix = None, recursive = true` (default): every slug
///   matches.
/// - `prefix = None, recursive = false`: only top-level slugs
///   (no `/` separator) match.
/// - `prefix = Some("a/b"), recursive = true`: slugs that equal
///   `"a/b"` or live underneath it match.
/// - `prefix = Some("a/b"), recursive = false`: only the
///   immediate children of the prefix and the prefix itself
///   match (so `a/b`, `a/b/c` ok; `a/b/c/d` filtered out).
fn slug_matches_filter(slug: &str, prefix: Option<&str>, recursive: bool) -> bool {
    let depth = match prefix {
        None | Some("") | Some("/") => slug.split('/').count(),
        Some(p) => {
            if slug == p {
                0
            } else if let Some(rest) = slug.strip_prefix(p)
                && let Some(suffix) = rest.strip_prefix('/')
            {
                suffix.split('/').count()
            } else {
                return false;
            }
        }
    };
    if recursive { true } else { depth <= 1 }
}

/// Read one memory and return a compact descriptor including the
/// slug, the parsed frontmatter fields, and a short summary.
///
/// `path` is the resolved on-disk path under the group repo —
/// post-FR-028 that is `memories/<slug>/<uuid>.md`, but legacy
/// mirrors still keep `memories/<slug>.md`; callers supply
/// whichever the enumeration walker returned.
async fn read_memory_descriptor(
    backend: &NativeBackend,
    entry: &GroupEntry,
    path: &str,
    slug: &str,
    version: Option<&str>,
) -> Result<serde_json::Value, mmcp_git::GitError> {
    let rev = parse_rev(version);
    let bytes = backend.read_file(&entry.handle, path, &rev).await?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (name, description, kind, mandatory, version_str, tags, source) =
        match MemoryFile::parse(&text) {
            Ok(file) => (
                Some(file.frontmatter.name),
                Some(file.frontmatter.description),
                file.frontmatter.kind.as_str().to_string(),
                file.frontmatter.mandatory,
                file.frontmatter.version.map(|v| v.to_string()),
                file.frontmatter.tags,
                file.frontmatter.source,
            ),
            Err(_) => (
                None,
                None,
                "rule".to_string(),
                false,
                None,
                Vec::new(),
                None,
            ),
        };
    // FR-41: every descriptor carries a segmented `path` so
    // structure-aware consumers (GUIs that render trees, callers
    // that filter by path) work with a typed `Vec<String>` instead
    // of re-splitting on `/`. `slug` is the leaf-only identifier
    // (last path segment); the full structural location lives in
    // `path`. Callers that want the joined form do `path.join("/")`.
    let path: Vec<&str> = slug.split('/').filter(|s| !s.is_empty()).collect();
    let leaf = path.last().copied().unwrap_or(slug);
    Ok(json!({
        "group": entry.manifest.group_id,
        "slug": leaf,
        "path": path,
        "name": name,
        "description": description,
        "kind": kind,
        "mandatory": mandatory,
        "latest_version": version_str,
        "tags": tags,
        "source": source,
    }))
}

fn parse_group_id(value: &str) -> Result<GroupId, McpError> {
    let uuid = Uuid::parse_str(value).map_err(|_| {
        McpError::invalid_params("group is not a valid UUID", Some(json!({ "group": value })))
    })?;
    Ok(GroupId::from_uuid(uuid))
}

/// Pick a user-visible label for the protected-group guard when
/// the caller addressed the memory by `id` only (slug absent) or
/// by neither (the error is surfaced later). Kept here so every
/// tool that runs `confirm_protected_write` before resolution
/// picks the same fallback.
fn memory_label_for_guard(slug: Option<&str>, id: Option<&str>) -> String {
    slug.map(str::to_string)
        .or_else(|| id.map(str::to_string))
        .unwrap_or_else(|| "<unknown>".to_string())
}

/// Parse an optional UUID argument from the wire. Rejects malformed
/// strings with a typed `invalid_memory_id` code so the AI client
/// can surface a clean error instead of a generic parse failure.
fn parse_optional_uuid(value: Option<&str>) -> Result<Option<Uuid>, McpError> {
    match value {
        None => Ok(None),
        Some(s) => Uuid::parse_str(s).map(Some).map_err(|_| {
            McpError::invalid_params(
                "memory id is not a valid UUID",
                Some(json!({ "code": "invalid_memory_id", "id": s })),
            )
        }),
    }
}

/// FR-38: parse the `source` arg into a UUID. Disambiguation
/// between group UUID and memory UUID is the caller's concern at
/// lookup time — the wire shape is opaque.
fn parse_optional_source(value: Option<&str>) -> Result<Option<Uuid>, McpError> {
    match value {
        None => Ok(None),
        Some(s) => Uuid::parse_str(s).map(Some).map_err(|_| {
            McpError::invalid_params(
                "source is not a valid UUID",
                Some(json!({ "code": "invalid_source", "source": s })),
            )
        }),
    }
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

/// Compute notes about the project's CLAUDE.md state (FR-45).
///
/// Read-only: the function inspects the file on disk but never writes
/// anything. `bootstrap_context` emits these through the standard
/// notes channel so callers can surface actionable `init_claude`
/// hints alongside every other signal. An empty vector means either
/// no project root was resolved (nothing to diagnose) or the file is
/// already healthy.
fn claude_md_notes(project_root: Option<&std::path::Path>) -> Vec<mmcp_proto::Note> {
    let Some(root) = project_root else {
        return Vec::new();
    };
    let claude_md = root.join("CLAUDE.md");
    if !claude_md.exists() {
        return vec![mmcp_proto::Note::warn(
            "claude_md_missing",
            "CLAUDE.md is missing at the project root. Running `init_claude` (action=override) bootstraps it with the mmcp pointer template so future sessions see the checkpoint protocol.",
        )
        .with_context(json!({
            "suggested_tool": "init_claude",
            "suggested_args": { "action": "override" },
        }))];
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
        return vec![mmcp_proto::Note::info(
            "claude_md_stale",
            format!(
                "CLAUDE.md carries an older mmcp block; current version is {CLAUDE_MD_BLOCK_VERSION}. Re-run `init_claude` (action=append) to upgrade the fenced region in place."
            ),
        )
        .with_context(json!({
            "suggested_tool": "init_claude",
            "suggested_args": { "action": "append" },
        }))];
    }
    // No fence at all — file is unmanaged.
    vec![mmcp_proto::Note::warn(
        "claude_md_unmanaged",
        "CLAUDE.md has no mmcp-managed block. Run `init_claude` (action=append) to insert the session-start protocol without touching user-authored content, or (action=convert) to split existing rule content into typed memories and replace the file with a stub.",
    )
    .with_context(json!({
        "suggested_tool": "init_claude",
        "suggested_args": { "action": "append" },
    }))]
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
        "refs": fm.refs.iter().map(memory_ref_to_json).collect::<Vec<_>>(),
        // FR-38: surface the provenance UUID on read so callers can
        // see who filed a memory without parsing the body.
        "source": fm.source,
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

/// FR-45 notes channel: wrap a JSON response payload and attach a
/// `notes` array when non-empty. `ok_json` stays for call sites that
/// never emit notes; tools that can emit them use this helper and
/// omit the field entirely when the queue is empty (via
/// `skip_serializing_if`-equivalent behaviour here: absent entry
/// instead of `"notes": []`).
///
/// Returns the same `CallToolResult` shape as `ok_json`, so callers
/// swap one for the other without changing their return type.
fn ok_json_with_notes(
    mut value: serde_json::Value,
    notes: Vec<mmcp_proto::Note>,
) -> CallToolResult {
    if !notes.is_empty() {
        if let Some(obj) = value.as_object_mut() {
            let serialised = serde_json::to_value(&notes).unwrap_or(json!([]));
            obj.insert("notes".to_string(), serialised);
        }
    }
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    CallToolResult::success(vec![Content::text(Cow::Owned(text))])
}


// `id_validation_to_notes` was hoisted to `crate::notes` so
// the CLI memory subcommands can reuse the same FR-45 codes.
// See `notes::id_validation_to_notes`.

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
        let home = MmcpHome::from_root(tmp.path().join("mmcp-home"));
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
        seed_group_with_memory_inner(
            state,
            slug,
            memory_slug,
            memory_body,
            false,
            mmcp_core::manifest::GroupScope::Project,
        )
        .await
    }

    /// Same as [`seed_group_with_memory`] but marks the group's
    /// manifest as `protected`, so tests can exercise the
    /// FR-019 guard.
    async fn seed_protected_group_with_memory(
        state: &ClientState,
        slug: &str,
        memory_slug: &str,
        memory_body: &str,
    ) -> GroupId {
        seed_group_with_memory_inner(
            state,
            slug,
            memory_slug,
            memory_body,
            true,
            mmcp_core::manifest::GroupScope::Project,
        )
        .await
    }

    /// Seed a group whose manifest carries an explicit
    /// [`GroupScope`](mmcp_core::manifest::GroupScope) so tests can
    /// exercise the FR-025 mandatory-memory filter without hand-
    /// rolling the manifest mutation.
    async fn seed_scoped_group_with_memory(
        state: &ClientState,
        slug: &str,
        memory_slug: &str,
        memory_body: &str,
        scope: mmcp_core::manifest::GroupScope,
    ) -> GroupId {
        seed_group_with_memory_inner(state, slug, memory_slug, memory_body, false, scope).await
    }

    async fn seed_group_with_memory_inner(
        state: &ClientState,
        slug: &str,
        memory_slug: &str,
        memory_body: &str,
        protected: bool,
        scope: mmcp_core::manifest::GroupScope,
    ) -> GroupId {
        let owner = Uuid::now_v7();
        let group_id = GroupId::new();
        let mut manifest = GroupManifest::new_user_owned(group_id, slug, owner);
        manifest.protected = protected;
        manifest.scope = scope;
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
                        mmcp_core::conventions::memory_path(memory_slug, Uuid::now_v7()),
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
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: group.to_string(),
                ..Default::default()
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
        assert_eq!(
            parsed.get("mirrored").and_then(|v| v.as_bool()),
            Some(true),
            "mirrored should be true for a group that exists in the local mirror",
        );
    }

    #[tokio::test]
    async fn list_features_response_omits_body() {
        // FR-048: list-style surfaces return body-free summaries.
        // Seed two FRs with bodies large enough that any accidental
        // inlining would balloon the response, then assert the
        // wire response carries metadata-only entries and stays
        // well under the body size used as a token-cap proxy.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "fr-listing", "seed-only", SAMPLE_MEMORY).await;
        let entry = state.groups.get(&group).await.expect("group entry");

        let big_body = "x".repeat(4_096);
        for (slug, number) in [("alpha", 1u32), ("beta", 2u32)] {
            let spec = mmcp_store::features::AddSpec {
                slug: Some(slug.into()),
                title: format!("Title {slug}"),
                description: format!("Desc {slug}"),
                body: big_body.clone(),
                number: Some(number),
                ..mmcp_store::features::AddSpec::default()
            };
            mmcp_store::features::add_feature(&state.backend, &entry, spec, &state.author)
                .await
                .expect("seed feature");
        }
        state.groups.refresh().await.expect("refresh");

        let server = McpServer::new(state, ServeMode::Full);
        let res = server
            .list_features(Parameters(ListFeaturesArgs {
                project: Some(group.to_string()),
                status: None,
                all: Some(true),
            }))
            .await
            .expect("list_features");
        let parsed = parse_ok_json(res);

        let features = parsed
            .get("features")
            .and_then(|v| v.as_array())
            .expect("features array");
        assert_eq!(features.len(), 2);

        for feature in features {
            assert!(
                feature.get("body").is_none(),
                "list_features must not inline bodies; found body on {feature:?}",
            );
            assert!(feature.get("slug").and_then(|v| v.as_str()).is_some());
            assert!(feature.get("title").and_then(|v| v.as_str()).is_some());
            assert!(
                feature
                    .get("description")
                    .and_then(|v| v.as_str())
                    .is_some()
            );
            assert!(feature.get("status").and_then(|v| v.as_str()).is_some());
            assert!(feature.get("number").is_some());
            assert!(feature.get("commit_id").is_some());
        }

        let serialized = serde_json::to_string(&parsed).expect("serialize");
        assert!(
            serialized.len() < big_body.len(),
            "list_features response ({} bytes) must stay much smaller than a single FR body ({} bytes)",
            serialized.len(),
            big_body.len(),
        );
    }

    #[tokio::test]
    async fn list_memories_reports_mirrored_false_for_unknown_group() {
        // FR-013: a group whose UUID is not in the local mirror must
        // return `mirrored: false` so AI callers distinguish
        // "empty-mirrored" from "never-pulled" without a second tool
        // call. Regression test for the pre-fix behaviour that
        // returned `{memories: []}` indistinguishable from an empty
        // mirrored group, silently masking missed checkpoint reads.
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let phantom_uuid = Uuid::now_v7().to_string();

        let res = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: phantom_uuid.clone(),
                ..Default::default()
            }))
            .await
            .expect("list_memories on unknown group should not error");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("mirrored").and_then(|v| v.as_bool()),
            Some(false),
            "unknown group must surface mirrored=false",
        );
        assert_eq!(
            parsed
                .get("memories")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(0),
            "unknown group still returns an empty memories array",
        );
    }

    #[tokio::test]
    async fn list_groups_returns_every_mirrored_group_with_metadata() {
        // FR-010: verify the standalone enumeration path returns
        // every seeded group, with manifest + memory counts, in a
        // single cheap call — no need to drive `bootstrap_context`
        // just to discover what is mirrored.
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        seed_group_with_memory(&state, "team-rust-2", "other", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .list_groups(Parameters(ListGroupsArgs::default()))
            .await
            .expect("list_groups");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("count").and_then(|v| v.as_u64()),
            Some(2),
            "count must match seeded group cardinality",
        );
        let groups = parsed
            .get("groups")
            .and_then(|v| v.as_array())
            .expect("groups array");
        let slugs: std::collections::HashSet<&str> = groups
            .iter()
            .filter_map(|g| g.get("slug").and_then(|v| v.as_str()))
            .collect();
        assert!(slugs.contains("team-rust"));
        assert!(slugs.contains("team-rust-2"));
        for group in groups {
            assert_eq!(
                group.get("memory_count").and_then(|v| v.as_u64()),
                Some(1),
                "each seeded group has exactly one memory",
            );
            assert!(
                group.get("uuid").and_then(|v| v.as_str()).is_some(),
                "uuid field must be populated",
            );
            assert_eq!(
                group.get("protected").and_then(|v| v.as_bool()),
                Some(false),
                "seeded groups are unprotected by default",
            );
            assert_eq!(
                group.get("is_project").and_then(|v| v.as_bool()),
                Some(false),
                "no `.mmcp.toml` in scope in tests — is_project is false",
            );
        }
    }

    #[tokio::test]
    async fn read_memory_returns_frontmatter_and_body() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: Some("rules".into()),
                id: None,
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
        let server = McpServer::new(state, ServeMode::Full);

        let err = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: Some("missing".into()),
                id: None,
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
        let server = McpServer::new(state, ServeMode::Full);

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
        assert_eq!(parsed.get("memory_count").and_then(|v| v.as_u64()), Some(1));
        let owner_kind = parsed
            .get("owner")
            .and_then(|v| v.get("kind"))
            .and_then(|v| v.as_str());
        assert_eq!(owner_kind, Some("user"));
    }

    #[tokio::test]
    async fn search_memories_group_filter_restricts_to_matching_group() {
        // Pins the per-group filter: both groups hold a slug
        // matching the query, but only one is listed after
        // `group` narrows the search set.
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "team-rust", "coding-rules", SAMPLE_MEMORY).await;
        seed_group_with_memory(&state, "team-python", "coding-habits", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .search_memories(Parameters(SearchMemoriesArgs {
                query: Some("coding".into()),
                queries: None,
                limit: None,
                group: Some("team-rust".into()),
                scope: None,
            }))
            .await
            .expect("search_memories");
        let parsed = parse_ok_json(res);
        let hits = parsed
            .get("hits")
            .and_then(|v| v.as_array())
            .expect("hits array");
        assert_eq!(
            hits.len(),
            1,
            "group filter must restrict search to the matching group; got: {hits:?}"
        );
        assert_eq!(
            hits[0].get("slug").and_then(|v| v.as_str()),
            Some("coding-rules")
        );
    }

    #[tokio::test]
    async fn search_memories_scope_filter_restricts_to_matching_scope() {
        let (state, _tmp) = test_state().await;
        seed_scoped_group_with_memory(
            &state,
            "global",
            "coding-rules",
            SAMPLE_MEMORY,
            mmcp_core::manifest::GroupScope::Global,
        )
        .await;
        seed_scoped_group_with_memory(
            &state,
            "team-project",
            "coding-habits",
            SAMPLE_MEMORY,
            mmcp_core::manifest::GroupScope::Project,
        )
        .await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .search_memories(Parameters(SearchMemoriesArgs {
                query: Some("coding".into()),
                queries: None,
                limit: None,
                group: None,
                scope: Some(ToolGroupScope::Global),
            }))
            .await
            .expect("search_memories");
        let parsed = parse_ok_json(res);
        let hits = parsed
            .get("hits")
            .and_then(|v| v.as_array())
            .expect("hits array");
        assert_eq!(
            hits.len(),
            1,
            "scope filter must restrict to matching GroupScope; got: {hits:?}"
        );
    }

    #[tokio::test]
    async fn search_memories_matches_slug_substring() {
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "team-rust", "coding-rules", SAMPLE_MEMORY).await;
        seed_group_with_memory(&state, "team-python", "style-guide", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .search_memories(Parameters(SearchMemoriesArgs {
                query: Some("coding".into()),
                queries: None,
                limit: None,
                group: None,
                scope: None,
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

    /// FR-43: multi-query form. Two overlapping queries hit the
    /// same memory; assert dedup-by-UUID (one row, not two) and the
    /// `matched_queries` list captures every input that hit it in
    /// caller order.
    #[tokio::test]
    async fn search_memories_multi_query_dedupes_by_memory() {
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "team-rust", "coding-rules", SAMPLE_MEMORY).await;
        seed_group_with_memory(&state, "team-python", "style-guide", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .search_memories(Parameters(SearchMemoriesArgs {
                query: None,
                queries: Some(vec!["coding".into(), "rules".into()]),
                limit: None,
                group: None,
                scope: None,
            }))
            .await
            .expect("multi-query search");
        let parsed = parse_ok_json(res);
        let hits = parsed
            .get("hits")
            .and_then(|v| v.as_array())
            .expect("hits array");
        assert_eq!(
            hits.len(),
            1,
            "two overlapping queries must dedupe to one row; got: {hits:?}",
        );
        let entry = &hits[0];
        let memory = entry.get("memory").expect("multi-mode wraps under `memory`");
        assert_eq!(
            memory.get("slug").and_then(|v| v.as_str()),
            Some("coding-rules")
        );
        let matched: Vec<&str> = entry
            .get("matched_queries")
            .and_then(|v| v.as_array())
            .expect("matched_queries array")
            .iter()
            .filter_map(|s| s.as_str())
            .collect();
        assert_eq!(matched, vec!["coding", "rules"]);
    }

    /// FR-43: a non-matching needle does not pollute `matched_queries`
    /// for hits surfaced by other needles.
    #[tokio::test]
    async fn search_memories_multi_query_lists_only_actual_matches() {
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "team-rust", "coding-rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .search_memories(Parameters(SearchMemoriesArgs {
                query: None,
                queries: Some(vec!["coding".into(), "no-such-thing".into()]),
                limit: None,
                group: None,
                scope: None,
            }))
            .await
            .expect("multi-query search");
        let parsed = parse_ok_json(res);
        let hits = parsed.get("hits").and_then(|v| v.as_array()).unwrap();
        assert_eq!(hits.len(), 1);
        let matched: Vec<&str> = hits[0]
            .get("matched_queries")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .filter_map(|s| s.as_str())
            .collect();
        assert_eq!(matched, vec!["coding"]);
    }

    /// FR-43: passing both `query` and `queries` must fail up front
    /// with a structured `invalid_search_args` code rather than
    /// silently picking one shape.
    #[tokio::test]
    async fn search_memories_rejects_both_query_and_queries() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);

        let err = server
            .search_memories(Parameters(SearchMemoriesArgs {
                query: Some("a".into()),
                queries: Some(vec!["b".into()]),
                limit: None,
                group: None,
                scope: None,
            }))
            .await
            .expect_err("both forms must error");
        let payload = err.data.as_ref().expect("error payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_search_args"),
        );
    }

    /// FR-43: passing neither errors with the same structured code.
    #[tokio::test]
    async fn search_memories_requires_at_least_one_query_form() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);

        let err = server
            .search_memories(Parameters(SearchMemoriesArgs {
                query: None,
                queries: None,
                limit: None,
                group: None,
                scope: None,
            }))
            .await
            .expect_err("no query must error");
        let payload = err.data.as_ref().expect("error payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_search_args"),
        );
    }

    /// FR-43: `limit` caps total deduped hits across all queries,
    /// not per-query. Three memories, two queries that all match,
    /// limit 2 → exactly two rows.
    #[tokio::test]
    async fn search_memories_multi_query_limit_caps_total() {
        let (state, _tmp) = test_state().await;
        seed_group_with_memory(&state, "team-rust", "coding-rules-a", SAMPLE_MEMORY).await;
        seed_group_with_memory(&state, "team-rust", "coding-rules-b", SAMPLE_MEMORY).await;
        seed_group_with_memory(&state, "team-rust", "coding-rules-c", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .search_memories(Parameters(SearchMemoriesArgs {
                query: None,
                queries: Some(vec!["coding".into(), "rules".into()]),
                limit: Some(2),
                group: None,
                scope: None,
            }))
            .await
            .expect("multi-query search");
        let parsed = parse_ok_json(res);
        let hits = parsed.get("hits").and_then(|v| v.as_array()).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[tokio::test]
    async fn list_versions_returns_commit_history_for_memory() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

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
        let server = McpServer::new(state, ServeMode::Full);
        let unknown = Uuid::now_v7();

        let res = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: unknown.to_string(),
                ..Default::default()
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
    async fn bootstrap_context_returns_instruction_only_no_memory_metadata() {
        // The post-slice-2c shape carries NO `memories` field and
        // NO per-entry metadata. It surfaces only:
        //   - `instructions`: SESSION_INSTRUCTIONS preamble.
        //   - `next_action.groups_in_scope`: addresses to enumerate.
        //   - `next_action.subscribed_reads`: pinned addresses.
        //   - `next_action.subscriptions_summary`: current subs.
        // The AI fetches metadata via `list_memories(group)`.
        let (state, tmp) = test_state().await;
        let global = seed_scoped_group_with_memory(
            &state,
            "global",
            "mandatory-rule",
            MANDATORY_MEMORY,
            mmcp_core::manifest::GroupScope::Global,
        )
        .await;
        seed_group_with_memory(&state, "globals2", "optional-note", OPTIONAL_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        // Pass `path` to a tempdir with no `.mmcp.toml` so the
        // resolver does not pick up the workspace config and skew
        // `subscribed_reads`.
        let no_project_dir = tmp.path().join("scratch");
        std::fs::create_dir_all(&no_project_dir).expect("scratch");
        let res = server
            .bootstrap_context(Parameters(BootstrapContextArgs {
                project: None,
                path: Some(no_project_dir.to_string_lossy().into_owned()),
            }))
            .await
            .expect("bootstrap_context");
        let parsed = parse_ok_json(res);

        // No metadata leakage at the top level.
        assert!(
            parsed.get("memories").is_none(),
            "bootstrap response must not carry a `memories` array",
        );

        let next_action = parsed
            .get("next_action")
            .expect("next_action field present");
        let groups_in_scope = next_action
            .get("groups_in_scope")
            .and_then(|v| v.as_array())
            .expect("groups_in_scope array");
        assert!(
            groups_in_scope
                .iter()
                .any(|g| g.get("uuid").and_then(|v| v.as_str()) == Some(&global.to_string())),
            "Global-scoped seed must appear in groups_in_scope; saw: {groups_in_scope:?}",
        );
        for entry in groups_in_scope {
            // Each entry is addresses + scope only — no name, no
            // mandatory flag, no tags. Discovery happens via
            // list_memories.
            assert!(entry.get("name").is_none());
            assert!(entry.get("mandatory").is_none());
            assert!(entry.get("tags").is_none());
        }

        // No project config → no subscriptions to honor.
        let subscribed = next_action
            .get("subscribed_reads")
            .and_then(|v| v.as_array())
            .expect("subscribed_reads array");
        assert!(subscribed.is_empty());

        // Imperatives present so AIs cannot mistake the response
        // for a manifest.
        assert!(next_action.get("imperative_mandatory").is_some());
        assert!(next_action.get("imperative_optional").is_some());

        let instructions = parsed
            .get("instructions")
            .and_then(|v| v.as_str())
            .expect("instructions string");
        assert!(
            instructions.contains("Session-start protocol"),
            "instructions must carry the session protocol preamble",
        );
        assert!(
            instructions.contains("STOP"),
            "instructions must lead with the STOP imperative so callers can't skim past it",
        );
        assert!(
            instructions.contains("Subscriptions:"),
            "instructions must document the subscriptions opt-in surface",
        );
    }

    #[tokio::test]
    async fn bootstrap_context_groups_in_scope_includes_global_when_no_project_configured() {
        // Global is always in scope, regardless of whether a
        // .mmcp.toml is present. A Project-scoped seed for an
        // unrelated project must NOT leak into groups_in_scope.
        let (state, _tmp) = test_state().await;
        let global = seed_scoped_group_with_memory(
            &state,
            "global",
            "rule-one",
            MANDATORY_MEMORY,
            mmcp_core::manifest::GroupScope::Global,
        )
        .await;
        let unrelated_project =
            seed_group_with_memory(&state, "team-project", "optional", OPTIONAL_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .bootstrap_context(Parameters(BootstrapContextArgs {
                project: None,
                path: None,
            }))
            .await
            .expect("bootstrap_context");
        let parsed = parse_ok_json(res);
        let groups_in_scope = parsed
            .pointer("/next_action/groups_in_scope")
            .and_then(|v| v.as_array())
            .expect("groups_in_scope array");
        let scoped_uuids: Vec<&str> = groups_in_scope
            .iter()
            .filter_map(|g| g.get("uuid").and_then(|v| v.as_str()))
            .collect();
        assert!(
            scoped_uuids
                .iter()
                .any(|u| *u == global.to_string()),
            "Global must always be in scope; saw: {scoped_uuids:?}",
        );
        assert!(
            !scoped_uuids
                .iter()
                .any(|u| *u == unrelated_project.to_string()),
            "unrelated Project-scoped group must NOT leak; saw: {scoped_uuids:?}",
        );
    }

    #[tokio::test]
    async fn unrelated_project_group_stays_out_of_scope() {
        // FR-025 regression guard. The unrelated project group's
        // mandatory memories were the historical leak source; under
        // the instruction-only shape the equivalent guard is that
        // the group itself never appears in groups_in_scope.
        let (state, _tmp) = test_state().await;
        let global = seed_scoped_group_with_memory(
            &state,
            "global",
            "global-rule",
            MANDATORY_MEMORY,
            mmcp_core::manifest::GroupScope::Global,
        )
        .await;
        let unrelated = seed_scoped_group_with_memory(
            &state,
            "gitoxide-like",
            "project-rule",
            MANDATORY_MEMORY,
            mmcp_core::manifest::GroupScope::Project,
        )
        .await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .bootstrap_context(Parameters(BootstrapContextArgs {
                project: None,
                path: None,
            }))
            .await
            .expect("bootstrap_context");
        let parsed = parse_ok_json(res);
        let scoped_uuids: Vec<&str> = parsed
            .pointer("/next_action/groups_in_scope")
            .and_then(|v| v.as_array())
            .expect("groups_in_scope array")
            .iter()
            .filter_map(|g| g.get("uuid").and_then(|v| v.as_str()))
            .collect();
        assert!(
            scoped_uuids.iter().any(|u| *u == global.to_string()),
            "Global must be in scope",
        );
        assert!(
            !scoped_uuids.iter().any(|u| *u == unrelated.to_string()),
            "unrelated Project-scoped group must stay out of scope; saw: {scoped_uuids:?}",
        );
    }

    #[tokio::test]
    async fn shared_group_enters_scope_only_when_project_subscribes() {
        // FR-025 + slice 2c: a Shared-scoped group reaches the
        // session only when the project's `.mmcp.toml` lists it in
        // `subscriptions.groups` (or via `subscriptions.languages`
        // for `lang/<x>` groups). Pre-subscribe the group is
        // absent from `groups_in_scope` AND from `subscribed_reads`;
        // post-subscribe both fields surface it.
        let (state, tmp) = test_state().await;
        let shared_group = seed_scoped_group_with_memory(
            &state,
            "team/house-rules",
            "team-rule",
            MANDATORY_MEMORY,
            mmcp_core::manifest::GroupScope::Shared,
        )
        .await;
        let server = McpServer::new(state, ServeMode::Full);

        // No project config in the project root → Shared group is
        // not in scope. Pass `path` explicitly to avoid racing on
        // process cwd against parallel tests.
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("mkdir project root");
        let path_str = project_root.to_string_lossy().into_owned();
        let res = server
            .bootstrap_context(Parameters(BootstrapContextArgs {
                project: None,
                path: Some(path_str.clone()),
            }))
            .await
            .expect("bootstrap_context without subscription");
        let parsed = parse_ok_json(res);
        let scoped_uuids: Vec<String> = parsed
            .pointer("/next_action/groups_in_scope")
            .and_then(|v| v.as_array())
            .expect("groups_in_scope")
            .iter()
            .filter_map(|g| g.get("uuid").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        assert!(
            !scoped_uuids.contains(&shared_group.to_string()),
            "Shared group must not be in scope pre-subscribe; saw: {scoped_uuids:?}",
        );

        // Subscribe the project to the Shared group and reconfirm.
        let toml_body = format!(
            "project_uuid = \"{}\"\n\n[subscriptions]\ngroups = [\"team/house-rules\"]\n",
            Uuid::now_v7()
        );
        std::fs::write(project_root.join(".mmcp.toml"), toml_body)
            .expect("seed project .mmcp.toml");
        let res = server
            .bootstrap_context(Parameters(BootstrapContextArgs {
                project: None,
                path: Some(path_str),
            }))
            .await
            .expect("bootstrap_context after subscription");
        let parsed = parse_ok_json(res);
        let scoped_uuids: Vec<String> = parsed
            .pointer("/next_action/groups_in_scope")
            .and_then(|v| v.as_array())
            .expect("groups_in_scope")
            .iter()
            .filter_map(|g| g.get("uuid").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        assert!(
            scoped_uuids.contains(&shared_group.to_string()),
            "subscribed Shared group must enter scope; saw: {scoped_uuids:?}",
        );
        // Full-group subscriptions surface every memory address in
        // `subscribed_reads`. The Shared group's `team-rule` must
        // appear there.
        let subscribed: Vec<(String, String)> = parsed
            .pointer("/next_action/subscribed_reads")
            .and_then(|v| v.as_array())
            .expect("subscribed_reads")
            .iter()
            .filter_map(|s| {
                Some((
                    s.get("group").and_then(|v| v.as_str())?.to_string(),
                    s.get("slug").and_then(|v| v.as_str())?.to_string(),
                ))
            })
            .collect();
        assert!(
            subscribed.contains(&(shared_group.to_string(), "team-rule".to_string())),
            "subscribed group's memory must appear in subscribed_reads; saw: {subscribed:?}",
        );
    }

    #[test]
    fn claude_md_notes_flags_missing_file() {
        let tmp = TempDir::new().expect("tempdir");
        let notes = claude_md_notes(Some(tmp.path()));
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].code, "claude_md_missing");
        assert_eq!(notes[0].level, mmcp_proto::NoteLevel::Warn);
    }

    #[test]
    fn claude_md_notes_flags_unmanaged_file() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(
            tmp.path().join("CLAUDE.md"),
            "# Legacy\n\nHand-authored without any mmcp fence.\n",
        )
        .expect("write claude");
        let notes = claude_md_notes(Some(tmp.path()));
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].code, "claude_md_unmanaged");
        assert_eq!(notes[0].level, mmcp_proto::NoteLevel::Warn);
    }

    #[test]
    fn claude_md_notes_is_silent_when_fence_matches_current_version() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(
            tmp.path().join("CLAUDE.md"),
            format!("# Managed\n\n<!-- mmcp:begin {CLAUDE_MD_BLOCK_VERSION} -->\n...\n<!-- mmcp:end {CLAUDE_MD_BLOCK_VERSION} -->\n"),
        )
        .expect("write claude");
        let notes = claude_md_notes(Some(tmp.path()));
        assert!(
            notes.is_empty(),
            "current-version fence should produce no notes"
        );
    }

    #[test]
    fn claude_md_notes_is_silent_without_project_root() {
        assert!(claude_md_notes(None).is_empty());
    }

    #[test]
    fn finding_to_note_maps_severity_and_carries_group_slug_context() {
        use crate::notes::finding_to_note;
        use mmcp_store::diagnostics::Finding;

        let warn = finding_to_note(&Finding {
            group: "g-uuid".to_string(),
            slug: Some("rules".to_string()),
            severity: "warning",
            code: "memory_body_empty",
            message: "body is empty".to_string(),
        });
        assert_eq!(warn.level, mmcp_proto::NoteLevel::Warn);
        assert_eq!(warn.code, "memory_body_empty");
        assert_eq!(warn.message, "body is empty");
        let ctx = warn.context.expect("warn context present");
        assert_eq!(ctx.get("group").and_then(|v| v.as_str()), Some("g-uuid"));
        assert_eq!(ctx.get("slug").and_then(|v| v.as_str()), Some("rules"));

        let err = finding_to_note(&Finding {
            group: "g-uuid".to_string(),
            slug: None,
            severity: "error",
            code: "manifest_unreadable",
            message: "manifest unreadable: io".to_string(),
        });
        assert_eq!(err.level, mmcp_proto::NoteLevel::Error);
        let ctx = err.context.expect("err context present");
        assert!(
            ctx.get("slug").is_none(),
            "slug field must be absent when Finding.slug is None; got: {ctx}",
        );

        let info = finding_to_note(&Finding {
            group: "(project)".to_string(),
            slug: None,
            severity: "info",
            code: "memory_no_tags",
            message: "hint".to_string(),
        });
        assert_eq!(info.level, mmcp_proto::NoteLevel::Info);
    }

    #[tokio::test]
    async fn init_claude_dry_run_override_against_missing_file_reports_plan() {
        let (state, tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let target = tmp.path().join("CLAUDE.md");

        let res = server
            .init_claude_unguarded(InitClaudeArgs {
                action: InitClaudeAction::Override,
                backup: None,
                dry_run: true,
                on_conflict: None,
                path: Some(target.to_string_lossy().into_owned()),
            })
            .await
            .expect("init_claude dry_run");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("action").and_then(|v| v.as_str()),
            Some("override")
        );
        assert_eq!(
            parsed.get("state_before").and_then(|v| v.as_str()),
            Some("missing")
        );
        assert_eq!(parsed.get("dry_run").and_then(|v| v.as_bool()), Some(true));
        assert!(parsed.get("wrote").map(|v| v.is_null()).unwrap_or(false));
        assert!(!target.exists(), "dry run must not write the file");
    }

    #[tokio::test]
    async fn init_claude_refuses_dirty_file_without_on_conflict_with_structured_error() {
        let (state, tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let target = tmp.path().join("CLAUDE.md");
        std::fs::write(&target, "# existing\n").expect("write fixture");

        let err = server
            .init_claude_unguarded(InitClaudeArgs {
                action: InitClaudeAction::Override,
                backup: None,
                dry_run: false,
                on_conflict: None,
                path: Some(target.to_string_lossy().into_owned()),
            })
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
        assert!(
            payload.get("choices").is_some(),
            "choices list must be present"
        );
    }

    #[tokio::test]
    async fn init_claude_writes_stub_when_file_is_missing() {
        let (state, tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let target = tmp.path().join("CLAUDE.md");

        let res = server
            .init_claude_unguarded(InitClaudeArgs {
                action: InitClaudeAction::Override,
                backup: None,
                dry_run: false,
                on_conflict: None,
                path: Some(target.to_string_lossy().into_owned()),
            })
            .await
            .expect("init_claude override");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("action").and_then(|v| v.as_str()),
            Some("override")
        );
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
        assert!(
            payload.get("retry_hint").is_some(),
            "retry_hint must be present"
        );
    }

    #[test]
    fn resolve_sync_config_returns_server_url_on_happy_path() {
        let tmp = TempDir::new().expect("tempdir");
        write_project_config(
            tmp.path(),
            "project_uuid = \"018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91\"\n\n[sync]\nserver_url = \"http://localhost:8787\"\n",
        );
        let (cfg, server_url) = resolve_sync_config(tmp.path()).expect("happy path should resolve");
        assert_eq!(server_url, "http://localhost:8787");
        assert_eq!(
            cfg.project_uuid.to_string(),
            "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"
        );
    }

    #[tokio::test]
    async fn resolve_sync_filter_rejects_empty_selector_with_selector_required() {
        let (state, _tmp) = test_state().await;
        let args = SyncToolArgs::default();
        let err = resolve_sync_filter(&args, &state.groups)
            .await
            .expect_err("empty selector must error");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("selector_required"),
            "empty selector must map to selector_required"
        );
        let accepted = payload
            .get("accepted")
            .and_then(|v| v.as_array())
            .expect("accepted array");
        assert_eq!(accepted.len(), 3, "three accepted selectors listed");
    }

    #[tokio::test]
    async fn resolve_sync_filter_rejects_multiple_selectors_with_selector_conflict() {
        let (state, _tmp) = test_state().await;
        let args = SyncToolArgs {
            group: Some("team-rust".into()),
            scope: None,
            all: Some(true),
        };
        let err = resolve_sync_filter(&args, &state.groups)
            .await
            .expect_err("group+all must conflict");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("selector_conflict")
        );
        let provided = payload
            .get("provided")
            .and_then(|v| v.as_array())
            .expect("provided array");
        let provided_strs: Vec<&str> = provided.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            provided_strs.contains(&"group") && provided_strs.contains(&"all"),
            "conflict payload should list both offenders; got {provided_strs:?}"
        );
    }

    #[tokio::test]
    async fn resolve_sync_filter_all_flag_returns_all_variant() {
        let (state, _tmp) = test_state().await;
        let args = SyncToolArgs {
            group: None,
            scope: None,
            all: Some(true),
        };
        let filter = resolve_sync_filter(&args, &state.groups)
            .await
            .expect("all should resolve");
        assert!(matches!(filter, mmcp_sync::SyncFilter::All));
    }

    #[tokio::test]
    async fn resolve_sync_filter_scope_arg_resolves_to_scope_variant() {
        let (state, _tmp) = test_state().await;
        let args = SyncToolArgs {
            group: None,
            scope: Some(ToolGroupScope::Shared),
            all: None,
        };
        let filter = resolve_sync_filter(&args, &state.groups)
            .await
            .expect("scope should resolve");
        assert!(matches!(
            filter,
            mmcp_sync::SyncFilter::Scope(mmcp_core::manifest::GroupScope::Shared)
        ));
    }

    #[tokio::test]
    async fn resolve_sync_filter_unknown_group_returns_unknown_group_code() {
        let (state, _tmp) = test_state().await;
        let args = SyncToolArgs {
            group: Some("no-such-group".into()),
            scope: None,
            all: None,
        };
        let err = resolve_sync_filter(&args, &state.groups)
            .await
            .expect_err("unknown group must error");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("unknown_group")
        );
        assert_eq!(
            payload.get("query").and_then(|v| v.as_str()),
            Some("no-such-group")
        );
    }

    #[tokio::test]
    async fn bootstrap_context_project_selector_resolves_by_uuid() {
        // FR-44: passing `project: <uuid>` pins the project group
        // without touching cwd. The bootstrap response lists the
        // memory(ies) in that group under the project-scope half.
        let (state, _tmp) = test_state().await;
        let project = seed_scoped_group_with_memory(
            &state,
            "explicit-target",
            "my-rule",
            OPTIONAL_MEMORY,
            mmcp_core::manifest::GroupScope::Project,
        )
        .await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .bootstrap_context(Parameters(BootstrapContextArgs {
                project: Some(project.to_string()),
                path: None,
            }))
            .await
            .expect("bootstrap_context with explicit project");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("project_uuid").and_then(|v| v.as_str()),
            Some(project.to_string().as_str()),
            "explicit project UUID must round-trip into the response",
        );
    }

    #[tokio::test]
    async fn bootstrap_context_unknown_project_returns_structured_code() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);

        let err = server
            .bootstrap_context(Parameters(BootstrapContextArgs {
                project: Some("no-such-group".into()),
                path: None,
            }))
            .await
            .expect_err("unknown project must error");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("unknown_project")
        );
        assert_eq!(
            payload.get("query").and_then(|v| v.as_str()),
            Some("no-such-group")
        );
    }

    #[tokio::test]
    async fn subscribe_tag_round_trips_through_config() {
        // subscribe(kind=tag, value=rust) writes the entry into
        // .mmcp.toml. A second identical call is a no-op
        // (`changed=false`); unsubscribe drops it back out.
        // Pass `path` explicitly so the test does not race against
        // sibling tests on the process-global cwd.
        use crate::commands::subscribe::{SubscribeMcpArgs, SubscriptionKind};

        let (state, tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);

        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");
        let project_uuid = Uuid::now_v7();
        std::fs::write(
            project_root.join(".mmcp.toml"),
            format!("project_uuid = \"{project_uuid}\"\n"),
        )
        .expect("seed config");
        let path_str = project_root.to_string_lossy().into_owned();

        let res = server
            .subscribe(Parameters(SubscribeMcpArgs {
                kind: SubscriptionKind::Tag,
                value: "rust".into(),
                path: Some(path_str.clone()),
            }))
            .await
            .expect("subscribe tag rust");
        let body = parse_ok_json(res);
        assert_eq!(body.get("changed").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(body.get("kind").and_then(|v| v.as_str()), Some("tag"));
        assert_eq!(body.get("value").and_then(|v| v.as_str()), Some("rust"));

        let cfg = mmcp_store::config::load(&project_root).expect("reload");
        assert_eq!(cfg.subscriptions.tags, vec!["rust".to_string()]);

        // Idempotent re-subscribe.
        let res = server
            .subscribe(Parameters(SubscribeMcpArgs {
                kind: SubscriptionKind::Tag,
                value: "rust".into(),
                path: Some(path_str.clone()),
            }))
            .await
            .expect("subscribe rust again");
        let body = parse_ok_json(res);
        assert_eq!(body.get("changed").and_then(|v| v.as_bool()), Some(false));

        // Unsubscribe drops the entry.
        let res = server
            .unsubscribe(Parameters(SubscribeMcpArgs {
                kind: SubscriptionKind::Tag,
                value: "rust".into(),
                path: Some(path_str.clone()),
            }))
            .await
            .expect("unsubscribe tag rust");
        let body = parse_ok_json(res);
        assert_eq!(body.get("changed").and_then(|v| v.as_bool()), Some(true));
        let cfg = mmcp_store::config::load(&project_root).expect("reload");
        assert!(cfg.subscriptions.tags.is_empty());

        // Unsubscribe again is a no-op.
        let res = server
            .unsubscribe(Parameters(SubscribeMcpArgs {
                kind: SubscriptionKind::Tag,
                value: "rust".into(),
                path: Some(path_str),
            }))
            .await
            .expect("unsubscribe again");
        let body = parse_ok_json(res);
        assert_eq!(body.get("changed").and_then(|v| v.as_bool()), Some(false));
    }

    #[tokio::test]
    async fn subscribe_memory_validates_target_exists() {
        // subscribe(kind=memory, value=<bad>) must error with
        // structured code instead of silently writing. Uses an
        // explicit `path` to avoid racing on cwd.
        use crate::commands::subscribe::{SubscribeMcpArgs, SubscriptionKind};

        let (state, tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);

        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(&project_root).expect("project root");
        std::fs::write(
            project_root.join(".mmcp.toml"),
            format!("project_uuid = \"{}\"\n", Uuid::now_v7()),
        )
        .expect("seed config");
        let path_str = project_root.to_string_lossy().into_owned();

        // Malformed value (no colon).
        let err = server
            .subscribe(Parameters(SubscribeMcpArgs {
                kind: SubscriptionKind::Memory,
                value: "not-a-memory-value".into(),
                path: Some(path_str.clone()),
            }))
            .await
            .expect_err("malformed memory must error");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("malformed_memory_value"),
        );

        // Well-formed but non-existent group UUID.
        let bogus = format!("{}:slug", Uuid::now_v7());
        let err = server
            .subscribe(Parameters(SubscribeMcpArgs {
                kind: SubscriptionKind::Memory,
                value: bogus,
                path: Some(path_str),
            }))
            .await
            .expect_err("unknown group must error");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("unknown_group"),
        );
    }

    #[tokio::test]
    async fn subscribe_without_project_config_errors() {
        // subscribe with no `.mmcp.toml` discoverable from the
        // explicit path must surface `not_in_project` rather than
        // an opaque I/O error.
        use crate::commands::subscribe::{SubscribeMcpArgs, SubscriptionKind};

        let (state, tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);

        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).expect("empty dir");

        let err = server
            .subscribe(Parameters(SubscribeMcpArgs {
                kind: SubscriptionKind::Tag,
                value: "rust".into(),
                path: Some(empty.to_string_lossy().into_owned()),
            }))
            .await
            .expect_err("missing config must error");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("not_in_project"),
        );
    }

    #[tokio::test]
    async fn status_project_selector_returns_minimal_shape() {
        // FR-44: `status(project=<uuid>)` returns the filesystem-
        // free minimal response shape; project_root and sync are
        // omitted because an explicit selector carries no local
        // filesystem guarantees.
        let (state, _tmp) = test_state().await;
        let project =
            seed_group_with_memory(&state, "explicit-target", "dummy", OPTIONAL_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .status(Parameters(StatusArgs {
                project: Some(project.to_string()),
            }))
            .await
            .expect("status with explicit project");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("project_configured").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed.get("project_uuid").and_then(|v| v.as_str()),
            Some(project.to_string().as_str())
        );
        assert!(
            parsed.get("project_root").is_none(),
            "project_root must be omitted when selector is explicit"
        );
        assert!(
            parsed.get("sync").is_none(),
            "sync must be omitted when selector is explicit"
        );
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
        assert_eq!(sync.get("configured").and_then(|v| v.as_bool()), Some(true));
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
        let server = McpServer::new(state, ServeMode::Full);
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
        let err =
            map_init_project_error_to_mcp(crate::commands::init::InitProjectError::SlugRequired);
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
        let err =
            map_init_project_error_to_mcp(crate::commands::init::InitProjectError::InvalidSlug {
                slug: "BAD SLUG".to_string(),
            });
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
        let err =
            map_init_project_error_to_mcp(crate::commands::init::InitProjectError::SlugMismatch {
                expected: "stored".to_string(),
                got: "passed".to_string(),
            });
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("slug_mismatch")
        );
        assert_eq!(
            payload.get("expected").and_then(|v| v.as_str()),
            Some("stored")
        );
        assert_eq!(payload.get("got").and_then(|v| v.as_str()), Some("passed"));
    }

    // ── create_group tool ────────────────────────────────────────────

    #[test]
    fn map_create_group_error_surfaces_invalid_slug_with_slug_echo() {
        let err = map_create_group_error_to_mcp(
            crate::commands::group::CreateGroupError::InvalidSlug {
                slug: "Bad Slug".to_string(),
            },
        );
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_slug")
        );
        assert_eq!(
            payload.get("slug").and_then(|v| v.as_str()),
            Some("Bad Slug"),
            "caller's original slug must be echoed so UIs can highlight it",
        );
    }

    #[test]
    fn map_create_group_error_surfaces_slug_already_exists_with_existing_group_id() {
        let existing = Uuid::now_v7();
        let err = map_create_group_error_to_mcp(
            crate::commands::group::CreateGroupError::SlugAlreadyExists {
                slug: "team-rust".to_string(),
                existing_group_id: existing,
            },
        );
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("slug_already_exists")
        );
        assert_eq!(
            payload.get("slug").and_then(|v| v.as_str()),
            Some("team-rust")
        );
        assert_eq!(
            payload.get("existing_group_id").and_then(|v| v.as_str()),
            Some(existing.to_string().as_str()),
            "existing group id must be echoed so the caller can address it",
        );
    }

    #[tokio::test]
    async fn create_group_tool_writes_group_and_returns_wire_payload() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state.clone(), ServeMode::Full);
        let res = server
            .create_group(Parameters(CreateGroupArgs {
                slug: "shared-rules".to_string(),
                display_name: Some("Shared Rules".to_string()),
                scope: Some(ToolGroupScope::Shared),
                protected: false,
            }))
            .await
            .expect("create_group");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("slug").and_then(|v| v.as_str()),
            Some("shared-rules")
        );
        assert_eq!(parsed.get("scope").and_then(|v| v.as_str()), Some("shared"));
        assert_eq!(
            parsed.get("display_name").and_then(|v| v.as_str()),
            Some("Shared Rules")
        );
        assert_eq!(parsed.get("protected").and_then(|v| v.as_bool()), Some(false));
        let group_id = parsed
            .get("group_id")
            .and_then(|v| v.as_str())
            .expect("group_id");
        let uuid = Uuid::parse_str(group_id).expect("valid uuid");
        assert!(
            state.groups.get(&GroupId::from_uuid(uuid)).await.is_some(),
            "new group must be visible in the index after the tool returns",
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

        assert!(
            report.created_config,
            "config must be written on first call"
        );
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
            subscriptions: Default::default(),
        };
        mmcp_store::config::save(&project_root, &cfg).expect("seed config");

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
            id: None,
            name: "Draft".into(),
            description: "Short desc".into(),
            kind: ToolMemoryKind::Rule,
            body: "# Draft\nBody.".into(),
            tags: Vec::new(),
            mandatory: false,
            refs: Vec::new(),
            source: None,
            override_,
            force: false,
        }
    }

    #[tokio::test]
    async fn write_memory_creates_fresh_slug_without_override() {
        let (state, _tmp) = test_state().await;
        // Seed one memory so the group repo exists; the write
        // targets a different slug.
        let group = seed_group_with_memory(&state, "rules", "existing", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        let res = server
            .write_memory_unguarded(write_memory_args(&group, "fresh", false))
            .await
            .expect("create");
        let parsed = parse_ok_json(res);
        assert_eq!(parsed.get("slug").and_then(|v| v.as_str()), Some("fresh"));
        assert_eq!(
            parsed.get("replaced").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    /// FR-38: a write that supplies `source` round-trips through
    /// the on-disk frontmatter and surfaces on the read response.
    #[tokio::test]
    async fn write_memory_source_round_trips_through_read() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "existing", SAMPLE_MEMORY).await;
        let server = McpServer::new(state.clone(), ServeMode::Full);
        let source_uuid = Uuid::now_v7();
        let mut args = write_memory_args(&group, "with-source", false);
        args.source = Some(source_uuid.to_string());
        let written = server
            .write_memory_unguarded(args)
            .await
            .expect("write with source");
        let written_id = parse_ok_json(written)
            .get("id")
            .and_then(|v| v.as_str())
            .expect("id echoed")
            .to_string();

        let res = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: Some("with-source".into()),
                id: Some(written_id),
                version: None,
            }))
            .await
            .expect("read back");
        let parsed = parse_ok_json(res);
        let fm = parsed.get("frontmatter").expect("frontmatter");
        assert_eq!(
            fm.get("source").and_then(|v| v.as_str()),
            Some(source_uuid.to_string().as_str()),
        );
    }

    /// FR-38: a malformed source string is rejected with the typed
    /// `invalid_source` code rather than being stamped into the
    /// frontmatter as garbage.
    #[tokio::test]
    async fn write_memory_rejects_non_uuid_source() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "existing", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        let mut args = write_memory_args(&group, "bad-source", false);
        args.source = Some("not-a-uuid".into());
        let err = server
            .write_memory_unguarded(args)
            .await
            .expect_err("must refuse non-UUID source");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_source"),
        );
    }

    #[tokio::test]
    async fn write_memory_rejects_existing_id_by_default() {
        // FR-028 flipped the collision key from slug to id. Two
        // memories may share a slug, but the primary key is the
        // UUID; re-using one without `override` must surface the
        // existing-exists code so callers don't silently overwrite.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "seed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        let first = server
            .write_memory_unguarded(write_memory_args(&group, "taken", false))
            .await
            .expect("first write");
        let pinned_id = parse_ok_json(first)
            .get("id")
            .and_then(|v| v.as_str())
            .expect("id echoed")
            .to_string();

        let mut retry = write_memory_args(&group, "taken", false);
        retry.id = Some(pinned_id.clone());
        let err = server
            .write_memory_unguarded(retry)
            .await
            .expect_err("must refuse id collision");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("memory_already_exists")
        );
        assert_eq!(payload.get("slug").and_then(|v| v.as_str()), Some("taken"));
    }

    #[tokio::test]
    async fn write_memory_accepts_duplicate_slug_with_distinct_minted_ids() {
        // Post-FR-028: two memories with the same slug but distinct
        // ids are a valid coexistence. No override needed.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "seed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        let first = server
            .write_memory_unguarded(write_memory_args(&group, "twins", false))
            .await
            .expect("first twin");
        let second = server
            .write_memory_unguarded(write_memory_args(&group, "twins", false))
            .await
            .expect("second twin");
        let first_id = parse_ok_json(first)
            .get("id")
            .and_then(|v| v.as_str())
            .expect("first id")
            .to_string();
        let second_id = parse_ok_json(second)
            .get("id")
            .and_then(|v| v.as_str())
            .expect("second id")
            .to_string();
        assert_ne!(first_id, second_id, "duplicate slugs must get distinct ids");
    }

    #[tokio::test]
    async fn write_memory_accepts_existing_id_when_override_is_true() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "seed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        let first = server
            .write_memory_unguarded(write_memory_args(&group, "replaced", false))
            .await
            .expect("first");
        let pinned_id = parse_ok_json(first)
            .get("id")
            .and_then(|v| v.as_str())
            .expect("id echoed")
            .to_string();

        let mut retry = write_memory_args(&group, "replaced", true);
        retry.id = Some(pinned_id.clone());
        let res = server
            .write_memory_unguarded(retry)
            .await
            .expect("override replaces");
        let parsed = parse_ok_json(res);
        assert_eq!(parsed.get("replaced").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(parsed.get("id").and_then(|v| v.as_str()), Some(pinned_id.as_str()));
    }

    #[tokio::test]
    async fn write_memory_override_surfaces_deprecated_arg_note() {
        // FR-45 populator: `override: true` on `mcp:write_memory`
        // emits a deprecated_arg_form note steering callers
        // toward `mcp:edit_memory` for partial updates.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "seed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        // First create so the id exists.
        let first = server
            .write_memory_unguarded(write_memory_args(&group, "target", false))
            .await
            .expect("first");
        let pinned_id = parse_ok_json(first)
            .get("id")
            .and_then(|v| v.as_str())
            .expect("id echoed")
            .to_string();

        // Second call with override: true — expect the note.
        let mut retry = write_memory_args(&group, "target", true);
        retry.id = Some(pinned_id.clone());
        let res = server
            .write_memory_unguarded(retry)
            .await
            .expect("override ok");
        let parsed = parse_ok_json(res);

        let notes = parsed
            .get("notes")
            .and_then(|v| v.as_array())
            .expect("notes array present on override");
        assert_eq!(notes.len(), 1, "exactly one deprecation note");
        let note = &notes[0];
        assert_eq!(note.get("level").and_then(|v| v.as_str()), Some("warn"));
        assert_eq!(
            note.get("code").and_then(|v| v.as_str()),
            Some("deprecated_arg_form")
        );
        let ctx = note.get("context").expect("context present");
        assert_eq!(
            ctx.get("tool").and_then(|v| v.as_str()),
            Some("write_memory")
        );
        assert_eq!(ctx.get("arg").and_then(|v| v.as_str()), Some("override"));
    }

    #[tokio::test]
    async fn write_memory_without_override_has_no_notes_field() {
        // Common case: no notes field present in the response when
        // nothing triggered a populator.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "seed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .write_memory_unguarded(write_memory_args(&group, "clean", false))
            .await
            .expect("create");
        let parsed = parse_ok_json(res);
        assert!(
            parsed.get("notes").is_none(),
            "empty notes must be omitted, got: {parsed:?}"
        );
    }

    // ── import_memory tool ───────────────────────────────────────

    fn import_memory_args(
        group: &GroupId,
        slug: &str,
        source: &str,
        format: Option<ToolImportSourceFormat>,
    ) -> ImportMemoryArgs {
        ImportMemoryArgs {
            group: group.to_string(),
            slug: slug.to_string(),
            source: source.to_string(),
            format,
            name: Some("Imported".into()),
            description: Some("From MCP import".into()),
            kind: Some(ToolMemoryKind::Rule),
            override_: false,
            force: false,
        }
    }

    #[test]
    fn map_adoc_convert_error_surfaces_parse_failed_code() {
        let err = map_adoc_convert_error_to_mcp(mmcp_store::AdocConvertError::Parse(
            "unterminated block at line 7".into(),
        ));
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("adoc_parse_failed")
        );
    }

    #[test]
    fn map_adoc_convert_error_surfaces_render_failed_code() {
        let err = map_adoc_convert_error_to_mcp(mmcp_store::AdocConvertError::Render(
            "unsupported node".into(),
        ));
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("adoc_render_failed")
        );
    }

    #[tokio::test]
    async fn import_memory_markdown_body_with_synth_fields_round_trips() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "seed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .import_memory_unguarded(import_memory_args(
                &group,
                "imported-md",
                "Body content for the imported memory.\n",
                None,
            ))
            .await
            .expect("import markdown");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("slug").and_then(|v| v.as_str()),
            Some("imported-md")
        );
        assert!(
            parsed.get("id").and_then(|v| v.as_str()).is_some(),
            "minted UUID must be echoed so the caller can address the memory",
        );
        assert!(
            parsed
                .get("commit_id")
                .and_then(|v| v.as_str())
                .is_some_and(|id| id.len() == 40),
            "commit id must be a full-length git SHA",
        );
    }

    #[tokio::test]
    async fn import_memory_adoc_body_is_converted_before_storage() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "seed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state.clone(), ServeMode::Full);

        let adoc = "= Imported Heading\n\nParagraph from an adoc source.\n";
        let res = server
            .import_memory_unguarded(import_memory_args(
                &group,
                "imported-adoc",
                adoc,
                Some(ToolImportSourceFormat::Adoc),
            ))
            .await
            .expect("import adoc");
        let parsed = parse_ok_json(res);
        let id = parsed
            .get("id")
            .and_then(|v| v.as_str())
            .expect("id echoed");

        // The stored memory must carry the CONVERTED markdown, not
        // the raw adoc source. Re-read it through the store to prove
        // the bridge fired before the commit.
        let entry = state
            .groups
            .get(&group)
            .await
            .expect("group indexed");
        let resolved = mmcp_store::resolve_memory(
            &state.backend,
            &entry.handle,
            Some("imported-adoc"),
            Some(Uuid::parse_str(id).expect("id parses")),
        )
        .await
        .expect("resolve imported memory");
        let raw = state
            .backend
            .read_file(&entry.handle, &resolved.path, &mmcp_git::Rev::head())
            .await
            .expect("read stored memory");
        let text = std::str::from_utf8(&raw).expect("utf8");
        assert!(
            text.contains("Imported Heading"),
            "converted heading must survive into storage; got:\n{text}"
        );
        assert!(
            !text.contains("= Imported Heading"),
            "stored memory must not carry the raw adoc fence; got:\n{text}"
        );
    }

    #[tokio::test]
    async fn import_memory_asciidoc_alias_routes_through_adoc_bridge() {
        // Wire-alias smoke test: deserialising `"asciidoc"` must
        // land on the `Adoc` variant so both spellings reach the
        // same conversion path.
        let parsed: ToolImportSourceFormat =
            serde_json::from_value(json!("asciidoc")).expect("asciidoc alias");
        assert!(parsed.is_adoc());
        let direct: ToolImportSourceFormat =
            serde_json::from_value(json!("adoc")).expect("adoc primary");
        assert!(direct.is_adoc());
        let md: ToolImportSourceFormat =
            serde_json::from_value(json!("markdown")).expect("markdown");
        assert!(!md.is_adoc());
    }

    #[tokio::test]
    async fn import_memory_rejects_partial_synth_frontmatter() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "seed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let mut args = import_memory_args(&group, "partial", "Body\n", None);
        args.description = None;
        let err = server
            .import_memory_unguarded(args)
            .await
            .expect_err("partial synth must error");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("synth_frontmatter_partial")
        );
    }

    // ── edit_memory (FR-016) ──────────────────────────────────────

    #[tokio::test]
    async fn edit_memory_replaces_body_leaving_frontmatter_intact() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "first", SAMPLE_MEMORY).await;
        let server = McpServer::new(state.clone(), ServeMode::Full);
        let res = server
            .edit_memory_unguarded(EditMemoryArgs {
                group: group.to_string(),
                slug: Some("first".into()),
                id: None,
                body: Some("# Edited\nNew body.".into()),
                ..Default::default()
            })
            .await
            .expect("edit");
        let parsed = parse_ok_json(res);
        assert_eq!(parsed.get("slug").and_then(|v| v.as_str()), Some("first"));

        // Reload and confirm body changed, frontmatter preserved.
        let read = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: Some("first".into()),
                id: None,
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
        let server = McpServer::new(state.clone(), ServeMode::Full);
        server
            .edit_memory_unguarded(EditMemoryArgs {
                group: group.to_string(),
                slug: Some("taggy".into()),
                id: None,
                tags_add: vec!["alpha".into(), "beta".into(), "sample".into()],
                tags_remove: vec!["sample".into()],
                ..Default::default()
            })
            .await
            .expect("edit");
        let read = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: Some("taggy".into()),
                id: None,
                version: None,
            }))
            .await
            .expect("read");
        let parsed = parse_ok_json(read);
        let tags: Vec<String> = parsed
            .get("frontmatter")
            .and_then(|v| v.get("tags"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            })
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
    async fn move_memory_relocates_to_nested_path() {
        // FR-41: move a flat memory to a nested slug path. The id
        // stays stable; resolving by id surfaces the new slug.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "flat", SAMPLE_MEMORY).await;
        let server = McpServer::new(state.clone(), ServeMode::Full);
        let res = server
            .move_memory_unguarded(MoveMemoryArgs {
                group: group.to_string(),
                slug: Some("flat".into()),
                new_slug: "nested/path/leaf".into(),
                ..Default::default()
            })
            .await
            .expect("move");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("new_slug").and_then(|v| v.as_str()),
            Some("nested/path/leaf")
        );
        assert!(
            !parsed
                .get("commit_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .is_empty()
        );
        // The new path is reachable via list_memories with the
        // matching prefix; the old slug is gone.
        let listed = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: group.to_string(),
                path_prefix: Some("nested".into()),
                ..Default::default()
            }))
            .await
            .expect("list");
        let listed = parse_ok_json(listed);
        let arr = listed
            .get("memories")
            .and_then(|v| v.as_array())
            .expect("memories");
        // Descriptors carry the leaf-only `slug` plus the full
        // segmented `path`; the moved memory surfaces as
        // slug `leaf` at path `nested/path/leaf`.
        assert!(
            arr.iter().any(|m| {
                let slug_is_leaf = m.get("slug").and_then(|v| v.as_str()) == Some("leaf");
                let path_matches = m
                    .get("path")
                    .and_then(|v| v.as_array())
                    .is_some_and(|segments| {
                        segments.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>()
                            == ["nested", "path", "leaf"]
                    });
                slug_is_leaf && path_matches
            }),
            "moved memory must surface under prefix filter; got {arr:?}",
        );
    }

    #[tokio::test]
    async fn move_memory_validates_new_slug() {
        // FR-41: a `..` segment is rejected so callers can't
        // escape `memories/` via the move tool.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "src", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        let err = server
            .move_memory_unguarded(MoveMemoryArgs {
                group: group.to_string(),
                slug: Some("src".into()),
                new_slug: "../escape".into(),
                ..Default::default()
            })
            .await
            .expect_err("invalid slug must error");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("invalid_slug")
        );
    }

    #[tokio::test]
    async fn list_memories_path_prefix_filters_to_subtree() {
        // FR-41: path_prefix + recursive=false trims the listing
        // to immediate children; recursive=true (default) walks
        // the whole subtree.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "top", SAMPLE_MEMORY).await;
        let server = McpServer::new(state.clone(), ServeMode::Full);
        // Set up a nested layout via two moves.
        server
            .move_memory_unguarded(MoveMemoryArgs {
                group: group.to_string(),
                slug: Some("top".into()),
                new_slug: "feedback/git/scope".into(),
                ..Default::default()
            })
            .await
            .expect("move");
        // Add a sibling under feedback with a shallower path.
        let entry = server
            .state
            .groups
            .get(&group)
            .await
            .expect("group entry");
        let sibling_id = Uuid::now_v7();
        server
            .state
            .backend
            .write_commit(
                &entry.handle,
                CommitSpec {
                    branch: mmcp_core::conventions::MAIN_BRANCH.to_string(),
                    author_name: "test".into(),
                    author_email: "test@example.com".into(),
                    message: "seed sibling".into(),
                    files: vec![(
                        mmcp_core::conventions::memory_path("feedback", sibling_id),
                        Some(SAMPLE_MEMORY.as_bytes().to_vec()),
                    )],
                },
            )
            .await
            .expect("seed sibling");

        // Recursive (default): both surface. Match on the
        // joined-path form (`path.join("/")`) so the assertion is
        // about the structural location, not the leaf-only slug.
        let res = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: group.to_string(),
                path_prefix: Some("feedback".into()),
                ..Default::default()
            }))
            .await
            .expect("list");
        let parsed = parse_ok_json(res);
        let joined: Vec<String> = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        m.get("path").and_then(|v| v.as_array()).map(|segs| {
                            segs.iter()
                                .filter_map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join("/")
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert!(joined.contains(&"feedback".to_string()), "got: {joined:?}");
        assert!(
            joined.contains(&"feedback/git/scope".to_string()),
            "got: {joined:?}"
        );

        // Non-recursive: only immediate children of `feedback` (and
        // `feedback` itself) match. `feedback/git/scope` is two
        // levels deep, so it's filtered out.
        let res = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: group.to_string(),
                path_prefix: Some("feedback".into()),
                recursive: Some(false),
            }))
            .await
            .expect("list");
        let parsed = parse_ok_json(res);
        let joined: Vec<String> = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        m.get("path").and_then(|v| v.as_array()).map(|segs| {
                            segs.iter()
                                .filter_map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join("/")
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert!(joined.contains(&"feedback".to_string()), "got: {joined:?}");
        assert!(
            !joined.contains(&"feedback/git/scope".to_string()),
            "non-recursive must exclude deeper paths; got: {joined:?}",
        );
    }

    #[tokio::test]
    async fn list_memories_descriptor_splits_slug_and_path() {
        // FR-41: descriptor splits the on-disk path into a leaf
        // `slug` (basename) and a segmented `path` (full location).
        // Structure-aware consumers rebuild the joined form via
        // `path.join("/")`.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "deep", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        // Move the memory into a nested slug so the path has more
        // than one segment.
        server
            .move_memory_unguarded(MoveMemoryArgs {
                group: group.to_string(),
                slug: Some("deep".into()),
                new_slug: "feedback/git/scope".into(),
                ..Default::default()
            })
            .await
            .expect("move");

        let res = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: group.to_string(),
                ..Default::default()
            }))
            .await
            .expect("list");
        let parsed = parse_ok_json(res);
        let entry = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .expect("one memory");
        assert_eq!(
            entry.get("slug").and_then(|v| v.as_str()),
            Some("scope"),
            "slug is the leaf only — no `/` separators",
        );
        let path: Vec<String> = entry
            .get("path")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .expect("path array");
        assert_eq!(path, vec!["feedback", "git", "scope"]);
    }

    #[tokio::test]
    async fn list_memories_flat_slug_yields_single_segment_path() {
        // Sanity: a flat slug surfaces as a single-element path
        // and the leaf slug equals that segment, so serde consumers
        // don't need a special case for top-level memories.
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "flat", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        let res = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: group.to_string(),
                ..Default::default()
            }))
            .await
            .expect("list");
        let parsed = parse_ok_json(res);
        let entry = parsed
            .get("memories")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .expect("one memory");
        assert_eq!(entry.get("slug").and_then(|v| v.as_str()), Some("flat"));
        let path: Vec<String> = entry
            .get("path")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .expect("path array");
        assert_eq!(path, vec!["flat"]);
    }

    #[tokio::test]
    async fn slug_matches_filter_truth_table() {
        // No prefix, recursive=true: every slug matches.
        assert!(slug_matches_filter("a", None, true));
        assert!(slug_matches_filter("a/b/c", None, true));
        // No prefix, recursive=false: only top-level slugs.
        assert!(slug_matches_filter("a", None, false));
        assert!(!slug_matches_filter("a/b", None, false));
        // Prefix match, recursive=true.
        assert!(slug_matches_filter("a/b", Some("a"), true));
        assert!(slug_matches_filter("a/b/c/d", Some("a/b"), true));
        // Prefix match, recursive=false: only depth ≤ 1 below
        // prefix.
        assert!(slug_matches_filter("a", Some("a"), false));
        assert!(slug_matches_filter("a/b", Some("a"), false));
        assert!(!slug_matches_filter("a/b/c", Some("a"), false));
        // Prefix mismatch.
        assert!(!slug_matches_filter("ab", Some("a"), true));
        assert!(!slug_matches_filter("b/a", Some("a"), true));
    }

    #[tokio::test]
    async fn edit_memory_returns_memory_not_found_when_slug_absent() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "existing", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        let err = server
            .edit_memory_unguarded(EditMemoryArgs {
                group: group.to_string(),
                slug: Some("ghost".into()),
                id: None,
                body: Some("n/a".into()),
                ..Default::default()
            })
            .await
            .expect_err("must surface not-found");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("memory_not_found")
        );
        assert_eq!(payload.get("slug").and_then(|v| v.as_str()), Some("ghost"));
    }

    // ── delete_memory (FR-017) ────────────────────────────────────

    #[tokio::test]
    async fn delete_memory_removes_slug_from_listing() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "rules", "doomed", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        server
            .delete_memory_unguarded(DeleteMemoryArgs {
                group: group.to_string(),
                slug: Some("doomed".into()),
                id: None,
                message: None,
                force: false,
            })
            .await
            .expect("delete");
        let list = server
            .list_memories(Parameters(ListMemoriesArgs {
                group: group.to_string(),
                ..Default::default()
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
        let server = McpServer::new(state, ServeMode::Full);
        let err = server
            .delete_memory_unguarded(DeleteMemoryArgs {
                group: group.to_string(),
                slug: Some("never-existed".into()),
                id: None,
                message: None,
                force: false,
            })
            .await
            .expect_err("must surface not-found");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("memory_not_found")
        );
    }

    // ── protected-group guard (FR-019 + FR-011 fallback) ──────────
    //
    // The elicitation-enabled path lives behind a Peer<RoleServer>
    // and is exercised end-to-end at the integration / MCP-client
    // layer. These unit tests cover the pre-elicitation fallback
    // (`ensure_not_protected`), which is the payload shape every
    // caller sees when the client lacks elicitation capability.

    async fn protected_entry_for(state: &ClientState, slug: &str) -> GroupEntry {
        let group_id =
            seed_protected_group_with_memory(state, slug, "anchored", SAMPLE_MEMORY).await;
        state
            .groups
            .get(&group_id)
            .await
            .expect("seeded protected group resolvable")
    }

    #[tokio::test]
    async fn write_memory_against_protected_group_errors_with_elicitation_hint() {
        let (state, _tmp) = test_state().await;
        let entry = protected_entry_for(&state, "global").await;
        let err = ensure_not_protected(&entry, "fresh", "create")
            .expect_err("protected-group fallback must error on create");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("protected_requires_elicitation")
        );
        assert_eq!(
            payload.get("group_slug").and_then(|v| v.as_str()),
            Some("global")
        );
        assert_eq!(
            payload.get("action").and_then(|v| v.as_str()),
            Some("create")
        );
    }

    #[tokio::test]
    async fn write_memory_override_against_protected_group_reports_override_action() {
        let (state, _tmp) = test_state().await;
        let entry = protected_entry_for(&state, "global").await;
        let err = ensure_not_protected(&entry, "existing", "override")
            .expect_err("protected-group fallback must gate override");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("action").and_then(|v| v.as_str()),
            Some("override")
        );
    }

    #[tokio::test]
    async fn edit_memory_against_protected_group_is_gated() {
        let (state, _tmp) = test_state().await;
        let entry = protected_entry_for(&state, "global").await;
        let err = ensure_not_protected(&entry, "rule", "edit")
            .expect_err("protected-group fallback must gate edit");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("protected_requires_elicitation")
        );
        assert_eq!(payload.get("action").and_then(|v| v.as_str()), Some("edit"));
    }

    #[tokio::test]
    async fn delete_memory_against_protected_group_is_gated() {
        let (state, _tmp) = test_state().await;
        let entry = protected_entry_for(&state, "global").await;
        let err = ensure_not_protected(&entry, "rule", "delete")
            .expect_err("protected-group fallback must gate delete");
        let payload = err.data.as_ref().expect("payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("protected_requires_elicitation")
        );
        assert_eq!(
            payload.get("action").and_then(|v| v.as_str()),
            Some("delete")
        );
    }

    #[tokio::test]
    async fn unprotected_sibling_group_continues_to_accept_writes() {
        // Seed one protected and one unprotected group in the same
        // mirror; the guard must only affect the protected one.
        let (state, _tmp) = test_state().await;
        let _protected =
            seed_protected_group_with_memory(&state, "global", "anchored", SAMPLE_MEMORY).await;
        let sibling = seed_group_with_memory(&state, "team-rust", "rules", SAMPLE_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);
        server
            .write_memory_unguarded(write_memory_args(&sibling, "added", false))
            .await
            .expect("write into unprotected sibling must succeed");
    }

    // ── edit_memory_body (FR-026) ─────────────────────────────────

    const SECTIONED_MEMORY: &str = "+++\nname = \"Sample\"\ndescription = \"A sectioned memory\"\nkind = \"rule\"\nmandatory = false\ntags = [\"sample\"]\n+++\n## Need\n\nneed body\n\n## Resolution\n\nresolution body\n";

    #[tokio::test]
    async fn read_memory_body_sections_returns_addressable_tree() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SECTIONED_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let res = server
            .read_memory_body_sections_inner(ReadMemoryBodySectionsArgs {
                group: group.to_string(),
                slug: Some("rules".into()),
                id: None,
            })
            .await
            .expect("read sections");
        let parsed = parse_ok_json(res);
        let sections = parsed
            .get("sections")
            .and_then(|v| v.as_array())
            .expect("sections array");
        let paths: Vec<&str> = sections
            .iter()
            .filter_map(|s| s.get("path").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(paths, vec!["preamble", "need", "resolution"]);
    }

    #[tokio::test]
    async fn edit_memory_body_upsert_replaces_section() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SECTIONED_MEMORY).await;
        let server = McpServer::new(state.clone(), ServeMode::Full);

        server
            .edit_memory_body_unguarded(EditMemoryBodyArgs {
                group: group.to_string(),
                slug: Some("rules".into()),
                id: None,
                ops: vec![ToolMemoryEditOp::UpsertSection {
                    path: "need".into(),
                    level: 2,
                    heading: "Need".into(),
                    body: "rewritten need body".into(),
                }],
                message: None,
                force: false,
            })
            .await
            .expect("edit body");

        // Reread to confirm the write landed.
        let res = server
            .read_memory(Parameters(ReadMemoryArgs {
                group: group.to_string(),
                slug: Some("rules".into()),
                id: None,
                version: None,
            }))
            .await
            .expect("read");
        let parsed = parse_ok_json(res);
        let body = parsed
            .get("body")
            .and_then(|v| v.as_str())
            .expect("body inline");
        assert!(body.contains("## Need\n\nrewritten need body"));
        assert!(
            body.contains("## Resolution\n\nresolution body"),
            "untouched section must survive; got:\n{body}",
        );
    }

    #[tokio::test]
    async fn edit_memory_body_section_not_found_errors_with_structured_code() {
        let (state, _tmp) = test_state().await;
        let group = seed_group_with_memory(&state, "team-rust", "rules", SECTIONED_MEMORY).await;
        let server = McpServer::new(state, ServeMode::Full);

        let err = server
            .edit_memory_body_unguarded(EditMemoryBodyArgs {
                group: group.to_string(),
                slug: Some("rules".into()),
                id: None,
                ops: vec![ToolMemoryEditOp::DeleteSection {
                    path: "does-not-exist".into(),
                }],
                message: None,
                force: false,
            })
            .await
            .expect_err("must error on missing section");
        let payload = err.data.as_ref().expect("error payload");
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("section_not_found"),
        );
        assert_eq!(
            payload.get("path").and_then(|v| v.as_str()),
            Some("does-not-exist"),
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

    /// FR-029 regression: every `#[tool(...)]` site in this file must
    /// carry `ToolAnnotations` with the exact hint bits the project
    /// committed to in the FR. If a new tool lands without
    /// `annotations(...)`, the helper will see `annotations = None`
    /// and fail loudly so the reviewer catches the omission.
    #[test]
    fn tool_annotations_match_fr029_matrix() {
        use rmcp::model::Tool;

        fn check(
            tool: Tool,
            expected_read_only: Option<bool>,
            expected_destructive: Option<bool>,
            expected_idempotent: Option<bool>,
            expected_open_world: Option<bool>,
        ) {
            let name = tool.name.clone();
            let ann = tool
                .annotations
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: missing ToolAnnotations"));
            assert!(
                ann.title.as_deref().map(str::is_empty) == Some(false),
                "{name}: annotations.title must be set to a human-readable label",
            );
            assert_eq!(
                ann.read_only_hint, expected_read_only,
                "{name}: read_only_hint mismatch",
            );
            assert_eq!(
                ann.destructive_hint, expected_destructive,
                "{name}: destructive_hint mismatch",
            );
            assert_eq!(
                ann.idempotent_hint, expected_idempotent,
                "{name}: idempotent_hint mismatch",
            );
            assert_eq!(
                ann.open_world_hint, expected_open_world,
                "{name}: open_world_hint mismatch",
            );
        }

        // Helper that expands the 4-tuple expectation to four args.
        fn check_bits(
            tool: Tool,
            bits: (Option<bool>, Option<bool>, Option<bool>, Option<bool>),
        ) {
            check(tool, bits.0, bits.1, bits.2, bits.3);
        }

        // ── Read-only tools ─────────────────────────────────────
        // read_only=true, idempotent=true, open_world=false.
        // destructive_hint intentionally unset (read-only implies it).
        let ro = (Some(true), None, Some(true), Some(false));
        check_bits(McpServer::list_groups_tool_attr(), ro);
        check_bits(McpServer::list_memories_tool_attr(), ro);
        check_bits(McpServer::read_memory_tool_attr(), ro);
        check_bits(McpServer::list_versions_tool_attr(), ro);
        check_bits(McpServer::group_info_tool_attr(), ro);
        check_bits(McpServer::search_memories_tool_attr(), ro);
        check_bits(McpServer::read_memory_body_sections_tool_attr(), ro);
        check_bits(McpServer::check_health_tool_attr(), ro);
        check_bits(McpServer::diagnose_tool_attr(), ro);
        check_bits(McpServer::debug_read_file_tool_attr(), ro);
        check_bits(McpServer::debug_list_tree_tool_attr(), ro);
        check_bits(McpServer::debug_git_log_tool_attr(), ro);
        check_bits(McpServer::bootstrap_context_tool_attr(), ro);
        check_bits(McpServer::status_tool_attr(), ro);
        check_bits(McpServer::read_feature_tool_attr(), ro);
        check_bits(McpServer::list_features_tool_attr(), ro);
        check_bits(McpServer::describe_tools_tool_attr(), ro);

        // ── Local mutation tools (open_world = false) ───────────
        // write_memory / import_memory: additive, not idempotent.
        let add = (Some(false), Some(false), Some(false), Some(false));
        check_bits(McpServer::write_memory_tool_attr(), add);
        check_bits(McpServer::import_memory_tool_attr(), add);

        // Destructive local mutations.
        let dmod = (Some(false), Some(true), Some(false), Some(false));
        check_bits(McpServer::edit_memory_tool_attr(), dmod);
        check_bits(McpServer::edit_memory_body_tool_attr(), dmod);
        check_bits(McpServer::debug_write_file_tool_attr(), dmod);
        check_bits(McpServer::update_feature_tool_attr(), dmod);

        // Destructive + idempotent (delete shapes + init_claude rewrite).
        let ddel = (Some(false), Some(true), Some(true), Some(false));
        check_bits(McpServer::delete_memory_tool_attr(), ddel);
        check_bits(McpServer::init_claude_tool_attr(), ddel);
        check_bits(McpServer::delete_feature_tool_attr(), ddel);

        // Non-destructive + idempotent.
        let iden = (Some(false), Some(false), Some(true), Some(false));
        check_bits(McpServer::debug_toggle_tool_attr(), iden);
        check_bits(McpServer::init_project_tool_attr(), iden);
        check_bits(McpServer::rename_feature_tool_attr(), iden);
        check_bits(McpServer::move_memory_tool_attr(), iden);
        check_bits(McpServer::subscribe_tool_attr(), iden);
        check_bits(McpServer::unsubscribe_tool_attr(), iden);

        // Non-destructive + non-idempotent (create_group, add_feature).
        let cre = (Some(false), Some(false), Some(false), Some(false));
        check_bits(McpServer::create_group_tool_attr(), cre);
        check_bits(McpServer::add_feature_tool_attr(), cre);

        // ── Sync tools (open_world = true) ──────────────────────
        // sync_fetch / sync_push: non-destructive, idempotent.
        let sw_safe = (Some(false), Some(false), Some(true), Some(true));
        check_bits(McpServer::sync_fetch_tool_attr(), sw_safe);
        check_bits(McpServer::sync_push_tool_attr(), sw_safe);

        // sync_pull / sync: destructive (remote replay can shadow
        // local work), idempotent.
        let sw_pull = (Some(false), Some(true), Some(true), Some(true));
        check_bits(McpServer::sync_pull_tool_attr(), sw_pull);
        check_bits(McpServer::sync_tool_attr(), sw_pull);

        // ── Archive tools (open_world = true) ───────────────────
        // export_archive: writes a file (not read-only) but does not
        // mutate the store; re-exporting is idempotent.
        check_bits(
            McpServer::export_archive_tool_attr(),
            (Some(false), Some(false), Some(true), Some(true)),
        );
        // import_archive: additive store write fed by an external
        // file; not idempotent under new_ids / overwrite.
        check_bits(
            McpServer::import_archive_tool_attr(),
            (Some(false), Some(false), Some(false), Some(true)),
        );
    }

    /// FR-49: every registered tool surfaces a non-empty `icons` list
    /// on the canonical accessor consumed by `describe_tools`, the
    /// `mmcp tools` CLI, and the live `tool_router`. A tool that
    /// drifts outside the matched buckets in `tool_icon_category`
    /// would still receive icons (default arm = Mutate), so this
    /// test alone does not catch unmapped names; it does catch any
    /// regression where the patching step is skipped or the helper
    /// returns an empty Vec.
    #[test]
    fn registered_tools_carry_icons_for_describe_tools_and_cli() {
        for tool in registered_tool_attrs() {
            let icons = tool
                .icons
                .as_ref()
                .unwrap_or_else(|| panic!("{}: tool must carry icons", tool.name));
            assert!(
                !icons.is_empty(),
                "{}: icons list must be non-empty",
                tool.name,
            );
            assert!(
                icons[0].src.starts_with("data:image/svg+xml"),
                "{}: icon src must be a data SVG; got {:?}",
                tool.name,
                icons[0].src,
            );
        }
    }

    /// FR-49: spot-check that category routing covers the obvious
    /// archetypes — one read tool, one debug tool, one sync tool,
    /// one feature tool, one mutate tool — so a future refactor of
    /// the category match can't silently re-bucket entire families.
    #[test]
    fn tool_icon_category_covers_each_archetype() {
        assert_eq!(tool_icon_category("read_memory"), ToolIconCategory::Read);
        assert_eq!(tool_icon_category("write_memory"), ToolIconCategory::Mutate);
        assert_eq!(tool_icon_category("read_feature"), ToolIconCategory::Feature);
        assert_eq!(
            tool_icon_category("debug_read_file"),
            ToolIconCategory::Debug,
        );
        assert_eq!(tool_icon_category("sync_pull"), ToolIconCategory::Sync);
        // Unmapped names default to Mutate; the FR-29 test catches
        // the absence of a tool from the list, not bucket drift.
        assert_eq!(
            tool_icon_category("future_tool_that_does_not_exist_yet"),
            ToolIconCategory::Mutate,
        );
    }

    /// FR-50: the patching seam decorates each tool with its
    /// mmcp.* advisory bits. Spot-check the four buckets — sync
    /// (network + requires_sync), debug (debug_gated), protected-
    /// group (write_memory hits the FR-019 guard), feature
    /// (requires_project) — so a refactor of `meta_for_tool`
    /// cannot silently strip the wire-visible hints.
    #[test]
    fn meta_for_tool_covers_each_namespace_bucket() {
        let sync = meta_for_tool("sync_pull").expect("sync_pull has meta");
        assert_eq!(
            sync.0.get("mmcp.requires_sync"),
            Some(&serde_json::Value::Bool(true)),
        );
        assert_eq!(
            sync.0.get("mmcp.network"),
            Some(&serde_json::Value::Bool(true)),
        );

        let debug = meta_for_tool("debug_read_file").expect("debug has meta");
        assert_eq!(
            debug.0.get("mmcp.debug_gated"),
            Some(&serde_json::Value::Bool(true)),
        );

        let protected = meta_for_tool("write_memory").expect("write_memory has meta");
        assert_eq!(
            protected.0.get("mmcp.protected_group_gated"),
            Some(&serde_json::Value::Bool(true)),
        );

        let feature = meta_for_tool("read_feature").expect("read_feature has meta");
        assert_eq!(
            feature.0.get("mmcp.requires_project"),
            Some(&serde_json::Value::Bool(true)),
        );

        // A tool with no advisory bits returns `None` so the wire
        // stays absent, not `{}`. `list_groups` is a pure-local
        // read with no preconditions.
        assert!(meta_for_tool("list_groups").is_none());
    }

    /// FR-45: every registered tool surfaces a permissive object
    /// `output_schema` so MCP clients can validate that the
    /// response is a JSON object without 37 separate typed
    /// response structs landing in this commit.
    #[test]
    fn registered_tools_carry_output_schema() {
        for tool in registered_tool_attrs() {
            let schema = tool
                .output_schema
                .as_ref()
                .unwrap_or_else(|| panic!("{}: tool must carry output_schema", tool.name));
            assert_eq!(
                schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "{}: schema must declare type=object",
                tool.name,
            );
            assert_eq!(
                schema
                    .get("additionalProperties")
                    .and_then(|v| v.as_bool()),
                Some(true),
                "{}: schema must allow additional properties",
                tool.name,
            );
        }
    }

    /// FR-45: the schema is shared (same `Arc`) across all tools so
    /// the per-tool patch is cheap. Cloning the Arc bumps the
    /// reference count rather than rebuilding the JsonObject.
    #[test]
    fn shared_output_schema_returns_same_arc() {
        let a = shared_output_schema();
        let b = shared_output_schema();
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "shared_output_schema must hand out the same Arc on repeat calls",
        );
    }

    /// FR-50: the patching seam decorates `registered_tool_attrs()`
    /// (consumed by `describe_tools` and the CLI) with the same
    /// meta the live router sees, so harnesses pre-flighting via
    /// either path get matching results.
    #[test]
    fn registered_tools_carry_meta_for_describe_tools() {
        let tools = registered_tool_attrs();
        let sync_pull = tools
            .iter()
            .find(|t| t.name.as_ref() == "sync_pull")
            .expect("sync_pull present");
        let meta = sync_pull
            .meta
            .as_ref()
            .expect("sync_pull surface must carry meta");
        assert_eq!(
            meta.0.get("mmcp.network"),
            Some(&serde_json::Value::Bool(true)),
        );

        let list_groups = tools
            .iter()
            .find(|t| t.name.as_ref() == "list_groups")
            .expect("list_groups present");
        assert!(
            list_groups.meta.is_none(),
            "list_groups has no advisory bits; meta must stay None",
        );
    }

    /// FR-30: `mmcp serve --mode readonly` filters destructive and
    /// additive mutators out of the registered tool surface so a
    /// harness bug or prompt-injection cannot reach them. The check
    /// runs against the in-memory `tool_router.map` so it does not
    /// need a full stdio loop.
    #[tokio::test]
    async fn serve_mode_readonly_excludes_mutating_tools() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Readonly);
        let names: std::collections::HashSet<String> = server
            .tool_router
            .map
            .keys()
            .map(|k| k.to_string())
            .collect();

        assert!(names.contains("read_memory"));
        assert!(names.contains("list_groups"));
        assert!(names.contains("describe_tools"));
        assert!(!names.contains("write_memory"));
        assert!(!names.contains("delete_memory"));
        assert!(!names.contains("edit_memory"));
        assert!(!names.contains("sync_pull"));
        assert!(!names.contains("sync_push"));
    }

    /// FR-30: `--mode edit` keeps additive mutators (`write_memory`,
    /// `import_memory`, `add_feature`) but still drops destructive
    /// rewrites and replay-style sync.
    #[tokio::test]
    async fn serve_mode_edit_keeps_additive_drops_destructive() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Edit);
        let names: std::collections::HashSet<String> = server
            .tool_router
            .map
            .keys()
            .map(|k| k.to_string())
            .collect();

        assert!(names.contains("write_memory"));
        assert!(names.contains("import_memory"));
        assert!(names.contains("add_feature"));
        assert!(names.contains("sync_fetch"));
        assert!(names.contains("sync_push"));
        assert!(!names.contains("delete_memory"));
        assert!(!names.contains("edit_memory"));
        assert!(!names.contains("update_feature"));
        assert!(!names.contains("sync_pull"));
        assert!(!names.contains("sync"));
    }

    /// FR-30: `--mode full` keeps every registered tool. Cross-checks
    /// against `registered_tool_attrs()` so any future tool addition
    /// is exercised here without an explicit name list.
    #[tokio::test]
    async fn serve_mode_full_registers_every_tool() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let live_count = server.tool_router.map.len();
        let canonical = McpServer::registered_tool_attrs().len();
        assert_eq!(live_count, canonical);
    }

    /// FR-30: the `status` MCP tool echoes the active mode so an
    /// operator probing a running server can tell which posture it
    /// was launched with without restarting it.
    #[tokio::test]
    async fn status_tool_reports_active_mode() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Readonly);
        let res = server
            .status(Parameters(StatusArgs::default()))
            .await
            .expect("status");
        let parsed = parse_ok_json(res);
        assert_eq!(
            parsed.get("mode").and_then(|v| v.as_str()),
            Some("readonly"),
        );
    }

    /// FR-31: `describe_tools` returns one entry per registered tool
    /// with the four annotation hint bits intact. The list mirrors
    /// FR-29's matrix; if a new tool ships without being added to
    /// `registered_tool_attrs`, this assertion catches the gap.
    #[tokio::test]
    async fn describe_tools_lists_every_registered_tool() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let res = server
            .describe_tools(Parameters(DescribeToolsArgs::default()))
            .await
            .expect("describe_tools");
        let parsed = parse_ok_json(res);

        // Count must match the canonical helper list. If this drifts,
        // either a new tool was added without a registered_tool_attrs
        // entry, or a tool was removed without dropping its entry.
        let helper_count = McpServer::registered_tool_attrs().len();
        let count = parsed.get("count").and_then(|v| v.as_u64()).expect("count");
        assert_eq!(count as usize, helper_count);

        let tools = parsed
            .get("tools")
            .and_then(|v| v.as_array())
            .expect("tools array");
        assert_eq!(tools.len(), helper_count);

        // Every entry surfaces a non-empty name + title and the four
        // hint bits (some may be JSON `null` for tools where the bit
        // is intentionally unset, e.g. read-only tools omit
        // destructive_hint per FR-29).
        for entry in tools {
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .expect("name str");
            assert!(!name.is_empty(), "tool name must not be empty");
            let title = entry
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("{name}: title must be present"));
            assert!(!title.is_empty(), "{name}: title must not be empty");
            assert!(entry.get("read_only").is_some(), "{name}: read_only key");
            assert!(entry.get("destructive").is_some(), "{name}: destructive key");
            assert!(entry.get("idempotent").is_some(), "{name}: idempotent key");
            assert!(entry.get("open_world").is_some(), "{name}: open_world key");
        }
    }

    #[tokio::test]
    async fn describe_tools_includes_describe_tools_itself() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let res = server
            .describe_tools(Parameters(DescribeToolsArgs::default()))
            .await
            .expect("describe_tools");
        let parsed = parse_ok_json(res);
        let tools = parsed
            .get("tools")
            .and_then(|v| v.as_array())
            .expect("tools array");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
            .collect();
        assert!(
            names.contains(&"describe_tools"),
            "describe_tools must appear in its own listing; got: {names:?}",
        );
    }

    /// FR-32: write_memory's `override` arg surfaces as a destructive
    /// hint via `describe_tools`, even though the tool itself is
    /// flagged `destructive_hint = false` (additive in the common
    /// case). Harnesses that match on the tool-level bit alone would
    /// miss the silent overwrite — the per-arg hint closes the gap.
    #[tokio::test]
    async fn describe_tools_surfaces_arg_risk_hints_for_write_memory_override() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let res = server
            .describe_tools(Parameters(DescribeToolsArgs::default()))
            .await
            .expect("describe_tools");
        let parsed = parse_ok_json(res);
        let tools = parsed
            .get("tools")
            .and_then(|v| v.as_array())
            .expect("tools array");
        let write_memory = tools
            .iter()
            .find(|t| t.get("name").and_then(|v| v.as_str()) == Some("write_memory"))
            .expect("write_memory entry");
        let hints = write_memory
            .get("arg_risk_hints")
            .and_then(|v| v.as_array())
            .expect("arg_risk_hints array");
        let override_hint = hints
            .iter()
            .find(|h| h.get("arg").and_then(|v| v.as_str()) == Some("override"))
            .expect("override hint");
        assert_eq!(
            override_hint.get("kind").and_then(|v| v.as_str()),
            Some("destructive"),
        );
        assert_eq!(
            override_hint.get("risk_when").and_then(|v| v.as_str()),
            Some("true"),
        );
        assert!(
            override_hint
                .get("reason")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "reason must be non-empty",
        );
    }

    /// FR-32: read-only tools have no risky args; the field still
    /// appears as an empty array so callers can match on shape
    /// without an `Option` branch.
    #[tokio::test]
    async fn describe_tools_arg_risk_hints_empty_for_read_only_tools() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let res = server
            .describe_tools(Parameters(DescribeToolsArgs::default()))
            .await
            .expect("describe_tools");
        let parsed = parse_ok_json(res);
        let tools = parsed
            .get("tools")
            .and_then(|v| v.as_array())
            .expect("tools array");
        let read_memory = tools
            .iter()
            .find(|t| t.get("name").and_then(|v| v.as_str()) == Some("read_memory"))
            .expect("read_memory entry");
        let hints = read_memory
            .get("arg_risk_hints")
            .and_then(|v| v.as_array())
            .expect("arg_risk_hints array even for read-only tools");
        assert!(hints.is_empty(), "read_memory must have no risky args");
    }

    /// FR-34: when every registered tool carries annotations (the
    /// real surface after FR-29), the diagnose helper emits zero
    /// `missing_tool_annotations` notes.
    #[test]
    fn collect_missing_annotation_notes_is_empty_on_real_surface() {
        let tools = registered_tool_attrs();
        let notes = collect_missing_annotation_notes(&tools);
        assert!(
            notes.is_empty(),
            "real tool surface must have no missing annotations; got: {notes:?}",
        );
    }

    /// FR-34: a synthetic tool with no `annotations` slot surfaces
    /// as a `missing_tool_annotations` warn note.
    #[test]
    fn collect_missing_annotation_notes_flags_unannotated_tool() {
        // Take one real tool, blank its annotations to simulate a
        // future-tool slip-through. The FR-29 build-time test
        // prevents this on the live surface; this test guards the
        // runtime check itself.
        let mut tools = registered_tool_attrs();
        let mut victim = tools.remove(0);
        victim.annotations = None;
        let victim_name = victim.name.to_string();
        tools.insert(0, victim);

        let notes = collect_missing_annotation_notes(&tools);
        assert_eq!(notes.len(), 1, "exactly one missing-annotations note");
        let note = &notes[0];
        assert_eq!(note.level, mmcp_proto::NoteLevel::Warn);
        assert_eq!(note.code, "missing_tool_annotations");
        let context = note.context.as_ref().expect("context payload");
        assert_eq!(
            context.get("tool").and_then(|v| v.as_str()),
            Some(victim_name.as_str()),
        );
    }

    /// FR-34: diagnose surfaces the annotation-coverage check on the
    /// notes channel. With the real tool surface this stays clean,
    /// so no `missing_tool_annotations` codes appear in the output.
    #[tokio::test]
    async fn diagnose_does_not_flag_annotations_on_clean_surface() {
        let (state, _tmp) = test_state().await;
        let server = McpServer::new(state, ServeMode::Full);
        let res = server
            .diagnose(Parameters(CheckHealthArgs { group: None }))
            .await
            .expect("diagnose");
        let parsed = parse_ok_json(res);
        let codes: Vec<String> = parsed
            .get("notes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| n.get("code").and_then(|c| c.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !codes.iter().any(|c| c == "missing_tool_annotations"),
            "real surface should not surface missing_tool_annotations; got: {codes:?}",
        );
    }
}
