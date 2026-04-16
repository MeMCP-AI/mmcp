# serena Issue Log

## 2026-04-16 — Serena MCP disconnected mid-session

**Error**: System notification: "The following MCP servers have disconnected: serena"

**Context**: All 23 Serena tools became unavailable. Occurred while implementing the import feature. Likely caused by the mmcp binary rebuild (Serena and mmcp share the same process? Or user restarted the MCP host).

**Impact**: Cannot read Serena memories for the migration. However, all 15 global memory contents were already read into conversation context before disconnection.

**Status**: Reconnected later in session, then disconnected again during diagnose feature work.

## 2026-04-16 — Serena + git-mcp disconnected (second occurrence)

**Error**: Both Serena (23 tools) and git-mcp (29 tools) disconnected simultaneously.

**Context**: During health/diagnose feature development. User said "reloaded!" indicating MCP host restart.

**Status**: User confirmed reload.
