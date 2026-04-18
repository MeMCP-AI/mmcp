import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ReachabilityEvent } from '../types';

export const onReachabilityChanged = (
  handler: (event: ReachabilityEvent) => void
): Promise<UnlistenFn> =>
  listen<ReachabilityEvent>('reachability:changed', (event) => handler(event.payload));

export const onAppInitFailed = (
  handler: (message: string) => void
): Promise<UnlistenFn> =>
  listen<{ message: string }>('app:init-failed', (event) =>
    handler(event.payload.message ?? 'unknown error')
  );
