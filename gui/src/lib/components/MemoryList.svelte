<script lang="ts">
  import FeatureBadge from './FeatureBadge.svelte';
  import KindBadge from './KindBadge.svelte';
  import {
    CheckSquare,
    Filter as FilterIcon,
    LoaderCircle,
    Pin,
    Search,
    Square,
    X
  } from 'lucide-svelte';
  import type { KindStr, MemoryFile } from '$lib/types';
  import type { KindDisplay } from '$lib/stores/settings.svelte';

  interface Props {
    slugs: string[] | undefined;
    groupSelected: boolean;
    loading: boolean;
    selectedSlug: string | null;
    kindDisplay: KindDisplay;
    filter: string;
    kindFilter: Set<KindStr>;
    mandatoryOnly: boolean;
    multi: Set<string>;
    bodyFor: (slug: string) => MemoryFile | undefined;
    onSelect: (slug: string) => void;
    onFilterChange: (q: string) => void;
    onToggleMulti: (slug: string) => void;
    onExtendMulti: (slug: string, visible: string[]) => void;
    onToggleKind: (kind: KindStr) => void;
    onToggleMandatoryOnly: () => void;
    onClearFilters: () => void;
    onSelectAll: (slugs: string[]) => void;
    onClearMulti: () => void;
  }

  let {
    slugs,
    groupSelected,
    loading,
    selectedSlug,
    kindDisplay,
    filter,
    kindFilter,
    mandatoryOnly,
    multi,
    bodyFor,
    onSelect,
    onFilterChange,
    onToggleMulti,
    onExtendMulti,
    onToggleKind,
    onToggleMandatoryOnly,
    onClearFilters,
    onSelectAll,
    onClearMulti
  }: Props = $props();

  const KINDS: KindStr[] = ['rule', 'snapshot', 'log', 'reference', 'scratch', 'feature'];

  // Show the facet panel collapsed by default; flips open when any
  // facet is set so returning users see what's filtering them.
  let facetsOpen = $state(false);
  $effect(() => {
    if (kindFilter.size > 0 || mandatoryOnly) facetsOpen = true;
  });

  const filtersActive = $derived(
    filter.trim().length > 0 || kindFilter.size > 0 || mandatoryOnly
  );

  function matches(slug: string, q: string): boolean {
    const body = bodyFor(slug);
    if (mandatoryOnly && body?.frontmatter.mandatory !== true) return false;
    if (kindFilter.size > 0) {
      const kind = body?.frontmatter.kind as KindStr | undefined;
      if (!kind || !kindFilter.has(kind)) return false;
    }
    if (!q) return true;
    const needle = q.toLowerCase();
    if (slug.toLowerCase().includes(needle)) return true;
    if (!body) return false;
    const name = body.frontmatter.name?.toLowerCase() ?? '';
    if (name.includes(needle)) return true;
    const tags = body.frontmatter.tags ?? [];
    return tags.some((t) => t.toLowerCase().includes(needle));
  }

  const filtered = $derived.by(() => {
    if (!slugs) return undefined;
    const q = filter.trim();
    return slugs.filter((s) => matches(s, q));
  });

  // Reserve consistent column widths so the slug column starts at
  // the same x regardless of whether a row has its body loaded
  // (kind badge absent) or is mandatory (pin absent). When the
  // kind display is off we drop the kind column entirely.
  const kindSlotClass = $derived(
    kindDisplay === 'off'
      ? 'hidden'
      : kindDisplay === 'icon'
        ? 'w-6'
        : 'w-11'
  );

  function handleRowClick(e: MouseEvent, slug: string, visible: string[]) {
    if (e.shiftKey) {
      onExtendMulti(slug, visible);
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      onToggleMulti(slug);
      return;
    }
    onSelect(slug);
  }
</script>

<aside
  class="flex h-full min-h-0 flex-col overflow-hidden border-r border-line bg-surface-1/40"
