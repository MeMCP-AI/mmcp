// Hand-written TS mirrors of the Rust DTOs in gui/src-tauri/src/commands/*.rs.
// Not code-generated.
// On a Rust DTO shape change, update the mirror here and grep for the field name to catch call sites.

// KindStr is derived from the single source of truth in
// $lib/utils/memory_kind (mirrors MemoryKind in
// crates/mmcp-core/src/memory/kind.rs); re-exported here so existing
// `import type { KindStr } from '$lib/types'` call sites keep working.
import type { KindStr } from '$lib/utils/memory_kind';
export type { KindStr };

export type GroupScope = 'global' | 'shared' | 'project';

export interface GroupEntry {
  group_id: string;
  slug: string;
  display_name: string | null;
  memory_count_hint: number;
  scope: GroupScope;
}

export interface MemoryRef {
  target: string;
  commit: string;
}

export type FeatureStatus =
  | 'requested'
  | 'approved'
  | 'pending'
  | 'completed'
  | 'blocked'
  | 'deferred'
  | 'duplicate'
  | 'superseded';

export interface FeatureMetadata {
  status: FeatureStatus;
  number: number | null;
  depends_on: string[];
  blocks: string[];
  superseded_by: MemoryRef | null;
}

export interface MemoryFrontmatter {
  id: string | null;
  name: string;
  description: string;
  kind: KindStr;
  mandatory: boolean;
  version: string | null;
  tags: string[];
  refs: MemoryRef[];
  feature?: FeatureMetadata | null;
}

export interface MemoryFile {
  frontmatter: MemoryFrontmatter;
  body: string;
}

/** Metadata-only listing entry: one memory's frontmatter plus the
 * owning group's tip commit at read time, no markdown body. Every
 * entry from the same `list_memory_descriptors` call carries the
 * same `commit` — see the Rust doc comment on `MemoryDescriptorDto`
 * for why group-level granularity is the right tradeoff here. */
export interface MemoryDescriptor {
  slug: string;
  commit: string;
  frontmatter: MemoryFrontmatter;
}

export interface SyncStatus {
  configured: boolean;
  server_url: string | null;
}

/** One group whose sync attempt itself errored. Mirrors the Rust
 * `GroupSyncFailureDto`; every OTHER scheduled group still ran to
 * completion even when this one appears here. */
export interface SyncGroupFailure {
  group_id: string;
  message: string;
}

export interface PullReport {
  updated: number;
  new_groups: number;
  failed: SyncGroupFailure[];
}

export interface PushReport {
  pushed: number;
  failed: SyncGroupFailure[];
}

/** Raw severity string emitted by mmcp-store (`"error" | "warning" | "info"`).
 * Kept as `string`, not a union, so an unrecognized value doesn't get silently dropped;
 * normalize via `normalizeSeverity` in `$lib/utils/diag.ts`.
 *
 * Mirrors `mmcp_store::diagnostics::Finding`.
 * `code` is a stable slug identifier (e.g. `manifest_unreadable`,
 * `memory_body_empty`); the GUI exposes it but does not group by it. */
export interface Finding {
  group: string;
  slug: string | null;
  severity: string;
  code: string;
  message: string;
}

export interface GroupReport {
  group_id: string;
  slug: string;
  manifest_ok: boolean;
  memory_count: number;
  findings: Finding[];
}

export interface DiagReport {
  project_findings: Finding[];
  groups: GroupReport[];
}

export interface ReachabilityEvent {
  online: boolean;
  reason: string | null;
}

// Mirrors the `kind` values gui/src-tauri/src/error.rs's
// `Serialize for GuiError` impl actually emits — every `GuiError`
// variant, one string each. Keep this union exhaustive: a Rust-side
// variant this list misses is invisible to any `kind`-based branch
// on the frontend (issue #153).
export interface GuiErrorPayload {
  kind:
    | 'store'
    | 'git'
    | 'sync'
    | 'sync_not_configured'
    | 'archive'
    | 'dialog'
    | 'utf8'
    | 'other';
  message?: string;
}

// --- mmcp-core config DTOs ---------------------------------------
// Mirror the Rust types in `crates/mmcp-core/src/config/*`.
// Fields are optional / nullable to match serde(default) + Option<T>.

export interface UserSyncConfig {
  server_url: string;
}

export interface UserAuthorConfig {
  name: string | null;
  email: string | null;
  /** Tri-state: null = unset (warn), true = enable, false = opt-out. */
  git_fallback: boolean | null;
}

export interface UserDefaultsConfig {
  group: string | null;
}

export interface UserLimitsConfig {
  max_auto_slug_length: number | null;
}

export interface UserConfig {
  sync: UserSyncConfig | null;
  author: UserAuthorConfig | null;
  defaults: UserDefaultsConfig | null;
  limits: UserLimitsConfig | null;
}

export interface ResolvedAuthor {
  name: string;
  email: string;
}

export interface LoadedUserConfig {
  path: string;
  config: UserConfig;
  resolved_author: ResolvedAuthor;
}

export interface ProjectSyncConfig {
  server_url: string;
}

export interface ProjectSubscriptionsConfig {
  no_default_global: boolean;
  auto_detect_languages: boolean;
  languages: string[];
  groups: string[];
  memories: string[];
  tags: string[];
}

export interface ProjectConfig {
  project_uuid: string;
  project_slug: string | null;
  sync: ProjectSyncConfig | null;
  subscriptions: ProjectSubscriptionsConfig;
}

export interface LoadedProjectConfig {
  root: string | null;
  config: ProjectConfig | null;
}
