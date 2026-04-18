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

export interface GroupEntry {
  group_id: string;
  slug: string;
  display_name: string | null;
  memory_count_hint: number;
}

export interface MemoryFrontmatter {
  id: string | null;
  name: string;
  description: string;
  kind: KindStr;
  mandatory: boolean;
  version: string | null;
  tags: string[];
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
  drained: number;
}

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

export interface GuiErrorPayload {
  kind:
    | 'store'
    | 'git'
    | 'sync'
    | 'sync_not_configured'
    | 'other';
  message?: string;
}
