# mmcp (local MCP) Feature Requests

### FR-001: Expose `read_memory` tool (2026-04-16) - RESOLVED

**Need**: The local mmcp MCP currently exposes `list_memories`, `list_versions`, `group_info`, and `search_memories`. But `read_memory` - the most critical tool for actually reading memory content - is not exposed. This is the highest priority gap.

**Status**: Resolved on 2026-04-16. `read_memory` now available with `group`, `slug`, and optional `version` parameters.

### FR-002: Expose `write_memory` tool (2026-04-16) - RESOLVED

**Need**: To edit memories through mmcp, a `write_memory` tool is needed. Currently all writes go through Serena or direct git operations.

**Status**: Resolved on 2026-04-16. `write_memory` now available with fully typed parameters: `group`, `slug`, `name`, `description`, `kind` (enum), `body`, `tags`, `mandatory`. Server builds frontmatter - AI never touches TOML.

### FR-003: Group creation via MCP tool (2026-04-16) - RESOLVED

**Need**: Creating groups currently requires either the server API or direct git operations. A `create_group` tool exposed via MCP would enable end-to-end workflows without leaving the AI conversation.

**Status**: Resolved (2026-04-17). Project-group bootstrap is unified into a single `init project` surface on both the CLI and MCP sides.

The first resolution pass split responsibilities across `mmcp init` (writes `.mmcp.toml`) and `mmcp init project` (creates the bare repo) — an AI session driving mmcp through MCP was blocked at step one because no tool could write `.mmcp.toml`. The follow-up refactor merged them: `mmcp init project [--slug <s>] [--config-only] [--project-uuid <u>]` (CLI) and `init_project({slug?, config_only?, project_uuid?})` (MCP) both bootstrap the project config and the backing group repo in one idempotent call, with `--config-only` for operators adopting a server-side project whose repo will land via `sync_pull`.

`ProjectConfig` now carries `project_slug: Option<String>` (serde-default-backward-compatible) so the canonical project name lives in the checked-in `.mmcp.toml` rather than only in the bare repo's group manifest. Slug resolution order on both paths: `slug` arg → stored `project_slug` → (CLI only) TTY prompt defaulting to the slugified project dir basename → `slug_required` error. The never-overwrite rule is strict: bare repo never re-initialized, `.mmcp.toml` never rewritten to change `project_uuid`, slug backfill the only enrichment allowed on existing configs. Disagreement between supplied args and stored values surfaces as `project_uuid_mismatch` or `slug_mismatch` rather than silent coercion. Bare `mmcp init` is no longer a leaf command — clap renders help listing the `project` and `claude` subcommands.

Elicitation-based slug defaulting on the MCP side is still gated on FR-011 (rmcp elicitation support). Shared-group creation — multiple repos per project keyed by the `ProjectConfig.groups.additional` slot — is explicitly deferred until that field's format is nailed down; the current plan is a separate `init shared` surface when the time comes.

### FR-005: Health + Diagnose tools (2026-04-16) - RESOLVED

**Need**: Way to validate manifest and memory file integrity, both surface-level and in-depth.

**Status**: Resolved. Two-level system delivered: `check_health` (surface errors only) and `diagnose` (deep analysis with info/warning hints across per-group + project-level concerns). Never auto-fixes - diagnostics only.

### FR-006: Universal frontmatter support (2026-04-16) - RESOLVED

**Need**: Memory files should accept any of TOML/YAML/JSON frontmatter, not just TOML `+++`.

**Status**: Resolved via gray_matter crate. Read auto-detects TOML `+++`, YAML `---`, JSON `---`, TOML `---`. Write preserves original format. Explicit `normalize()` for TOML conversion.

### FR-007: Feature request tracking integrated into mmcp (2026-04-17)

**Need**: FRs currently live in flat markdown files under `mcp/<server>/fr.md` at the project root. Once mmcp stabilizes, these could live as memories in a dedicated "mmcp-feedback" group synced to the mmcp-server so FRs survive across machines and can be queried via the MCP itself.

**Status**: Open. Waiting for server deployment.

### FR-004: Debug/raw access tools (2026-04-16) - RESOLVED

**Need**: Sometimes need raw git access for troubleshooting - read/write arbitrary files, list tree, inspect commits.

**Status**: Resolved on 2026-04-16. Five debug tools added, gated behind `--debug` flag and runtime `debug_toggle`:
- `debug_toggle(enabled)` - runtime enable/disable
- `debug_read_file(group, path, rev?)` - read any file
- `debug_write_file(group, path, content, message?)` - write any file
- `debug_list_tree(group, prefix?, rev?)` - list blobs
- `debug_git_log(group, path?, limit?)` - raw commit history

### FR-008: `bootstrap_context` tool + server-instruction delivery (2026-04-17) - RESOLVED

