import { ApiError } from '$lib/api/client';

// Cross-store error formatter. `ApiError` has a pre-formatted
// `.message`; everything else falls back to a best-effort string.
export function formatErr(err: unknown): string {
  if (err instanceof ApiError) return err.message;
  if (err && typeof err === 'object' && 'message' in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

export function shortCommit(hex: string, len = 7): string {
  return hex.slice(0, len);
}
