import { onReachabilityChanged } from '$lib/api/events';
import type { UnlistenFn } from '@tauri-apps/api/event';

type State =
  | { t: 'unknown' }
  | { t: 'online' }
  | { t: 'offline'; reason: string };

class ReachabilityStore {
  state = $state<State>({ t: 'unknown' });
  private unlisten: UnlistenFn | null = null;
  /// Optional callback invoked the moment the probe flips from
  /// offline → online. Used by the main window to auto-trigger a
  /// silent sync pull once the server comes back.
  onRestore: (() => void) | null = null;

  async mount() {
    if (this.unlisten) return;
    this.unlisten = await onReachabilityChanged((event) => {
      const wasOffline = this.state.t === 'offline';
      const next: State = event.online
        ? { t: 'online' }
        : { t: 'offline', reason: event.reason ?? 'unreachable' };
      this.state = next;
      if (wasOffline && next.t === 'online') this.onRestore?.();
    });
  }

  unmount() {
    this.unlisten?.();
    this.unlisten = null;
  }

  get online(): boolean {
    return this.state.t === 'online';
  }
}

export const reachabilityStore = new ReachabilityStore();
