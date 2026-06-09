<script lang="ts">
  // Archive selection dialog. The advanced memory filter sits at the
  // top (include/exclude by kind and tag, any/all tags, text search,
  // mandatory tri-state); the group selection table sits below it with
  // a header global checkbox (no All/Selected mode). Advanced rows can
  // drill into individual memories. Drives both directions: export
  // lists the local mirror; import inspects a chosen archive.

  import { onMount } from 'svelte';
  import { ChevronDown, ChevronRight } from 'lucide-svelte';
  import { listGroups } from '$lib/api/groups';
  import { listMemorySlugs } from '$lib/api/memory';
  import {
    emptyFilter,
    exportArchive,
    importArchive,
    inspectArchive,
    pickImportPath,
    type ArchiveFilter,
    type ArchiveGroupListing
  } from '$lib/api/archive';
  import type { GroupEntry } from '$lib/types';

  let { mode, onClose }: { mode: 'export' | 'import'; onClose: () => void } = $props();

  const KINDS = ['rule', 'snapshot', 'log', 'reference', 'scratch', 'feature', 'issue'];

  type SelectableGroup = { id: string; slug: string };

  let busy = $state(true);
  let error = $state<string | null>(null);
  let result = $state<string | null>(null);

  let localGroups = $state<GroupEntry[]>([]);
  let archiveGroups = $state<ArchiveGroupListing[]>([]);
  let archivePath = $state<string | null>(null);

  let selectedGroupIds = $state<string[]>([]);
  let advanced = $state(false);
  let expanded = $state<string[]>([]);
  let memoriesByGroup = $state<Record<string, string[]>>({});

  // Filter facets.
  let pickedMemory = $state<string[]>([]);
  let includeKinds = $state<string[]>([]);
  let excludeKinds = $state<string[]>([]);
  let includeTags = $state('');
  let allTags = $state(false);
  let excludeTags = $state('');
  let search = $state('');
  let mandatory = $state<'any' | 'mandatory' | 'non-mandatory'>('any');

  // Export-only / import-only options.
  let gzip = $state(false);
  let into = $state('');
  let overwrite = $state(false);
  let newIds = $state(false);

  const groups = $derived<SelectableGroup[]>(
    mode === 'export'
      ? localGroups.map((g) => ({ id: g.group_id, slug: g.slug }))
      : archiveGroups.map((g) => ({ id: g.group_id, slug: g.slug }))
  );
  const allSelected = $derived(groups.length > 0 && selectedGroupIds.length === groups.length);
  const someSelected = $derived(selectedGroupIds.length > 0 && !allSelected);
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

  function indeterminate(node: HTMLInputElement, value: boolean) {
    node.indeterminate = value;
    return {
      update(v: boolean) {
        node.indeterminate = v;
      }
    };
  }

  function toggle(list: string[], value: string): string[] {
    return list.includes(value) ? list.filter((v) => v !== value) : [...list, value];
  }

  function toggleAll() {
    selectedGroupIds = allSelected ? [] : groups.map((g) => g.id);
  }

  async function toggleExpand(id: string) {
    expanded = toggle(expanded, id);
    if (mode === 'export' && memoriesByGroup[id] === undefined) {
      try {
        memoriesByGroup = { ...memoriesByGroup, [id]: await listMemorySlugs(id) };
      } catch (e) {
        error = String(e);
      }
    }
  }

  function groupMemories(id: string): string[] {
    return memoriesByGroup[id] ?? [];
  }

  function splitTokens(value: string): string[] {
    return value
      .split(/[\s,]+/)
      .map((t) => t.trim())
      .filter((t) => t.length > 0);
  }

  function buildFilter(): ArchiveFilter {
    const filter = emptyFilter();
    filter.memory = pickedMemory;
    filter.kind = includeKinds;
    filter.exclude_kind = excludeKinds;
    filter.tag = splitTokens(includeTags);
    filter.all_tags = allTags;
    filter.exclude_tag = splitTokens(excludeTags);
    filter.search = search.trim() === '' ? null : search.trim();
    filter.mandatory = mandatory === 'any' ? null : mandatory === 'mandatory';
    return filter;
  }

  async function submit() {
    busy = true;
    error = null;
    result = null;
    try {
      if (mode === 'export') {
        const report = await exportArchive(selectedGroupIds, buildFilter(), gzip);
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
          selectedGroupIds,
          buildFilter(),
          into.trim() === '' ? null : into.trim(),
          overwrite,
          newIds
        );
        if (!report) {
          onClose();
          return;
        }
        const sum = (pick: (g: (typeof report.groups)[number]) => number) =>
          report.groups.reduce((n, g) => n + pick(g), 0);
        result = `Imported ${report.groups.length} group(s): ${sum((g) => g.created)} created, ${sum((g) => g.overwritten)} overwritten, ${sum((g) => g.skipped)} skipped, ${sum((g) => g.conflicts)} conflicts`;
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
    class="flex max-h-[85vh] w-[40rem] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 text-sm text-fg shadow-xl"
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
        <!-- Filter first, then the groups it narrows. -->
        <button
          type="button"
          class="mb-2 flex w-full items-center gap-1 rounded-md px-1 py-1 text-left text-fg-muted hover:bg-surface-2 hover:text-fg"
          onclick={() => (advanced = !advanced)}
          aria-expanded={advanced}
        >
          {#if advanced}
            <ChevronDown size={14} class="text-fg-subtle" />
          {:else}
            <ChevronRight size={14} class="text-fg-subtle" />
          {/if}
          <span>Advanced filter</span>
        </button>

        {#if advanced}
          <div class="mb-3 space-y-2 rounded-md border border-line p-3">
            <p class="text-fg-muted">
              Memory filter — AND across facets. Ticking individual memories in the table below
              narrows further.
            </p>

            <div>
              <span class="text-fg-subtle">Include kinds</span>
              <div class="flex flex-wrap gap-2">
                {#each KINDS as k (k)}
                  <label class="flex items-center gap-1 text-fg-muted">
                    <input
                      type="checkbox"
                      checked={includeKinds.includes(k)}
                      onchange={() => (includeKinds = toggle(includeKinds, k))}
                    />
                    {k}
                  </label>
                {/each}
              </div>
            </div>

            <div>
              <span class="text-fg-subtle">Exclude kinds</span>
              <div class="flex flex-wrap gap-2">
                {#each KINDS as k (k)}
                  <label class="flex items-center gap-1 text-fg-muted">
                    <input
                      type="checkbox"
                      checked={excludeKinds.includes(k)}
                      onchange={() => (excludeKinds = toggle(excludeKinds, k))}
                    />
                    {k}
                  </label>
                {/each}
              </div>
            </div>

            <label class="block">
              <span class="text-fg-subtle">Include tags (space/comma separated)</span>
              <input
                type="text"
                bind:value={includeTags}
                class="mt-0.5 w-full rounded-md border border-line bg-surface-2 px-2 py-1 text-fg"
              />
            </label>
            <label class="flex items-center gap-2 text-fg-muted">
              <input type="checkbox" bind:checked={allTags} />
              Require all included tags
            </label>
            <label class="block">
              <span class="text-fg-subtle">Exclude tags</span>
              <input
                type="text"
                bind:value={excludeTags}
                class="mt-0.5 w-full rounded-md border border-line bg-surface-2 px-2 py-1 text-fg"
              />
            </label>
            <label class="block">
              <span class="text-fg-subtle">Search (name, description, tags, slug, body)</span>
              <input
                type="text"
                bind:value={search}
                class="mt-0.5 w-full rounded-md border border-line bg-surface-2 px-2 py-1 text-fg"
              />
            </label>
            <label class="flex items-center gap-2 text-fg-muted">
              <span class="w-24">Mandatory</span>
              <select
                bind:value={mandatory}
                class="rounded-md border border-line bg-surface-2 px-2 py-1 text-fg"
              >
                <option value="any">Any</option>
                <option value="mandatory">Only mandatory</option>
                <option value="non-mandatory">Only non-mandatory</option>
              </select>
            </label>
          </div>
        {/if}

        <p class="mb-1 text-fg-muted">Groups</p>
        <!-- Selection table with a header global checkbox -->
        <table class="mb-3 w-full table-fixed border-collapse overflow-hidden rounded-md border border-line">
          <thead>
            <tr class="bg-surface-2 text-left text-fg-muted">
              <th class="w-8 px-2 py-1">
                <input
                  type="checkbox"
                  checked={allSelected}
                  use:indeterminate={someSelected}
                  onchange={toggleAll}
                  aria-label="Select all groups"
                />
              </th>
              <th class="px-2 py-1 font-normal">Group</th>
              {#if advanced}<th class="w-20 px-2 py-1"></th>{/if}
            </tr>
          </thead>
          <tbody>
            {#each groups as g (g.id)}
              <tr class="border-t border-line">
                <td class="px-2 py-1">
                  <input
                    type="checkbox"
                    checked={selectedGroupIds.includes(g.id)}
                    onchange={() => (selectedGroupIds = toggle(selectedGroupIds, g.id))}
                  />
                </td>
                <td class="truncate px-2 py-1">{g.slug}</td>
                {#if advanced}
                  <td class="px-2 py-1 text-right">
                    <button
                      type="button"
                      class="text-xs text-fg-subtle hover:text-fg"
                      onclick={() => toggleExpand(g.id)}
                      >{expanded.includes(g.id) ? 'Hide' : 'Memories'}</button
                    >
                  </td>
                {/if}
              </tr>
              {#if advanced && expanded.includes(g.id)}
                <tr class="border-t border-line bg-surface-2">
                  <td></td>
                  <td colspan="2" class="px-2 py-1">
                    {#each groupMemories(g.id) as slug (slug)}
                      <label class="flex items-center gap-2 py-0.5 text-fg-muted">
                        <input
                          type="checkbox"
                          checked={pickedMemory.includes(slug)}
                          onchange={() => (pickedMemory = toggle(pickedMemory, slug))}
                        />
                        <span class="truncate">{slug}</span>
                      </label>
                    {:else}
                      <span class="text-fg-subtle">no memories</span>
                    {/each}
                  </td>
                </tr>
              {/if}
            {:else}
              <tr><td colspan="3" class="px-2 py-2 text-fg-subtle">
                {busy ? 'Loading…' : 'No groups available.'}
              </td></tr>
            {/each}
          </tbody>
        </table>

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
          disabled={busy || selectedGroupIds.length === 0}
        >
          {actionLabel}
        </button>
      {/if}
    </footer>
  </div>
</div>
