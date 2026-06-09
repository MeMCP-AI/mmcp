import { invoke } from '@tauri-apps/api/core';

// Memory filter facets. Keys are snake_case to match the Rust
// `ArchiveFilterDto` (Tauri deserializes nested objects via serde,
// which does not camelCase-convert nested fields).
export interface ArchiveFilter {
  memory: string[];
  exclude_memory: string[];
  kind: string[];
  exclude_kind: string[];
  tag: string[];
  all_tags: boolean;
  exclude_tag: string[];
  search: string | null;
  mandatory: boolean | null;
  has_refs: boolean | null;
}

export function emptyFilter(): ArchiveFilter {
  return {
    memory: [],
    exclude_memory: [],
    kind: [],
    exclude_kind: [],
    tag: [],
    all_tags: false,
    exclude_tag: [],
    search: null,
    mandatory: null,
    has_refs: null
  };
}

export interface ArchiveGroupListing {
  group_id: string;
  slug: string;
  scope: string;
  memory_slugs: string[];
  tags: string[];
}

export interface ExportArchiveReport {
  output: string;
  group_count: number;
  memory_count: number;
}

export interface GroupImportOutcome {
  source_group_id: string;
  target_group_id: string;
  slug: string;
  created_group: boolean;
  created: number;
  overwritten: number;
  skipped: number;
  conflicts: number;
}

export interface ImportArchiveReport {
  input: string;
  groups: GroupImportOutcome[];
}

// Export the chosen groups (empty = all) narrowed by `filter` to a
// native save dialog. Resolves to `null` when the dialog is dismissed.
export const exportArchive = (groupIds: string[], filter: ArchiveFilter, gzip: boolean) =>
  invoke<ExportArchiveReport | null>('export_archive', { groupIds, filter, gzip });

// Open a native picker for an archive to import; `null` if dismissed.
export const pickImportPath = () => invoke<string | null>('pick_import_path');

// Enumerate an archive's groups (scope, memory slugs, tags).
export const inspectArchive = (input: string) =>
  invoke<ArchiveGroupListing[]>('inspect_archive', { input });

// Distinct tags across the chosen local groups (empty = all), for the
// export dialog's tag autocomplete.
export const localTags = (groupIds: string[]) =>
  invoke<string[]>('local_tags', { groupIds });

// Import a previously-picked archive with the dialog's selection.
export const importArchive = (
  input: string,
  onlyGroups: string[],
  filter: ArchiveFilter,
  intoGroup: string | null,
  overwrite: boolean,
  newIds: boolean
) =>
  invoke<ImportArchiveReport | null>('import_archive', {
    input,
    onlyGroups,
    filter,
    intoGroup,
    overwrite,
    newIds
  });
