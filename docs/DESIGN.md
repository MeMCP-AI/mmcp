# mmcp — Design Document

> Status: **Draft**. Greenfield project. This document is the single source of truth until code exists.

## 1. Goal

Replace the Serena MCP memory system with a distributed, user-oriented memory MCP server. Fix the pain points of the current file-plus-index model:

- No user/org/permissions layer — Serena memories are just files on disk.
- No versioning — edits overwrite, history lives in git only if you remember to commit.
- No staleness awareness — snapshots rot silently, AI acts on outdated info.
- No session awareness — every AI session starts blind, no signal about what was read, what is mandatory, or what changed since last time.
- Smart loading policy lives in `CLAUDE.md` per project, not in the memory system itself — adding a new language means editing every project.

mmcp fixes these by modelling memories as versioned content in git repositories, fronted by a server that owns identity, permissions, and session state. It is offline-first, GitHub-style in its control layer, and exposes itself to AI clients through MCP.

## 2. High-Level Architecture

Two cleanly separated layers:

### 2.1 Content layer — git

- **One group = one git repository.**
- Memories are Markdown files with TOML frontmatter (`+++` delimited) living inside the group repo under `memories/<slug>/<uuid>.md`. Each memory carries a stable UUIDv7 primary key in frontmatter; the slug is a human-readable directory label that may repeat across memories in the same group. A legacy flat `memories/<slug>.md` layout is still reachable for mirrors that have not yet run the UUID migration.
- Each group repo also carries a single self-describing `.mmcp.toml` manifest at its root (schema version, group id, slug, display name, owner hint, creation timestamp). The manifest is the only data file in the repo that is not a user-authored memory, and it exists so each repo stays self-describing for disaster recovery, forks, exports, and the client's local group enumeration walk.
- No other manifest files, no submodules, no other server metadata is stuffed into repo files.
- Full edit history lives in git commits.
- Default git backend is **native, in-process**: `mmcp-server` owns bare repos on disk via `gix` and serves them to clients over the git smart HTTP protocol on its own `axum` port. No external git service required.
- Alternative backends (Forgejo, Gitea, GitHub, GitLab) are supported via a `GitBackend` trait; users who prefer an existing forge point mmcp at it and mmcp stores only the group-to-repo mapping.

Git does exactly what it is good at: content, history, diff, merge. Nothing else.

### 2.2 Control layer — mmcp server + database

- Users, orgs, groups (GitHub-style teams), ACLs, group-to-repo mapping, permissions and propagation.
- Session tracking, mandatory-memory state, read/verify history.
- Elicitation orchestration.
- Pure Rust, backed by a SQL database. Git knows nothing about any of it.

The server is the equivalent of GitHub's web layer; the git repos are the equivalent of the bare repos GitHub stores underneath.

### 2.3 Offline-first posture

The client works fully without a server, like `git` works fully without GitHub. The server is a sync point. Local-only projects never need a server; users can promote a local-only setup to server-synced later.

## 3. Workspace Layout

Rust workspace, edition 2024.

```
crates/
  mmcp-core/      Shared types: data model, frontmatter schema, body parser, semver logic, ACL rules
  mmcp-git/       Git storage abstraction: GitBackend trait + native gix implementation
  mmcp-store/     Local-first store layer: resolve_memory, feature CRUD, body op applier, group index
  mmcp-db/        SeaORM entities, migrations, shared queries (Postgres + SQLite)
  mmcp-auth/      Argon2 + PASETO + axum-login glue
  mmcp-proto/     MCP tool schemas shared by client and server (built on rmcp)
  mmcp-session/   Session tracking, transcript inspection, turn counter, compaction detect
  mmcp-sync/      Push/pull/diff/merge engine layered over mmcp-git
  mmcp-server/    Binary: HTTP/SSE, WebUI backend, git smart HTTP, auth, Postgres
  mmcp-client/    Binary: MCP stdio server + sync engine + CLI + hook subcommands
webui/            SvelteKit + Tailwind frontend (separate JS toolchain, no
                  Rust crossover; served by mmcp-server's static routes)
gui/              Tauri 2 + SvelteKit + Tailwind desktop client; Rust backend
                  under gui/src-tauri/ (excluded from the workspace; path deps
                  into crates/mmcp-*)
```

### Crate responsibilities

