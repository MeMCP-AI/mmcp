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