**Need**: The AI had no single entry point to load its mandatory + project-scoped memories at session start. Slug names and project-stack "YES/NO" flags lived in CLAUDE.md which drifted against the real memory store. Needed a tool-driven bootstrap where the server decides what to load, based on frontmatter flags and project config, and CLAUDE.md becomes a pointer.

**Status**: Resolved. `bootstrap_context(scope?)` returns memories with bodies inline; `all`/`mandatory`/`project` scopes. Emits structured diagnostics (`claude_md_missing` / `claude_md_unmanaged` / `claude_md_stale`) suggesting `init_claude` when appropriate. Session-start protocol moved into `get_info().instructions` so every MCP client sees the checkpoint list on handshake.

### FR-009: `init_claude` tool + `mmcp init claude` CLI (2026-04-17) - RESOLVED

**Need**: No programmatic way to generate, append to, or convert CLAUDE.md. Hand-editing produced drift against the real tool surface (wrong slug casing, wrong project flags). Needed a managed file with a versioned fence so mmcp updates can upgrade the block atomically.

**Status**: Resolved. `init_claude(action, backup?, dry_run?, on_conflict?, path?)` and `mmcp init claude [--override|--convert|--append]`. Convert splits content into typed memories (not a single-blob import). Fenced block `<!-- mmcp:begin v1 --> ... <!-- mmcp:end v1 -->` with DO-NOT-EDIT marker. Structured `conflict_unresolved` error when file is dirty/untracked — shape matches future MCP elicitation exchange.

### FR-010: Enumerate groups tool (2026-04-17)

**Need**: `list_memories` and `group_info` both require a group UUID up front, but there is no pure "list all groups the caller has access to" MCP tool. `bootstrap_context` now returns groups implicitly (via the memories it surfaces), but a standalone `list_groups` would simplify admin flows and exploration from an AI session that hasn't been bootstrapped yet.

**Status**: Open. `bootstrap_context` partially addresses this, but an explicit `list_groups(owner_scope?)` would be clearer and cheaper (no memory body fetches).

### FR-011: MCP elicitation support for conflict resolution (2026-04-17)

**Need**: `init_claude` currently returns a structured `conflict_unresolved` error when the target file is dirty/untracked and `on_conflict` is not pre-supplied. This forces a retry-with-answer pattern on the caller. When rmcp exposes `ElicitationRequest`, swap the error for a live prompt so the AI can answer synchronously mid-tool-call. Wire contract (choice enum: `override` / `backup_override` / `cancel`) already matches elicitation schema shape.

**Status**: Open. Blocked on rmcp exposing elicitation. Internal refactor only; external API stays stable.

### FR-012: `list_groups` / `enumerate_groups` for bootstrapping without cwd (2026-04-17)

**Need**: `bootstrap_context` discovers the project group by reading `.mmcp.toml` from the MCP server's current working directory. That's correct for the common case but fragile: if the server is launched from a different cwd than the project, the project_uuid resolution fails silently and `scope=project` returns empty. Consider accepting an optional explicit `project_root` arg, or an explicit `project_uuid`, in `bootstrap_context`.

**Status**: Open.

### FR-014: Expose `sync_pull` / `sync_push` / `sync` MCP tools (2026-04-17) - RESOLVED

**Need**: The sync engine in `mmcp-sync` already powers three CLI subcommands (`mmcp pull`, `mmcp push`, `mmcp sync`) but none of them are reachable through MCP. An AI session can write memories into the local mirror via `write_memory`, but has no way to propagate those commits to the configured `[sync] server_url` without the operator dropping to a shell and running the CLI. Symmetric gap on reads: the session cannot refresh the local mirror against upstream, so edits made elsewhere stay invisible until a manual `mmcp pull` runs.

**Status**: Resolved. Three new MCP tools — `sync_pull`, `sync_push`, `sync` — now wrap the same `SyncEngine` the CLI uses via a shared `commands::sync::build_engine` helper. Missing-config cases surface as a structured `sync_not_configured` error carrying `project_uuid` + retry hint; engine failures map to distinct codes (`sync_conflict`, `sync_remote`, `sync_transport`, `sync_not_found`, `sync_git`, `sync_invalid_version`) so callers branch without parsing human messages. `GroupHandleResolver` picked up a `Send + Sync` supertrait bound to satisfy the rmcp router's future bounds — no behavior change because every existing resolver was already thread-safe.

### FR-015: Expose `status` MCP tool for local sync state (2026-04-17) - RESOLVED

**Need**: `mmcp status` prints the local project's sync state (configured server, groups mirrored, pending commits, last push). Without an MCP equivalent an AI session has to either read `.mmcp.toml` manually or call several tools in sequence (`bootstrap_context` + `group_info` per group + `list_versions`) to approximate the same picture. Adds friction before every pull/push decision.

