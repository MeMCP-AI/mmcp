# mmcp (local MCP) Feature Requests

### FR-001: Expose `read_memory` tool (2026-04-16)

**Need**: The local mmcp MCP currently exposes `list_memories`, `list_versions`, `group_info`, and `search_memories`. But `read_memory` — the most critical tool for actually reading memory content — is not exposed. This is the highest priority gap.

### FR-002: Expose `write_memory` tool (2026-04-16)

**Need**: To edit memories through mmcp, a `write_memory` tool is needed. Currently all writes go through Serena or direct git operations.

### FR-003: Group creation via MCP tool (2026-04-16)

**Need**: Creating groups currently requires either the server API or direct git operations. A `create_group` tool exposed via MCP would enable end-to-end workflows without leaving the AI conversation.
