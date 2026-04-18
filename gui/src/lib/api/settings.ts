import { invoke } from '@tauri-apps/api/core';

// The backend treats settings as an opaque JSON blob so adding a
// setting frontend-side never needs a Rust edit. The frontend-side
// schema lives in `$lib/stores/settings.svelte.ts`.
export interface SettingsBlob<T = unknown> {
  0: T;
}

export const loadSettings = <T = unknown>() => invoke<SettingsBlob<T>>('load_settings');
export const saveSettings = <T = unknown>(settings: SettingsBlob<T>) =>
  invoke<void>('save_settings', { settings });
