import { invoke } from '@tauri-apps/api/core';

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

// Export groups to an archive chosen via the backend save dialog.
// An empty `groupIds` exports every mirrored group. Resolves to
// `null` when the operator dismisses the dialog.
export const exportArchive = (groupIds: string[], gzip: boolean) =>
  invoke<ExportArchiveReport | null>('export_archive', { groupIds, gzip });

// Import an archive chosen via the backend open dialog. `intoGroup`
// remaps every memory into one existing group. Resolves to `null`
// when the operator dismisses the dialog or declines a protected
// group write.
export const importArchive = (
  intoGroup: string | null,
  overwrite: boolean,
  newIds: boolean
) => invoke<ImportArchiveReport | null>('import_archive', { intoGroup, overwrite, newIds });
