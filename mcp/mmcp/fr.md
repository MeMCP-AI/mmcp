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
