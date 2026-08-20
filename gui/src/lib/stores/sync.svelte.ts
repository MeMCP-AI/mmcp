import { syncPull, syncPush, syncStatus } from '$lib/api/sync';
import { formatErr } from '$lib/utils/error';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { Finding, PullReport, PushReport } from '$lib/types';

// One-line summary of a `Finding[]` list, e.g. "2 group(s) failed: <id>: <msg>; ...".
function summarizeFailures(failed: Finding[]): string {
  const details = failed.map((f) => `${f.group}: ${f.message}`).join('; ');
  return `${failed.length} group(s) failed: ${details}`;
}

// One-line summary of a pull report's `manifest_failures` list.
function summarizeManifestFailures(report: PullReport): string {
  const details = report.manifest_failures.map((f) => `${f.remote_name}: ${f.message}`).join('; ');
  return `${report.manifest_failures.length} remote(s) unreachable: ${details}`;
}

// A pull is a full success only when every group AND every remote's
// manifest poll succeeded. Checking `failed` alone would silently
// hide an unreachable remote, exactly the no-silent-failure gap
// `manifest_failures` exists to close.
function pullFailed(report: PullReport): boolean {
  return report.failed.length > 0 || report.manifest_failures.length > 0;
}

function summarizePullFailure(report: PullReport): string {
  const parts = [`${report.updated} updated`, `${report.new_groups} new`];
  if (report.failed.length > 0) parts.push(summarizeFailures(report.failed));
  if (report.manifest_failures.length > 0) parts.push(summarizeManifestFailures(report));
  return parts.join(', ');
}

// A push is a full success only when every targeted remote's own
// outcome carries no failed group. Scanning `by_remote` (not a
// flattened total) is required, or a failure on a non-first remote
// would silently read as success.
function pushFailed(report: PushReport): boolean {
  return report.by_remote.some((r) => r.failed.length > 0);
}

function totalPushed(report: PushReport): number {
  return report.by_remote.reduce((sum, r) => sum + r.pushed, 0);
}

function summarizePushFailure(report: PushReport): string {
  return report.by_remote
    .map((r) => {
      const suffix = r.failed.length > 0 ? `, ${summarizeFailures(r.failed)}` : '';
      return `${r.remote_name}: ${r.pushed} pushed${suffix}`;
    })
    .join('; ');
}

// Broadcast by the Settings window after a successful
// `set_reference_point` so the main window refreshes the sync
// status (and hence the Pull/Push controls) without a restart.
const WORKSPACE_CHANGED_EVENT = 'workspace:changed';

type Phase =
  | { t: 'unknown' }
  | { t: 'failed' }
  | { t: 'not_configured' }
  | { t: 'idle'; remotesSummary: string }
  | { t: 'syncing'; op: 'pull' | 'push'; remotesSummary: string }
  | { t: 'ok'; op: 'pull' | 'push'; remotesSummary: string; summary: string }
  | { t: 'err'; op: 'pull' | 'push'; remotesSummary: string; message: string };

class SyncStore {
  phase = $state<Phase>({ t: 'unknown' });
  private unlisten: UnlistenFn | null = null;

  async refreshStatus() {
    await this.attachListener();
    try {
      const status = await syncStatus();
      if (status.configured && status.remotes_summary) {
        const summary = status.remotes_summary;
        const prev = this.remotesSummary();
        // Reset to idle when we were pristine, or when the
        // workspace switch pointed at a different remote set:
        // leaving the old `ok`/`err`/`syncing` phase up after a
        // switch would stamp the wrong summary on the status bar.
        if (
          this.phase.t === 'unknown' ||
          this.phase.t === 'failed' ||
          this.phase.t === 'not_configured' ||
          prev !== summary
        ) {
          this.phase = { t: 'idle', remotesSummary: summary };
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
    const summary = this.remotesSummary();
    if (!summary) return;
    this.phase = { t: 'syncing', op: 'pull', remotesSummary: summary };
    try {
      const report = await syncPull();
      if (pullFailed(report)) {
        // A per-group or per-remote failure must never read as full success.
        this.phase = {
          t: 'err',
          op: 'pull',
          remotesSummary: summary,
          message: summarizePullFailure(report)
        };
        return;
      }
      this.phase = {
        t: 'ok',
        op: 'pull',
        remotesSummary: summary,
        summary: `${report.updated} updated, ${report.new_groups} new`
      };
    } catch (err) {
      this.phase = {
        t: 'err',
        op: 'pull',
        remotesSummary: summary,
        message: formatErr(err)
      };
    }
  }

  async push() {
    const summary = this.remotesSummary();
    if (!summary) return;
    this.phase = { t: 'syncing', op: 'push', remotesSummary: summary };
    try {
      const report = await syncPush();
      if (pushFailed(report)) {
        // See the matching comment in `pull()`: a per-remote failure
        // must never silently read as full success.
        this.phase = {
          t: 'err',
          op: 'push',
          remotesSummary: summary,
          message: summarizePushFailure(report)
        };
        return;
      }
      this.phase = {
        t: 'ok',
        op: 'push',
        remotesSummary: summary,
        summary: `${totalPushed(report)} group(s) pushed`
      };
    } catch (err) {
      this.phase = {
        t: 'err',
        op: 'push',
        remotesSummary: summary,
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

  private remotesSummary(): string | null {
    switch (this.phase.t) {
      case 'idle':
      case 'syncing':
      case 'ok':
      case 'err':
        return this.phase.remotesSummary;
      default:
        return null;
    }
  }
}

export const syncStore = new SyncStore();
