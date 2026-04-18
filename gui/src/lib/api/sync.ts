import { invoke } from '@tauri-apps/api/core';
import type { PullReport, PushReport, SyncStatus } from '../types';

export const syncStatus = () => invoke<SyncStatus>('sync_status');
export const syncPull = () => invoke<PullReport>('sync_pull');
export const syncPush = () => invoke<PushReport>('sync_push');
