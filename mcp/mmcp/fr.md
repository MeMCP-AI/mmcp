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
