// Hand-written TS mirrors of the Rust DTOs in
// gui/src-tauri/src/commands/*.rs. When a future FR wires
// specta / tauri-specta, this file will be auto-generated. For
// now, if a Rust DTO shape changes, change the mirror here and
// grep for the field name to update call sites.

export type KindStr =
  | 'rule'
  | 'snapshot'
  | 'log'
  | 'reference'
  | 'scratch'
  | 'feature';

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
  | 'open'
  | 'resolved'
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

export interface SyncStatus {
  configured: boolean;
  server_url: string | null;
}

export interface PullReport {
  updated: number;
  new_groups: number;
}

export interface PushReport {
  pushed: number;
}

/** Raw severity string as emitted by mmcp-store. Rust uses
 * `"error" | "warning" | "info"`; keep the TS type permissive
 * (string) and funnel everything through `normalizeSeverity` in
 * `$lib/utils/diag.ts` so a stale client never silently drops an
 * issue because of a spelling mismatch. */
export interface Issue {
  group: string;
  slug: string | null;
  severity: string;
  message: string;
}

export interface GroupReport {
  group_id: string;
  slug: string;
  manifest_ok: boolean;
  memory_count: number;
  issues: Issue[];
}

export interface DiagReport {
  project_issues: Issue[];
  groups: GroupReport[];
}

export interface ReachabilityEvent {
  online: boolean;
  reason: string | null;
}

export interface CommitMeta {
  id: string;
  short_id: string;
  subject: string;
  message: string;
  author_name: string;
  author_email: string;
  /** Seconds since the Unix epoch. */
  timestamp: number;
}

export interface DiffSpan {
  text: string;
  /**
   * True for the fragment of an insert/delete line that actually
   * diverged from its counterpart (inline word-level highlight).
   * False for the equal/context fragments surrounding it.
   */
  emphasized: boolean;
}

export type DiffRow =
  | { kind: 'equal'; old_lineno: number; new_lineno: number; text: string }
  | { kind: 'insert'; new_lineno: number; text: string; spans: DiffSpan[] }
  | { kind: 'delete'; old_lineno: number; text: string; spans: DiffSpan[] };

export interface DiffResult {
  /** `null` when the memory didn't exist at the base (pure insert). */
  from: string | null;
  to: string;
  rows: DiffRow[];
  inserted: number;
  deleted: number;
}

export interface GuiErrorPayload {
  kind:
    | 'store'
    | 'git'
    | 'sync'
    | 'sync_not_configured'
    | 'other';
  message?: string;
}

// --- mmcp-core config DTOs ---------------------------------------
// Mirror the Rust types in `crates/mmcp-core/src/config/*`. Fields
// are optional / nullable to match serde(default) + Option<T>.

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

export interface UserConfig {
  sync: UserSyncConfig | null;
  author: UserAuthorConfig | null;
  defaults: UserDefaultsConfig | null;
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

export interface ProjectGroupsConfig {
  no_default: boolean;
  additional: string[];
}

export interface ProjectLanguagesConfig {
  /** Serde renames `use_` to `use` — the field name on the wire. */
  use: string[];
  auto_detect: boolean;
}

export interface ProjectConfig {
  project_uuid: string;
  project_slug: string | null;
  sync: ProjectSyncConfig | null;
  groups: ProjectGroupsConfig;
  languages: ProjectLanguagesConfig;
}

export interface LoadedProjectConfig {
  root: string | null;
  config: ProjectConfig | null;
}
