<script lang="ts">
  // Segmented-pill selector for the UI variant. Shows every option
  // up-front (icon ± label) rather than cycling blind, so the user
  // sees which layouts exist and can jump directly. Lives in the
  // chrome of every variant so switching never requires a trip
  // through Settings.

  import {
    Compass,
    LayoutGrid,
    LayoutPanelLeft,
    Newspaper
  } from 'lucide-svelte';
  import type { UiVariant } from '$lib/stores/settings.svelte';

  interface Props {
    current: UiVariant;
    onSelect: (variant: UiVariant) => void;
    /** `compact` strips the text label to keep narrow toolbars
     * from overflowing. Default keeps labels visible so the
     * affordance is self-describing. */
    compact?: boolean;
  }

  let { current, onSelect, compact = false }: Props = $props();

  const OPTIONS: {
    id: UiVariant;
    label: string;
    Icon: typeof LayoutPanelLeft;
  }[] = [
    { id: 'classic', label: 'Classic', Icon: LayoutPanelLeft },
    { id: 'repo', label: 'Repo', Icon: LayoutGrid },
    { id: 'feed', label: 'Feed', Icon: Newspaper },
    { id: 'hub', label: 'Hub', Icon: Compass }
  ];
</script>

<div
  class="inline-flex items-stretch overflow-hidden rounded-md border border-line bg-surface-0 text-xs"
  role="radiogroup"
  aria-label="Layout variant"
>
  {#each OPTIONS as opt (opt.id)}
    {@const active = current === opt.id}
    <button
      type="button"
      role="radio"
      aria-checked={active}
      class="inline-flex items-center gap-1 px-2 py-1 transition-colors first:rounded-l-md last:rounded-r-md
        {active
        ? 'bg-sky-500/15 text-selected-fg'
        : 'text-fg-muted hover:bg-surface-2 hover:text-fg'}"
      title={`${opt.label} layout`}
      onclick={() => onSelect(opt.id)}
    >
      <opt.Icon size={12} />
      {#if !compact}
        <span>{opt.label}</span>
      {/if}
    </button>
  {/each}
</div>
