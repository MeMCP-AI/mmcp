<script lang="ts">
  import {
    CloudDownload,
    CloudUpload,
    Columns2,
    History as HistoryIcon,
    Pencil,
    Plus,
    Rows2,
    Settings as SettingsIcon,
    Stethoscope,
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
    onDiagnose: () => void;
    onSettings: () => void;
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
    onDiagnose,
    onSettings,
    onToggleLayout
  }: Props = $props();

  // Label is hidden below sm (< 640 px) — only the icon stays visible
  // so the toolbar still fits on a narrow window. Aria-label keeps
  // screen readers happy and `title` gives sighted users a tooltip
  // when they hover the bare icon.
  const btn =
    'inline-flex items-center gap-1.5 rounded-md px-2 py-1.5 text-sm text-zinc-200 ' +
    'hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent ' +
    'transition-colors sm:px-2.5';
  const sep = 'h-5 w-px bg-zinc-800 mx-1 sm:mx-2';
</script>

<header
  class="flex h-11 shrink-0 items-center gap-0.5 overflow-x-auto border-b border-zinc-800 bg-zinc-900 px-2 sm:gap-1 sm:px-3"
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

  <span class={sep}></span>

  <button class={btn} onclick={onDiagnose} aria-label="Diagnose" title="Diagnose">
    <Stethoscope size={14} />
    <span class="hidden sm:inline">Diagnose</span>
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

  <button class={btn} onclick={onSettings} aria-label="Settings" title="Settings">
    <SettingsIcon size={14} />
    <span class="hidden sm:inline">Settings</span>
  </button>
</header>
