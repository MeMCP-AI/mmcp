<script lang="ts">
  // Simple-to-advanced archive selection dialog. Simple: export/import
  // all groups or a chosen subset. Advanced: drill into a group and
  // pick individual memory slugs. Drives both directions: for export it
  // lists the local mirror; for import it inspects a chosen archive.

  import { onMount } from 'svelte';
  import { listGroups } from '$lib/api/groups';
  import { listMemorySlugs } from '$lib/api/memory';
  import {
    exportArchive,
    importArchive,
    inspectArchive,
    pickImportPath,
    type ArchiveGroupListing
  } from '$lib/api/archive';
  import type { GroupEntry } from '$lib/types';

  let { mode, onClose }: { mode: 'export' | 'import'; onClose: () => void } = $props();

  // Selectable units. For export these come from the local mirror; for
  // import from the inspected archive.
  type SelectableGroup = { id: string; slug: string };

  let busy = $state(true);
  let error = $state<string | null>(null);
  let result = $state<string | null>(null);

  let localGroups = $state<GroupEntry[]>([]);
  let archiveGroups = $state<ArchiveGroupListing[]>([]);
  let archivePath = $state<string | null>(null);

  let groupScope = $state<'all' | 'selected'>('all');
  let selectedGroupIds = $state<string[]>([]);
  let advanced = $state(false);
  let selectedMemorySlugs = $state<string[]>([]);
  let expanded = $state<string[]>([]);
  let memoriesByGroup = $state<Record<string, string[]>>({});

  // Export-only.
  let gzip = $state(false);
  // Import-only. Empty `into` recreates the archived groups.
  let into = $state('');
  let overwrite = $state(false);
  let newIds = $state(false);

  const groups = $derived<SelectableGroup[]>(
    mode === 'export'
      ? localGroups.map((g) => ({ id: g.group_id, slug: g.slug }))
      : archiveGroups.map((g) => ({ id: g.group_id, slug: g.slug }))
  );

  const title = $derived(mode === 'export' ? 'Export Archive' : 'Import Archive');
  const actionLabel = $derived(mode === 'export' ? 'Export' : 'Import');

  onMount(() => {
    void init();
  });

  async function init() {
    busy = true;
    error = null;
    try {
      localGroups = await listGroups();
      if (mode === 'import') {
        const path = await pickImportPath();
        if (!path) {
          onClose();
          return;
        }
        archivePath = path;
        archiveGroups = await inspectArchive(path);
        const memories: Record<string, string[]> = {};
        for (const g of archiveGroups) memories[g.group_id] = g.memory_slugs;
        memoriesByGroup = memories;
      }
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function toggle(list: string[], value: string): string[] {
    return list.includes(value) ? list.filter((v) => v !== value) : [...list, value];
  }

  function toggleGroup(id: string) {
    selectedGroupIds = toggle(selectedGroupIds, id);
  }

  function toggleMemory(slug: string) {
    selectedMemorySlugs = toggle(selectedMemorySlugs, slug);
  }

  async function toggleExpand(id: string) {
    expanded = toggle(expanded, id);
    if (mode === 'export' && memoriesByGroup[id] === undefined) {
      try {
        const slugs = await listMemorySlugs(id);
        memoriesByGroup = { ...memoriesByGroup, [id]: slugs };
      } catch (e) {
        error = String(e);
      }
    }
  }

  function groupMemories(id: string): string[] {
    return memoriesByGroup[id] ?? [];
  }

  async function submit() {
    busy = true;
    error = null;
    result = null;
    try {
      const groupIds = groupScope === 'all' ? [] : selectedGroupIds;
      const memorySlugs = advanced ? selectedMemorySlugs : [];
      if (mode === 'export') {
        const report = await exportArchive(groupIds, memorySlugs, gzip);
        if (!report) {
          onClose();
          return;
        }
        result = `Exported ${report.group_count} group(s), ${report.memory_count} memories to ${report.output}`;
      } else {
        if (!archivePath) {
          onClose();
          return;
        }
        const report = await importArchive(
          archivePath,
          groupIds,
          memorySlugs,
          into.trim() === '' ? null : into.trim(),
          overwrite,
          newIds
        );
        if (!report) {
          onClose();
          return;
        }
        const created = report.groups.reduce((n, g) => n + g.created, 0);
        const overwritten = report.groups.reduce((n, g) => n + g.overwritten, 0);
        const skipped = report.groups.reduce((n, g) => n + g.skipped, 0);
        const conflicts = report.groups.reduce((n, g) => n + g.conflicts, 0);
        result = `Imported ${report.groups.length} group(s): ${created} created, ${overwritten} overwritten, ${skipped} skipped, ${conflicts} conflicts`;
      }
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
  role="presentation"
  onclick={(e) => {
    if (e.target === e.currentTarget && !busy) onClose();
  }}
>
  <div
    class="flex max-h-[80vh] w-[36rem] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 text-sm text-fg shadow-xl"
    role="dialog"
    aria-modal="true"
    aria-label={title}
  >
    <header class="flex items-center justify-between border-b border-line px-4 py-2">
      <h2 class="text-sm font-medium text-fg">{title}</h2>
      <button type="button" class="text-fg-subtle hover:text-fg" onclick={onClose} aria-label="Close"
        >✕</button
      >
    </header>

    <div class="flex-1 overflow-y-auto px-4 py-3">
      {#if error}
        <p class="mb-3 rounded-md border border-red-700 bg-red-950/40 px-3 py-2 text-red-200">
          {error}
        </p>
      {/if}
      {#if result}
        <p class="mb-3 rounded-md border border-line bg-surface-2 px-3 py-2 text-fg">{result}</p>
      {/if}

      {#if !result}
        <!-- Group scope -->
        <div class="mb-3 flex items-center gap-2">
          <span class="text-fg-muted">Groups:</span>
          <button
            type="button"
            class="rounded-md px-2 py-0.5 {groupScope === 'all'
              ? 'bg-surface-2 text-fg'
              : 'text-fg-muted hover:bg-surface-2'}"
            onclick={() => (groupScope = 'all')}>All</button
          >
          <button
            type="button"
            class="rounded-md px-2 py-0.5 {groupScope === 'selected'
              ? 'bg-surface-2 text-fg'
              : 'text-fg-muted hover:bg-surface-2'}"
            onclick={() => (groupScope = 'selected')}>Selected</button
          >
          <label class="ml-auto flex items-center gap-1 text-fg-muted">
            <input type="checkbox" bind:checked={advanced} />
            Advanced (pick memories)
          </label>
        </div>

        <!-- Group / memory list -->
        <ul class="mb-3 max-h-56 overflow-y-auto rounded-md border border-line">
          {#each groups as g (g.id)}
            {@const groupSelected = groupScope === 'all' || selectedGroupIds.includes(g.id)}
            <li class="border-b border-line last:border-b-0">
              <div class="flex items-center gap-2 px-3 py-1.5">
                {#if groupScope === 'selected'}
                  <input
                    type="checkbox"
                    checked={selectedGroupIds.includes(g.id)}
                    onchange={() => toggleGroup(g.id)}
                  />
                {/if}
                <span class="flex-1 truncate {groupSelected ? 'text-fg' : 'text-fg-subtle'}"
                  >{g.slug}</span
                >
                {#if advanced}
                  <button
                    type="button"
                    class="text-xs text-fg-subtle hover:text-fg"
                    onclick={() => toggleExpand(g.id)}
                    >{expanded.includes(g.id) ? 'Hide' : 'Memories'}</button
                  >
                {/if}
              </div>
              {#if advanced && expanded.includes(g.id)}
                <ul class="bg-surface-2 px-6 py-1">
                  {#each groupMemories(g.id) as slug (slug)}
                    <li class="py-0.5">
                      <label class="flex items-center gap-2 text-fg-muted">
                        <input
                          type="checkbox"
                          checked={selectedMemorySlugs.includes(slug)}
                          onchange={() => toggleMemory(slug)}
                        />
                        <span class="truncate">{slug}</span>
                      </label>
                    </li>
                  {:else}
                    <li class="py-0.5 text-fg-subtle">no memories</li>
                  {/each}
                </ul>
              {/if}
            </li>
          {:else}
            <li class="px-3 py-2 text-fg-subtle">
              {busy ? 'Loading…' : 'No groups available.'}
            </li>
          {/each}
        </ul>
        {#if advanced}
          <p class="mb-3 text-xs text-fg-subtle">
            With no memory checked, every memory in the selected groups is included.
          </p>
        {/if}

        <!-- Mode-specific options -->
        {#if mode === 'export'}
          <label class="flex items-center gap-2 text-fg-muted">
            <input type="checkbox" bind:checked={gzip} />
            gzip-compress the archive
          </label>
        {:else}
          <div class="space-y-2">
            <label class="flex items-center gap-2 text-fg-muted">
              <span class="w-28">Import into</span>
              <select
                bind:value={into}
                class="flex-1 rounded-md border border-line bg-surface-2 px-2 py-1 text-fg"
              >
                <option value="">Recreate archived groups</option>
                {#each localGroups as g (g.group_id)}
                  <option value={g.group_id}>{g.slug}</option>
                {/each}
              </select>
            </label>
            <label class="flex items-center gap-2 text-fg-muted">
              <input type="checkbox" bind:checked={overwrite} />
              Overwrite memories that already exist with different content
            </label>
            <label class="flex items-center gap-2 text-fg-muted">
              <input type="checkbox" bind:checked={newIds} />
              Fork: mint fresh ids instead of preserving identities
            </label>
          </div>
        {/if}
      {/if}
    </div>

    <footer class="flex items-center justify-end gap-2 border-t border-line px-4 py-2">
      <button
        type="button"
        class="rounded-md px-3 py-1.5 text-fg-muted hover:bg-surface-2"
        onclick={onClose}>Close</button
      >
      {#if !result}
        <button
          type="button"
          class="inline-flex items-center rounded-md bg-sky-600 px-3 py-1.5 font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-50"
          onclick={submit}
          disabled={busy || (groupScope === 'selected' && selectedGroupIds.length === 0)}
        >
          {actionLabel}
        </button>
      {/if}
    </footer>
  </div>
</div>
