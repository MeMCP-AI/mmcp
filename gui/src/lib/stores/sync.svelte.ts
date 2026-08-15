import { syncPull, syncPush, syncStatus } from '$lib/api/sync';
import { formatErr } from '$lib/utils/error';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { SyncGroupFailure } from '$lib/types';

// Turns a report's `failed` list into a single summary string, e.g.
// "2 group(s) failed: group-a: pending edit not found; group-b:
// transport error". Used whenever a pull/push report carries a
// non-empty `failed` list so the failure actually reaches the user
// instead of the phase silently reading as a full `ok`.
function summarizeFailures(failed: SyncGroupFailure[]): string {
  const details = failed.map((f) => `${f.group_id}: ${f.message}`).join('; ');
  return `${failed.length} group(s) failed: ${details}`;
}

// Broadcast by the Settings window after a successful
// `set_reference_point` so the main window refreshes the sync
// status (and hence the Pull/Push controls) without a restart.
const WORKSPACE_CHANGED_EVENT = 'workspace:changed';

type Phase =
  | { t: 'unknown' }
  | { t: 'failed' }
  | { t: 'not_configured' }
  | { t: 'idle'; serverUrl: string }
  | { t: 'syncing'; op: 'pull' | 'push'; serverUrl: string }
  | { t: 'ok'; op: 'pull' | 'push'; serverUrl: string; summary: string }
  | { t: 'err'; op: 'pull' | 'push'; serverUrl: string; message: string };

class SyncStore {
  phase = $state<Phase>({ t: 'unknown' });
  private unlisten: UnlistenFn | null = null;

  async refreshStatus() {
    await this.attachListener();
    try {
      const status = await syncStatus();
      if (status.configured && status.server_url) {
        const url = status.server_url;
        const prev = this.serverUrl();
        // Reset to idle when we were pristine, or when the
        // workspace switch pointed at a different server — leaving
        // the old `ok`/`err`/`syncing` phase up after a switch
        // would stamp the wrong server URL on the status bar.
        if (
          this.phase.t === 'unknown' ||
          this.phase.t === 'failed' ||
          this.phase.t === 'not_configured' ||
          prev !== url
        ) {
          this.phase = { t: 'idle', serverUrl: url };
        }
      } else {
        this.phase = { t: 'not_configured' };
      }
    } catch {
      // Distinct from the pristine `unknown` initial value so the UI
      // can tell "not loaded yet" apart from "the status call itself
      // failed" instead of silently hiding the Pull/Push controls
      // under the same phase as before any load was attempted.
      this.phase = { t: 'failed' };
    }
  }

  private async attachListener() {
    if (this.unlisten) return;
    this.unlisten = await listen(WORKSPACE_CHANGED_EVENT, () => {
      void this.refreshStatus();
    });
  }

  unmount() {
    this.unlisten?.();
    this.unlisten = null;
  }

  async pull() {
    const url = this.serverUrl();
    if (!url) return;
    this.phase = { t: 'syncing', op: 'pull', serverUrl: url };
    try {
      const report = await syncPull();
      if (report.failed.length > 0) {
        // Partial or total per-group failure: the engine still
        // completed every OTHER scheduled group (see
        // `GroupSyncFailureDto`'s doc comment), but a real failure
        // must never silently read as full success, so this reuses
        // the existing `err` phase rather than inventing a third
        // "partial" state.
        this.phase = {
          t: 'err',
          op: 'pull',
          serverUrl: url,
          message: `${report.updated} updated, ${report.new_groups} new, ${summarizeFailures(report.failed)}`
        };
        return;
      }
      this.phase = {
        t: 'ok',
        op: 'pull',
        serverUrl: url,
        summary: `${report.updated} updated, ${report.new_groups} new`
      };
    } catch (err) {
      this.phase = {
        t: 'err',
        op: 'pull',
        serverUrl: url,
        message: formatErr(err)
      };
    }
  }

  async push() {
    const url = this.serverUrl();
    if (!url) return;
    this.phase = { t: 'syncing', op: 'push', serverUrl: url };
    try {
      const report = await syncPush();
      if (report.failed.length > 0) {
        // See the matching comment in `pull()`: a per-group failure
        // must never silently read as full success.
        this.phase = {
          t: 'err',
          op: 'push',
          serverUrl: url,
          message: `${report.pushed} group(s) pushed, ${summarizeFailures(report.failed)}`
        };
        return;
      }
      this.phase = {
        t: 'ok',
        op: 'push',
        serverUrl: url,
        summary: `${report.pushed} group(s) pushed`
      };
    } catch (err) {
      this.phase = {
        t: 'err',
        op: 'push',
        serverUrl: url,
        message: formatErr(err)
      };
    }
  }

  get configured(): boolean {
    return (
      this.phase.t !== 'unknown' &&
      this.phase.t !== 'failed' &&
      this.phase.t !== 'not_configured'
    );
  }

  get inFlight(): boolean {
    return this.phase.t === 'syncing';
  }

  private serverUrl(): string | null {
    switch (this.phase.t) {
      case 'idle':
      case 'syncing':
      case 'ok':
      case 'err':
        return this.phase.serverUrl;
      default:
        return null;
    }
  }
}

export const syncStore = new SyncStore();