- **mmcp-core**: pure data model + logic. No I/O, no network, no async. Owns `MemoryKind`, `MemoryFrontmatter` (with UUID primary key and fluent `::new(...).with_*(...)` builder), `FeatureMetadata`, the CommonMark body parser powering FR-026 (`Section`, `parse_sections`, `render_sections`), and path conventions (`memory_path(slug, id)` + `legacy_memory_path(slug)`).
- **mmcp-git**: `GitBackend` trait and its implementations. Default `NativeBackend` uses `gix` on bare repos and persists the per-group `.mmcp.toml` manifest as a real commit on `main`. Exposes `list_tree` (blobs under a prefix) and `list_subtrees` (directories under a prefix) so consumers can walk the two-level memory layout without recursion. Alternative backends for Forgejo, Gitea, GitHub, GitLab talk to external forges over REST.
- **mmcp-store**: local-first programmatic store. One `resolve_memory(slug?, id?)` primitive for every addressing path; path-based write/delete helpers; semantic body-op applier (FR-026); feature CRUD and `rename_feature` (FR-027); group index + manifest lifecycle; diagnostics; session store. Zero dependency on `rmcp`, `clap`, or `inquire` — shared by `mmcp-client`, the Tauri backend under `gui/src-tauri/`, and any third-party Rust consumer.
- **mmcp-db**: SeaORM entity definitions and migrations. **Server-only**: the server uses it for users, orgs, ACLs, and memory version metadata. The client intentionally does not depend on it — its state lives in git repos and flat per-session files under `~/.mmcp/`.
- **mmcp-auth**: password hashing, PASETO token issuance/validation, `axum-login` traits, OAuth + passkey wiring.
- **mmcp-proto**: typed MCP tool request/response shapes and a structured `ProtoError` surface (including `NotImplemented` for gaps). Shared so client and server never drift on schemas.
- **mmcp-session**: pure compaction-detection primitives (`TranscriptSignature`, `compute_signature`, `detect_compaction`). Persistence of per-session state lives next to the consumer that owns it — the client keeps it in flat files, a future server-side representation will keep it in the database.
- **mmcp-sync**: push/pull/diff/merge engine. Uses `mmcp-git` for repo ops and `mmcp-db` for pending-push state on the server side.
- **mmcp-server** *(binary)*: `axum`-based HTTP/SSE daemon. Hosts MCP-over-HTTP, WebUI REST API, git smart HTTP, auth endpoints. Owns the Postgres database and the bare git repos on disk (when using `NativeBackend`). `#![forbid(unsafe_code)]`.
- **gui** *(separate app, not in main workspace)*: Tauri 2 desktop client. Rust backend at `gui/src-tauri/` consumes `mmcp-core`, `mmcp-git`, `mmcp-store`, `mmcp-sync` via path deps and exposes them as IPC commands grouped by concern: groups (`list_groups`, `refresh_groups`), memory CRUD (`list_memory_slugs`, `load_memory`, `create_memory` / `update_memory` / `delete_memory`), commit history (`list_memory_history`, `load_memory_at`, `diff_memory`), diagnostics (`run_diagnose`), sync (`sync_status`, `sync_pull`, `sync_push`), workspace (`pick_directory`, `set_reference_point`), settings (`load_settings`, `save_settings`), and config CRUD (`load_user_config` / `save_user_config`, `load_project_config` / `save_project_config`). A 15 s reachability probe emits `reachability:changed` events. Frontend is SvelteKit 2 + Svelte 5 runes + Tailwind v4 + Lucide icons, compiled as a pure SPA via `@sveltejs/adapter-static` and served in-process by the Tauri webview. Talks to `mmcp-store` directly — no HTTP, no MCP round-trip.
- **mmcp-client** *(binary + library)*: runs on the user's machine. Delegates every memory operation to `mmcp-store`, which keeps the CLI, the MCP tool bodies, and the Tauri-backed desktop client aligned on one programmatic surface. Keeps per-session state in flat TOML files under `~/.mmcp/sessions/`. Ships a one-shot migration example binary under `examples/` (`migrate_uuidify` for FR-028's two-level layout). Does **not** depend on `mmcp-db`. One binary, multiple entry points via `clap` subcommands:
  - `mmcp serve` — the MCP stdio server that Claude Code and other AI clients talk to
  - `mmcp init` / `status` / `sync` / `pull` / `push` — CLI workflow commands
  - `mmcp hook user-prompt` — the command invoked by the Claude Code `UserPromptSubmit` hook
  - A library target (`mmcp_client`) exposes the `commands`, `config`, and `state` modules so integration tests under `tests/` can drive them without spawning the binary.
- **webui** *(separate frontend, not in main workspace)*: SvelteKit 2 + Svelte 5 + Tailwind v4 single-page app, compiled via `@sveltejs/adapter-static`. Talks to `mmcp-server` via its REST API. Type sharing with `mmcp-core` is via JSON over the wire rather than a compiled-in Rust dependency.

### Transport

- **Client ↔ AI** (default): stdio MCP.
- **Client ↔ server** (default): HTTP + SSE for the MCP-over-HTTP transport. User may configure other transports.
- Both sides auto-negotiate.

## 4. Data Model

### 4.1 Identity

- **User** — an individual account. Owns a personal namespace.
- **Org** — a collection of users. GitHub-style.
- **Group (team)** — a named subset inside an org with its own ACL. Sits between user and org.
- **Membership** — a user belongs to zero or more orgs; inside each org belongs to zero or more groups.

### 4.2 Memory groups

The unit of storage and permissioning. Each memory group is backed by exactly one git repository.

- **`global`** — auto-loaded by default for every project of the owning user. Holds universal rules.
- **`<project-uuid>`** — auto-loaded when a project with that UUID is active.
- **`lang/<language>`** — language convention groups (e.g. `lang/rust`). Auto-loaded when the project declares or auto-detects that language.
- **Custom groups** — arbitrary user- or org-owned groups for shared conventions, team playbooks, etc.

### 4.3 Memory file format

A memory is a Markdown file at `memories/<slug>/<uuid>.md` in its group's repo, with TOML frontmatter delimited by `+++` fences. The UUID in the path matches the `id` field in frontmatter and is the canonical primary key; the slug directory is the human-readable grouping and may contain more than one memory (duplicate slugs are legal and distinguished by their UUID).

