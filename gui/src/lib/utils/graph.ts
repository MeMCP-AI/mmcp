// Memory reference / dependency graph helpers.
//
// Every variant that renders a "related" panel needs the same
// two lookups:
//
//   - resolveRef(target)   → (slug, groupId, body) from a UUID by
//     scanning cached memories across every group.
//   - findBacklinks(id)    → every cached memory whose frontmatter
//     `refs` points at `id`.
//
// Scans are bounded by whatever the memories store has already
// cached locally. Groups the user hasn't opened yet are missed by
// design — the alternative is loading every memory body in the
// mirror before the right panel can paint, which doesn't scale.

import { memoriesStore } from '$lib/stores/memories.svelte';
import type { MemoryFile } from '$lib/types';

export interface ResolvedRef {
  target: string;
  slug: string | null;
  groupId: string | null;
  body: MemoryFile | null;
}

/// Find the cached memory carrying `target` as its frontmatter
/// `id`. Returns a skeleton with only `target` populated when no
/// cached memory matches.
export function resolveRef(target: string): ResolvedRef {
  for (const gid of Object.keys(memoriesStore.slugs)) {
    for (const slug of memoriesStore.slugs[gid] ?? []) {
      const body = memoriesStore.bodyFor(gid, slug);
      if (body && body.frontmatter.id === target) {
        return { target, slug, groupId: gid, body };
      }
    }
  }
  return { target, slug: null, groupId: null, body: null };
}

export interface Backlink {
  groupId: string;
  slug: string;
  body: MemoryFile;
}

/// Every cached memory whose frontmatter `refs` list points at
/// `targetId`, excluding the caller's own `(groupId, slug)` pair.
/// Empty list when nothing cached references the target.
export function findBacklinks(
  targetId: string,
  excludeGroupId: string | null = null,
  excludeSlug: string | null = null
): Backlink[] {
  const out: Backlink[] = [];
  for (const gid of Object.keys(memoriesStore.slugs)) {
    for (const slug of memoriesStore.slugs[gid] ?? []) {
      if (gid === excludeGroupId && slug === excludeSlug) continue;
      const body = memoriesStore.bodyFor(gid, slug);
      if (!body) continue;
      if (body.frontmatter.refs.some((r) => r.target === targetId)) {
        out.push({ groupId: gid, slug, body });
      }
    }
  }
  return out;
}
