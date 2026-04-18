import { invoke } from '@tauri-apps/api/core';
import type {
  LoadedProjectConfig,
  LoadedUserConfig,
  ProjectConfig,
  ResolvedAuthor,
  UserConfig
} from '$lib/types';

export function loadUserConfig(): Promise<LoadedUserConfig> {
  return invoke<LoadedUserConfig>('load_user_config');
}

export function saveUserConfig(config: UserConfig): Promise<ResolvedAuthor> {
  return invoke<ResolvedAuthor>('save_user_config', { config });
}

export function loadProjectConfig(path: string | null): Promise<LoadedProjectConfig> {
  return invoke<LoadedProjectConfig>('load_project_config', { path });
}

export function saveProjectConfig(root: string, config: ProjectConfig): Promise<void> {
  return invoke<void>('save_project_config', { args: { root, config } });
}