```markdown
+++
id = "0196e5bb-a000-7000-8000-000000000001"   # UUIDv7 primary key
name = "Rust Coding Rules"
description = "Strict Rust coding conventions for this project"
kind = "rule"                  # see §4.4
mandatory = true               # must be read at least once per session
version = "1.3.2"              # current semver (managed by server at push time)
tags = ["rust", "style"]
+++

# Rust Coding Rules
...
```

TOML is chosen over YAML for consistency with `.mmcp/config.toml` and `Cargo.toml`, for native typed arrays and datetimes, and to avoid YAML's whitespace-significant parsing pitfalls. The `gray_matter` crate handles `+++`-delimited TOML frontmatter out of the box.

Frontmatter fields:

| Field         | Type       | Required | Purpose                                                       |
| ------------- | ---------- | -------- | ------------------------------------------------------------- |
| `id`          | uuid       | managed  | UUIDv7 primary key. Minted on first write; stable across renames and edits. Absent only on pre-migration legacy files. |
| `name`        | string     | yes      | Human-readable title                                          |
| `description` | string     | yes      | One-line summary for relevance inference                      |
| `kind`        | enum       | yes      | See §4.4                                                      |
| `mandatory`   | bool       | no       | If true, server enforces read-once-per-session                |
| `version`     | semver     | managed  | Assigned by server on push; clients do not hand-edit          |
| `tags`        | `[string]` | no       | Free-form classification                                      |
| `bump_intent` | enum       | no       | `patch` / `minor` / `major` — AI hint for next version bump   |
| `feature`     | table      | no       | Structured FR lifecycle metadata; present only when `kind = "feature"`. See §4.4. |

Only frontmatter fields defined by the schema are honored. Unknown fields are preserved verbatim on edit (future compat), but ignored by logic.

**Addressing.** Every memory-addressed tool routes through a single `resolve_memory(slug?, id?)` primitive: slug-only walks `memories/<slug>/` and returns the single entry (or `memory_ambiguous` when duplicates exist), id-only scans every slug subdirectory for `<id>.md`, slug+id verifies the frontmatter id matches. Missing both yields `resolve_args_missing`. The same primitive handles the legacy flat-layout fallback transparently.

### 4.4 Memory kinds

Default kinds (extensible by users):

| Kind       | Behavior                                                                 |
| ---------- | ------------------------------------------------------------------------ |
| `rule`     | Stable convention or guideline. Session-agnostic.                        |
| `snapshot` | Point-in-time fact about the project (status, counts, test results). Server always attaches a "may be stale" warning on retrieval. |
| `log`      | Append-only record (decisions, incidents). Edits only add entries.       |
| `reference`| Pointer to external resource (Linear project, Grafana dashboard, spec).  |
| `scratch`  | Short-lived working notes. Not versioned, no warnings.                   |
| `feature`  | Feature request. Carries a structured `[feature]` frontmatter block with `status` (requested / approved / pending / completed / blocked / deferred / duplicate / superseded), `number` (auto-assigned per group as `max(existing) + 1`), and UUID cross-references in `depends_on` / `blocks`. |
| `issue`    | Issue tracker entry, sister kind to `feature`. Carries a structured `[issue]` frontmatter block with its own distinct `status` (open / closed / wontfix / blocked / deferred / duplicate / superseded), sharing the same `number` counter and cross-reference machinery as `feature` without sharing its status vocabulary. |

Users can define custom kinds with their own behavior metadata:

```toml
[kinds.custom.architecture]
warn_stale = true
append_only = false
default_mandatory = false
```

### 4.5 ACL model

GitHub-inspired, three-tier:

- **Owner** — the user or org that created the group repo. Full control, cannot be revoked.
- **Members** — principals (users, groups, orgs) granted explicit access, with a role:
  - `read` — pull only
  - `write` — push allowed
  - `admin` — push + manage ACL
- **Propagation** — granting a role to an org implicitly grants it to all its groups and members, unless overridden.

A user's effective permission on a group is the maximum of all paths (direct membership, group membership, org membership). Principle of least surprise: grants add, they do not subtract.

## 5. Versioning

### 5.1 Semver rules

Every memory has a semver version. The bump level for a given edit is decided by the AI (with user override) and enforced at push time:

| Bump  | Trigger                                                                  |
| ----- | ------------------------------------------------------------------------ |
| major | New implementation, structural refactor, rule reversal                   |
| minor | Rule addition or removal, new section                                    |
| patch | Wording fix, typo, clarification, example tweak                          |

- **AI cannot set an explicit version number.** It can only request a bump level.
- Default bump intent is `minor` (configurable per group or per memory).
- The server computes the actual version at push time from the current canonical version + bump intent.
- **Version assignment is centralized at push**, which prevents offline-edit conflicts on numbers: Alice and Bob can both request `minor` bumps offline; the server assigns `1.3.0` and `1.4.0` in the order their pushes land and merge.

### 5.2 Tags

Tags are branch-like named pointers managed by the server on top of the git history:

- `latest` — always points to the most recent published version. Updated automatically on push.
- `beta` — user-controlled preview pointer, for staged rollouts.
- Custom tags are allowed.

Clients pull by tag; the default tag is `latest`.

### 5.3 History queries

The client can fetch the full history of any memory (from the underlying git repo), diff between two versions, and revert. Revert is itself a new commit with a bump level.

## 6. Session Tracking

### 6.1 Session identity and on-disk layout

