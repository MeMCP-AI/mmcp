import { authStore } from '$lib/stores/auth.svelte';

// Resolution order for the mmcp-server base URL:
//   1. `VITE_MMCP_SERVER_URL` baked in at build time.
//   2. `/mmcp` on the same origin, for deployments that reverse-proxy
//      the server behind the webui.
//   3. `http://127.0.0.1:8787`, the local-dev default from `.mmcp.toml`.
const FALLBACK_URL = 'http://127.0.0.1:8787';

export function serverBaseUrl(): string {
  const fromEnv = import.meta.env.VITE_MMCP_SERVER_URL;
  if (typeof fromEnv === 'string' && fromEnv.length > 0) return fromEnv.replace(/\/$/, '');
  return FALLBACK_URL;
}

/** JSON-bearing HTTP error with the server's raw body for diagnostics. */
export class ApiError extends Error {
  constructor(
    public status: number,
    public body: string,
    public method: string,
    public path: string
  ) {
    super(`${method} ${path} → ${status}: ${body || '<empty>'}`);
    this.name = 'ApiError';
  }
}

interface RequestOptions {
  method?: 'GET' | 'POST' | 'PUT' | 'DELETE';
  body?: unknown;
  /** Set `false` to bypass auto-attached bearer (e.g. /auth/login). */
  auth?: boolean;
}

/// Core HTTP wrapper. Attaches the bearer token stored in
/// `authStore` when present, parses JSON responses, and normalises
/// non-2xx responses into `ApiError` so call sites can `try/catch`
/// once and format.
export async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const { method = 'GET', body, auth = true } = opts;
  const headers: Record<string, string> = { accept: 'application/json' };
  if (body !== undefined) headers['content-type'] = 'application/json';
  if (auth && authStore.token) headers['authorization'] = `Bearer ${authStore.token}`;

  const resp = await fetch(`${serverBaseUrl()}${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body)
  });

  if (!resp.ok) {
    const text = await resp.text().catch(() => '');
    if (resp.status === 401 && auth) authStore.clear();
    throw new ApiError(resp.status, text, method, path);
  }
  // 204 No Content and empty bodies: callers that declare `T = void`
  // accept the undefined cast.
  if (resp.status === 204) return undefined as T;
  const text = await resp.text();
  if (!text) return undefined as T;
  return JSON.parse(text) as T;
}
