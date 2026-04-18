# mmcp - Stack Reference

> Status: **Draft**. Companion to `docs/DESIGN.md`. This document lists every crate and tool mmcp depends on, why it was chosen, and which crates in the workspace consume it.

## 1. Workspace Crates

Rust workspace, edition 2024. All crates live under `crates/` except the web frontend.

### 1.1 Libraries

| Crate            | Role                                                                                          | Depends on                                           |
| ---------------- | ---------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| `mmcp-core`      | Pure data model: `User`, `Org`, `Group`, `Memory`, `Version`, `Acl`, `MemoryKind` (including `Feature` with `fr` serde alias), `MemoryFrontmatter` with `id: Option<Uuid>` primary key and a fluent `MemoryFrontmatter::new(...).with_*(...)` builder, `FeatureMetadata` carrying `status` / `number` / UUID-typed `depends_on` / `blocks`, markdown body parser (`Section`, `parse_sections`, `render_sections`) backing FR-026's semantic editor, frontmatter schema, semver bump logic, ACL resolver, load-set resolver, `.mmcp.toml` group manifest, and path conventions (`memory_path(slug, id)` two-level layout + `legacy_memory_path(slug)` fallback). No I/O, no async. | (leaf)                                               |
| `mmcp-git`       | `GitBackend` trait + `NativeBackend` (via `gix`) + external forge backends (Forgejo, Gitea, GitHub, GitLab). Abstracts all git storage, including read/write of the per-group `.mmcp.toml` manifest. Enumerates both files and directories under a prefix (`list_tree` + `list_subtrees`) so consumers can walk the two-level `memories/<slug>/<uuid>.md` layout without a recursive traversal. | `mmcp-core`                                          |
| `mmcp-store`     | Local-first programmatic store layer. Single source of UUID-first addressing: `resolve_memory(slug?, id?) -> ResolvedMemory`, `write_memory_by_id`, path-based primitives `write_file_at_path` / `delete_file_at_path`, semantic body op applier (`MemoryEditOp`, `apply_ops`) backing FR-026's `edit_memory_body`, feature CRUD (`add_feature` / `read_feature` / `update_feature` / `delete_feature` / `list_features` / `rename_feature` / `parse_cross_refs`), group index and manifest lifecycle, diagnostics, session store. Consumed by `mmcp-client`, `mmcp-gui`, and any third-party Rust code driving the store. Zero dependency on `rmcp`, `clap`, `inquire`, or `egui`. | `mmcp-core`, `mmcp-git`, `mmcp-session`, `mmcp-sync` |
| `mmcp-db`        | **Server-only** SeaORM entity definitions, migrations, and repository helpers. Targets Postgres (production) and SQLite (single-user deployments). The client does not depend on this crate. | `mmcp-core`                                          |
| `mmcp-auth`      | Password hashing, PASETO tokens, `axum-login` trait impls, OAuth and passkey wiring.          | `mmcp-core`, `mmcp-db`                               |
| `mmcp-proto`     | MCP tool schemas (request + response types) and a `ProtoError` surface (including `NotImplemented`). Shared by client and server so they never drift. Every memory-addressed tool takes an optional `slug` + optional `id` pair; feature cross-refs are UUID strings on the wire. | `mmcp-core`                                          |
| `mmcp-session`   | Compaction detection primitives: `TranscriptSignature`, `compute_signature`, `detect_compaction`. Pure, dependency-light, consumed by whichever storage layer wants them. | (leaf — no mmcp deps)                                |
| `mmcp-sync`      | Push/pull/diff/merge engine. Drives `mmcp-git` for repo ops; pending-push state is currently an in-process `PendingQueue` (server-side persistence via `mmcp-db` is reserved for §5 but not yet wired). | `mmcp-core`, `mmcp-git`, `mmcp-proto`                |

### 1.2 Binaries

| Crate         | Role                                                                                                                    | Depends on                                                                                     |
| ------------- | ----------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `mmcp-server` | `axum`-based HTTP/SSE daemon. Hosts MCP-over-HTTP, WebUI REST API, git smart HTTP, auth endpoints. Owns the Postgres DB and the bare git repos on disk (when using `NativeBackend`). | `mmcp-core`, `mmcp-git`, `mmcp-db`, `mmcp-auth`, `mmcp-proto`, `mmcp-session`, `mmcp-sync`     |
| `mmcp-client` | Binary plus library target. Runs as the MCP stdio server, the sync engine, the hook command, and the user-facing CLI. Delegates every memory operation to `mmcp-store` so the CLI, the MCP tool bodies, and any third-party consumer converge on one programmatic surface. Ships one-shot migration example binaries (`migrate_uuidify`, `migrate_fr_slugs`). **Does not depend on `mmcp-db`.** | `mmcp-core`, `mmcp-git`, `mmcp-auth`, `mmcp-proto`, `mmcp-session`, `mmcp-store`, `mmcp-sync`  |
| `mmcp-gui`    | Desktop visual client built on `eframe`/`egui`. Reads, writes, and browses local memories directly through `mmcp-store` — no MCP round-trip. Handles the FR-028 two-level layout transparently via `resolve_memory`. | `mmcp-core`, `mmcp-git`, `mmcp-store`, `mmcp-sync`                                             |

