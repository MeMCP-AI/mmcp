<script lang="ts">
  import KindBadge from './KindBadge.svelte';
  import { CheckSquare, LoaderCircle, Pin, Search, Square, X } from 'lucide-svelte';
  import type { KindStr, MemoryFile } from '$lib/types';
  import type { KindDisplay } from '$lib/stores/settings.svelte';

  interface Props {
    slugs: string[] | undefined;
    groupSelected: boolean;
    loading: boolean;
    selectedSlug: string | null;
    kindDisplay: KindDisplay;
    filter: string;
    multi: Set<string>;
    bodyFor: (slug: string) => MemoryFile | undefined;
    onSelect: (slug: string) => void;
    onFilterChange: (q: string) => void;
    onToggleMulti: (slug: string) => void;
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
    multi,
    bodyFor,
    onSelect,
    onFilterChange,
    onToggleMulti,
    onSelectAll,
    onClearMulti
  }: Props = $props();

  // Case-insensitive contains match over slug + name + tags so
  // typing "rule" finds both a slug named `rule-*` and a memory
  // whose frontmatter tags that term explicitly.
  function matches(slug: string, q: string): boolean {
    if (!q) return true;
    const needle = q.toLowerCase();
    if (slug.toLowerCase().includes(needle)) return true;
    const body = bodyFor(slug);
    if (!body) return false;
    const name = body.frontmatter.name?.toLowerCase() ?? '';
    if (name.includes(needle)) return true;
    const tags = body.frontmatter.tags ?? [];
    return tags.some((t) => t.toLowerCase().includes(needle));
  }

  const filtered = $derived.by(() => {
    if (!slugs) return undefined;
    if (!filter.trim()) return slugs;
    return slugs.filter((s) => matches(s, filter.trim()));
  });
</script>

<aside
  class="flex h-full min-h-0 flex-col overflow-hidden border-r border-zinc-800 bg-zinc-900/40"
>
  <div
    class="flex h-9 shrink-0 items-center gap-2 border-b border-zinc-900 px-3 text-xs font-semibold uppercase tracking-wide text-zinc-400"
  >
    <span>Memories</span>
    {#if slugs}
      <span class="text-[10px] font-normal normal-case text-zinc-500">
        {#if filter.trim() && filtered}
          {filtered.length} / {slugs.length}
        {:else}
          {slugs.length}
        {/if}
      </span>
    {/if}
    {#if multi.size > 0}
      <span
        class="ml-auto inline-flex items-center gap-1 rounded-full bg-sky-500/15 px-1.5 py-0.5 text-[10px] font-semibold normal-case text-sky-200 ring-1 ring-inset ring-sky-500/40"
      >
        {multi.size} selected
        <button
          type="button"
          class="rounded-sm p-0.5 text-sky-200 hover:bg-sky-500/30"
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
    <div class="flex shrink-0 items-center gap-1 border-b border-zinc-900 bg-zinc-950/40 px-2 py-1.5">
      <div class="relative flex-1">
        <Search
          size={12}
          class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-zinc-500"
        />
        <input
          type="text"
          class="w-full rounded-md border border-zinc-800 bg-zinc-950 py-1 pl-7 pr-7 text-xs text-zinc-100 placeholder:text-zinc-600 focus:border-zinc-600 focus:outline-none"
          placeholder="Filter by slug, name, tag…"
          value={filter}
          oninput={(e) => onFilterChange((e.currentTarget as HTMLInputElement).value)}
        />
        {#if filter}
          <button
            type="button"
            class="absolute right-1 top-1/2 -translate-y-1/2 rounded-sm p-0.5 text-zinc-500 hover:bg-zinc-800 hover:text-zinc-200"
            onclick={() => onFilterChange('')}
            aria-label="Clear filter"
            title="Clear filter"
          >
            <X size={12} />
          </button>
        {/if}
      </div>
      {#if filtered && filtered.length > 0}
        {@const allChecked = filtered.every((s) => multi.has(s))}
        <button
          type="button"
          class="rounded-md p-1 text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"
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
  {/if}

  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if !groupSelected}
      <div class="px-3 py-2 text-xs text-zinc-500">Select a group on the left.</div>
    {:else if loading || slugs === undefined}
      <div class="flex items-center gap-2 px-3 py-2 text-xs text-zinc-500">
        <LoaderCircle size={12} class="animate-spin" />
        Loading memories…
      </div>
    {:else if slugs.length === 0}
      <div class="px-3 py-2 text-xs text-zinc-500">No memories in this group.</div>
    {:else if filtered && filtered.length === 0}
      <div class="px-3 py-2 text-xs text-zinc-500">
        No matches for <code class="rounded bg-zinc-800 px-1 py-0.5 text-zinc-300">{filter}</code>.
      </div>
    {:else if filtered}
      <ul class="flex flex-col">
        {#each filtered as slug (slug)}
          {@const selected = selectedSlug === slug}
          {@const checked = multi.has(slug)}
          {@const body = bodyFor(slug)}
          {@const kind = body?.frontmatter.kind as KindStr | undefined}
          {@const mandatory = body?.frontmatter.mandatory === true}
          <li class="flex items-stretch">
            <button
              type="button"
              class="flex shrink-0 items-center justify-center px-2 text-zinc-500 hover:text-zinc-200
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
              class="flex flex-1 items-center gap-2 py-1.5 pr-3 text-left text-sm transition-colors
                {selected
                ? 'bg-sky-500/15 text-sky-100'
                : 'text-zinc-200 hover:bg-zinc-800/70'}"
              onclick={(e) => {
                if (e.ctrlKey || e.metaKey) {
                  onToggleMulti(slug);
                } else {
                  onSelect(slug);
                }
              }}
            >
              {#if kind && kindDisplay !== 'off'}
                <KindBadge {kind} mode={kindDisplay} />
              {/if}
              <span class="truncate">{slug}</span>
              {#if mandatory}
                <Pin
                  size={10}
                  class="ml-auto shrink-0 text-amber-400"
                  aria-label="Mandatory memory"
                />
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</aside>
