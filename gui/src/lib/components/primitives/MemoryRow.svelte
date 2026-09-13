<script lang="ts">
  // Compact row for listing a memory inside a group / search hit.
  // Every listing-style layout that isn't the feed uses this shape:
  // kind icon + slug + name + feature badge + mandatory pin. Kept
  // narrow enough to fit in tree panels and dropdowns alike.

  import FeatureBadge from '../FeatureBadge.svelte';
  import KindBadge from '../KindBadge.svelte';
  import MandatoryPill from './MandatoryPill.svelte';
  import type { MemoryFrontmatter } from '$lib/types';

  interface Props {
    slug: string;
    /** Frontmatter-only descriptor: carries no memory body text (see `MemoryDescriptor`/`MemoryFile`). */
    descriptor: { frontmatter: MemoryFrontmatter } | undefined;
    /** Sub-label under the slug, typically `scope · group`. */
    subtitle?: string | null;
    active?: boolean;
    onSelect: () => void;
    /** Forwarded onto the row's own mousedown, for a caller guarding against a blur-triggered close. */
    onMouseDown?: (e: MouseEvent) => void;
  }

  let {
    slug,
    descriptor,
    subtitle = null,
    active = false,
    onSelect,
    onMouseDown
  }: Props = $props();

  const fm = $derived(descriptor?.frontmatter);
</script>

<button
  type="button"
  class="flex w-full items-center gap-2 rounded-md border border-line bg-surface-1 px-3 py-2 text-left transition-colors hover:border-line-strong hover:bg-surface-2
    {active ? 'border-sky-500/40 bg-sky-500/10 text-selected-fg' : 'text-fg'}"
  onclick={onSelect}
  onmousedown={onMouseDown}
  title={fm?.name ?? slug}
>
  {#if fm?.kind}
    <KindBadge kind={fm.kind} mode="icon" />
  {/if}
  <div class="min-w-0 flex-1">
    <div class="truncate text-sm">
      {fm?.name ?? slug}
    </div>
    <div class="truncate text-[11px] text-fg-subtle">
      {subtitle ?? slug}
    </div>
  </div>
  {#if fm?.feature}
    <FeatureBadge status={fm.feature.status} number={fm.feature.number} label={false} />
  {/if}
  {#if fm?.mandatory}
    <MandatoryPill label={false} size={11} />
  {/if}
</button>