Claude Code passes a stable `session_id` to MCP servers on every tool call. This is the thread identifier, stable across all turns and tool calls, including `/compact`, `--resume`, and `--continue`. Only `fork_session: true` generates a new id.

mmcp uses `session_id` directly as its session key. No machine id, no user id hashing.

Client-side, per-session state lives in flat TOML files at **`~/.mmcp/sessions/<session_id>.toml`**, one file per Claude Code session. Each file holds the session id, the owning user (when authenticated), the project UUID, the turn counter, the transcript path and its last-seen signature, the post-compaction flag, creation and last-seen timestamps, and every per-memory read the session has recorded so far. Writes are atomic via temp-file-rename so the hook process and the serve process can share a file without tearing each other's edits. There is no database on the client.

Server-side session tracking for multi-session views and shared dashboards will live in the Postgres database once those features are needed; today there is no server-side session table.

### 6.2 Per-turn granularity via hook

The raw MCP protocol does not expose per-message ids. mmcp fills the gap with a `UserPromptSubmit` hook:

1. Claude Code fires `UserPromptSubmit` on every user prompt, before the model sees it.
2. The hook calls `mmcp-client hook user-prompt`, passing the hook JSON on stdin (`session_id`, `cwd`, `transcript_path`, prompt text).
3. The client opens the flat `SessionStore` at `~/.mmcp/sessions/`, upserts the session row, inspects the transcript for compaction, and increments the turn counter.
4. The hook emits a small context injection into the prompt: `[mmcp session=<id> turn=#N id=<message-uuid>]`. This is visible to the model so it can correlate later tool calls with the session state file.
5. Any mmcp tool call in that turn can reference the session via the id the model just saw.

Every subsequent mmcp tool call in that turn can be tagged with `turn = N` for audit, "first touch this turn" detection, and mandatory-memory enforcement.

### 6.3 Compaction detection

`/compact` does **not** generate a new session id. The transcript file is rewritten in place (smaller). mmcp detects compaction by:

- Stat'ing `transcript_path` on every hook call.
- Comparing length/hash to the last-seen value for this `session_id`.
- If the file shrank → compaction happened → mark the session as "post-compaction" → on the next memory retrieval, attach a "you just compacted, re-verify mandatory memories" warning.

### 6.4 Hook installation bootstrap

On first mmcp tool call in any project, if the `UserPromptSubmit` hook is not installed for that project, the server sends an `elicitation/create` request:

```
mmcp wants to install a hook in .claude/settings.json to track
message turns. Choose:
  [install]       install now for this project
  [skip_session]  skip for this session, ask again next time
  [never]         never ask again for this project
```

Claude Code supports `elicitation/create` natively — no configuration required on the user side. The server persists the user's choice in the project config (`never`) or in session-scoped state (`skip_session`).

### 6.5 Warning attachment rules

Every memory retrieval response carries a `warnings: [...]` field. Warnings are computed server-side based on session state:

| Condition                                              | Warning                                               |
| ------------------------------------------------------ | ----------------------------------------------------- |
| Memory `kind = snapshot`                               | "this content may be out of date, verify if critical" |
| First retrieval of this memory in this session         | "you have not seen this memory this session"          |
| Memory updated since last retrieval in this session    | "this memory changed since you last read it"          |
| Session is post-compaction and memory was read pre-compaction | "you compacted after last reading this, re-verify"   |
| Memory is `mandatory` and not yet read this session    | **hard error — see §6.6**                             |

### 6.6 Mandatory memory enforcement

Memories with `mandatory: true` in their frontmatter must be read at least once per session (where "session" accounts for pre/post compaction independently — see §6.3).

The MCP protocol has no prerequisite-blocking mechanism, so enforcement is the strongest form that is actually possible:

1. **Error-and-gate**: any mmcp tool call other than `read_memory` returns an error if mandatory memories are unread for the current session, listing them explicitly.
2. **Hook-level injection**: the `UserPromptSubmit` hook output includes a prominent line naming unread mandatory memories, so the model sees it every turn.
3. **Elicitation fallback** (optional): if the model ignores both, server can surface an elicitation dialog asking the user to confirm before proceeding.

This is strong enough in practice. It is not a cryptographic gate.

## 7. Staleness Model

Staleness is a property of the **retrieval context**, not of the memory. The same memory returns different warnings depending on who retrieves it and when. See §6.5 for the warning matrix.

Two orthogonal axes:

- **Edit history** — tracked via git commits (§5).
- **Session reality-verification** — tracked via §6.

Both are needed. Git history alone does not tell you "has anyone verified this against reality recently"; session state alone does not tell you "has the content changed".

## 8. Offline Mode and Sync

### 8.1 Offline posture

mmcp-client has a local state directory (`~/.mmcp/`) containing:

- Cloned copies of every group repo the user has access to.
- A local SQLite database mirroring the subset of server state relevant to this user (group memberships, ACLs, session history, pending operations).
- A pending-push queue of local edits not yet synced.

The client is fully functional without the server. All reads come from the local clones; all writes go into local commits and the pending-push queue.

### 8.2 Sync primitives

- `mmcp pull` — fetch updates from the server for all cached group repos. Server-side state (ACLs, memberships) also refreshes.
- `mmcp push` — push pending local commits. Server assigns versions at this point (§5.1).
- `mmcp sync` — pull then push.
- `mmcp status` — show pending edits and sync state.

