# git-mcp Feature Requests

### FR-001: `git_diff` with `--stat` equivalent (2026-04-16)

**Need**: Before committing, I frequently need a quick summary of changed files + insertions/deletions. The `git_diff` tool gives full diff content, but a stat mode (like `git diff --stat`) would save context. Currently I use `stat: true` which works, but the documentation doesn't mention it — discovered by trial.

### FR-002: `git_log` with `--oneline` or `--format` (2026-04-16)

**Need**: When checking recent commits for commit message style, a compact one-line-per-commit format would be much more efficient than full log output. A `format` or `oneline` parameter would help.

### FR-003: `git_commit` with heredoc-style message (2026-04-16)

**Need**: Multi-line commit messages with `\n` escapes work, but the JSON string encoding makes it harder to read/verify the message shape before committing. A raw multi-line parameter or separate `title`/`body` fields would improve ergonomics.

### FR-004: `git_filter_branch` / `git_rewrite_history` tool (2026-04-16)

**Need**: Had to rewrite 26 commits to strip `Co-Authored-By` trailers that violated project conventions. Had to fall back to `Bash` with `git filter-branch`. A dedicated tool for history rewrites (remove/edit trailers, squash, reword en masse) would be safer and more discoverable than raw filter-branch.

**Proposed**: `git_rewrite_history(range, msg_filter: regex, replacement)` or separate tools `git_strip_trailer(name, range)`, `git_reword(commit, new_message)`.

### FR-005: `git_amend` tool with explicit intent (2026-04-16)

**Need**: `git_commit --amend` exists but requires knowing the field. A dedicated `git_amend(message?, add_files?)` tool would make the intent explicit and safer than the current commit-with-amend-flag pattern.

### FR-006: Commit message linting / trailer validation (2026-04-16)

**Need**: No way to validate commit messages against project conventions (no Co-Authored-By, subject + body required, etc.) before creating the commit. A `git_validate_message(message, rules)` tool or a pre-commit lint would have caught the 26 violations immediately.