**Status**: Resolved. New `status` MCP tool returns `{project_configured, project_root, project_uuid, sync: {configured, server_url?}, groups: [{slug, uuid, memory_count}]}`. Pure-local, no network; `project_configured: false` when no `.mmcp.toml` is in scope (no error) so the AI can decide whether to prompt for `mmcp init`. Mirrored groups are surfaced regardless of project presence, which is useful for diagnosing machine-wide mirror state from outside any project.

### FR-013: `list_memories` should signal "group not in local mirror" (2026-04-16)

**Need**: Today `list_memories(group)` returns `{"memories": []}` in two very different situations: (a) the group is mirrored locally but contains no memories, and (b) the group is not in the local mirror at all. `group_info(group)` on the same UUID errors with `group not found in local mirror`. That asymmetry is easy to misread from an AI session — an empty list looks like "nothing to do" when the real state is "you are looking at the wrong place." Surfaces as wasted checkpoints and skipped rule reads.

**How to apply**: Either mirror `group_info`'s error path in `list_memories` (return the same "not found" error when the group is absent from the index), or extend the response shape to `{"memories": [...], "mirrored": true|false}` so callers can distinguish the two states without a second call.

**Status**: Open. Discovered while running the re-read checkpoint against the project group, which is not mirrored locally yet — the empty response led to a premature stop.

### FR-016: `edit_memory` tool for partial updates (2026-04-17) - RESOLVED

**Need**: The current `write_memory` MCP tool is the only write path. It requires every frontmatter field (name, description, kind, tags, mandatory) on every call and rebuilds the file from those typed args — so an AI session that "just wants to fix a typo in the body" must re-specify all metadata defensively, risking silent drift when a field is forgotten. There is no way to mutate one slice of a memory in place.

**Status**: Resolved (2026-04-17). New `edit_memory(group, slug, body?, name?, description?, kind?, tags_add?, tags_remove?, mandatory?, message?)` tool reads the existing `MemoryFile` via the shared `update_memory_file` primitive, applies per-field deltas, re-renders, and commits. All mutator fields are optional; absent fields leave their slice untouched. `tags_add` / `tags_remove` are additive operators with sort+dedup so repeated calls converge. Errors with code `memory_not_found` when the slug is absent; `map_memory_error_to_mcp` shares the same payload shape with `write_memory` and `delete_memory`.

### FR-017: `delete_memory` tool (2026-04-17) - RESOLVED

**Need**: `write_memory` and the forthcoming `edit_memory` can create and update, but there is no supported path to remove a memory through MCP. `debug_write_file` with an empty body is a workaround, and it requires debug mode enabled — not a viable surface for an AI session iterating on a draft rule and wanting to discard it.