>
  <div
    class="flex h-9 shrink-0 items-center gap-2 border-b border-line px-3 text-xs font-semibold uppercase tracking-wide text-fg-muted"
  >
    <span>Memories</span>
    {#if slugs}
      <span class="text-[10px] font-normal normal-case text-fg-subtle">
        {#if filtersActive && filtered}
          {filtered.length} / {slugs.length}
        {:else}
          {slugs.length}
        {/if}
      </span>
    {/if}
    {#if multi.size > 0}
      <span
        class="ml-auto inline-flex items-center gap-1 rounded-full bg-sky-500/15 px-1.5 py-0.5 text-[10px] font-semibold normal-case text-selected-fg ring-1 ring-inset ring-sky-500/40"
      >
        {multi.size} selected
        <button
          type="button"
          class="rounded-sm p-0.5 text-selected-fg hover:bg-sky-500/30"
          onclick={onClearMulti}
          aria-label="Clear selection"
          title="Clear selection"
        >
          <X size={10} />
        </button>
      </span>
    {/if}
  </div>

  {#if groupSelected && slugs && slugs.length > 0}
    <div class="flex shrink-0 flex-col gap-1.5 border-b border-line bg-surface-0/40 px-2 py-1.5">
      <div class="flex items-center gap-1">
        <div class="relative flex-1">
          <Search
            size={12}
            class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-fg-subtle"
          />
          <input
            type="text"
            class="w-full rounded-md border border-line bg-surface-0 py-1 pl-7 pr-7 text-xs text-fg placeholder:text-fg-subtle focus:border-line-strong focus:outline-none"
            placeholder="Filter by slug, name, tag…"
            value={filter}
            oninput={(e) => onFilterChange((e.currentTarget as HTMLInputElement).value)}
          />
          {#if filter}
            <button
              type="button"
              class="absolute right-1 top-1/2 -translate-y-1/2 rounded-sm p-0.5 text-fg-subtle hover:bg-surface-2 hover:text-fg"
              onclick={() => onFilterChange('')}
              aria-label="Clear filter"
              title="Clear filter"
            >
              <X size={12} />
            </button>
          {/if}
        </div>
        <button
          type="button"
          class="relative rounded-md p-1 transition-colors
            {facetsOpen || kindFilter.size > 0 || mandatoryOnly
            ? 'bg-sky-500/15 text-selected-fg'
            : 'text-fg-muted hover:bg-surface-2 hover:text-fg'}"
          onclick={() => (facetsOpen = !facetsOpen)}
          aria-label="Toggle facet filters"
          aria-expanded={facetsOpen}
          title="More filters"
        >
          <FilterIcon size={13} />
          {#if kindFilter.size > 0 || mandatoryOnly}
            <span
              class="absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-sky-400 ring-2 ring-surface-0"
            ></span>
          {/if}
        </button>
        {#if filtered && filtered.length > 0}
          {@const allChecked = filtered.every((s) => multi.has(s))}
          <button
            type="button"
            class="rounded-md p-1 text-fg-muted hover:bg-surface-2 hover:text-fg"
            onclick={() => (allChecked ? onClearMulti() : onSelectAll(filtered))}
            aria-label={allChecked ? 'Clear all' : 'Select all'}
            title={allChecked ? 'Clear all visible' : 'Select all visible'}
          >
            {#if allChecked}
              <CheckSquare size={13} />
            {:else}
              <Square size={13} />
            {/if}
          </button>
        {/if}
      </div>

      {#if facetsOpen}
        <div class="flex flex-col gap-1.5">
          <div class="flex flex-wrap items-center gap-1">
            <span class="text-[10px] uppercase tracking-wide text-fg-subtle">Kinds</span>
            {#each KINDS as k (k)}
              {@const active = kindFilter.has(k)}
              <button
                type="button"
                class="rounded-md transition-opacity {active ? '' : 'opacity-50 hover:opacity-100'}"
                onclick={() => onToggleKind(k)}
                aria-pressed={active}
                title={active ? `Don't filter on ${k}` : `Filter on ${k}`}
              >
                <KindBadge kind={k} mode="icon_and_text" />
              </button>
            {/each}
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <label
              class="inline-flex cursor-pointer items-center gap-1.5 rounded-md px-1.5 py-0.5 text-[11px] text-fg-muted ring-1 ring-inset
                {mandatoryOnly
                ? 'bg-amber-500/15 text-amber-200 ring-amber-500/40'
                : 'ring-line-strong hover:bg-surface-2'}"
            >
              <input
                type="checkbox"
                class="sr-only"
                checked={mandatoryOnly}
                onchange={onToggleMandatoryOnly}
              />
              <Pin size={10} class={mandatoryOnly ? 'text-amber-300' : 'text-fg-subtle'} />
              Mandatory only
            </label>
            {#if filtersActive}
              <button
                type="button"
                class="ml-auto text-[11px] text-fg-subtle hover:text-fg"
                onclick={onClearFilters}
                title="Clear all filters"
              >
                Clear filters
              </button>
            {/if}
          </div>
        </div>
      {/if}
    </div>
  {/if}

  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if !groupSelected}
      <div class="px-3 py-2 text-xs text-fg-subtle">Select a group on the left.</div>
    {:else if loading || slugs === undefined}
      <div class="flex items-center gap-2 px-3 py-2 text-xs text-fg-subtle">
        <LoaderCircle size={12} class="animate-spin" />
        Loading memories…
      </div>
    {:else if slugs.length === 0}
      <div class="px-3 py-2 text-xs text-fg-subtle">No memories in this group.</div>
    {:else if filtered && filtered.length === 0}
      <div class="px-3 py-2 text-xs text-fg-subtle">No memories match the current filters.</div>
    {:else if filtered}
      <ul class="flex flex-col">
        {#each filtered as slug (slug)}
          {@const selected = selectedSlug === slug}
          {@const checked = multi.has(slug)}
          {@const body = bodyFor(slug)}
          {@const kind = body?.frontmatter.kind as KindStr | undefined}
          {@const mandatory = body?.frontmatter.mandatory === true}
          {@const feature = body?.frontmatter.feature ?? null}
          <li class="flex items-stretch">
            <button
              type="button"
              class="flex w-7 shrink-0 items-center justify-center text-fg-subtle hover:text-fg
                {checked ? 'text-sky-300' : ''}"
              onclick={() => onToggleMulti(slug)}
              aria-label={checked ? `Deselect ${slug}` : `Select ${slug}`}
              title={checked ? 'Deselect' : 'Select'}
            >
              {#if checked}
                <CheckSquare size={13} />
              {:else}
                <Square size={13} />
              {/if}
            </button>
            <button
              type="button"
              class="flex min-w-0 flex-1 items-center gap-2 py-1.5 pr-2 text-left text-sm transition-colors
                {selected
                ? 'bg-sky-500/15 text-selected-fg'
                : 'text-fg hover:bg-surface-2/70'}"
              onclick={(e) => handleRowClick(e, slug, filtered ?? [])}
            >
              {#if kindDisplay !== 'off'}
                <span class="flex shrink-0 items-center justify-start {kindSlotClass}">
                  {#if kind}
                    <KindBadge {kind} mode={kindDisplay} />
                  {/if}
                </span>
              {/if}
              <span
                class="min-w-0 flex-1 truncate"
                title={body?.frontmatter.name
                  ? `${slug} — ${body.frontmatter.name}`
                  : slug}
              >
                {slug}
              </span>
              {#if feature}
                <span class="shrink-0">
                  <FeatureBadge
                    status={feature.status}
                    number={feature.number}
                    label={false}
                  />
                </span>
              {/if}
              <span class="flex w-4 shrink-0 items-center justify-center">
                {#if mandatory}
                  <Pin size={10} class="text-amber-400" aria-label="Mandatory memory" />
                {/if}
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</aside>