### 1.3 Frontend

| Directory | Role                                                                                                                   | Depends on      |
| --------- | ---------------------------------------------------------------------------------------------------------------------- | --------------- |
| `webui/`  | Leptos fullstack frontend (SSR + hydration). GitHub-style UI for end users and administrators. Talks to `mmcp-server` via REST. Not in the main Cargo workspace; built and deployed separately. | `mmcp-core` (type sharing) |

## 2. External Crates

Grouped by concern. "Consumers" lists which mmcp crates use the dependency directly.

### 2.1 Protocol and transport

| Crate                 | Purpose                                                                 | Consumers                         |
| --------------------- | ----------------------------------------------------------------------- | --------------------------------- |
| `rmcp`                | Official Rust MCP SDK. `#[tool]`, `#[tool_router]`, `#[tool_handler]` macros. `ServerHandler` trait. stdio transport confirmed; HTTP/SSE transport to verify during prototyping. `mmcp-proto` describes the shared tool-schema intent in its docs but does not depend on the SDK directly today — the actual `#[tool]`-annotated handlers live in `mmcp-client::commands::serve`. | `mmcp-client`                     |
| `axum`                | HTTP server framework. SSE and WebSocket support, tower middleware ecosystem. | `mmcp-server`                     |
| `reqwest`             | HTTP client. Used by the sync engine to talk to `mmcp-server`, and planned for the external git backends (Forgejo/Gitea/GitHub/GitLab REST APIs) once those land. | `mmcp-sync`, `mmcp-server`        |
| `tower`, `tower-http` | Middleware layer: tracing, timeouts, CORS, compression, rate limiting. | `mmcp-server`                     |
| `tower-sessions`      | Session storage backend for `axum-login`.                               | `mmcp-server`                     |
| `rustls`              | Pure-Rust TLS. Used by `reqwest` and `axum` via feature flags.          | transitive                        |

### 2.2 Storage

| Crate                    | Purpose                                                                 | Consumers                  |
| ------------------------ | ----------------------------------------------------------------------- | -------------------------- |
| `gix` (gitoxide)         | Pure Rust git library. Clone, fetch, push, commit I/O, history walk, merge. Enables the `NativeBackend` without libgit2 or any C dependency. | `mmcp-git`                 |
| `sea-orm`                | Async ORM over `sqlx`. Same entity definitions work across Postgres (server) and SQLite (client mirror). | `mmcp-db`                  |
| `sea-orm-migration`      | Programmatic migrations, paired with `sea-orm`.                         | `mmcp-db`                  |
| `sqlx`                   | Transitive (via `sea-orm`). Connection pooling, compile-time checked raw queries where we need them. | transitive                 |
| `gray_matter`            | Markdown + frontmatter parser. Used in TOML mode for `+++`-delimited memories. | `mmcp-core`                |
| `pulldown-cmark`         | CommonMark parser. Drives `mmcp-core::memory::body::parse_sections` so FR-026's semantic editor targets headings by path rather than substrings. | `mmcp-core`                |
| `toml`                   | TOML parsing for `.mmcp/config.toml`, `~/.mmcp/config.toml`, and server configuration. | `mmcp-core`, `mmcp-client`, `mmcp-server` |
| `serde`, `serde_json`    | Serialization baseline. JSON for MCP tool payloads and REST responses.  | everywhere                 |

### 2.3 Identity and security

| Crate                 | Purpose                                                                           | Consumers          |
| --------------------- | --------------------------------------------------------------------------------- | ------------------ |
| `rusty_paseto`        | PASETO v4 token implementation. Used for session tokens and API bearer tokens. Chosen over JWT because PASETO's secure defaults eliminate the algorithm confusion class of vulnerabilities and mmcp controls both issuer and verifier. | `mmcp-auth`        |
| `axum-login`          | User identification, authentication, and authorization middleware for `axum`. Provides `AuthUser` and `AuthnBackend` traits, session-backed login, `login_required!` and `permission_required!` route guards. | `mmcp-auth`, `mmcp-server` |
| `webauthn-rs`         | WebAuthn / passkey primitives used by the auth layer. Paired with the OAuth flow described in DESIGN §12; the OAuth adapter crate is still TBD. | `mmcp-auth`        |
| `argon2`              | Password hashing using the current best-practice KDF.                             | `mmcp-auth`        |
| `uuid` v7             | Time-ordered UUIDs used as the primary key for groups, projects, and memories (FR-028). Chronologically sortable, database-friendly. | every library + binary crate except `mmcp-session` |

