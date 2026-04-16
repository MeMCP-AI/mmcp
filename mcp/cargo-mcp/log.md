# cargo-mcp Issue Log

## 2026-04-16 — cargo_clippy parameter deserialization failure

**Error**: `MCP error -32602: failed to deserialize parameters: invalid type: string "true", expected a boolean`

**Context**: Called `cargo_clippy` with `all_targets: true`. The MCP server rejected the boolean parameter, treating it as a string `"true"` instead of the boolean `true`.

**Workaround**: Re-fetched the tool schema via `ToolSearch`, then re-invoked. Second call succeeded — likely a transient serialization mismatch after the tool was initially loaded from the deferred list.

**Status**: Resolved on retry. Root cause unclear — may be a race between schema loading and invocation.

