<script lang="ts">
  import KindBadge from './KindBadge.svelte';
  import { marked } from 'marked';
  import type { KindStr, MemoryFile } from '$lib/types';
  import type { KindDisplay } from '$lib/stores/settings.svelte';

  interface Props {
    memory: MemoryFile | undefined;
    slug: string | null;
    loading: boolean;
    kindDisplay: KindDisplay;
  }

  let { memory, slug, loading, kindDisplay }: Props = $props();

  marked.setOptions({ breaks: false, gfm: true });
  const html = $derived.by(() => {
    if (!memory) return '';
    return marked.parse(memory.body) as string;
  });
</script>

<section class="flex h-full flex-col bg-zinc-950">
  {#if !slug}
    <div class="m-auto flex flex-col items-center gap-2 text-sm text-zinc-500">
      <span>Select a memory to view its body.</span>
    </div>
  {:else if loading || !memory}
    <div class="m-auto flex items-center gap-2 text-sm text-zinc-500">Loading memory…</div>
  {:else}
    {@const fm = memory.frontmatter}
    <article class="flex-1 overflow-y-auto">
      <div class="mx-auto max-w-3xl px-6 py-6">
        <!-- frontmatter card -->
        <div class="rounded-lg border border-zinc-800 bg-zinc-900 p-5">
          <h1 class="text-lg font-semibold text-zinc-50">{fm.name}</h1>
          <div class="mt-0.5 text-xs text-zinc-500">{slug}</div>
          <p class="mt-3 text-sm text-zinc-300">{fm.description}</p>
          <div class="mt-3 flex flex-wrap items-center gap-1.5">
            <KindBadge kind={fm.kind as KindStr} mode="icon_and_text" />
            {#if fm.mandatory}
              <span class="inline-flex items-center rounded-md bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-amber-300 ring-1 ring-inset ring-amber-500/30">
                mandatory
              </span>
            {/if}
            {#if fm.version}
              <span class="inline-flex items-center rounded-md bg-zinc-800 px-1.5 py-0.5 text-[10px] font-semibold text-zinc-300">
                v{fm.version}
              </span>
            {/if}
            {#each fm.tags as tag (tag)}
              <span class="inline-flex items-center rounded-md bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-300">
                {tag}
              </span>
            {/each}
          </div>
        </div>

        <!-- body card -->
        <div class="mt-5 rounded-lg border border-zinc-800 bg-zinc-900/40 p-5">
          <div class="prose prose-invert prose-zinc max-w-none prose-sm prose-pre:bg-zinc-950 prose-pre:ring-1 prose-pre:ring-zinc-800 prose-headings:tracking-tight">
            {@html html}
          </div>
        </div>
      </div>
    </article>
  {/if}
  <!-- Intentional tailwind marker so kindDisplay prop is in scope -->
  <span class="hidden">{kindDisplay}</span>
</section>
