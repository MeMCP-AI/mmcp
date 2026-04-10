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

- **One group = one git repository.** The repo is dumb storage.
- Memories are Markdown files with TOML frontmatter (`+++` delimited) living inside the group repo.
- Full edit history lives in git commits.
- No manifest files, no submodules, no server-metadata stuffed into repo files.
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
  mmcp-core/      Shared types: data model, frontmatter schema, semver logic, ACL rules
  mmcp-git/       Git storage abstraction: GitBackend trait + native gix implementation
  mmcp-db/        SeaORM entities, migrations, shared queries (Postgres + SQLite)
  mmcp-auth/      Argon2 + PASETO + axum-login glue
  mmcp-proto/     MCP tool schemas shared by client and server (built on rmcp)
  mmcp-session/   Session tracking, transcript inspection, turn counter, compaction detect
  mmcp-sync/      Push/pull/diff/merge engine layered over mmcp-git
  mmcp-server/    Binary: HTTP/SSE, WebUI backend, git smart HTTP, auth, Postgres
  mmcp-client/    Binary: MCP stdio server + sync engine + CLI + hook subcommands
webui/            Leptos frontend (separate cargo project, not in main workspace)
```

### Crate responsibilities

- **mmcp-core**: pure data model + logic. No I/O, no network, no async. Used by every other crate.
- **mmcp-git**: `GitBackend` trait and its implementations. Default `NativeBackend` uses `gix` on bare repos. Alternative backends for Forgejo, Gitea, GitHub, GitLab talk to external forges over REST.
- **mmcp-db**: SeaORM entity definitions and migrations. Same entities target Postgres on the server and SQLite in the client's local mirror.
- **mmcp-auth**: password hashing, PASETO token issuance/validation, `axum-login` traits, OAuth + passkey wiring.
- **mmcp-proto**: typed MCP tool request/response shapes built on top of `rmcp`. Shared so client and server never drift on schemas.
- **mmcp-session**: session key management, transcript file inspection, turn counter, compaction detection, mandatory-memory read tracking.
- **mmcp-sync**: push/pull/diff/merge engine. Uses `mmcp-git` for repo ops and `mmcp-db` for pending-push state.
- **mmcp-server** *(binary)*: `axum`-based HTTP/SSE daemon. Hosts MCP-over-HTTP, WebUI REST API, git smart HTTP, auth endpoints. Owns the Postgres database and the bare git repos on disk (when using `NativeBackend`). `#![forbid(unsafe_code)]`.
- **mmcp-client** *(binary)*: runs on the user's machine. One binary, multiple entry points via `clap` subcommands:
  - `mmcp serve` — the MCP stdio server that Claude Code and other AI clients talk to
  - `mmcp init` / `status` / `sync` / `pull` / `push` — CLI workflow commands
  - `mmcp hook user-prompt` — the command invoked by the Claude Code `UserPromptSubmit` hook
- **webui** *(separate crate, not in main workspace)*: Leptos fullstack frontend. Talks to `mmcp-server` via its REST API. Shares types with `mmcp-core` via Cargo dependency.

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

A memory is a single `.md` file inside its group's repo, with TOML frontmatter delimited by `+++` fences.

