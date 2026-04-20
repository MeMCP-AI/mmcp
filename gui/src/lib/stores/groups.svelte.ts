import { listGroups, refreshGroups } from '$lib/api/groups';
import type { GroupEntry } from '$lib/types';

class GroupsStore {
  groups = $state<GroupEntry[]>([]);
  loading = $state(false);
  error = $state<string | null>(null);

  async load() {
    this.loading = true;
    this.error = null;
    try {
      this.groups = await listGroups();
    } catch (err) {
      this.error = formatErr(err);
    } finally {
      this.loading = false;
    }
  }

  async refresh() {
    this.loading = true;
    this.error = null;
    try {
      this.groups = await refreshGroups();
    } catch (err) {
      this.error = formatErr(err);
    } finally {
      this.loading = false;
    }
  }

  /// Refresh without flipping the loading flag — used by
  /// `mirror:changed` listeners so a background rediscovery doesn't
  /// flash the list into a loading state while the user is
  /// navigating.
  async refreshQuiet() {
    try {
      this.groups = await refreshGroups();
    } catch (err) {
      this.error = formatErr(err);
    }
  }
}

function formatErr(err: unknown): string {
  if (err && typeof err === 'object' && 'message' in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

export const groupsStore = new GroupsStore();
