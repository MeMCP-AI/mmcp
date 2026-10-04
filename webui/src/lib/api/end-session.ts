/** Longest wait for the server to confirm a logout, in milliseconds. */
export const SERVER_LOGOUT_TIMEOUT_MS = 5000;

/**
 * Ends the session of this browser.
 *
 * The local credentials go first, before any network call, so a stalled request never keeps the user signed in.
 * The server logout then runs under a time limit, and a failure of it is logged and never thrown.
 */
export async function endSession(
  clearLocalState: () => void,
  sendServerLogout: (signal: AbortSignal) => Promise<void>,
  timeoutMs: number = SERVER_LOGOUT_TIMEOUT_MS
): Promise<void> {
  clearLocalState();
  try {
    await sendServerLogout(AbortSignal.timeout(timeoutMs));
  } catch (error) {
    console.error('server logout failed', error);
  }
}
