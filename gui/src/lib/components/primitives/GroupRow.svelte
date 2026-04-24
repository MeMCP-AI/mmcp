<script lang="ts">
  // Group listing row — scope icon + name + slug + memory count +
  // pin toggle. Used by Hub home's pinned list and Hub scope's
  // filterable list; consolidating keeps the two in visual sync.

  import { Star } from 'lucide-svelte';
  import ScopeIcon from './ScopeIcon.svelte';
  import { SCOPE_META } from '$lib/utils/scope';
  import type { GroupEntry } from '$lib/types';

  interface Props {
    group: GroupEntry;
    memoryCount?: number | null;
    pinned?: boolean;
    showPinToggle?: boolean;
    /** Shown in smaller text below the name. Defaults to
     * `scope · slug` when the display name differs from the
     * slug; otherwise omitted. */
    subtitle?: string | null;
    onOpen: () => void;
    onTogglePin?: () => void;
  }

  let {
    group,
    memoryCount = null,
    pinned = false,
    showPinToggle = false,
    subtitle,
    onOpen,
    onTogglePin
  }: Props = $props();

  const meta = $derived(SCOPE_META[group.scope]);
  const fallbackSubtitle = $derived(
    group.display_name && group.display_name !== group.slug
      ? `${meta.label} · ${group.slug}`
      : meta.label
  );
</script>

<div
  class="flex items-center gap-2 rounded-lg border border-line bg-surface-1 px-3 py-2 hover:border-line-strong"
>
  <button
    type="button"
    class="flex min-w-0 flex-1 items-center gap-3 text-left"
    onclick={onOpen}
    title={group.slug}
  >
    <ScopeIcon scope={group.scope} extraClass="shrink-0 text-fg-muted" />
    <div class="min-w-0 flex-1">
      <div class="truncate text-sm text-fg">
        {group.display_name ?? group.slug}
      </div>
      <div class="truncate text-[11px] text-fg-subtle">
        {subtitle ?? fallbackSubtitle}
      </div>
    </div>
    {#if memoryCount !== null}
      <span class="shrink-0 text-[11px] text-fg-subtle">
        {memoryCount} memories
      </span>
    {/if}
  </button>
  {#if showPinToggle && onTogglePin}
    <button
      type="button"
      class="rounded-md p-1 text-fg-muted hover:bg-surface-2 hover:text-amber-300"
      title={pinned ? 'Unpin from home' : 'Pin on home'}
      aria-label={pinned ? 'Unpin group' : 'Pin group'}
      onclick={onTogglePin}
    >
      {#if pinned}
        <Star size={13} class="fill-amber-300 text-amber-300" />
      {:else}
        <Star size={13} />
      {/if}
    </button>
  {/if}
</div>
