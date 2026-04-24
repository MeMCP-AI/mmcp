<script lang="ts">
  // Classic-variant action bar. Carries memory-level actions
  // (new / edit / delete / history), sync push-pull, and the
  // classic-only columns/stacked layout toggle. App-shell chrome
  // (theme / settings / variant switcher / diagnose) lives in
  // CommonNavbar and CommonFooter so it's reachable from every
  // variant uniformly.

  import {
    CloudDownload,
    CloudUpload,
    Columns2,
    History as HistoryIcon,
    Pencil,
    Plus,
    Rows2,
    Trash2
  } from 'lucide-svelte';

  export type LayoutMode = 'columns' | 'stacked';

  interface Props {
    canCreate: boolean;
    canEdit: boolean;
    canDelete: boolean;
    canViewHistory: boolean;
    syncReady: boolean;
    layout: LayoutMode;
    onNew: () => void;
    onEdit: () => void;
    onDelete: () => void;
    onHistory: () => void;
    onPull: () => void;
    onPush: () => void;
    onToggleLayout: () => void;
  }

  let {
    canCreate,
    canEdit,
    canDelete,
    canViewHistory,
    syncReady,
    layout,
    onNew,
    onEdit,
    onDelete,
    onHistory,
    onPull,
    onPush,
    onToggleLayout
  }: Props = $props();

  const btn =
    'inline-flex items-center gap-1.5 rounded-md px-2 py-1.5 text-sm text-fg ' +
    'hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent ' +
    'transition-colors sm:px-2.5';
  const sep = 'h-5 w-px bg-surface-2 mx-1 sm:mx-2';
</script>

<header
  class="flex h-11 shrink-0 items-center gap-0.5 overflow-x-auto border-b border-line bg-surface-1 px-2 sm:gap-1 sm:px-3"
>
  <button class={btn} disabled={!canCreate} onclick={onNew} aria-label="New" title="New">
    <Plus size={14} />
    <span class="hidden sm:inline">New</span>
  </button>
  <button class={btn} disabled={!canEdit} onclick={onEdit} aria-label="Edit" title="Edit">
    <Pencil size={14} />
    <span class="hidden sm:inline">Edit</span>
  </button>
  <button class={btn} disabled={!canDelete} onclick={onDelete} aria-label="Delete" title="Delete">
    <Trash2 size={14} />
    <span class="hidden sm:inline">Delete</span>
  </button>
  <button
    class={btn}
    disabled={!canViewHistory}
    onclick={onHistory}
    aria-label="History"
    title="Show memory history"
  >
    <HistoryIcon size={14} />
    <span class="hidden sm:inline">History</span>
  </button>

  <span class={sep}></span>

  <button class={btn} disabled={!syncReady} onclick={onPull} aria-label="Pull" title="Pull">
    <CloudDownload size={14} />
    <span class="hidden sm:inline">Pull</span>
  </button>
  <button class={btn} disabled={!syncReady} onclick={onPush} aria-label="Push" title="Push">
    <CloudUpload size={14} />
    <span class="hidden sm:inline">Push</span>
  </button>

  <span class="ml-auto"></span>

  <button
    class={btn}
    onclick={onToggleLayout}
    aria-label={layout === 'columns' ? 'Stack groups and memories' : 'Side-by-side layout'}
    title={layout === 'columns'
      ? 'Stack groups and memories vertically'
      : 'Split groups and memories side-by-side'}
  >
    {#if layout === 'columns'}
      <Rows2 size={14} />
    {:else}
      <Columns2 size={14} />
    {/if}
    <span class="hidden sm:inline">Layout</span>
  </button>
</header>
