import { invoke } from '@tauri-apps/api/core';
import type { SyncStatus } from '$lib/types';

/// Tell the backend to reload `.mmcp.toml` discovery against a new
/// anchor directory. Pass `null` to clear the override and fall
/// back to the process cwd on the next launch (the bundle is
/// rebuilt immediately either way).
///
/// The frontend is expected to have already persisted the path via
/// `settingsStore.setReferencePoint(...)` before calling this — the
/// settings file is the durable source of truth; this command just
/// applies the change to the live AppState.
export function setReferencePoint(path: string | null): Promise<SyncStatus> {
  return invoke<SyncStatus>('set_reference_point', { path });
}

/// Open a native folder picker parented to the main window. Runs
/// through a Rust command rather than `@tauri-apps/plugin-dialog`
/// on the JS side so the dialog's parent is always the main
/// window, not whichever webview happened to call — a Settings
/// window dialog that floats over Settings instead of the app
/// feels orphaned.
export function pickDirectory(
  defaultPath: string | null,
  title: string | null
): Promise<string | null> {
  return invoke<string | null>('pick_directory', {
    defaultPath,
    title
  });
}