### 2.4 Plumbing

| Crate                             | Purpose                                                                                       | Consumers                  |
| --------------------------------- | --------------------------------------------------------------------------------------------- | -------------------------- |
| `tokio`                           | Async runtime. Required by `axum`, `sqlx`/`sea-orm`, `rmcp`.                                  | all async crates           |
| `thiserror`                       | Typed error enums for library crates.                                                         | all libraries              |
| `anyhow`                          | Top-level error propagation in binary crates and the programmatic store layer.                | `mmcp-store`, `mmcp-server`, `mmcp-client`, `mmcp-gui` |
| `tracing`                         | Structured logging. Replaces `println!` entirely per Rust coding rules.                      | `mmcp-store`, `mmcp-server`, `mmcp-client`, `mmcp-gui` |
| `tracing-subscriber`              | `tracing` output formatting and filtering.                                                    | `mmcp-server`, `mmcp-client`, `mmcp-gui` |
| `clap` v4 (derive feature)        | CLI parsing for `mmcp-client` subcommands.                                                    | `mmcp-client`              |
| `semver`                          | Semantic version parsing and bumping. Drives the version-assignment logic in `mmcp-sync`.    | `mmcp-core`, `mmcp-sync`   |
| `jiff`                            | Modern date/time library. Chosen over `chrono` and `time` for better API and correctness.    | `mmcp-core`, `mmcp-store`, `mmcp-auth`, `mmcp-sync`, `mmcp-server`, `mmcp-client` |
| `bytes`                           | Efficient byte buffers for git object reads.                                                  | `mmcp-git`                 |
| `async-trait`                     | Async trait methods until Rust's native support covers all our cases.                        | `mmcp-git`, `mmcp-db`      |
| `notify`                          | Cross-platform filesystem watcher. The client uses it to rebuild its in-memory group index when `~/.mmcp/repos/` or the project `.mmcp/config.toml` changes. | `mmcp-client`              |
| `sha2`                            | SHA-256 digest used by `mmcp-session::compute_signature` for transcript fingerprints.        | `mmcp-session`             |

### 2.5 Frontend (Leptos)

| Crate                | Purpose                                                                | Consumers |
| -------------------- | ---------------------------------------------------------------------- | --------- |
| `leptos`             | Core fullstack Rust web framework. Fine-grained reactivity, SSR + hydration, server functions. | `webui`   |
| `leptos_axum`        | `axum` integration for Leptos server functions.                        | `webui`   |
| `leptos_meta`        | HTML head metadata management.                                         | `webui`   |
| `leptos_router`      | Client-side and server-side routing.                                   | `webui`   |
| `cargo-leptos`       | Build tool. Handles WASM compilation, SSR bundling, asset pipeline.    | build-time |

### 2.6 Testing

| Crate        | Purpose                                                                 | Consumers       |
| ------------ | ----------------------------------------------------------------------- | --------------- |
| `proptest`   | Property-based testing. Used for frontmatter / manifest round-trip proofs. | `mmcp-core`     |
| `wiremock`   | HTTP mocking for the sync client against a fake `mmcp-server`.          | `mmcp-sync`     |
| `tempfile`   | Temporary directories for git repo and store-layer tests.               | `mmcp-git`, `mmcp-store`, `mmcp-session`, `mmcp-sync`, `mmcp-server`, `mmcp-client` |
| `assert_cmd`, `predicates` | End-to-end CLI smoke tests that drive the built `mmcp` binary. | `mmcp-client`   |

## 3. External Services and Tools

Not Rust crates, but required by the project.

| Tool               | Role                                                                                       | Used by        |
| ------------------ | ------------------------------------------------------------------------------------------ | -------------- |
| **PostgreSQL**     | Primary database for `mmcp-server`. Users customize via `database_url`; also supported: SQLite for single-user deployments, via SeaORM's multi-backend support. | `mmcp-server`  |
| **SQLite**         | Default local database for `mmcp-client`'s offline cache and session state mirror.        | `mmcp-client`  |
| **cargo-nextest**  | Fast test runner with better output than `cargo test`. Recommended but optional.          | dev            |
| **cargo-deny**     | Dependency policy enforcement (licenses, advisories, bans).                                | dev, CI        |
| **cargo-leptos**   | Leptos build tool (see §2.5).                                                              | `webui` build  |
| **rustfmt**        | Formatter. Enforced in CI.                                                                 | dev, CI        |
| **clippy**         | Linter. `-D warnings` in CI per Rust coding rules.                                         | dev, CI        |

