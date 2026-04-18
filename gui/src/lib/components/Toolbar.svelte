<script lang="ts">
  import {
    CloudDownload,
    CloudUpload,
    Pencil,
    Plus,
    Settings as SettingsIcon,
    Stethoscope,
    Trash2
  } from 'lucide-svelte';

  interface Props {
    canCreate: boolean;
    canEditOrDelete: boolean;
    syncReady: boolean;
    onNew: () => void;
    onEdit: () => void;
    onDelete: () => void;
    onPull: () => void;
    onPush: () => void;
    onDiagnose: () => void;
    onSettings: () => void;
  }

  let {
    canCreate,
    canEditOrDelete,
    syncReady,
    onNew,
    onEdit,
    onDelete,
    onPull,
    onPush,
    onDiagnose,
    onSettings
  }: Props = $props();

  const btn =
    'inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-sm text-zinc-200 ' +
    'hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent ' +
    'transition-colors';
  const sep = 'h-5 w-px bg-zinc-800 mx-2';
</script>

<header
  class="flex h-11 shrink-0 items-center gap-1 border-b border-zinc-800 bg-zinc-900 px-3"
>
  <button class={btn} disabled={!canCreate} onclick={onNew}>
    <Plus size={14} /> New
  </button>
  <button class={btn} disabled={!canEditOrDelete} onclick={onEdit}>
    <Pencil size={14} /> Edit
  </button>
  <button class={btn} disabled={!canEditOrDelete} onclick={onDelete}>
    <Trash2 size={14} /> Delete
  </button>

  <span class={sep}></span>

  <button class={btn} disabled={!syncReady} onclick={onPull}>
    <CloudDownload size={14} /> Pull
  </button>
  <button class={btn} disabled={!syncReady} onclick={onPush}>
    <CloudUpload size={14} /> Push
  </button>

  <span class={sep}></span>

  <button class={btn} onclick={onDiagnose}>
    <Stethoscope size={14} /> Diagnose
  </button>

  <span class="ml-auto"></span>

  <button class={btn} onclick={onSettings}>
    <SettingsIcon size={14} /> Settings
  </button>
</header>
