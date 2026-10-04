import { authStore } from '$lib/stores/auth.svelte';

// Resolution order for the mmcp-server base URL:
//   1. `VITE_MMCP_SERVER_URL` baked in at build time, for a web UI hosted apart from the server.
//      The server must list the origin serving this page in `MMCP_CORS_ALLOWED_ORIGINS`.
//   2. Empty, so each request goes to the origin that served this page: the server itself.
export function serverBaseUrl(): string {
  const fromEnv = import.meta.env.VITE_MMCP_SERVER_URL;
  if (typeof fromEnv === 'string' && fromEnv.length > 0) return fromEnv.replace(/\/$/, '');
  return '';
}

/** Origin of the server the requests reach: the configured base URL, else this page's origin. */
export function serverOrigin(): string {
  return serverBaseUrl() || window.location.origin;
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
  /** Aborts the request, for a call that must not wait on a stalled server. */
  signal?: AbortSignal;
  /** Lets the request outlive the page, for a call sent while the page closes. */
  keepalive?: boolean;
}

/// Core HTTP wrapper. Attaches the bearer token stored in
/// `authStore` when present, parses JSON responses, and normalises
/// non-2xx responses into `ApiError` so call sites can `try/catch`
/// once and format.
export async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const { method = 'GET', body, auth = true, signal, keepalive } = opts;
  const headers: Record<string, string> = { accept: 'application/json' };
  if (body !== undefined) headers['content-type'] = 'application/json';
  if (auth && authStore.token) headers['authorization'] = `Bearer ${authStore.token}`;

  const resp = await fetch(`${serverBaseUrl()}${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    signal,
    keepalive
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
