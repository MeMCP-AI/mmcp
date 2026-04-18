import { invoke } from '@tauri-apps/api/core';
import type { GroupEntry } from '../types';

export const listGroups = () => invoke<GroupEntry[]>('list_groups');
export const refreshGroups = () => invoke<GroupEntry[]>('refresh_groups');