```markdown
+++
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
| `name`        | string     | yes      | Human-readable title                                          |
| `description` | string     | yes      | One-line summary for relevance inference                      |
| `kind`        | enum       | yes      | See §4.4                                                      |
| `mandatory`   | bool       | no       | If true, server enforces read-once-per-session                |
| `version`     | semver     | managed  | Assigned by server on push; clients do not hand-edit          |
| `tags`        | `[string]` | no       | Free-form classification                                      |
| `bump_intent` | enum       | no       | `patch` / `minor` / `major` — AI hint for next version bump   |

Only frontmatter fields defined by the schema are honored. Unknown fields are preserved verbatim on edit (future compat), but ignored by logic.

### 4.4 Memory kinds

Default kinds (extensible by users):

| Kind       | Behavior                                                                 |
| ---------- | ------------------------------------------------------------------------ |
| `rule`     | Stable convention or guideline. Session-agnostic.                        |
| `snapshot` | Point-in-time fact about the project (status, counts, test results). Server always attaches a "may be stale" warning on retrieval. |
| `log`      | Append-only record (decisions, incidents). Edits only add entries.       |
| `reference`| Pointer to external resource (Linear project, Grafana dashboard, spec).  |
| `scratch`  | Short-lived working notes. Not versioned, no warnings.                   |

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

### 6.1 Session identity

Claude Code passes a stable `session_id` to MCP servers on every tool call. This is the thread identifier, stable across all turns and tool calls, including `/compact`, `--resume`, and `--continue`. Only `fork_session: true` generates a new id.

mmcp uses `session_id` directly as its session key. No machine id, no user id hashing. Server maintains a `sessions` table.

### 6.2 Per-turn granularity via hook

The raw MCP protocol does not expose per-message ids. mmcp fills the gap with a `UserPromptSubmit` hook:

1. Claude Code fires `UserPromptSubmit` on every user prompt, before the model sees it.
2. The hook calls `mmcp-client hook user-prompt`, passing the hook JSON on stdin (`session_id`, `cwd`, `transcript_path`, prompt text).
3. `mmcp-client` contacts the server (or local cache if offline), increments a per-session message counter, and gets back `{turn: N, message_id: <uuid>}`.
4. The hook emits a small context injection into the prompt: `[mmcp turn #N id=<uuid>]`. This is visible to the model.
5. The server now knows the current turn number for this session.

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

[groups]
# If true, skip auto-loading the `global` group.
# The project's own group (keyed by project_uuid) is always loaded regardless.
no_default = false

# Extra groups to pull beyond the defaults.
additional = [
    "team-acme/shared",
]

[languages]
# Language convention groups to auto-load.
# Resolved against the `lang/` namespace (e.g. lang/rust).
use = ["rust"]

# If true, client scans project files (Cargo.toml, package.json, etc.)
# and adds detected languages to the load set.
auto_detect = true
```

### 9.3 Resolution rules

On session start, the client computes the effective group load set:

1. Always load `<project_uuid>`.
2. If `groups.no_default = false`, load `global`.
3. Load every entry in `languages.use` as `lang/<name>`.
4. If `languages.auto_detect = true`, detect project languages and add matching `lang/*` groups.
5. Load every entry in `groups.additional`.

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

Tools exposed by `mmcp-client` to the AI:

| Tool              | Purpose                                                                 |
| ----------------- | ----------------------------------------------------------------------- |
| `list_memories`   | List memories in the effective group load set, with kind and mandatory flags |
| `read_memory`     | Read a memory by name or id; server attaches warnings (§6.5)            |
| `write_memory`    | Create or update a memory; AI specifies bump intent                     |
| `verify_memory`   | Mark a memory as "verified against reality" for this session/turn       |
| `list_versions`   | Show version history for a memory                                       |
| `diff_memory`     | Show diff between two versions                                          |
| `search_memories` | Full-text search over the effective load set                            |
| `group_info`      | Get metadata about a group (owner, ACL summary, memory count)           |

Tool descriptions will include explicit instructions about session-specific expectations (e.g. "after compaction, re-read mandatory memories before using this tool"), since MCP descriptions are the primary channel for nudging model behavior.

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
    async fn create_group_repo(&self, group: &GroupRef) -> Result<RepoHandle>;
    async fn clone_to(&self, repo: &RepoHandle, dst: &Path) -> Result<()>;
    async fn fetch(&self, repo: &RepoHandle, refs: &[RefSpec]) -> Result<()>;
    async fn push(&self, repo: &RepoHandle, refs: &[RefSpec]) -> Result<PushReport>;
    async fn read_file(&self, repo: &RepoHandle, path: &str, rev: &Rev) -> Result<Bytes>;
    async fn write_commit(&self, repo: &RepoHandle, commit: CommitSpec) -> Result<CommitId>;
    async fn tag(&self, repo: &RepoHandle, name: &str, target: &CommitId) -> Result<()>;
    async fn walk_history(&self, repo: &RepoHandle, path: &str) -> Result<Vec<CommitMeta>>;
}
```

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

## 14. Locked Stack Decisions

Recorded here so subsequent design changes have a reference point. See `docs/STACK.md` for the full crate-by-crate inventory and rationale.

| Area                   | Choice                                                                 |
| ---------------------- | ---------------------------------------------------------------------- |
| Language               | Rust, edition 2024                                                     |
| Async runtime          | `tokio`                                                                |
| HTTP framework         | `axum`                                                                 |
| MCP SDK                | `rmcp` (official Rust MCP SDK)                                         |
| WebUI framework        | Leptos (fullstack Rust, fine-grained reactivity, SSR + hydration)      |
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
