<script lang="ts">
  import { Wifi, WifiOff, LoaderCircle } from 'lucide-svelte';

  type ReachState = { t: 'unknown' } | { t: 'online' } | { t: 'offline'; reason: string };
  type SyncPhase =
    | { t: 'unknown' }
    | { t: 'not_configured' }
    | { t: 'idle'; serverUrl: string }
    | { t: 'syncing'; op: 'pull' | 'push'; serverUrl: string }
    | { t: 'ok'; op: 'pull' | 'push'; serverUrl: string; summary: string }
    | { t: 'err'; op: 'pull' | 'push'; serverUrl: string; message: string };

  interface Props {
    reachability: ReachState;
    sync: SyncPhase;
    selectedGroupSlug: string | null;
    selectedMemoryCount: number | null;
  }

  let { reachability, sync, selectedGroupSlug, selectedMemoryCount }: Props = $props();

  const tone = {
    muted: 'text-zinc-500',
    online: 'text-emerald-400',
    offline: 'text-rose-400',
    warn: 'text-amber-400'
  };
</script>

<footer
  class="flex h-7 shrink-0 items-center gap-3 border-t border-zinc-800 bg-zinc-950 px-3 text-xs {tone.muted}"
>
  {#if sync.t !== 'not_configured'}
    {#if reachability.t === 'online'}
      <span class="inline-flex items-center gap-1 {tone.online}">
        <Wifi size={12} /> online
      </span>
    {:else if reachability.t === 'offline'}
      <span class="inline-flex items-center gap-1 {tone.offline}" title={reachability.reason}>
        <WifiOff size={12} /> offline
      </span>
    {:else}
      <span class="inline-flex items-center gap-1">
        <LoaderCircle size={12} class="animate-spin" /> probing…
      </span>
    {/if}
    <span class="h-3 w-px bg-zinc-800"></span>
  {/if}

  <span>
    {#if sync.t === 'not_configured'}
      sync: not configured
    {:else if sync.t === 'unknown'}
      sync: starting…
    {:else if sync.t === 'idle'}
      sync: idle ({sync.serverUrl})
    {:else if sync.t === 'syncing'}
      <span class="inline-flex items-center gap-1 {tone.warn}">
        <LoaderCircle size={12} class="animate-spin" />
        sync: {sync.op}ing {sync.serverUrl}
      </span>
    {:else if sync.t === 'ok'}
      <span class={tone.online}>sync: {sync.op} ok ({sync.serverUrl}) — {sync.summary}</span>
    {:else if sync.t === 'err'}
      <span class={tone.offline} title={sync.message}
        >sync: {sync.op} failed ({sync.serverUrl})</span
      >
    {/if}
  </span>

  <span class="ml-auto">
    {#if selectedGroupSlug}
      group: {selectedGroupSlug}
      {#if selectedMemoryCount !== null}
        ({selectedMemoryCount} memories)
      {/if}
    {/if}
  </span>
</footer>
