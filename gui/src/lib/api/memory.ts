import { invoke } from '@tauri-apps/api/core';
import type { MemoryDescriptor, MemoryFile } from '../types';

export const listMemorySlugs = (groupId: string) =>
  invoke<string[]>('list_memory_slugs', { groupId });

/// Metadata-only listing for one group: every memory's frontmatter
/// plus a shared change-detection commit id, no bodies. One IPC call
/// per group instead of one `loadMemory` round trip per memory.
export const listMemoryDescriptors = (groupId: string) =>
  invoke<MemoryDescriptor[]>('list_memory_descriptors', { groupId });

export const loadMemory = (groupId: string, slug: string) =>
  invoke<MemoryFile>('load_memory', { groupId, slug });

export const createMemory = (groupId: string, slug: string, memory: MemoryFile) =>
  invoke<string>('create_memory', { groupId, slug, memory });

export const updateMemory = (groupId: string, slug: string, memory: MemoryFile) =>
  invoke<string>('update_memory', { groupId, slug, memory });

export const deleteMemory = (groupId: string, slug: string) =>
  invoke<string>('delete_memory', { groupId, slug });