Sync happens automatically on a schedule when the server is reachable; the explicit commands are for manual control.

### 8.3 Conflict resolution

Conflicts are resolved git-style: if Alice and Bob both edit the same memory offline, the second pusher gets a merge conflict and must resolve it locally before re-pushing. Resolution is AI+user driven — mmcp does not auto-merge prose.

Line-based conflict markers (git default) are the starting point. Structured per-section merging is a future enhancement if line-based proves painful.

### 8.4 Identity reconciliation

Local-only users have a local identity (git-style `user.name`/`user.email` stored in `~/.mmcp/config.toml`). When they first connect to a server, they can claim a server account; the server maps their local commits to the server identity going forward. Prior local commits keep their original local identity as a git committer, with a server-side annotation linking them.

## 9. `.mmcp/` Project Configuration

### 9.1 Location and contents

Per-project config lives in `.mmcp/` at the project root. Committed to the project's own version control (like `.serena/`).

```
.mmcp/
  config.toml       Committed. Project identity and explicit group configuration.
  cache/            Gitignored. Per-project client cache (transcript hashes, session state).
  local.db          Gitignored. Offline mirror of mmcp state for this project.
  .gitignore        Excludes cache/ and local.db.
```

### 9.2 `config.toml` schema

```toml
# .mmcp/config.toml
# Project-level mmcp configuration. Commit this to version control.

# Stable project identity. Generated by `mmcp init`, never changes.
project_uuid = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"

[sync]
# mmcp server to sync with. Omit the whole [sync] table for local-only mode.
server_url = "https://mmcp.example.com"

[subscriptions]
# Single opt-in surface. Replaces the older [groups] + [languages] sections.

# If true, skip auto-loading the `global` group.
# The project's own group (keyed by project_uuid) is always loaded regardless.
no_default_global = false

# If true, client scans project files (Cargo.toml, package.json, etc.)
# and adds detected languages to the load set.
auto_detect_languages = true

# Language convention groups to auto-load. Each entry resolves to
# `lang/<name>` and pulls every memory in that group into scope.
languages = ["rust"]

# Extra groups to fully subscribe to beyond the defaults.
groups = ["team-acme/shared"]

# Individual non-mandatory memory pins, formatted `<group_uuid>:<slug>`.
memories = []

# Tag filter — non-mandatory memories from in-scope groups whose
# frontmatter tags overlap this set surface in `bootstrap_context`.
tags = ["git", "testing"]
```

### 9.3 Resolution rules

On session start, the client computes the effective group load set:

1. Always load `<project_uuid>`.
2. If `subscriptions.no_default_global = false`, load `global`.
3. Load every entry in `subscriptions.languages` as `lang/<name>`.
4. If `subscriptions.auto_detect_languages = true`, detect project languages and add matching `lang/*` groups.
5. Load every entry in `subscriptions.groups`.

Load set is the union; permission failures produce a warning, not a hard error.

## 10. Language Groups

A first-class concept replacing the per-project CLAUDE.md conditional-loading pattern.

- Language groups live in the `lang/` namespace (e.g. `lang/rust`, `lang/python`, `lang/typescript`).
- They are normal mmcp groups with normal ACLs, normal versioning, normal memories.
- Clients resolve them via `[languages]` in project config.
- Adding a new language to the ecosystem is a one-time server-side operation: create `lang/<name>` group, populate it. Every project that declares or auto-detects that language picks it up.

Auto-detection heuristics (shipped with the client, extensible via server-side rules):

| Marker                              | Language          |
| ----------------------------------- | ----------------- |
| `Cargo.toml`                        | `rust`            |
| `package.json` without `svelte.config.js` | `typescript` (or `javascript` — heuristic on `tsconfig.json`) |
| `pyproject.toml`, `setup.py`, `requirements.txt` | `python` |
| `go.mod`                            | `go`              |
| `pom.xml`, `build.gradle`           | `java`            |
| ...                                 | ...               |

## 11. MCP Tool Surface

Tools exposed by `mmcp-client` to the AI. Every memory-addressed tool accepts an optional `slug` + optional `id` pair and resolves through the shared `resolve_memory` primitive (§4.3).

**Session and discovery**

| Tool                 | Purpose                                                                 |
| -------------------- | ----------------------------------------------------------------------- |
| `bootstrap_context`  | Single round-trip that returns mandatory and project-scoped memories with bodies inline. Called at session start, after compaction, and at every task boundary (per `CLAUDE.md`). |
| `list_groups`        | Enumerate every group present in the local mirror with its manifest metadata (slug, display name, owner hint, protected flag, memory count). |
| `list_memories`      | List memories in a group, with kind, mandatory flag, and resolved UUID. |
| `list_versions`      | Walk the commit history for a memory.                                   |
| `group_info`         | Manifest metadata for a single group (id, slug, display name, owner, schema version, created_at, memory count). |
| `search_memories`    | Case-insensitive substring search across slug and `name` across every mirrored group. |
| `status`             | Local project state: project config + mirrored groups + sync target.    |

**Memory CRUD**

