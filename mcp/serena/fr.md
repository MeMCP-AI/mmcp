# serena Feature Requests

### FR-001: Batch `read_memory` (2026-04-16)

**Need**: CLAUDE.md mandates reading all `global_*` memories at session start. This requires 5-8 sequential `read_memory` calls. A `read_memories(names: [])` batch operation would cut the round trips and speed up session initialization.

### FR-002: Memory search by type/prefix (2026-04-16)

**Need**: `list_memories` returns all memories. Filtering by prefix (e.g. `global_`) or by a tag/type requires client-side string matching. A `list_memories(prefix: "global_")` filter would be more efficient.
