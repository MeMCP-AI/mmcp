import type { LoginOk } from '$lib/types';
import { request } from './client';

export function login(handle: string, password: string): Promise<LoginOk> {
  return request<LoginOk>('/auth/login', {
    method: 'POST',
    auth: false,
    body: { handle, password }
  });
}

/**
 * Ends the server session the login created, which the session cookie alone keeps alive.
 * The request is sent with `keepalive`, so closing the page does not cancel it.
 */
export function logout(signal?: AbortSignal): Promise<void> {
  return request<void>('/auth/logout', { method: 'POST', auth: false, signal, keepalive: true });
}