| Tool                         | Purpose                                                                 |
| ---------------------------- | ----------------------------------------------------------------------- |
| `read_memory`                | Read a memory's frontmatter + body. Addressing: `slug`, `id`, or both. `version` selects a branch, tag, or commit hex. |
| `write_memory`               | Strict CREATE with typed args (no source parsing). Mints a UUIDv7 when `id` is absent; `override: true` opts into replace-whole-file. |
| `import_memory`              | Import a memory from a markdown source (with embedded `+++` / `---` frontmatter, or raw body plus synth `name` / `description` / `kind`). `format: "adoc"` routes through the AsciiDoc bridge first. |
| `edit_memory`                | Partial update: body / name / description / kind / tags_add / tags_remove / refs_add / refs_remove / mandatory / message (commit message override). Every mutator is optional. |
| `delete_memory`              | Commit a deletion on `main`. Refuses silently-on-no-op — missing slug surfaces `memory_not_found`. |
| `read_memory_body_sections`  | Return the parsed section tree of the body (heading path ids, levels, line ranges). FR-026. |
| `edit_memory_body`           | Apply an ordered list of semantic body ops: UpsertSection / DeleteSection / InsertSectionBefore / InsertSectionAfter / MoveSectionBefore / MoveSectionAfter / ReplaceSectionBody, plus line-level escape hatches. Transactional. FR-026. |

**Feature-request surface** (`kind = "feature"`)

| Tool              | Purpose                                                                 |
| ----------------- | ----------------------------------------------------------------------- |
| `add_feature`     | Create a feature. `number` auto-assigns per group; `depends_on` / `blocks` accept UUID strings. |
| `read_feature`    | Typed FR record (slug, title, status, number, depends_on, blocks, body). |
| `update_feature`  | Partial mutator. `depends_on` / `blocks` are full-list replacements. `number` is server-managed and not editable (FR-37). |
| `delete_feature`  | Guarded delete — refuses `not_a_feature` for non-FR memories.           |
| `list_features`   | List features sorted by `number` ascending. Default returns only `open` features; `all: true` includes every status, and `status: "<variant>"` pins a single lifecycle state (explicit selector wins over the default hide). |
| `rename_feature`  | Atomic rename of every memory under `memories/<old_slug>/` to `memories/<new_slug>/`. UUIDs stay stable so cross-refs keep resolving. FR-027. |

**Sync and project lifecycle**

| Tool            | Purpose                                                                  |
| --------------- | ------------------------------------------------------------------------ |
| `sync_fetch`    | Read each in-scope group's remote head into a local remote-tracking ref without advancing `main`. Mirrors `git fetch`. |
| `sync_pull`     | Pull every configured group from the sync server.                        |
| `sync_push`     | Push every local change back.                                            |
| `sync`          | Pull-then-push convenience.                                              |
| `create_group`  | Bootstrap a standalone `~/.mmcp/repos/<uuid>.git` with a manifest. Use `init_project` instead when the group is the project's own backing store. |
| `init_project`  | Create or adopt `.mmcp.toml` + bare repo for the project's group.        |
| `init_claude`   | Manage the fenced mmcp block in `CLAUDE.md`.                             |
| `check_health`  | Surface-level validation (manifest readable, memories parse).            |
| `diagnose`      | Deep structural analysis (empty bodies, cross-group slug collisions, config gaps). |

**Debug tools** (gated behind `debug_toggle(enabled=true)`, off in normal use)

| Tool                | Purpose                                                          |
| ------------------- | ---------------------------------------------------------------- |
| `debug_toggle`      | Turn the raw-access tools on/off.                                |
| `debug_read_file`   | Read any path at any rev inside a group repo.                    |
| `debug_write_file`  | Write raw bytes to a group repo.                                 |
| `debug_list_tree`   | List a tree prefix at a rev.                                     |
| `debug_git_log`     | Walk commit history for an arbitrary path.                       |

**Cross-cutting fields** (apply to multiple tools)

| Field                   | Tools                                                  | Purpose |
| ----------------------- | ------------------------------------------------------ | ------- |
| `notes: Vec<Note>`      | every successful response                              | FR-45 standard advisory channel. Each note carries `level` (info / warning / error), a stable `code` (e.g. `id_mismatch_accepted`, `dangling_ref`, `malformed_frontmatter`, `sync_partial_failure`), a human `message`, and a `context` blob. Errors stay in `McpError`; notes are success-path only. |
| `force: bool`           | `write_memory` / `edit_memory` / `edit_memory_body` / `delete_memory` / `import_memory` | FR-28 / D4 bypass for the filename-vs-frontmatter id mismatch rejection on a `ByFilename` write. Defaults to `false` so drift is caught loudly. `delete_memory` and `import_memory` carry the flag for parity but treat it as a no-op on the happy path. |
| `project: String?`      | every project-scoped tool (FR-44)                      | Explicit group selector (UUID or slug). When omitted, the server walks `cwd` for `.mmcp.toml`. Memory-CRUD tools that target a specific group use the `group` field instead — `project` is reserved for tools whose semantics depend on the project's `.mmcp.toml`. |

Tool descriptions include explicit instructions about session-specific expectations (e.g. "after compaction, re-read mandatory memories before using this tool"), since MCP descriptions are the primary channel for nudging model behavior.

## 12. Auth

Auth is first-class from day one. The mmcp WebUI is a GitHub-style interface for both end users and administrators, so login flows must be complete and polished on initial release.

### 12.1 Authentication methods

Supported at launch:

