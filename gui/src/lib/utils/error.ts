// Tauri rejects with the serialized `GuiError` (`{ kind, message }`),
// so `String(e)` would render `[object Object]`. Pull out the
// human-readable message (falling back to `kind` when the backend
// sent no message, e.g. `sync_not_configured`) instead of the
// literal string "null" a bare `'message' in err` narrowing would
// produce for that case.

export function formatErr(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err && typeof err === 'object') {
    const o = err as { message?: unknown; kind?: unknown };
    if (o.message != null) return String(o.message);
    if (o.kind != null) return String(o.kind);
  }
  return String(err);
}
