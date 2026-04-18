class SelectionStore {
  groupId = $state<string | null>(null);
  slug = $state<string | null>(null);

  selectGroup(groupId: string) {
    if (this.groupId !== groupId) {
      this.groupId = groupId;
      this.slug = null;
    }
  }

  selectMemory(slug: string) {
    this.slug = slug;
  }

  clearMemory() {
    this.slug = null;
  }
}

export const selectionStore = new SelectionStore();