- **Password + PASETO session token** — baseline login for any account.
- **OAuth2** — login via GitHub, Google, and any configured provider. Wired via `oauth2-passkey-axum`.
- **Passkeys (WebAuthn)** — passwordless login and second-factor, supported natively through the same crate.

Password hashing uses `argon2`. Session tokens use `rusty_paseto` (PASETO v4 local tokens for server-held sessions, v4 public tokens if we later expose issuer-verifiable API tokens). JWT is deliberately avoided for internal tokens because mmcp controls both ends and PASETO offers stronger defaults.

### 12.2 Session and authorization middleware

`axum-login` provides the identification/authentication/authorization middleware layer. Applications implement the `AuthUser` and `AuthnBackend` traits over `mmcp-db` entities; `axum-login` handles session lifecycles via `tower-sessions`. Route protection uses `login_required!` and `permission_required!` macros.

Authorization is permission-based. Permissions are derived from mmcp's ACL model (§4.5) and attached to the session on login.

### 12.3 Token storage on the client

- **Preferred**: OS keychain via the `keyring` crate (Windows Credential Manager, macOS Keychain, Secret Service / libsecret on Linux).
- **Fallback**: encrypted file under `~/.mmcp/credentials.toml`, protected with a user-supplied passphrase.

### 12.4 Local-only mode

When no `[sync]` block is configured, the client skips auth entirely. The local user is trusted on their own machine. Group repos are opened directly; no session tokens, no login flow.

## 13. Git Backend Architecture

Git storage is abstracted behind a `GitBackend` trait defined in `mmcp-git`. The server and client depend on the trait, not any specific implementation, so the storage layer is swappable without touching the control plane.

### 13.1 Trait shape

```rust
#[async_trait]
pub trait GitBackend: Send + Sync {
    async fn create_group_repo(
        &self,
        manifest: &GroupManifest,
    ) -> Result<RepoHandle, GitError>;

    async fn read_manifest(
        &self,
        repo: &RepoHandle,
    ) -> Result<GroupManifest, GitError>;

    async fn write_manifest(
        &self,
        repo: &RepoHandle,
        manifest: &GroupManifest,
    ) -> Result<String, GitError>;

    async fn clone_to(
        &self,
        remote_url: &str,
        dst: &Path,
        creds: &Credentials,
    ) -> Result<(), GitError>;

    async fn fetch(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
        creds: &Credentials,
    ) -> Result<(), GitError>;

    async fn push(
        &self,
        repo: &RepoHandle,
        remote_url: &str,
        refs: &[RefSpec],
        creds: &Credentials,
    ) -> Result<PushReport, GitError>;

    async fn read_file(&self, repo: &RepoHandle, path: &str, rev: &Rev) -> Result<Bytes, GitError>;
    async fn write_commit(&self, repo: &RepoHandle, spec: CommitSpec) -> Result<String, GitError>;
    async fn tag(&self, repo: &RepoHandle, name: &str, target: &str) -> Result<(), GitError>;
    async fn walk_history(&self, repo: &RepoHandle, path: &str) -> Result<Vec<CommitMeta>, GitError>;
    async fn list_tree(&self, repo: &RepoHandle, path_prefix: &str, rev: &Rev) -> Result<Vec<String>, GitError>;
    async fn list_subtrees(&self, repo: &RepoHandle, path_prefix: &str, rev: &Rev) -> Result<Vec<String>, GitError>;
}
```

The manifest methods (`create_group_repo`, `read_manifest`, `write_manifest`) keep the per-group `.mmcp.toml` inside the backend so every consumer reads and writes it through the same primitive. `list_tree` returns blob entries directly under a prefix; `list_subtrees` is its mirror for directory entries. Together the two enumeration calls let the store walk the two-level `memories/<slug>/<uuid>.md` layout from §4.3 without recursing into the whole tree. The transport primitives (`clone_to`, `fetch`, `push`) take a `remote_url: &str` plus a `Credentials` argument so the caller picks SSH agent, personal access token, or ambient-environment auth per invocation.

### 13.2 Default backend: native in-process git

`NativeBackend` is shipped with `mmcp-server` and used by default. It has no external dependencies beyond the mmcp binary itself.

- **Storage**: bare repositories on disk under `<data_dir>/repos/<group_uuid>.git`, managed directly by `gix`.
- **In-process access**: the server process uses `gix` APIs directly for all repo operations. No subprocess spawning, no file locks held by other programs.
- **Network access**: `mmcp-server` exposes the git smart HTTP protocol on its own `axum` port under `/git/<group_uuid>.git/`. Clients run normal `git fetch` / `git push` (or the `mmcp-sync` engine) against that URL.
- **Auth**: the smart HTTP handler reuses `axum-login` session state. Git-level auth is the same as the rest of the API. No duplicate user database.
- **ACL enforcement**: happens in the mmcp control layer before the git handler touches the repo. Read/write permission is checked against the ACL rules in §4.5, then the git operation proceeds or is rejected.

**Implementation note on smart HTTP**: `gix` has solid client-side fetch/push/clone, and the server-side smart HTTP responder (upload-pack and receive-pack over HTTP, pkt-line framing) may or may not be complete in the current `gix` release. If it is not, we implement the protocol ourselves on top of `gix`'s object database. This is well-documented (Git's own smart-http-protocol documentation is the spec) and keeps the zero-dependency story intact.

### 13.3 Alternative backends

For users who want their memory storage in an existing forge, mmcp ships the following opt-in backends behind the same trait:

