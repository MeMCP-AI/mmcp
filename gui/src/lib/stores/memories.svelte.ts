import { listMemorySlugs, loadMemory } from '$lib/api/memory';
import type { MemoryFile } from '$lib/types';

// Per-group slug lists + parsed MemoryFile cache. Background
// refreshes land in `pendingBodies` when they'd clobber the
// currently-viewed memory; the viewer renders a banner and swaps
// only when the user opts in.
class MemoriesStore {
  slugs = $state<Record<string, string[]>>({});
  bodies = $state<Record<string, MemoryFile>>({});
  pendingBodies = $state<Record<string, MemoryFile>>({});
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

  /// Silently refetch a single memory. If the fresh copy matches
  /// the cached one, no-op. If it differs AND the caller marks it
  /// as currently-viewed, stash as pending so the viewer can raise
  /// the "new version available" banner; otherwise replace the
  /// cache so the next time the user navigates there they see the
  /// fresh copy immediately.
  async refreshBody(groupId: string, slug: string, isCurrentlyViewed: boolean) {
    const key = `${groupId}:${slug}`;
    try {
      const fresh = await loadMemory(groupId, slug);
      const current = this.bodies[key];
      if (current && memoriesEqual(current, fresh)) {
        if (this.pendingBodies[key]) delete this.pendingBodies[key];
        return;
      }
      if (current && isCurrentlyViewed) {
        this.pendingBodies[key] = fresh;
      } else {
        this.bodies[key] = fresh;
        if (this.pendingBodies[key]) delete this.pendingBodies[key];
      }
    } catch (err) {
      this.error = formatErr(err);
    }
  }

  /// Refetch the slug list for `groupId` and silently reconcile
  /// every already-cached body in that group. `currentlyViewed`
  /// identifies the one memory the user is reading right now —
  /// its fresh copy goes to `pendingBodies` instead of overwriting
  /// `bodies` (see `refreshBody`). Memories that no longer appear
  /// in the fresh slug list (deleted upstream) get dropped.
  async refreshGroup(
    groupId: string,
    currentlyViewed: { groupId: string; slug: string } | null
  ) {
    try {
      this.slugs[groupId] = await listMemorySlugs(groupId);
    } catch (err) {
      this.error = formatErr(err);
      return;
    }
    const survivors = new Set(this.slugs[groupId] ?? []);
    const cachedSlugs = Object.keys(this.bodies)
      .filter((k) => k.startsWith(`${groupId}:`))
      .map((k) => k.slice(groupId.length + 1));
    for (const slug of cachedSlugs) {
      if (!survivors.has(slug)) {
        delete this.bodies[`${groupId}:${slug}`];
        delete this.pendingBodies[`${groupId}:${slug}`];
        continue;
      }
      const isCurrent =
        currentlyViewed?.groupId === groupId && currentlyViewed?.slug === slug;
      void this.refreshBody(groupId, slug, isCurrent);
    }
  }

  promotePending(groupId: string, slug: string) {
    const key = `${groupId}:${slug}`;
    const pending = this.pendingBodies[key];
    if (!pending) return;
    this.bodies[key] = pending;
    delete this.pendingBodies[key];
  }

  dismissPending(groupId: string, slug: string) {
    delete this.pendingBodies[`${groupId}:${slug}`];
  }

  invalidate(groupId: string, slug?: string) {
    if (slug) {
      delete this.bodies[`${groupId}:${slug}`];
      delete this.pendingBodies[`${groupId}:${slug}`];
    }
    delete this.slugs[groupId];
  }

  bodyFor(groupId: string, slug: string): MemoryFile | undefined {
    return this.bodies[`${groupId}:${slug}`];
  }

  pendingFor(groupId: string, slug: string): MemoryFile | undefined {
    return this.pendingBodies[`${groupId}:${slug}`];
  }

  isLoadingBody(groupId: string, slug: string): boolean {
    return !!this.loadingBody[`${groupId}:${slug}`];
  }
}

function memoriesEqual(a: MemoryFile, b: MemoryFile): boolean {
  // Memory bodies are small and serde-serialised, so JSON
  // stringification is fast and field order is stable enough for
  // equality. Cheaper than walking the frontmatter manually.
  return JSON.stringify(a) === JSON.stringify(b);
}

function formatErr(err: unknown): string {
  if (err && typeof err === 'object' && 'message' in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

export const memoriesStore = new MemoriesStore();
