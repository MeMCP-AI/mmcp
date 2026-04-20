// Bearer-token store.
//
// Persisted in `localStorage` so a hard reload keeps the session.
// Kept as a Svelte 5 `$state` class so consumers (layout, guards,
// `$lib/api/client`) can subscribe to changes without wiring
// custom event buses.

const STORAGE_KEY = 'mmcp_token';

type Session = { token: string; userId: string; expiresAt: number };

class AuthStore {
  token = $state<string | null>(null);
  userId = $state<string | null>(null);
  /** Unix seconds. `null` when not logged in. */
  expiresAt = $state<number | null>(null);

  /** Load from localStorage on first mount. No-op on SSR. */
  hydrate() {
    if (typeof window === 'undefined') return;
    try {
      const raw = window.localStorage.getItem(STORAGE_KEY);
      if (!raw) return;
      const parsed = JSON.parse(raw) as Session;
      // NOTE(gg 2026-04-19): expiry is advisory — the server is the
      // real arbiter and will 401 an expired token, which the client
      // wrapper translates back into `clear()`.
      this.token = parsed.token;
      this.userId = parsed.userId;
      this.expiresAt = parsed.expiresAt;
    } catch {
      this.clear();
    }
  }

  set(session: Session) {
    this.token = session.token;
    this.userId = session.userId;
    this.expiresAt = session.expiresAt;
    if (typeof window !== 'undefined') {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(session));
    }
  }

  clear() {
    this.token = null;
    this.userId = null;
    this.expiresAt = null;
    if (typeof window !== 'undefined') {
      window.localStorage.removeItem(STORAGE_KEY);
    }
  }
}

export const authStore = new AuthStore();
