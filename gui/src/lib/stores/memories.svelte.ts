import { listMemorySlugs, loadMemory } from '$lib/api/memory';
import type { MemoryFile } from '$lib/types';

// Per-group slug lists + parsed MemoryFile cache.
class MemoriesStore {
  slugs = $state<Record<string, string[]>>({});
  bodies = $state<Record<string, MemoryFile>>({});
  loadingSlugs = $state<Record<string, boolean>>({});
  loadingBody = $state<Record<string, boolean>>({});
  error = $state<string | null>(null);

  async loadSlugs(groupId: string) {
    this.loadingSlugs[groupId] = true;
    this.error = null;
    try {
      this.slugs[groupId] = await listMemorySlugs(groupId);
    } catch (err) {
      this.error = formatErr(err);
    } finally {
      this.loadingSlugs[groupId] = false;
    }
  }

  async loadBody(groupId: string, slug: string) {
    const key = `${groupId}:${slug}`;
    if (this.bodies[key]) return;
    this.loadingBody[key] = true;
    this.error = null;
    try {
      this.bodies[key] = await loadMemory(groupId, slug);
    } catch (err) {
      this.error = formatErr(err);
    } finally {
      this.loadingBody[key] = false;
    }
  }

  invalidate(groupId: string, slug?: string) {
    if (slug) {
      delete this.bodies[`${groupId}:${slug}`];
    }
    delete this.slugs[groupId];
  }

  bodyFor(groupId: string, slug: string): MemoryFile | undefined {
    return this.bodies[`${groupId}:${slug}`];
  }

  isLoadingBody(groupId: string, slug: string): boolean {
    return !!this.loadingBody[`${groupId}:${slug}`];
  }
}

function formatErr(err: unknown): string {
  if (err && typeof err === 'object' && 'message' in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

export const memoriesStore = new MemoriesStore();