| Backend          | Transport | Notes                                                                 |
| ---------------- | --------- | --------------------------------------------------------------------- |
| `ForgejoBackend` | REST API  | Talks to a self-hosted Forgejo instance. mmcp stores only group-to-repo mapping; Forgejo owns git content and repo-level ACLs. |
| `GiteaBackend`   | REST API  | Nearly identical API to Forgejo; likely one implementation with a config flag. |
| `GitHubBackend`  | REST API  | For users who want memories in GitHub repos. Subject to GitHub rate limits. |
| `GitLabBackend`  | REST API  | Same rationale as GitHub.                                             |

Backend selection is a server-side configuration choice. The client is agnostic — it always talks to `mmcp-server`, which proxies git operations through whichever backend is configured.

### 13.4 Group manifest

Every group repository carries a self-describing `.mmcp.toml` file at its tree root on the `main` branch. The file is an mmcp-managed artifact committed during `GitBackend::create_group_repo` and updated through `GitBackend::write_manifest`. Its schema lives in `mmcp_core::manifest::GroupManifest`:

```toml
schema_version = 1
group_id       = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91"
slug           = "team-rust"
display_name   = "Rust team memories"
created_at     = 1776000000000

[owner]
kind = "user"
id   = "018f7b12-0000-7000-8000-000000000000"
```

The manifest exists to:

- Let any consumer walk a filesystem of bare repos and rebuild the groups index without a database. The client's `GroupIndex` uses exactly this walk to discover every group under `~/.mmcp/repos/` on startup.
- Preserve group identity when a repo is forked to another host or exported to an external forge. The group id travels with the content.
- Refuse manifests from newer mmcp builds explicitly via the `schema_version` field so users see a clear upgrade prompt rather than silent field loss.

`GitBackend` exposes three methods that manage the manifest:

- `create_group_repo(&self, manifest: &GroupManifest) -> RepoHandle` — initializes the bare repo and commits the initial manifest if one is not already present. Idempotent against rerunning with the same manifest.
- `read_manifest(&self, repo: &RepoHandle) -> GroupManifest` — reads `HEAD:.mmcp.toml`, decodes UTF-8, and parses via `GroupManifest::from_toml`.
- `write_manifest(&self, repo: &RepoHandle, manifest: &GroupManifest) -> String` — commits a new manifest revision on `main` with a fixed message and returns the new commit id.

## 14. Locked Stack Decisions

Recorded here so subsequent design changes have a reference point. See `docs/STACK.md` for the full crate-by-crate inventory and rationale.

| Area                   | Choice                                                                 |
| ---------------------- | ---------------------------------------------------------------------- |
| Language               | Rust, edition 2024                                                     |
| Async runtime          | `tokio`                                                                |
| HTTP framework         | `axum`                                                                 |
| MCP SDK                | `rmcp` (official Rust MCP SDK)                                         |
| WebUI framework        | SvelteKit 2 + Svelte 5 + Tailwind v4 (pure SPA via `@sveltejs/adapter-static`) |
| ORM                    | SeaORM (multi-backend: Postgres on server, SQLite on client mirror)    |
| Git library            | `gix` (gitoxide)                                                       |
| Git backend default    | Native in-process (bare repos + smart HTTP served by `mmcp-server`)    |
| Tokens                 | `rusty_paseto` (PASETO v4)                                             |
| Auth middleware        | `axum-login` + `tower-sessions`                                        |
| OAuth + passkeys       | `oauth2-passkey-axum` (day-one feature)                                |
| Password hashing       | `argon2`                                                               |
| Client keychain        | `keyring`                                                              |
| Frontmatter format     | TOML, `+++` delimited                                                  |
| Markdown+frontmatter   | `gray_matter` (TOML mode)                                              |
| Date/time              | `jiff`                                                                 |
| CLI parsing            | `clap` v4 derive                                                       |
| Error handling         | `thiserror` (libs), `anyhow` (binaries)                                |
| Logging                | `tracing` + `tracing-subscriber`                                       |
| UUIDs                  | `uuid` v7 (time-ordered)                                               |
| Testing                | standard + `proptest` + `criterion` + `wiremock`                       |

## 15. Open Questions

Parked for later, not blocking an initial prototype:

1. **Structured-section merging** for conflict resolution — deferred until line-based proves painful.
2. **Verification semantics** — should `verify_memory` require the AI to restate key facts, or is a no-arg call sufficient? Restating is stronger but costs tokens.
3. **Cross-memory atomic updates** — a single push can carry multiple independent commits across different group repos, but they are not atomically visible. Do we need a "bundle" concept, or is eventual consistency fine?
4. **Fork/branch semantics** — when a user wants to propose changes to a group they have read-only access to, how does the PR-like flow work?
5. **Rate limiting** on elicitation — the spec says SHOULD, we need to pick concrete limits.
6. **Hook failure behavior** — if the server is unreachable and the local cache is stale, does the hook block, warn, or silently degrade? Probably silently degrade with a cached counter.
7. **Smart HTTP server implementation** — confirm whether `gix` ships a ready-to-use server-side responder, or whether we implement the pkt-line framing ourselves. Not a blocker either way; resolved during prototyping.
8. **Forgejo/Gitea API divergence** — whether one implementation with a config flag is enough, or they need separate backends. Resolved once we prototype against both.
