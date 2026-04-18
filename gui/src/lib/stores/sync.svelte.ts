import { syncPull, syncPush, syncStatus } from '$lib/api/sync';

type Phase =
  | { t: 'unknown' }
  | { t: 'not_configured' }
  | { t: 'idle'; serverUrl: string }
  | { t: 'syncing'; op: 'pull' | 'push'; serverUrl: string }
  | { t: 'ok'; op: 'pull' | 'push'; serverUrl: string; summary: string }
  | { t: 'err'; op: 'pull' | 'push'; serverUrl: string; message: string };

class SyncStore {
  phase = $state<Phase>({ t: 'unknown' });

  async refreshStatus() {
    try {
      const status = await syncStatus();
      if (status.configured && status.server_url) {
        // Preserve transient phases (syncing / ok / err) if we're
        // mid-op; only reset to idle when we were in unknown /
        // not_configured.
        if (this.phase.t === 'unknown' || this.phase.t === 'not_configured') {
          this.phase = { t: 'idle', serverUrl: status.server_url };
        }
      } else {
        this.phase = { t: 'not_configured' };
      }
    } catch {
      this.phase = { t: 'unknown' };
    }
  }

  async pull() {
    const url = this.serverUrl();
    if (!url) return;
    this.phase = { t: 'syncing', op: 'pull', serverUrl: url };
    try {
      const report = await syncPull();
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
      this.phase = {
        t: 'ok',
        op: 'push',
        serverUrl: url,
        summary: `${report.drained} edit(s) pushed`
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
    return this.phase.t !== 'unknown' && this.phase.t !== 'not_configured';
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

function formatErr(err: unknown): string {
  if (err && typeof err === 'object' && 'message' in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

export const syncStore = new SyncStore();
