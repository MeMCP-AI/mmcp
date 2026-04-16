# mmcp (local MCP) Feature Requests

### FR-001: Expose `read_memory` tool (2026-04-16) - RESOLVED

**Need**: The local mmcp MCP currently exposes `list_memories`, `list_versions`, `group_info`, and `search_memories`. But `read_memory` - the most critical tool for actually reading memory content - is not exposed. This is the highest priority gap.

**Status**: Resolved on 2026-04-16. `read_memory` now available with `group`, `slug`, and optional `version` parameters.

### FR-002: Expose `write_memory` tool (2026-04-16) - RESOLVED

**Need**: To edit memories through mmcp, a `write_memory` tool is needed. Currently all writes go through Serena or direct git operations.

**Status**: Resolved on 2026-04-16. `write_memory` now available with fully typed parameters: `group`, `slug`, `name`, `description`, `kind` (enum), `body`, `tags`, `mandatory`. Server builds frontmatter - AI never touches TOML.

### FR-003: Group creation via MCP tool (2026-04-16)

**Need**: Creating groups currently requires either the server API or direct git operations. A `create_group` tool exposed via MCP would enable end-to-end workflows without leaving the AI conversation.

**Status**: Open.

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
