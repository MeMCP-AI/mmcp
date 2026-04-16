# git-mcp Feature Requests

### FR-001: `git_diff` with `--stat` equivalent (2026-04-16)

**Need**: Before committing, I frequently need a quick summary of changed files + insertions/deletions. The `git_diff` tool gives full diff content, but a stat mode (like `git diff --stat`) would save context. Currently I use `stat: true` which works, but the documentation doesn't mention it — discovered by trial.

### FR-002: `git_log` with `--oneline` or `--format` (2026-04-16)

**Need**: When checking recent commits for commit message style, a compact one-line-per-commit format would be much more efficient than full log output. A `format` or `oneline` parameter would help.

### FR-003: `git_commit` with heredoc-style message (2026-04-16)

**Need**: Multi-line commit messages with `\n` escapes work, but the JSON string encoding makes it harder to read/verify the message shape before committing. A raw multi-line parameter or separate `title`/`body` fields would improve ergonomics.
