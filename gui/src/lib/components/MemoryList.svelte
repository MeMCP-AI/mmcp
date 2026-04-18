<script lang="ts">
  import KindBadge from './KindBadge.svelte';
  import { LoaderCircle } from 'lucide-svelte';
  import type { KindStr, MemoryFile } from '$lib/types';
  import type { KindDisplay } from '$lib/stores/settings.svelte';

  interface Props {
    slugs: string[] | undefined;
    groupSelected: boolean;
    loading: boolean;
    selectedSlug: string | null;
    kindDisplay: KindDisplay;
    bodyFor: (slug: string) => MemoryFile | undefined;
    onSelect: (slug: string) => void;
  }

  let {
    slugs,
    groupSelected,
    loading,
    selectedSlug,
    kindDisplay,
    bodyFor,
    onSelect
  }: Props = $props();
</script>

<aside class="flex h-full flex-col border-r border-zinc-800 bg-zinc-900/40">
  <div class="flex h-9 shrink-0 items-center px-3 text-xs font-semibold uppercase tracking-wide text-zinc-400">
    Memories
  </div>
  <div class="flex-1 overflow-y-auto">
    {#if !groupSelected}
      <div class="px-3 py-2 text-xs text-zinc-500">Select a group on the left.</div>
    {:else if loading || slugs === undefined}
      <div class="flex items-center gap-2 px-3 py-2 text-xs text-zinc-500">
        <LoaderCircle size={12} class="animate-spin" />
        Loading memories…
      </div>
    {:else if slugs.length === 0}
      <div class="px-3 py-2 text-xs text-zinc-500">No memories in this group.</div>
    {:else}
      <ul class="flex flex-col">
        {#each slugs as slug (slug)}
          {@const selected = selectedSlug === slug}
          {@const body = bodyFor(slug)}
          {@const kind = body?.frontmatter.kind as KindStr | undefined}
          <li>
            <button
              type="button"
              class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors
                {selected
                ? 'bg-sky-500/15 text-sky-100'
                : 'text-zinc-200 hover:bg-zinc-800/70'}"
              onclick={() => onSelect(slug)}
            >
              {#if kind && kindDisplay !== 'off'}
                <KindBadge {kind} mode={kindDisplay} />
              {/if}
              <span class="truncate">{slug}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</aside>
