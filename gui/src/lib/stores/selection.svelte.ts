// Selection is two orthogonal concepts:
//   - The `slug` that's loaded in the viewer (single-item view).
//   - The `multi` set the toolbar's batch actions operate on.
//
// Clicking a row updates `slug` so the user can read it. Checkbox
// clicks toggle `multi` independently — batch delete, for example,
// operates over `multi` when it's non-empty, falling back to
// `slug` otherwise.

class SelectionStore {
  groupId = $state<string | null>(null);
  slug = $state<string | null>(null);
  filter = $state<string>('');
  multi = $state<Set<string>>(new Set());

  selectGroup(groupId: string) {
    if (this.groupId !== groupId) {
      this.groupId = groupId;
      this.slug = null;
      this.filter = '';
      this.multi = new Set();
    }
  }

  selectMemory(slug: string) {
    this.slug = slug;
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
  }

  clearMulti() {
    if (this.multi.size > 0) this.multi = new Set();
  }

  selectMultiAll(slugs: string[]) {
    this.multi = new Set(slugs);
  }
}

export const selectionStore = new SelectionStore();
