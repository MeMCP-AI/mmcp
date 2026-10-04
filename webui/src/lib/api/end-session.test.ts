import { expect, spyOn, test } from 'bun:test';
import { endSession } from './end-session';

/** A request that never answers and rejects once its signal aborts, like a stalled `fetch`. */
function stalledRequest(signal: AbortSignal): Promise<void> {
  return new Promise((_resolve, reject) => {
    signal.addEventListener('abort', () => reject(signal.reason));
  });
}

test('the local state is cleared before the server logout is sent', async () => {
  const events: string[] = [];

  await endSession(
    () => events.push('cleared'),
    async () => {
      events.push('sent');
    }
  );

  expect(events).toEqual(['cleared', 'sent']);
});

test('a stalled server logout leaves the local state already cleared', async () => {
  let cleared = false;

  const pending = endSession(
    () => {
      cleared = true;
    },
    stalledRequest,
    50
  );

  expect(cleared).toBe(true);
  const stillWaiting = await Promise.race([
    pending.then(() => false),
    new Promise<boolean>((resolve) => setTimeout(() => resolve(true), 10))
  ]);
  expect(stillWaiting).toBe(true);
  await pending;
});

test('a stalled server logout is abandoned after the time limit', async () => {
  const error = spyOn(console, 'error').mockImplementation(() => {});
  const started = Date.now();

  await endSession(() => {}, stalledRequest, 30);

  expect(Date.now() - started).toBeLessThan(2000);
  expect(error).toHaveBeenCalledTimes(1);
  error.mockRestore();
});

test('a failed server logout is logged and does not throw', async () => {
  const error = spyOn(console, 'error').mockImplementation(() => {});

  await endSession(
    () => {},
    async () => {
      throw new Error('network down');
    }
  );

  expect(error).toHaveBeenCalledTimes(1);
  error.mockRestore();
});