**Status**: Resolved (2026-04-17). New `delete_memory(group, slug, message?)` tool commits a file deletion via the shared `delete_memory_file` primitive (which leans on `build_tree`'s `(path, None)` delete semantic on the native backend). Returns `{group, slug, commit_id}`. Double-delete errors `memory_not_found` rather than silently succeeding. The removal shows up in `list_versions` like any other mutation so it is auditable.

### FR-018: `write_memory` becomes strict CREATE with an `override` escape hatch (2026-04-17) - RESOLVED

**Need**: Today `write_memory` silently overwrites when the slug already exists, which is the wrong default for an AI session iterating on rules: the natural "re-run defensively" pattern blasts away any refinements a subsequent `edit_memory` would have applied. The tool needs to be CREATE-only by default, with an explicit opt-in when the caller genuinely wants replace-whole-file behavior.

**Status**: Resolved (2026-04-17). `WriteMemoryArgs` gained `override: bool` (serde-renamed from the Rust field `override_` to sidestep the keyword). Default false → strict CREATE via `create_memory_file`; collision errors `memory_already_exists` with a retry hint naming `edit_memory`, `delete_memory`, and the `override: true` escape hatch. Default true → continues today's replace-whole-file behavior through the shared `import_memory` upsert wrapper, now with `"replaced": true` on the response so audit trails distinguish the two flows.

Per user direction ("full consistency across surfaces"), the CLI `mmcp import` command mirrors the semantics: new `--override` flag, strict CREATE by default, with the collision error naming both `--override` and `mmcp__edit_memory` so operators see the escape hatches explicitly. The `SESSION_INSTRUCTIONS` constant was rewritten to lead with the three-tool CRUD separation (create / edit / delete) so AI sessions see the contract on handshake.

### FR-019: Elicitation-gated guard on mutations to protected groups (2026-04-17) - RESOLVED

**Need**: Global memories are cross-project shared rules — a single accidental or misguided modification propagates to every project that pulls the shared group. Today the CRUD surface treats them identically to scratch notes: an AI session can override, edit, or delete `global-coding-rules` just as easily as a per-project note. The natural AI pattern of "defensive writing" becomes dangerous at the global scope. A guard is needed so any mutation of a protected memory requires explicit, user-visible confirmation delivered through MCP elicitation — no bool-arg escape hatch on the MCP path, no silent pass-through.

**Status**: Resolved (2026-04-17) in its pre-elicitation form. `GroupManifest.protected: bool` carries the flag (serde-default-false, skip-when-false, backward-compatible with pre-flag manifests). The `ensure_not_protected` helper gates every MCP mutation path — `write_memory` (create AND override), `edit_memory`, `delete_memory`, and the debug-mode `debug_write_file` escape hatch — with a stable `{code: "protected_requires_elicitation", group_slug, group_id, slug, action, retry_hint}` payload. Read-only paths stay unaffected. No bool-arg bypass on the MCP surface.

CLI parity lands via `protected_confirm` in `mmcp import`: TTY invocations fire `inquire::Confirm` (default no); non-TTY invocations require `--force`. When rmcp ships elicitation (FR-011), the guard migrates to firing an `ElicitationRequest` with the same `{group, slug, action}` context — the wire `code` stays stable so callers that already branch on it continue to work. Flipping an existing group's manifest to `protected: true` stays an operational follow-up: the code is in place but no group in this tree is marked protected yet.

**How to apply**:

1. **Identify protection**. Add `protected: bool` (serde-default `false`, serde-skip-when-false) to `GroupManifest` so any group can opt in without schema churn. Set `true` on the `global` group's manifest at seed time; future protected groups (language convention groups, shared team standards) opt in the same way. The flag lives in the manifest rather than the project config so it travels with the repo — a project pulling the `global` group inherits the protection automatically.

2. **Guard the three write paths**. `write_memory` (with `override: true` from FR-018), `edit_memory` (FR-016), and `delete_memory` (FR-017) consult the target group's manifest and, when `protected == true`, issue an MCP `ElicitationRequest` describing: group slug, memory slug, action (`create_override` / `edit` / `delete`), diff preview for `edit`, and a checkbox confirming intent. The write proceeds only after a positive elicitation response; a decline or cancel returns `{code: "protected_write_cancelled"}`.

3. **Pre-elicitation graceful degradation**. Until FR-011 lands (rmcp exposes elicitation), the MCP surface hard-errors with `{code: "protected_requires_elicitation", group, slug, action}` on every mutation targeting a protected group. No bypass — explicitly no `confirm_protected_write: true` back-door, which would defeat the point of routing confirmation through the user. Operators who genuinely need to mutate global memories today run the CLI (which has operator-intent-by-construction), same as the shared-group bulk flows.

4. **CLI parity**. The CLI's `mmcp import`, `mmcp memory delete`, and future `mmcp memory edit` do *not* require elicitation — the operator at a shell IS the confirmation — but they print a clear "writing into a protected group" notice before committing, and gate the write behind an interactive `inquire::Confirm` prompt on a TTY (non-TTY requires `--force`). This matches the CLAUDE.md / dirty-file prompt pattern already used by `mmcp init claude`.

5. **Read paths stay open**. `list_memories`, `read_memory`, `list_versions`, `group_info`, `search_memories`, `bootstrap_context` are unaffected. Protection is a write concern only.

**Status**: Open. Depends on FR-016 / FR-017 / FR-018 for the surface the guard attaches to, and on FR-011 for the fully-featured elicitation path. Until FR-011 ships, the MCP side is hard-gated (operators use the CLI), which is acceptable because protected-group mutations are rare and deliberate by definition.

### FR-020: Extract shared lib crate `mmcp-store` from `mmcp-client` (2026-04-17)

**Need**: `mmcp-client` is a standalone executable and the project owner has reaffirmed it must stay one — it is not a library, and no other workspace member may depend on it. Today, however, `crates/mmcp-client/src/lib.rs` exposes `pub mod commands; pub mod config; pub mod home; pub mod state;`, which is a de facto library surface mixing three very different kinds of code in one crate:

1. **CLI dispatch glue** — the clap subcommand tree, TTY prompting via `inquire`, progress printing, exit-code shaping. These are binary-only concerns and must stay binary-only.
2. **MCP protocol marshalling** — `commands/serve.rs` accepts `rmcp::CallToolRequestParam`, routes to tool handlers, serialises `serde_json::Value` responses. These are protocol-bound and belong next to the CLI, not in a general-purpose library.
3. **Genuinely reusable store logic** — home-directory resolution, group-index construction, project-config loading, the non-protocol memory CRUD helpers (`list_memory_files`, `read_memory_descriptor`, and the write / edit / delete counterparts introduced by FR-016 / FR-017 / FR-018), sync orchestration wrappers over `mmcp-sync`, and `check_health` / `diagnose` bodies. These are pure Rust logic that any local-first mmcp consumer — CLI, MCP server, GUI, integration tests — needs verbatim.

A second consumer has now arrived: `mmcp-gui`, a desktop visual client. It needs category 3 and nothing else. With the current layout, `mmcp-gui` would have to either depend on `mmcp-client` (contradicting the "client is bin-only" policy) or re-implement category 3 from scratch (violating `global-coding-rules` rule 3 SSOT and rule 4 DRY). Both are unacceptable.

**Resolution**: extract category 3 into a new workspace crate, `mmcp-store`, that owns all local-first store logic and has no knowledge of clap, rmcp, inquire, or exit codes.

**Proposed module layout for `crates/mmcp-store/`**:

- `mmcp_store::home` — `MmcpHome`, `ResolvedAuthor`, `MmcpHome::discover`, `MmcpHome::from_root`, the `REPOS_SUBDIR` / `SESSIONS_SUBDIR` / `USER_CONFIG_FILE` constants. Moved verbatim from `mmcp-client/src/home.rs`.
- `mmcp_store::groups` — `GroupEntry`, `GroupIndex`, `GroupIndex::build`, `list`, `get`, `refresh`. Moved verbatim from `mmcp-client/src/state/groups.rs`. Keeps the async `Mutex`-gated cache shape.
- `mmcp_store::config` — project-config loader currently in `mmcp-client/src/config.rs`. The on-disk `ProjectConfig` struct itself already lives in `mmcp-core::config::project` and stays there; only the loader-with-fallback wrapper moves.
- `mmcp_store::memory` — typed read / write / edit / delete. Signature shape: `async fn read(home: &MmcpHome, backend: &NativeBackend, group: &GroupEntry, slug: &str, rev: Option<&str>) -> Result<MemoryFile, StoreError>`; `async fn write(home, backend, group, spec: WriteSpec) -> Result<WriteOutcome, StoreError>`; same pattern for `edit`, `delete`. No `serde_json::Value`, no `CallToolRequestParam`. Bodies are extracted from the currently-private `async fn` blocks in `mmcp-client/src/commands/serve.rs` (around lines 1597-1710 and the soon-to-land FR-016 / FR-017 / FR-018 counterparts).
- `mmcp_store::sync` — `pull`, `push`, local `status`. Thin typed wrappers around `mmcp-sync`.
- `mmcp_store::diagnostics` — `check_health` and `diagnose` with typed return structs (the `DiagnoseReport` etc. that the MCP tools already produce, but defined here as the canonical home).
- `mmcp_store::error` — one `thiserror` enum covering all store-layer failures. MCP wrappers and CLI command handlers map these into their respective outward shapes.

**After the extraction, `mmcp-client` reshapes as follows**:

- `crates/mmcp-client/src/lib.rs` is **deleted**. `mmcp-client` becomes `[[bin]]`-only, matching the project owner's intent. No other workspace member can depend on it because it no longer exposes a library target.
- `crates/mmcp-client/Cargo.toml` adds a `mmcp-store = { workspace = true }` dep and drops whatever is now dead.
- Every call site in `mmcp-client/src/main.rs`, `commands/*.rs`, `state/*.rs` either (a) is deleted because it was category 3 and now lives in `mmcp-store`, or (b) becomes a one-line delegation: clap handler parses args → calls `mmcp_store::*` → formats the result for the terminal; `rmcp` tool handler deserialises the request → calls `mmcp_store::*` → serialises the `TypedOutcome`. Both wrapper paths should be so thin they are obviously correct.
- Existing integration tests under `crates/mmcp-client/tests/` that currently import from `mmcp-client::*` migrate their imports to `mmcp-store::*`. No behavioural change.

**Test fixtures**: the home-resolution / seeded-bare-repo test helpers currently used by `mmcp-client` integration tests move to `mmcp-store::testing` under a `#[cfg(any(test, feature = "testing"))]` gate so `mmcp-gui` and any future consumer can reuse the same fixtures (this is filed separately as FR-022, but the `testing` module shape is cheapest to introduce during the initial extraction rather than as a follow-up).

**Non-goals for FR-020**: no API-shape redesign beyond the move; no renaming of functions unless their current name is misleading outside the MCP context (e.g. `read_memory_descriptor` → `read_memory` is reasonable because "descriptor" was an MCP-response-shape term); no change to on-disk layout, frontmatter format, sync wire format, or git commit shape. Pure code relocation + signature cleanup.

**Blocks**: `mmcp-gui` phases 2-5 (everything past the empty-window scaffold commit). The scaffold itself is unblocked because it only depends on `eframe`/`egui`, and registering the new crate in the workspace.

**Status**: RESOLVED (2026-04-17). Extracted across a ten-commit chain: `mmcp-store` registered as a new workspace lib crate; home / config / groups / memory / sync / diagnostics / sessions modules migrated in order; every shim file in `mmcp-client` removed once the callers rebound to `mmcp_store::*`; `mmcp-client/src/lib.rs` deleted so the crate is `[[bin]]`-only as intended. The consolidated `StoreError` (with `#[from]` for `GitError`, `ManifestError`, `SessionError`) replaces `StateError`. Integration tests migrate to the new paths; the 131-test baseline stays green. `mmcp-gui` can now depend on `mmcp-store` directly without pulling the client bin. FR-022's testing fixtures landed in the same chain (commit 9 below).

### FR-021: Incremental sync progress hooks on `mmcp-sync` (2026-04-17)

**Need**: `mmcp-sync::{pull, push}` today return a single `Result` when the whole operation finishes. A GUI consumer (`mmcp-gui`) wants to render a determinate progress indicator — bytes transferred, refs processed, current phase (negotiation / pack transfer / index build) — so the user sees the app is doing work rather than staring at a spinner during a multi-megabyte pack transfer. The MCP `sync` / `sync_pull` / `sync_push` tools surface the same gap for any tool-calling AI that wants to report progress back to the user, though the need is less acute there because tool calls are typically short-lived.

**Resolution options**, in increasing invasiveness:

1. **Callback parameter** — extend each entry point with an optional `progress: Option<&mut dyn FnMut(SyncProgress)>` argument. Simplest; zero runtime cost when absent. Downside: mutable reference through async code is awkward; in practice this means `Arc<Mutex<dyn FnMut>>` or a `Sync + Send` bound, which is noisy at call sites.
2. **Broadcast channel** — extend each entry point with an optional `progress_tx: Option<tokio::sync::broadcast::Sender<SyncProgress>>`. Natural fit for async; GUI spawns a subscriber task that converts `SyncProgress` events into egui repaints. `mmcp-sync` emits events on phase transitions and on byte-count updates throttled to ≤10 Hz. Absent sender → no events; absent subscribers → broadcast drops silently.
3. **Typed state stream** — return `impl Stream<Item = SyncEvent>` instead of a `Result`. Cleanest API; biggest breaking change. Probably overkill for v1.

Recommend option 2. `SyncProgress` enum shape:

```rust
pub enum SyncProgress {
    PhaseStarted { phase: SyncPhase },
    BytesTransferred { phase: SyncPhase, done: u64, total: Option<u64> },
    RefProcessed { name: String, index: usize, total: usize },
    PhaseCompleted { phase: SyncPhase },
}

pub enum SyncPhase { Negotiate, ReceivePack, IndexPack, ApplyRefs, SendPack }
```

Throttling happens inside `mmcp-sync` so consumers don't need debouncing logic of their own. Non-breaking: existing call sites pass `None` and see current behaviour; new call sites pass `Some(tx)` and receive events. The GUI maps each `SyncProgress` into a `TaskOutcome` variant on its background channel → status bar repaint.

**Non-goals**: no cancellation support in this FR (that is a separate concern and requires wiring through the git backend's cooperative cancel points); no retry / resumption support (same). Progress only.

**Blocks**: nothing hard. The GUI ships with a spinner in phase 3; FR-021 upgrades that to a progress bar without protocol change.

**Depends on**: nothing. Can land independently of FR-020 — `mmcp-sync` is already a standalone lib crate. The GUI picks up the improvement whenever it arrives.

**Status**: Open.

### FR-022: Shared test fixtures via `mmcp-store::testing` (2026-04-17)

**Need**: `mmcp-client/tests/` contains fixture code that stands up a `tempfile`-backed `MMCP_HOME`, seeds bare repos with canonical group manifests, writes representative memory files, and tears the whole thing down cleanly between tests. Once `mmcp-store` exists (FR-020), multiple consumers — `mmcp-client` (CLI + MCP integration tests), `mmcp-gui` (end-to-end widget tests that exercise read / write / sync paths), and potentially `mmcp-server` (sync endpoint tests) — all want the same fixtures. Today, copying them would fork maintenance; keeping them private to `mmcp-client` locks the GUI out of the one tested path that already exists.

**Resolution**: expose fixtures from `mmcp-store::testing` under a `#[cfg(any(test, feature = "testing"))]` gate. Public surface (draft):

- `struct ScratchHome { root: TempDir, home: MmcpHome }` — constructs and owns a temp `MMCP_HOME`, returns a live `MmcpHome` pointing at it. `Drop` cleans up.
- `impl ScratchHome { fn seed_group(&self, slug: &str) -> SeededGroup; }` — creates a bare repo under `repos/`, writes a valid `GroupManifest`, returns a handle.
- `struct SeededGroup { entry: GroupEntry, ... }` — helpers: `write_memory(slug, frontmatter, body)`, `commit(message)`, `head_rev()`.
- `fn ephemeral_author() -> ResolvedAuthor` — deterministic author for reproducible commits in tests.
- Re-exports of any trait bounds (`AsyncRead`, etc.) downstream tests commonly need.

Gate with a `testing` cargo feature rather than `#[cfg(test)]` alone so downstream integration tests in *other* crates can opt in — `#[cfg(test)]` only activates for the defining crate's own tests. Naming: the feature is `testing` (idiomatic in the Rust ecosystem: `tokio`, `axum`, `sqlx` all use this name), not `test-utils` or `fixtures`.

**Scope boundary**: `mmcp-store::testing` owns fixtures for the local store surface only (home, groups, memories, manifests). It does NOT own:

- HTTP mocking for the sync server — that stays as `wiremock` usage in consumer test suites; the mocks are too HTTP-endpoint-specific to share.
- MCP protocol fixtures — those remain in `mmcp-client` or `mmcp-proto` alongside the code they exercise.
- Database fixtures — those belong to `mmcp-db` and server-side tests.

**Non-goals**: no test-framework abstraction (no `test_case!` macros, no test-harness wrappers). Plain Rust helper structs + functions; consumers call them from inside their own `#[tokio::test]` blocks.

**Blocks**: nothing urgent. `mmcp-gui` can ship phases 1-3 with unit tests on pure state logic only. Fixture-backed integration tests become valuable once the write path lands in phase 5.

**Depends on**: FR-020. The `mmcp-store` crate must exist before its `testing` module can live in it. If FR-020 is scoped to execute the extraction and the fixture move in one pass, FR-022 is effectively folded into FR-020 and can be closed as a duplicate on landing.

**Status**: RESOLVED (2026-04-17). `mmcp_store::testing` shipped behind the `testing` cargo feature with `ScratchHome`, `SeededGroup`, and `ephemeral_author`. `mmcp-client`'s `group_index` and `offline_flow` integration suites migrate to the shared fixture; `mmcp-gui` and any future consumer pick it up via `mmcp-store = { workspace = true, features = ["testing"] }`.

**Status**: Open.

### FR-023: `bootstrap_context` response is too large and gets truncated by the client harness (2026-04-17)

**Need**: `bootstrap_context` emits one MCP content item whose `text` field contains the entire `{diagnostics, memories, project_root, project_uuid}` struct serialised as JSON — with every memory body inlined as a JSON-escaped string. On real projects that breaches the Claude Code harness's per-tool-output cap: on 2026-04-17 a call on the `gitoxide` project returned 52.5 KB and the harness persisted the whole result to a sidecar file while giving the AI only a 2 KB preview. The preview cuts mid-JSON, mid-memory, mid-body. The AI never receives the mandatory-memory bodies it called the tool to load, so every downstream "re-read rules at this checkpoint" becomes a no-op against stale context. This directly undermines FR-008's contract ("bodies inline in a single round trip") and the checkpoint protocol that both `get_info().instructions` and CLAUDE.md mandate. It also gets worse as the memory set grows — any project that accumulates more than ~15 mandatory + project memories of moderate size will hit the cap. The safety net "just call `read_memory` when truncated" is unreliable: the AI usually does not notice the truncation (the preview looks like a normal partial JSON dump) and proceeds as if the memories were loaded.

**Root cause** (two compounding factors):

1. **Single giant content item with nested JSON encoding.** The tool response is `[{type: "text", text: "{\"memories\": [...]}"}]`. All bodies are escaped into one string — every `\n` becomes `\\n`, every `"` becomes `\\"`, every backslash doubles. For the gitoxide capture the raw memory bodies sum to ~36 KB; the JSON-escaped wrapper inflates that to ~52 KB. The harness measures the wrapper, not the bodies, so ~16 KB of the cap budget is pure escape overhead.
2. **No scoping knobs beyond the `scope` tri-state.** The caller cannot ask for "just the bodies for these slugs" or "metadata only, no bodies". It is all-or-nothing within each of `mandatory` / `project` / `all`. A focused checkpoint re-read that needs only `global-coding-rules` + `global-git-conventions` has to pull every mandatory memory to get those two.

**Resolution** (layered, so each layer helps independently and the layers compose):

1. **Emit one MCP content item per memory, as plain markdown text (not JSON-stringified).** Each `Content::text(...)` item holds one memory rendered as a short markdown block: a small header (name, slug, group, reason, tags) followed by the body verbatim. A final content item carries the diagnostics + project header as its own block. Three wins: (a) removes the full JSON-escape inflation layer — the wire cost drops to roughly the sum of raw bodies plus per-memory header framing; (b) when the harness truncates at the tool-result boundary it truncates whole memories, not mid-escape-sequence, so the AI sees the first N memories intact and can notice the absence of the last ones; (c) matches how MCP content arrays are meant to be used — each item is one semantic chunk. Structured data that tools currently parse (slug, group, kind, etc.) moves into the MCP item's `annotations` or a trailing machine-readable content item, so programmatic consumers keep their existing shape.

2. **Add focused filter args: `slugs: Option<Vec<String>>`, `groups: Option<Vec<Uuid>>`, `include_bodies: Option<bool>` (default `true`).** A checkpoint re-read that only needs two specific memories can pass `slugs: ["global-coding-rules", "global-git-conventions"]` and pay only for those two bodies. An initial discovery call can pass `include_bodies: false` to get the metadata manifest cheaply, then chase the bodies via `read_memory`. Filters compose with `scope`: `scope=mandatory, slugs=[…]` returns the intersection (so the caller cannot accidentally pull a non-mandatory memory by listing its slug; if strict-union semantics are preferred, revisit after FR-011). The args all serde-default to the current "give me everything in scope" behaviour so existing callers do not break.

3. **Server-side soft cap with structured degradation.** When the computed body total exceeds a configurable threshold (default 32 KB, override via env or config), the server drops to metadata-only automatically and emits a final content item with `{truncated: true, reason: "soft_cap_exceeded", soft_cap_bytes, actual_bytes, retry_hint: "call read_memory per slug, or re-call bootstrap_context with slugs=[…] to fetch a subset"}`. This prevents the silent harness truncation that is the actual failure mode today — the AI sees an explicit structured signal instead of a preview that looks like success.

4. **Surface memories as MCP resources too.** Each mirrored memory already has a stable identity `mmcp://<group-uuid>/<slug>` (optionally `@<rev>`). Registering them as MCP resources gives clients a standard read path (`ReadMcpResource`) independent of bootstrap. Useful for IDE-side UIs, for AI clients that prefer resource enumeration, and as a second fallback when bootstrap degrades to metadata-only. Read-only; writes still go through `write_memory` / `edit_memory` / `delete_memory`. This layer is optional for shipping the fix — layers 1-3 fully address the size bug — but it rounds out the surface and is cheap to add once the per-memory markdown rendering of layer 1 exists.

**How to apply**:

1. Refactor `bootstrap_context` to build a `Vec<Content>` rather than a single-item vec. Each `BootstrapMemory` → one `Content::text(render_memory_markdown(&memory))`. The render helper writes a 3–5 line header (`## <name>`, `- slug: …`, `- group: …`, `- reason: …`, `- tags: …`) and then the raw body. A leading header item covers `project_root` / `project_uuid` / diagnostics; a trailing footer item carries the truncation sentinel when degraded. Annotate each item with `annotations: { audience: ["assistant"], priority: 1.0 }` so clients that dim low-priority content do not bury memory bodies.
2. Extend `BootstrapContextArgs` with `slugs`, `groups`, `include_bodies`. Apply filters inside the existing selector pipeline — after `scope` resolves the candidate set, filters narrow it; `include_bodies=false` strips the body slice before render. Add filter validation errors with stable codes (`unknown_slug`, `unknown_group`) following the same error-shape convention as other mmcp tools.
3. Wire the soft cap through a new config knob (default `32 * 1024`). Sum `body.len()` across the filtered set; if over cap, discard bodies and set the truncation sentinel. Keep the threshold configurable per-install because harnesses differ (Claude Code ≈ 50 KB preview, other clients cap higher).
4. Document the new shape in `get_info().instructions` so the session-start protocol mentions the filter args. Update `SESSION_INSTRUCTIONS` to lead with "call bootstrap_context at checkpoints; use `slugs` / `include_bodies` for focused reloads".
5. Tests: add a size-bounded integration test that seeds 20 memories of 3 KB each, calls `bootstrap_context(scope=all)`, and asserts (a) content-item count == 20 + header, (b) no single content item exceeds a sanity bound (say 8 KB), (c) soft-cap degradation fires at the configured threshold, (d) `slugs=[a,b]` returns exactly those two + header. Snapshot one rendered markdown block to lock the header format.

**Non-goals**:

- No protocol change for `read_memory` / `edit_memory` / `delete_memory` / `list_memories` — they already return small focused payloads and do not trigger the cap.
- No compression of memory bodies on the wire. The bodies are already markdown text; compressing before JSON-encoding would help size but defeats human-readability of the persisted sidecar, which is currently the only fallback when truncation fires today.
- No change to on-disk storage, manifest shape, or sync wire format.
- MCP resources (layer 4) is optional and can land as a follow-up FR if layers 1-3 shrink the response enough that resource enumeration feels like over-engineering.

**Blocks**: real adoption of the checkpoint protocol on any project with a non-trivial memory set. Until this lands, the AI silently violates "re-read rules at every checkpoint" on larger projects because the re-read returns truncated bodies.

**Depends on**: nothing. Pure local refactor of the `bootstrap_context` handler + its arg shape. The rmcp side already supports multi-content results (`CallToolResult::content: Vec<Content>`).

**Status**: Open. Reported 2026-04-17 after observing a 52.5 KB `bootstrap_context` response get persisted-and-previewed by Claude Code on the gitoxide project, leaving the AI working against stale global-coding-rules memory content.
