<script lang="ts">
  // Archive selection dialog.
  //
  //   • Search is always available (top).
  //   • Advanced filter is a collapsible panel above the groups; its
  //     toggle is centered and sits at the bottom of the panel, so it
  //     closes from the bottom.
  //   • Kinds and tags are tri-state (neutral / include / exclude).
  //     Tags autocomplete against the real tag universe.
  //   • Groups are filtered and grouped by scope (global/shared/
  //     project), with a global checkbox in the table header.
  //
  // Drives both directions: export lists the local mirror; import
  // inspects a chosen archive.

  import { onMount } from 'svelte';
  import { ChevronDown, ChevronUp } from 'lucide-svelte';
  import { listGroups } from '$lib/api/groups';
  import { listMemorySlugs } from '$lib/api/memory';
  import {
    emptyFilter,
    exportArchive,
    importArchive,
    inspectArchive,
    localTags,
    pickImportPath,
    type ArchiveFilter,
    type ArchiveGroupListing
  } from '$lib/api/archive';
  import type { GroupEntry } from '$lib/types';

  let { mode, onClose }: { mode: 'export' | 'import'; onClose: () => void } = $props();

  const KINDS = ['rule', 'snapshot', 'log', 'reference', 'scratch', 'feature', 'issue'];
  const SCOPES: { id: string; label: string }[] = [
    { id: 'global', label: 'Global' },
    { id: 'shared', label: 'Shared' },
    { id: 'project', label: 'Project' }
  ];
  type TriState = 'include' | 'exclude';

  type SelectableGroup = { id: string; slug: string; scope: string };

  let busy = $state(true);
  let error = $state<string | null>(null);
  let result = $state<string | null>(null);

  let localGroups = $state<GroupEntry[]>([]);
  let archiveGroups = $state<ArchiveGroupListing[]>([]);
  let archivePath = $state<string | null>(null);

  let selectedGroupIds = $state<string[]>([]);
  let visibleScopes = $state<string[]>(['global', 'shared', 'project']);

  // Facets.
  let search = $state('');
  let advanced = $state(false);
  let kindState = $state<Record<string, TriState>>({});
  let tagState = $state<Record<string, TriState>>({});
  let tagInput = $state('');
  let availableTags = $state<string[]>([]);
  let mandatory = $state<'any' | 'mandatory' | 'non-mandatory'>('any');
  let hasRefs = $state<'any' | 'yes' | 'no'>('any');
  let tagsLoaded = $state(false);
  let pickedMemory = $state<string[]>([]);
  let expanded = $state<string[]>([]);
  let memoriesByGroup = $state<Record<string, string[]>>({});

  // Export-only / import-only.
  let gzip = $state(false);
  let into = $state('');
  let overwrite = $state(false);
  let newIds = $state(false);

  const groups = $derived<SelectableGroup[]>(
    mode === 'export'
      ? localGroups.map((g) => ({ id: g.group_id, slug: g.slug, scope: g.scope }))
      : archiveGroups.map((g) => ({ id: g.group_id, slug: g.slug, scope: g.scope }))
  );
  const visibleGroups = $derived(groups.filter((g) => visibleScopes.includes(g.scope)));
  const allSelected = $derived(
    visibleGroups.length > 0 && visibleGroups.every((g) => selectedGroupIds.includes(g.id))
  );
  const someSelected = $derived(
    visibleGroups.some((g) => selectedGroupIds.includes(g.id)) && !allSelected
  );
  const tagSuggestions = $derived(
    tagInput.trim() === ''
      ? []
      : availableTags
          .filter(
            (t) => t.toLowerCase().includes(tagInput.trim().toLowerCase()) && tagState[t] === undefined
          )
          .slice(0, 8)
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
        const tags = new Set<string>();
        for (const g of archiveGroups) {
          memories[g.group_id] = g.memory_slugs;
          for (const t of g.tags) tags.add(t);
        }
        memoriesByGroup = memories;
        availableTags = [...tags].sort();
        tagsLoaded = true;
      }
      // The export tag universe scans every memory across the mirror,
      // so it loads lazily in the background when the filter opens
      // (see ensureExportTags) rather than blocking the form.
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  // Load the export tag universe once, off the critical path, so the
  // autocomplete fills in without ever disabling the Export button.
  async function ensureExportTags() {
    if (mode !== 'export' || tagsLoaded) return;
    tagsLoaded = true;
    try {
      availableTags = await localTags([]);
    } catch {
      // Autocomplete simply stays empty; not worth surfacing.
      tagsLoaded = false;
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
    selectedGroupIds = allSelected ? [] : visibleGroups.map((g) => g.id);
  }

  // Cycle a tri-state entry: absent -> include -> exclude -> absent.
  function cycle(map: Record<string, TriState>, key: string): Record<string, TriState> {
    const next = { ...map };
    if (next[key] === undefined) next[key] = 'include';
    else if (next[key] === 'include') next[key] = 'exclude';
    else delete next[key];
    return next;
  }

  function triClass(state: TriState | undefined): string {
    if (state === 'include') return 'border-emerald-600 bg-emerald-950/40 text-emerald-300';
    if (state === 'exclude') return 'border-red-600 bg-red-950/40 text-red-300';
    return 'border-line text-fg-muted hover:bg-surface-2';
  }

  function triGlyph(state: TriState | undefined): string {
    if (state === 'include') return '+';
    if (state === 'exclude') return '−';
    return '';
  }

  function addTag(tag: string) {
    const t = tag.trim();
    if (t === '') return;
    if (tagState[t] === undefined) tagState = { ...tagState, [t]: 'include' };
    tagInput = '';
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

  function keysWhere(map: Record<string, TriState>, state: TriState): string[] {
    return Object.keys(map).filter((k) => map[k] === state);
  }

  function buildFilter(): ArchiveFilter {
    const filter = emptyFilter();
    filter.memory = pickedMemory;
    filter.kind = keysWhere(kindState, 'include');
    filter.exclude_kind = keysWhere(kindState, 'exclude');
    filter.tag = keysWhere(tagState, 'include');
    filter.exclude_tag = keysWhere(tagState, 'exclude');
    filter.search = search.trim() === '' ? null : search.trim();
    filter.mandatory = mandatory === 'any' ? null : mandatory === 'mandatory';
    filter.has_refs = hasRefs === 'any' ? null : hasRefs === 'yes';
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
    class="flex max-h-[88vh] w-[42rem] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 text-sm text-fg shadow-xl"
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
        <!-- Search, always available -->
        <input
          type="search"
          placeholder="Search name, description, tags, slug, body…"
          bind:value={search}
          class="mb-3 w-full rounded-md border border-line bg-surface-2 px-3 py-1.5 text-fg"
        />

        <!-- Advanced filter panel (collapses from the bottom toggle) -->
        {#if advanced}
          <div class="space-y-3 rounded-md border border-line p-3">
            <div>
              <span class="text-fg-subtle">Kinds</span>
              <div class="mt-1 flex flex-wrap gap-1.5">
                {#each KINDS as k (k)}
                  <button
                    type="button"
                    class="rounded-md border px-2 py-0.5 {triClass(kindState[k])}"
                    onclick={() => (kindState = cycle(kindState, k))}
                  >
                    {triGlyph(kindState[k])}{k}
                  </button>
                {/each}
              </div>
            </div>

            <div>
              <span class="text-fg-subtle">Tags</span>
              <div class="mt-1 flex flex-wrap gap-1.5">
                {#each Object.keys(tagState) as t (t)}
                  <button
                    type="button"
                    class="rounded-md border px-2 py-0.5 {triClass(tagState[t])}"
                    onclick={() => (tagState = cycle(tagState, t))}
                    title="Click to cycle include / exclude / remove"
                  >
                    {triGlyph(tagState[t])}{t}
                  </button>
                {/each}
              </div>
              <div class="relative mt-1">
                <input
                  type="text"
                  placeholder="Add a tag…"
                  bind:value={tagInput}
                  onkeydown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault();
                      addTag(tagInput);
                    }
                  }}
                  class="w-full rounded-md border border-line bg-surface-2 px-2 py-1 text-fg"
                />
                {#if tagSuggestions.length > 0}
                  <ul
                    class="absolute z-10 mt-0.5 max-h-40 w-full overflow-y-auto rounded-md border border-line bg-surface-1 shadow-lg"
                  >
                    {#each tagSuggestions as t (t)}
                      <li>
                        <button
                          type="button"
                          class="block w-full px-2 py-1 text-left text-fg-muted hover:bg-surface-2"
                          onclick={() => addTag(t)}>{t}</button
                        >
                      </li>
                    {/each}
                  </ul>
                {/if}
              </div>
            </div>

            <div class="flex flex-wrap items-center gap-4">
              <label class="flex items-center gap-2 text-fg-muted">
                <span>Mandatory</span>
                <select
                  bind:value={mandatory}
                  class="rounded-md border border-line bg-surface-2 px-2 py-1 text-fg"
                >
                  <option value="any">Any</option>
                  <option value="mandatory">Only</option>
                  <option value="non-mandatory">Exclude</option>
                </select>
              </label>
              <label class="flex items-center gap-2 text-fg-muted">
                <span>References</span>
                <select
                  bind:value={hasRefs}
                  class="rounded-md border border-line bg-surface-2 px-2 py-1 text-fg"
                >
                  <option value="any">Any</option>
                  <option value="yes">Has refs</option>
                  <option value="no">No refs</option>
                </select>
              </label>
            </div>
          </div>
        {/if}

        <!-- Collapse toggle: centered, at the bottom of the filter -->
        <div class="mb-3 flex justify-center">
          <button
            type="button"
            class="flex items-center gap-1 rounded-md px-2 py-0.5 text-fg-subtle hover:bg-surface-2 hover:text-fg"
            onclick={() => {
              advanced = !advanced;
              if (advanced) void ensureExportTags();
            }}
            aria-expanded={advanced}
          >
            {#if advanced}
              <ChevronUp size={14} /> Hide filters
            {:else}
              <ChevronDown size={14} /> Advanced filters
            {/if}
          </button>
        </div>

        <!-- Group scope filter -->
        <div class="mb-1 flex items-center gap-2">
          <span class="text-fg-muted">Groups</span>
          <span class="ml-2 flex gap-1">
            {#each SCOPES as s (s.id)}
              <button
                type="button"
                class="rounded-md border px-2 py-0.5 text-xs {visibleScopes.includes(s.id)
                  ? 'border-sky-600 bg-sky-950/40 text-sky-300'
                  : 'border-line text-fg-subtle hover:bg-surface-2'}"
                onclick={() => (visibleScopes = toggle(visibleScopes, s.id))}>{s.label}</button
              >
            {/each}
          </span>
        </div>

        <!-- Group table, grouped by scope, with a header global checkbox -->
        <table class="mb-3 w-full table-fixed border-collapse overflow-hidden rounded-md border border-line">
          <thead>
            <tr class="bg-surface-2 text-left text-fg-muted">
              <th class="w-8 px-2 py-1">
                <input
                  type="checkbox"
                  checked={allSelected}
                  use:indeterminate={someSelected}
                  onchange={toggleAll}
                  aria-label="Select all visible groups"
                />
              </th>
              <th class="px-2 py-1 font-normal">Group</th>
              {#if advanced}<th class="w-20 px-2 py-1"></th>{/if}
            </tr>
          </thead>
          <tbody>
            {#each SCOPES as s (s.id)}
              {@const scopeGroups = visibleGroups.filter((g) => g.scope === s.id)}
              {#if scopeGroups.length > 0}
                <tr class="border-t border-line bg-surface-1">
                  <td colspan="3" class="px-2 py-0.5 text-xs uppercase tracking-wide text-fg-subtle"
                    >{s.label}</td
                  >
                </tr>
                {#each scopeGroups as g (g.id)}
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
                {/each}
              {/if}
            {/each}
            {#if visibleGroups.length === 0}
              <tr><td colspan="3" class="px-2 py-2 text-fg-subtle">
                {busy ? 'Loading…' : 'No groups in the selected scopes.'}
              </td></tr>
            {/if}
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