## 4. Rationale for Key Choices

### 4.1 Why Leptos over a JS framework

- Fullstack Rust: types from `mmcp-core` flow into the frontend without a code-generator step.
- Performance: fine-grained reactivity puts Leptos among the fastest web frameworks in the js-framework-benchmark - faster than React, Vue, and Svelte in DOM operations. Perf is not a tradeoff here.
- SSR + hydration out of the box, which is required for a GitHub-style administrative UI with server-rendered page shells.
- Tradeoff accepted: larger initial WASM bundle than a minimal JS SPA, and a smaller ecosystem of pre-built components. Both are manageable for a project that renders forms, lists, and diff views rather than complex data viz.

### 4.2 Why SeaORM over raw `sqlx` or Diesel

- Same entity definitions work against Postgres and SQLite out of the box. Diesel requires per-backend schema files; raw `sqlx` requires per-backend query files.
- User-customizable database URLs at runtime: one codepath, configured via `database_url = "postgres://..."` or `"sqlite://..."`.
- Built on top of `sqlx`, so compile-time checked raw queries remain available where we need them.
- Active development, modern API, good Axum integration patterns.

### 4.3 Why PASETO over JWT

- PASETO eliminates algorithm confusion attacks by design. JWT's `alg: none` and header-controlled algorithm selection are well-known footguns.
- Secure defaults: v4 public tokens use Ed25519; v4 local tokens use XChaCha20-Poly1305. No knobs to get wrong.
- mmcp controls both token issuance and verification. The JWT interop argument (needed for third-party OAuth providers) does not apply to mmcp's internal session and API tokens.
- Even the maintainer of `jwt-simple` recommends PASETO for projects with control over both ends.
- If we later need to issue JWTs for external interop (e.g. to a third-party webhook receiver that only speaks JWT), we add `jsonwebtoken` alongside PASETO; it is not an either-or choice.

### 4.4 Why `axum-login` + `oauth2-passkey-axum`

- `axum-login` is the closest Rust has to Django-style integrated auth: user identification, session management via `tower-sessions`, permission-based authorization, and ergonomic route guards.
- `oauth2-passkey-axum` is designed to slot into `axum-login` sessions, so OAuth and WebAuthn passkey flows share the same session store as password login.
- Both are actively maintained and tower/axum-native, so they compose with the rest of the middleware stack without adapters.

### 4.5 Why `gix` over `git2`

- Pure Rust. No libgit2 C dependency, no build-time system library requirement, cross-compiles cleanly.
- Memory safety and better error handling idioms than a C wrapper.
- Active development by a dedicated team. API is stabilizing.
- Tradeoff: server-side smart HTTP responder may not yet be complete in the released version. If missing, we implement it ourselves on top of `gix`'s object database - a bounded scope task documented by the git smart-http-protocol spec.

### 4.6 Why TOML frontmatter over YAML

- One config language across the project: `Cargo.toml`, `.mmcp/config.toml`, memory frontmatter. One parser, one mental model.
- Native typed arrays, tables, and datetimes. No YAML "Norway problem", no indentation traps, no ambiguous `1.0` versus `"1.0"` coercion.
- `gray_matter` supports TOML frontmatter with `+++` fences as a first-class mode; no custom parsing needed.
- Hugo, Zola, and other modern static site generators already use this convention, so it is familiar to users who have worked with markdown + frontmatter elsewhere.

### 4.7 Why native in-process git backend as default

- Zero operational dependencies. One binary, one process, one data directory. No sidecars, no docker-compose required for a first-run experience.
- Same auth as the rest of the API: git smart HTTP reuses `axum-login` session state. Users do not configure a second credential system just to push and pull.
- ACL enforcement happens in the control plane before git touches the repo, so mmcp's permission model drives git access directly rather than being mirrored into a forge's separate ACL layer.
- External forge backends (Forgejo, Gitea, GitHub, GitLab) remain available for users who want their content in an existing forge, but they are opt-in, not default.

## 5. Version Policy

- **Rust**: latest stable, MSRV pinned in the workspace `Cargo.toml` (`rust-version = "1.85"` today). Toolchain upgrades happen on the next stable release; the pin moves with them and is verified in CI.
- **Crate versions**: use latest stable for all dependencies. Bump on `cargo update` cycles, not piecemeal.
- **PASETO version**: v4 only. v1/v2/v3 are not supported.
- **MCP spec version**: track whatever `rmcp` supports. Currently `2025-06-18`.
- **Git protocol**: smart HTTP v2 when it becomes the default; v1 fallback until then.
