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

/** Wire mirror of the Rust `MemoryDescriptorDto`. */
export interface MemoryDescriptor {
  slug: string;
  commit: string;
  frontmatter: MemoryFrontmatter;
}

/** Wire mirror of the Rust `MemoryDescriptorListDto`, the `list_memory_descriptors`
 * response shape. `skipped` makes a partial listing observable instead of the
 * caller silently receiving a truncated `descriptors` array with no signal.
 * Each skipped entry is a `Finding` (see below): the same skip/failure record
 * shape shared with `PullReport`/`PushReport` and `DiagReport`. */
export interface MemoryDescriptorList {
  descriptors: MemoryDescriptor[];
  skipped: Finding[];
}

export interface SyncStatus {
  configured: boolean;
  server_url: string | null;
}

export interface PullReport {
  updated: number;
  new_groups: number;
  /** Groups whose own attempt errored, as `Finding`s (see below). */
  failed: Finding[];
}

export interface PushReport {
  pushed: number;
  /** Groups whose own attempt errored, as `Finding`s (see below). */
  failed: Finding[];
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

// Mirrors every `kind` string emitted by `Serialize for GuiError` in gui/src-tauri/src/error/gui.rs.
// A missing variant is invisible to any `kind`-based branch here, so keep the union exhaustive.
export interface GuiErrorPayload {
  kind:
    | 'store'
    | 'git'
    | 'sync'
    | 'sync_not_configured'
    | 'group_not_in_mirror'
    | 'archive'
    | 'dialog'
    | 'utf8'
    | 'invalid_memory_kind'
    | 'invalid_group_id'
    | 'invalid_version'
    | 'invalid_feature_status'
    | 'invalid_issue_status'
    | 'io'
    | 'current_dir_unavailable'
    | 'not_a_directory'
    | 'tauri_path'
    | 'settings_json'
    | 'watcher'
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
