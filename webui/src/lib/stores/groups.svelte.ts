import { listManifest } from '$lib/api/server';
import { formatErr } from '$lib/format';
import type { RemoteGroup } from '$lib/types';

class GroupsStore {
  groups = $state<RemoteGroup[]>([]);
  loading = $state(false);
  error = $state<string | null>(null);

  async load() {
    this.loading = true;
    this.error = null;
    try {
      this.groups = await listManifest();
    } catch (err) {
      this.error = formatErr(err);
    } finally {
      this.loading = false;
    }
  }

  reset() {
    this.groups = [];
    this.error = null;
    this.loading = false;
  }
}

export const groupsStore = new GroupsStore();
