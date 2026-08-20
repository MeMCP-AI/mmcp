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

/** Wire mirror of the Rust `SyncStatusDto`. `remotes_summary` is the
 * effective remote set's label: the sole remote's name, or
 * `"N remote(s), default '<name>'"`. */
export interface SyncStatus {
  configured: boolean;
  remotes_summary: string | null;
}

/** One `mmcp-server`-transport remote whose manifest poll itself
 * errored during a pull's fetch phase. Mirrors the Rust
 * `RemoteManifestFailureDto`: a remote-level failure, not a group-level
 * one, so it carries a remote name rather than a `Finding`'s `group`. */
export interface RemoteManifestFailure {
  remote_name: string;
  code: string;
  message: string;
}

export interface PullReport {
  updated: number;
  new_groups: number;
  /** Groups whose own attempt errored, as `Finding`s (see below). */
  failed: Finding[];
  /** Remotes whose manifest poll itself errored; never silently dropped. */
  manifest_failures: RemoteManifestFailure[];
}

/** One remote's push outcome. Mirrors the Rust `RemotePushOutcomeDto`. */
export interface RemotePushOutcome {
  remote_name: string;
  pushed: number;
  /** Groups whose own attempt errored, as `Finding`s (see below). */
  failed: Finding[];
}

/** Wire mirror of the Rust `PushReportDto`: one outcome per targeted
 * remote (today always exactly one, `PushScope::Default`; kept
 * per-remote for when a scope picker lands). */
export interface PushReport {
  by_remote: RemotePushOutcome[];
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

/** Wire mirror of the Rust `ReachabilityEvent`. Three states, not a
 * `boolean` plus nullable reason: `not_applicable` is distinct from
 * `offline`, emitted whenever the active default remote has no
 * manifest endpoint to probe (no sync configured, or a `direct-git`
 * default), so the badge resets instead of keeping a PREVIOUS
 * workspace's online/offline reading. */
export type ReachabilityEvent =
  | { status: 'online' }
  | { status: 'offline'; reason: string }
  | { status: 'not_applicable' };

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
// A field marked optional (`field?:`) mirrors a Rust
// `#[serde(skip_serializing_if = ...)]` field: on load the JSON key
// is ABSENT (not `null`) when the value is empty/unset. A field typed
// `T | null` mirrors a plain `Option<T>` with no skip: Rust always
// emits the key, `null` when unset.

/** Mirrors `mmcp_core::config::RemoteAuth`'s kebab-case serialization. */
export type RemoteAuth = 'none' | 'ssh-agent' | 'bearer';

export interface MmcpServerRemote {
  kind: 'mmcp-server';
  name: string;
  url: string;
  default: boolean;
  include_in_push_all: boolean;
}

export interface DirectGitRemote {
  kind: 'direct-git';
  name: string;
  url: string;
  auth: RemoteAuth;
  /** Absent when unset. At project level an absent value resolves to
   * the project's own group; at user level an absent value is a
   * loud validation error, never silently defaulted. */
  group?: string;
  default: boolean;
  include_in_push_all: boolean;
}

/** Mirrors `mmcp_core::config::Remote`'s internally-tagged `kind`. */
export type Remote = MmcpServerRemote | DirectGitRemote;

/** Mirrors `mmcp_core::config::SyncConfig`, reused verbatim on both
 * `UserConfig.sync` and `ProjectConfig.sync`. */
export interface SyncConfig {
  /** Legacy single-remote shorthand, composed with `remotes` at
   * resolution time rather than validated as mutually exclusive. */
  server_url: string | null;
  remotes: Remote[];
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
  min_password_length: number | null;
  max_password_length: number | null;
  max_handle_length: number | null;
}

export interface UserConfig {
  /** Absent when the user declares no `[sync]` table at all. */
  sync?: SyncConfig;
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
  /** Absent on configs written by pre-slug versions of `mmcp init`. */
  project_slug?: string;
  /** Absent when the project declares no `[sync]` table at all. */
  sync?: SyncConfig;
  /** When true, this project uses ONLY its own `sync.remotes`; the
   * user-level remotes are not inherited. */
  project_remote_only: boolean;
  subscriptions: ProjectSubscriptionsConfig;
}

export interface LoadedProjectConfig {
  root: string | null;
  config: ProjectConfig | null;
}
