# cargo-mcp Feature Requests

### FR-001: `cargo_check` for out-of-workspace crates (2026-04-16)

**Need**: Phase 8 introduced `webui/` as a separate Cargo project (excluded from workspace via `workspace.exclude`). `cargo_check` with `path` pointing to the webui directory doesn't work because the tool operates relative to the workspace root. Had to fall back to `Bash` with `cd webui && cargo check --features ssr`.

**Proposed**: Accept an absolute `path` parameter that `cd`s into the target directory before running cargo, or add a `manifest_path` parameter mapping to `--manifest-path`.

### FR-002: `cargo_fmt_check` with path parameter (2026-04-16)

**Need**: Want to run `cargo fmt --check` on the webui sub-project separately from the main workspace. Same issue as FR-001 — the tool doesn't support operating on a separate project root.

### FR-003: `cargo_clippy` with `--deny warnings` flag (2026-04-16)

**Need**: The plan's verification step requires zero clippy warnings. Currently I check the stderr output manually. A `deny_warnings: bool` parameter that maps to `-- -D warnings` would make the exit code sufficient to determine pass/fail.

### FR-004: `cargo_fmt_check` summary mode for workspace-wide drift (2026-04-17)

**Need**: Running `cargo_fmt_check` on a workspace with ~40 drifted files produced 105 KB of diff output, which exceeded the MCP tool-result budget and had to be dumped to a file. Reading the file back for analysis was expensive. A `summary: bool` mode that returns just the list of files needing formatting (like `rustfmt --emit files-with-diff` or a simple `files_with_diffs: Vec<String>` field) would make fmt audits practical without blowing context budget.

**Proposed**: `cargo_fmt_check(summary: bool = false)`. When `summary=true`, emit a compact JSON `{ files_with_diffs: ["path", ...], total: N }` instead of the raw rustfmt output.

### FR-005: `cargo_llvm_cov` wrapper (2026-04-17)

**Need**: Coverage audits are a recurring need and currently go through `Bash` with `cargo llvm-cov --summary-only`. A first-class `cargo_llvm_cov` tool with summary/per-file/html modes would make coverage reports scriptable from AI sessions without shelling out.

**Proposed**: `cargo_llvm_cov(summary_only: bool, html: bool, package: Option<String>)`. Returns either the per-file table or a structured JSON representation.
