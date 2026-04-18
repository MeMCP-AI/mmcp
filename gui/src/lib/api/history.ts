import { invoke } from '@tauri-apps/api/core';
import type { CommitMeta, MemoryFile } from '$lib/types';

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
