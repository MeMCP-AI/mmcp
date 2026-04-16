# cargo-mcp Feature Requests

### FR-001: `cargo_check` for out-of-workspace crates (2026-04-16)

**Need**: Phase 8 introduced `webui/` as a separate Cargo project (excluded from workspace via `workspace.exclude`). `cargo_check` with `path` pointing to the webui directory doesn't work because the tool operates relative to the workspace root. Had to fall back to `Bash` with `cd webui && cargo check --features ssr`.

**Proposed**: Accept an absolute `path` parameter that `cd`s into the target directory before running cargo, or add a `manifest_path` parameter mapping to `--manifest-path`.

### FR-002: `cargo_fmt_check` with path parameter (2026-04-16)

**Need**: Want to run `cargo fmt --check` on the webui sub-project separately from the main workspace. Same issue as FR-001 — the tool doesn't support operating on a separate project root.

### FR-003: `cargo_clippy` with `--deny warnings` flag (2026-04-16)

**Need**: The plan's verification step requires zero clippy warnings. Currently I check the stderr output manually. A `deny_warnings: bool` parameter that maps to `-- -D warnings` would make the exit code sufficient to determine pass/fail.
