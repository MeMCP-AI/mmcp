<script lang="ts">
  import KindBadge from './KindBadge.svelte';
  import { marked } from 'marked';
  import { RefreshCw, X } from 'lucide-svelte';
  import type { KindStr, MemoryFile } from '$lib/types';
  import type { KindDisplay } from '$lib/stores/settings.svelte';

  interface Props {
    memory: MemoryFile | undefined;
    slug: string | null;
    loading: boolean;
    kindDisplay: KindDisplay;
    /** Fresh version of the same memory sitting in the wings — a
     * background refresh detected it differs from the on-screen
     * copy and stashed it rather than clobbering the user's read.
     * The viewer shows a banner and lets the user opt in. */
    pending?: MemoryFile | null;
    onAcceptPending?: () => void;
    onDismissPending?: () => void;
  }

  let {
    memory,
    slug,
    loading,
    kindDisplay,
    pending = null,
    onAcceptPending,
    onDismissPending
  }: Props = $props();

  marked.setOptions({ breaks: false, gfm: true });
  const html = $derived.by(() => {
    if (!memory) return '';
    return marked.parse(memory.body) as string;
  });
</script>

<section class="flex h-full min-h-0 flex-col overflow-hidden bg-surface-0">
  {#if !slug}
    <div class="m-auto flex flex-col items-center gap-2 text-sm text-fg-subtle">
      <span>Select a memory to view its body.</span>
    </div>
  {:else if loading || !memory}
    <div class="m-auto flex items-center gap-2 text-sm text-fg-subtle">Loading memory…</div>
  {:else}
    {@const fm = memory.frontmatter}
    {#if pending}
      <!-- Pending-version banner. Pins above the article so the
           reader's scroll isn't disturbed; click to adopt the fresh
           copy, X to keep reading the current one. -->
      <div
        class="flex shrink-0 items-center gap-2 border-b border-sky-500/40 bg-sky-500/10 px-3 py-1.5 text-xs text-sky-100"
      >
        <RefreshCw size={12} class="text-sky-300" />
        <span>A newer version of this memory is available.</span>
        <button
          type="button"
          class="ml-auto inline-flex items-center gap-1 rounded-md bg-sky-500/30 px-2 py-0.5 font-medium text-sky-50 hover:bg-sky-500/50"
          onclick={() => onAcceptPending?.()}
        >
          View new version
        </button>
        <button
          type="button"
          class="rounded-md p-1 text-sky-300 hover:bg-sky-500/20 hover:text-sky-100"
          aria-label="Dismiss"
          title="Keep current version"
          onclick={() => onDismissPending?.()}
        >
          <X size={11} />
        </button>
      </div>
    {/if}
    <article class="min-h-0 flex-1 overflow-y-auto">
      <div class="mx-auto max-w-3xl px-4 py-5 sm:px-6 sm:py-6">
        <!-- frontmatter card -->
        <div class="rounded-lg border border-line bg-surface-1 p-4 sm:p-5">
          <h1 class="text-lg font-semibold text-fg" title={fm.name}>{fm.name}</h1>
          <div class="mt-0.5 text-xs text-fg-subtle" title={slug}>{slug}</div>
          <p class="mt-3 text-sm text-fg-muted" title={fm.description}>{fm.description}</p>
          <div class="mt-3 flex flex-wrap items-center gap-1.5">
            <KindBadge kind={fm.kind as KindStr} mode="icon_and_text" />
            {#if fm.mandatory}
              <span
                class="inline-flex items-center rounded-md bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-amber-300 ring-1 ring-inset ring-amber-500/30"
                title="Mandatory memory — always read at session start"
              >
                mandatory
              </span>
            {/if}
            {#if fm.version}
              <span
                class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] font-semibold text-fg-muted"
                title="Schema version"
              >
                v{fm.version}
              </span>
            {/if}
            {#each fm.tags as tag (tag)}
              <span
                class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] text-fg-muted"
                title={tag}
              >
                {tag}
              </span>
            {/each}
          </div>
        </div>

        <!-- body card -->
        <div class="mt-5 rounded-lg border border-line bg-surface-1/40 p-4 sm:p-5">
          <div
            class="prose prose-zinc prose-sm max-w-none prose-pre:bg-surface-0 prose-pre:ring-1 prose-pre:ring-line prose-headings:tracking-tight"
          >
            {@html html}
          </div>
        </div>
      </div>
    </article>
  {/if}
  <!-- Intentional tailwind marker so kindDisplay prop is in scope -->
  <span class="hidden">{kindDisplay}</span>
</section>
