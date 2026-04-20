import type { LoginOk } from '$lib/types';
import { request } from './client';

export function login(handle: string, password: string): Promise<LoginOk> {
  return request<LoginOk>('/auth/login', {
    method: 'POST',
    auth: false,
    body: { handle, password }
  });
}
