import { onReachabilityChanged } from '$lib/api/events';
import type { UnlistenFn } from '@tauri-apps/api/event';

type State =
  | { t: 'unknown' }
  | { t: 'online' }
  | { t: 'offline'; reason: string };

class ReachabilityStore {
  state = $state<State>({ t: 'unknown' });
  private unlisten: UnlistenFn | null = null;

  async mount() {
    if (this.unlisten) return;
    this.unlisten = await onReachabilityChanged((event) => {
      this.state = event.online
        ? { t: 'online' }
        : { t: 'offline', reason: event.reason ?? 'unreachable' };
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
