// Selection is two orthogonal concepts:
//   - The `slug` that's loaded in the viewer (single-item view).
//   - The `multi` set the toolbar's batch actions operate on.
//
// Clicking a row updates `slug` so the user can read it. Checkbox
// and Ctrl/Cmd-clicks toggle `multi` independently. Shift-click
// treats `anchor` (the last normally-clicked row) as one end of
// a range and extends the multi-set to cover everything in between
// in the filtered display order. Batch delete operates over
// `multi` when it's non-empty, falling back to `slug` otherwise.

import type { KindStr } from '$lib/types';

class SelectionStore {
  groupId = $state<string | null>(null);
  slug = $state<string | null>(null);
  filter = $state<string>('');
  multi = $state<Set<string>>(new Set());
  anchor = $state<string | null>(null);
  kindFilter = $state<Set<KindStr>>(new Set());
  mandatoryOnly = $state(false);

  selectGroup(groupId: string) {
    if (this.groupId !== groupId) {
      this.groupId = groupId;
      this.slug = null;
      this.filter = '';
      this.multi = new Set();
      this.anchor = null;
      this.kindFilter = new Set();
      this.mandatoryOnly = false;
    }
  }

  selectMemory(slug: string) {
    this.slug = slug;
    this.anchor = slug;
  }

  clearMemory() {
    this.slug = null;
  }

  setFilter(q: string) {
    this.filter = q;
  }

  toggleMulti(slug: string) {
    const next = new Set(this.multi);
    if (next.has(slug)) {
      next.delete(slug);
    } else {
      next.add(slug);
    }
    this.multi = next;
    this.anchor = slug;
  }

  /// Select every slug from the current anchor (inclusive) to
  /// `slug` (inclusive) in the given visible ordering. Falls back
  /// to a single-item toggle if there is no anchor yet.
  extendMulti(slug: string, visible: string[]) {
    if (!this.anchor) {
      this.toggleMulti(slug);
      return;
    }
    const a = visible.indexOf(this.anchor);
    const b = visible.indexOf(slug);
    if (a === -1 || b === -1) {
      this.toggleMulti(slug);
      return;
    }
    const [lo, hi] = a <= b ? [a, b] : [b, a];
    const next = new Set(this.multi);
    for (let i = lo; i <= hi; i++) next.add(visible[i]);
    this.multi = next;
  }

  clearMulti() {
    if (this.multi.size > 0) this.multi = new Set();
  }

  selectMultiAll(slugs: string[]) {
    this.multi = new Set(slugs);
  }

  toggleKindFilter(kind: KindStr) {
    const next = new Set(this.kindFilter);
    if (next.has(kind)) next.delete(kind);
    else next.add(kind);
    this.kindFilter = next;
  }

  clearKindFilter() {
    if (this.kindFilter.size > 0) this.kindFilter = new Set();
  }

  setMandatoryOnly(v: boolean) {
    this.mandatoryOnly = v;
  }

  clearAllFilters() {
    this.filter = '';
    this.kindFilter = new Set();
    this.mandatoryOnly = false;
  }
}

export const selectionStore = new SelectionStore();
