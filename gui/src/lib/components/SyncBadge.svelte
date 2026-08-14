<script lang="ts">
  // Compact server status chip. Every variant's top chrome drops
  // this in so the user can see at a glance which mmcp server the
  // GUI is pointed at and whether the 15-second probe says it's
  // reachable. Colour mirrors the reachability state (green on,
  // red off, zinc pending); the hostname is the raw URL the probe
  // is hitting so it's unambiguous in multi-env setups.

  import { LoaderCircle, Wifi, WifiOff } from '@lucide/svelte';
  import { reachabilityStore } from '$lib/stores/reachability.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';

  const phase = $derived(syncStore.phase);
  const reach = $derived(reachabilityStore.state);

  const serverUrl = $derived.by(() => {
    switch (phase.t) {
      case 'idle':
      case 'syncing':
      case 'ok':
      case 'err':
        return phase.serverUrl;
      default:
        return null;
    }
  });

  // syncStore.configured is the SSOT for this check — do not
  // re-derive it here. It differs subtly from a bare
  // `phase.t !== 'not_configured'` check: `syncStore.configured` is
  // also false while `phase.t` is `'unknown'` or `'failed'`, so the
  // badge correctly reads "no server" during those phases instead of
  // showing a blank-hostname spinner (issue #153).
  const configured = $derived(syncStore.configured);

  // Host + port only — full URLs get long quickly.
  const hostish = $derived.by(() => {
    if (!serverUrl) return null;
    try {
      const u = new URL(serverUrl);
      return u.host || serverUrl;
    } catch {
      return serverUrl;
    }
  });

  const tone = $derived.by(() => {
    if (!configured) return 'text-fg-subtle ring-line';
    if (reach.t === 'online') return 'text-emerald-300 ring-emerald-500/40';
    if (reach.t === 'offline') return 'text-rose-300 ring-rose-500/40';
    return 'text-fg-muted ring-line';
  });

  const tipReach = $derived.by(() => {
    if (!configured) return 'No sync server configured';
    if (reach.t === 'online') return 'Online';
    if (reach.t === 'offline') return `Offline — ${reach.reason}`;
    return 'Probing…';
  });
</script>

<div
  class="inline-flex items-center gap-1.5 rounded-md bg-surface-0 px-2 py-0.5 text-[11px] ring-1 ring-inset {tone}"
  title={`${tipReach}${serverUrl ? ` · ${serverUrl}` : ''}`}
>
  {#if !configured}
    <WifiOff size={11} />
    <span>no server</span>
  {:else if reach.t === 'online'}
    <Wifi size={11} />
    <span class="font-mono truncate max-w-[12rem]">{hostish}</span>
  {:else if reach.t === 'offline'}
    <WifiOff size={11} />
    <span class="font-mono truncate max-w-[12rem]">{hostish}</span>
  {:else}
    <LoaderCircle size={11} class="animate-spin" />
    <span class="font-mono truncate max-w-[12rem]">{hostish}</span>
  {/if}
</div>
