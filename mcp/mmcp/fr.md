# mmcp Feature Requests

This file is an archive stub.

As of 2026-04-17, mmcp feature requests live as typed `fr` memories in
the project group rather than in a flat markdown file. The 23
pre-migration entries (FR-001 through FR-023) were moved into typed
memories verbatim and are now addressable by slug
(`fr-001-read-memory-tool`, `fr-002-write-memory-tool`, …,
`fr-023-bootstrap-context-size`).

## How to author or query FRs now

- MCP: `mcp__mmcp__add_feature` / `read_feature` / `update_feature` /
  `delete_feature` / `list_features`. See FR-007
  (`fr-007-feature-request-tracking`) for the full tool contract.
- CLI: `mmcp feature add | read | update | delete | list`. Auto-resolves
  the project group from the working directory.
- Status lifecycle: `open | resolved | blocked | deferred | duplicate`.
- Cross-refs: `depends_on` / `blocks` are typed slug lists on each FR.

## Why stub instead of delete

Kept as a sign-post so operators arriving via git history or a fresh
clone see immediately where the FR surface moved to, without having to
reconstruct the migration from commit messages.
