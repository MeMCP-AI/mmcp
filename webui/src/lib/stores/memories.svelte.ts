import { groupInfo, listMemories } from '$lib/api/server';
import { formatErr } from '$lib/format';
import type { GroupInfo, MemoryDescriptor } from '$lib/types';

// Per-group descriptor cache. Server-side webui only needs summaries
// (list_memories) since read_memory is client-side-only per the MCP
// tool split; editing is intentionally out of scope here.
class MemoriesStore {
  byGroup = $state<Record<string, MemoryDescriptor[]>>({});
  info = $state<Record<string, GroupInfo>>({});
  loading = $state<Record<string, boolean>>({});
  error = $state<string | null>(null);

  async load(groupId: string) {
    this.loading[groupId] = true;
    this.error = null;
    try {
      const [mems, info] = await Promise.all([
        listMemories(groupId),
        groupInfo(groupId).catch(() => null)
      ]);
      this.byGroup[groupId] = mems;
      if (info) this.info[groupId] = info;
    } catch (err) {
      this.error = formatErr(err);
    } finally {
      this.loading[groupId] = false;
    }
  }
}

export const memoriesStore = new MemoriesStore();
