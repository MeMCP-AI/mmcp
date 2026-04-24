<script lang="ts">
  // Canonical memory reader — frontmatter header followed by the
  // rendered markdown body. Used by every variant that opens a
  // memory for reading (Hub /memory, Repo viewer, Feed reader).
  //
  // `slots: { sidebar? }` renders to the right on wide layouts so
  // a caller can drop its related panel inline without this
  // component having to know about refs / backlinks.

  import { marked } from 'marked';
  import FeatureBadge from '../FeatureBadge.svelte';
  import KindBadge from '../KindBadge.svelte';
  import MandatoryPill from './MandatoryPill.svelte';
  import TagChip from './TagChip.svelte';
  import VersionPill from './VersionPill.svelte';
  import type { MemoryFile } from '$lib/types';

  interface Props {
    memory: MemoryFile;
    /** Optional slot-like snippet rendered in a right-side
     * column on wide viewports. When supplied the reader
     * switches to a two-column grid; when omitted the body
     * takes the full width. */
    sidebar?: import('svelte').Snippet;
    /** Max-width wrapper around the article — defaults to
     * `max-w-3xl` matching the pre-refactor look. */
    maxWidthClass?: string;
  }

  let { memory, sidebar, maxWidthClass = 'max-w-3xl' }: Props = $props();

  marked.setOptions({ breaks: false, gfm: true });
  const html = $derived(marked.parse(memory.body) as string);

  const fm = $derived(memory.frontmatter);
</script>

{#if sidebar}
  <div class="mx-auto grid max-w-6xl grid-cols-1 gap-6 p-6 sm:p-8 lg:grid-cols-[1fr_280px]">
    <article>
      <h1 class="text-xl font-semibold text-fg" title={fm.name}>{fm.name}</h1>
      <p class="mt-1 text-sm text-fg-muted">{fm.description}</p>
      <div class="mt-3 flex flex-wrap items-center gap-1.5">
        <KindBadge kind={fm.kind} mode="icon_and_text" />
        {#if fm.feature}
          <FeatureBadge status={fm.feature.status} number={fm.feature.number} />
        {/if}
        {#if fm.mandatory}
          <MandatoryPill />
        {/if}
        {#if fm.version}
          <VersionPill version={fm.version} />
        {/if}
        {#each fm.tags as tag (tag)}
          <TagChip {tag} hash />
        {/each}
      </div>
      <div
        class="prose prose-zinc prose-sm mt-6 max-w-none prose-pre:bg-surface-1 prose-pre:ring-1 prose-pre:ring-line prose-headings:tracking-tight"
      >
        {#if memory.body.trim()}
          {@html html}
        {:else}
          <p class="italic text-fg-subtle">(empty body)</p>
        {/if}
      </div>
    </article>
    <aside class="flex flex-col gap-4 text-sm">
      {@render sidebar()}
    </aside>
  </div>
{:else}
  <article class="mx-auto {maxWidthClass} px-6 py-6">
    <h1 class="text-xl font-semibold text-fg" title={fm.name}>{fm.name}</h1>
    <p class="mt-1 text-sm text-fg-muted">{fm.description}</p>
    <div class="mt-3 flex flex-wrap items-center gap-1.5">
      <KindBadge kind={fm.kind} mode="icon_and_text" />
      {#if fm.feature}
        <FeatureBadge status={fm.feature.status} number={fm.feature.number} />
      {/if}
      {#if fm.mandatory}
        <MandatoryPill />
      {/if}
      {#if fm.version}
        <VersionPill version={fm.version} />
      {/if}
      {#each fm.tags as tag (tag)}
        <TagChip {tag} hash />
      {/each}
    </div>
    <div
      class="prose prose-zinc prose-sm mt-6 max-w-none prose-pre:bg-surface-1 prose-pre:ring-1 prose-pre:ring-line prose-headings:tracking-tight"
    >
      {#if memory.body.trim()}
        {@html html}
      {:else}
        <p class="italic text-fg-subtle">(empty body)</p>
      {/if}
    </div>
  </article>
{/if}
