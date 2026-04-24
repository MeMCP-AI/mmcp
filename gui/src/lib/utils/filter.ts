// Shared memory-filter predicate. Every variant runs roughly the
// same "does this memory match the current filters?" check — text
// query over slug / name / description / tags, kind membership,
// mandatory-only. Centralising keeps the rules consistent so a
// filter tweak lands in every layout at once.

import type { KindStr, MemoryFile } from '$lib/types';

export interface MemoryFilterOptions {
  /** Trimmed + lower-cased already, or raw text. Matches slug /
   * frontmatter.name / description / tags via substring. */
  query?: string;
  /** Empty set means "all kinds pass". */
  kinds?: Set<KindStr>;
  /** When true, only memories with `frontmatter.mandatory === true`
   * survive. */
  mandatoryOnly?: boolean;
}

/// `slug` is supplied alongside the body because some callers only
/// need to match against the slug (e.g. before the body is
/// cached). A missing body short-circuits tag / name matches but
/// still allows slug-only matches on unloaded memories.
export function matchesMemoryFilter(
  slug: string,
  body: MemoryFile | undefined,
  opts: MemoryFilterOptions
): boolean {
  const { query = '', kinds, mandatoryOnly = false } = opts;

  if (mandatoryOnly && body?.frontmatter.mandatory !== true) return false;

  if (kinds && kinds.size > 0) {
    const k = body?.frontmatter.kind;
    if (!k || !kinds.has(k)) return false;
  }

  const q = query.trim().toLowerCase();
  if (!q) return true;

  if (slug.toLowerCase().includes(q)) return true;
  if (!body) return false;

  if (body.frontmatter.name.toLowerCase().includes(q)) return true;
  if (body.frontmatter.description.toLowerCase().includes(q)) return true;
  return body.frontmatter.tags.some((t) => t.toLowerCase().includes(q));
}
