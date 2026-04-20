import { serverHealth } from '$lib/api/server';
import { formatErr } from '$lib/format';
import type { HealthInfo } from '$lib/types';

// Server-perspective health pulse. Powers the top-bar indicator so
// the admin immediately sees whether the upstream mmcp-server is
// reachable without clicking through.
class HealthStore {
  state = $state<HealthInfo | null>(null);
  error = $state<string | null>(null);
  probing = $state(false);
  private timer: ReturnType<typeof setInterval> | null = null;

  async probe() {
    this.probing = true;
    try {
      this.state = await serverHealth();
      this.error = null;
    } catch (err) {
      this.state = null;
      this.error = formatErr(err);
    } finally {
      this.probing = false;
    }
  }

  mount() {
    void this.probe();
    // 30s cadence matches the gui's reachability beat closely enough
    // that an operator watching both sees the same flicker.
    if (this.timer === null) {
      this.timer = setInterval(() => void this.probe(), 30_000);
    }
  }

  unmount() {
    if (this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }
}

export const healthStore = new HealthStore();
