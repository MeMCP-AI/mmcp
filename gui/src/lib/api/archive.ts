import { invoke } from '@tauri-apps/api/core';

export interface ArchiveGroupListing {
  group_id: string;
  slug: string;
  memory_slugs: string[];
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

// Export the chosen groups (empty = all) and optional memory-slug
// subset (empty = all memories) to an archive picked via a native save
// dialog. Resolves to `null` when the operator dismisses the dialog.
export const exportArchive = (groupIds: string[], memorySlugs: string[], gzip: boolean) =>
  invoke<ExportArchiveReport | null>('export_archive', { groupIds, memorySlugs, gzip });

// Open a native picker for an archive to import; `null` if dismissed.
export const pickImportPath = () => invoke<string | null>('pick_import_path');

// Enumerate an archive's groups and the memory slugs each carries.
export const inspectArchive = (input: string) =>
  invoke<ArchiveGroupListing[]>('inspect_archive', { input });

// Import a previously-picked archive with the dialog's selection.
// `onlyGroups` / `onlyMemorySlugs` empty mean "everything"; `intoGroup`
// null recreates the archived groups. Resolves to `null` when the
// operator declines a protected-group write.
export const importArchive = (
  input: string,
  onlyGroups: string[],
  onlyMemorySlugs: string[],
  intoGroup: string | null,
  overwrite: boolean,
  newIds: boolean
) =>
  invoke<ImportArchiveReport | null>('import_archive', {
    input,
    onlyGroups,
    onlyMemorySlugs,
    intoGroup,
    overwrite,
    newIds
  });
