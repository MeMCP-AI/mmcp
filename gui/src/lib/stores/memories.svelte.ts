import { listMemoryDescriptors, listMemorySlugs, loadMemory } from '$lib/api/memory';
import { formatErr } from '$lib/utils/error';
import type { MemoryDescriptor, MemoryFile } from '$lib/types';

// Per-group slug lists + parsed MemoryFile cache. Background
// refreshes land in `pendingBodies` when they'd clobber the
// currently-viewed memory; the viewer renders a banner and swaps
// only when the user opts in.
//
// `descriptors` + `groupCommit` back the metadata-only listing
// (issue #126): one `list_memory_descriptors` call per group returns
// every memory's frontmatter plus the group's tip commit, so callers
// that only need frontmatter (the home dashboard, global search)
// never trigger a body download. `refreshGroup` uses `groupCommit`
// to skip re-downloading cached bodies when the group hasn't
// actually changed.
class MemoriesStore {
  slugs = $state<Record<string, string[]>>({});
  descriptors = $state<Record<string, MemoryDescriptor[]>>({});
  groupCommit = $state<Record<string, string>>({});
  bodies = $state<Record<string, MemoryFile>>({});
  pendingBodies = $state<Record<string, MemoryFile>>({});
  loadingSlugs = $state<Record<string, boolean>>({});
  loadingDescriptors = $state<Record<string, boolean>>({});
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

  /// Metadata-only listing for one group: frontmatter for every
  /// memory plus the group's tip commit, no bodies. Also seeds
  /// `slugs[groupId]` so callers that only need the slug list (e.g.
  /// `FeatureRelations`, `graph.ts`) keep working off this single
  /// call instead of a separate `loadSlugs` round trip.
  async loadDescriptors(groupId: string) {
    this.loadingDescriptors[groupId] = true;
    this.error = null;
    try {
      const list = await listMemoryDescriptors(groupId);
      this.descriptors[groupId] = list;
      this.slugs[groupId] = list.map((d) => d.slug);
      if (list.length > 0) this.groupCommit[groupId] = list[0].commit;
    } catch (err) {
      this.error = formatErr(err);
    } finally {
      this.loadingDescriptors[groupId] = false;
    }
  }

  descriptorsFor(groupId: string): MemoryDescriptor[] {
    return this.descriptors[groupId] ?? [];
  }

  isLoadingDescriptors(groupId: string): boolean {
    return !!this.loadingDescriptors[groupId];
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

  /// Refetch the descriptor list for `groupId` and silently
  /// reconcile every already-cached body in that group.
  /// `currentlyViewed` identifies the one memory the user is reading
  /// right now — its fresh copy goes to `pendingBodies` instead of
  /// overwriting `bodies` (see `refreshBody`). Memories that no
  /// longer appear in the fresh listing (deleted upstream) get
  /// dropped.
  ///
  /// Cached bodies are only re-downloaded when the group's tip
  /// commit actually changed since the last listing — comparing
  /// `groupCommit[groupId]` catches the common case (some OTHER
  /// group's file changed, or an unrelated file in this same repo)
  /// without a wasted `loadMemory` round trip per cached slug.
  async refreshGroup(
    groupId: string,
    currentlyViewed: { groupId: string; slug: string } | null
  ) {
    let list: MemoryDescriptor[];
    try {
      list = await listMemoryDescriptors(groupId);
    } catch (err) {
      this.error = formatErr(err);
      return;
    }
    this.descriptors[groupId] = list;
    this.slugs[groupId] = list.map((d) => d.slug);

    const freshCommit = list[0]?.commit ?? null;
    const staleCommit = this.groupCommit[groupId] ?? null;
    const tipChanged = freshCommit !== null && freshCommit !== staleCommit;
    if (freshCommit !== null) this.groupCommit[groupId] = freshCommit;

    const survivors = new Set(list.map((d) => d.slug));
    const cachedSlugs = Object.keys(this.bodies)
      .filter((k) => k.startsWith(`${groupId}:`))
      .map((k) => k.slice(groupId.length + 1));
    for (const slug of cachedSlugs) {
      if (!survivors.has(slug)) {
        delete this.bodies[`${groupId}:${slug}`];
        delete this.pendingBodies[`${groupId}:${slug}`];
        continue;
      }
      if (!tipChanged) continue;
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
    delete this.descriptors[groupId];
    delete this.groupCommit[groupId];
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

export const memoriesStore = new MemoriesStore();
