import { invoke } from '@tauri-apps/api/core';
import type { CommitMeta, DiffResult, MemoryFile } from '$lib/types';

export function listMemoryHistory(
  groupId: string,
  slug: string
): Promise<CommitMeta[]> {
  return invoke<CommitMeta[]>('list_memory_history', { groupId, slug });
}

export function loadMemoryAt(
  groupId: string,
  slug: string,
  commit: string
): Promise<MemoryFile> {
  return invoke<MemoryFile>('load_memory_at', { groupId, slug, commit });
}

/// Pass `from = null` to diff against an empty base (the commit
/// that introduced the file). Response is ordered top-to-bottom in
/// display order — equals, inserts, and deletes interleaved.
export function diffMemory(
  groupId: string,
  slug: string,
  from: string | null,
  to: string
): Promise<DiffResult> {
  return invoke<DiffResult>('diff_memory', { groupId, slug, from, to });
}
