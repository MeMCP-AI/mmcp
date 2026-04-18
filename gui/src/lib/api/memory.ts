import { invoke } from '@tauri-apps/api/core';
import type { MemoryFile } from '../types';

export const listMemorySlugs = (groupId: string) =>
  invoke<string[]>('list_memory_slugs', { groupId });

export const loadMemory = (groupId: string, slug: string) =>
  invoke<MemoryFile>('load_memory', { groupId, slug });

export const createMemory = (groupId: string, slug: string, memory: MemoryFile) =>
  invoke<string>('create_memory', { groupId, slug, memory });

export const updateMemory = (groupId: string, slug: string, memory: MemoryFile) =>
  invoke<string>('update_memory', { groupId, slug, memory });

export const deleteMemory = (groupId: string, slug: string) =>
  invoke<string>('delete_memory', { groupId, slug });
