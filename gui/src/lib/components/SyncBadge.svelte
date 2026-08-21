<script lang="ts">
  // Compact server status chip. Every variant's top chrome drops
  // this in so the user can see at a glance which effective remote
  // set the GUI is pointed at and whether the 15-second probe says
  // it's reachable. Colour mirrors the reachability state (green on,
  // red off, zinc pending); the label is the resolved remote-set
  // summary (a remote's name, or a count plus its default's name).

  import { AlertTriangle, CircleDashed, LoaderCircle, Wifi, WifiOff } from '@lucide/svelte';
  import { reachabilityStore } from '$lib/stores/reachability.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';

  const phase = $derived(syncStore.phase);
  const reach = $derived(reachabilityStore.state);
  const broken = $derived(phase.t === 'broken');

  const remotesSummary = $derived.by(() => {
    switch (phase.t) {
      case 'idle':
      case 'syncing':
      case 'ok':
      case 'err':
        return phase.remotesSummary;
      default:
        return null;
    }
  });

  // `syncStore.configured` is the SSOT: it is false during `unknown` and `failed` too.
  const configured = $derived(syncStore.configured);

  const tone = $derived.by(() => {
    if (broken) return 'text-amber-300 ring-amber-500/40';
    if (!configured) return 'text-fg-subtle ring-line';
    if (reach.t === 'online') return 'text-emerald-300 ring-emerald-500/40';
    if (reach.t === 'offline') return 'text-rose-300 ring-rose-500/40';
    return 'text-fg-muted ring-line';
  });

  const tipReach = $derived.by(() => {
    if (phase.t === 'broken') return `Sync config error: ${phase.message}`;
    if (!configured) return 'No sync server configured';
    if (reach.t === 'online') return 'Online';
    if (reach.t === 'offline') return `Offline: ${reach.reason}`;
    if (reach.t === 'not_applicable') return 'Reachability not checked for this remote';
    return 'Probing…';
  });
</script>

<div
  class="inline-flex items-center gap-1.5 rounded-md bg-surface-0 px-2 py-0.5 text-[11px] ring-1 ring-inset {tone}"
  title={`${tipReach}${remotesSummary ? ` · ${remotesSummary}` : ''}`}
>
  {#if broken}
    <AlertTriangle size={11} />
    <span>config error</span>
  {:else if !configured}
    <WifiOff size={11} />
    <span>no server</span>
  {:else if reach.t === 'online'}
    <Wifi size={11} />
    <span class="font-mono truncate max-w-[12rem]">{remotesSummary}</span>
  {:else if reach.t === 'offline'}
    <WifiOff size={11} />
    <span class="font-mono truncate max-w-[12rem]">{remotesSummary}</span>
  {:else if reach.t === 'not_applicable'}
    <CircleDashed size={11} />
    <span class="font-mono truncate max-w-[12rem]">{remotesSummary}</span>
  {:else}
    <LoaderCircle size={11} class="animate-spin" />
    <span class="font-mono truncate max-w-[12rem]">{remotesSummary}</span>
  {/if}
</div>
