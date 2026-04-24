<script lang="ts">
  // App-shell footer. Carries sync / reachability status and the
  // diagnose entry point so every variant surfaces them in the
  // same place regardless of layout.

  import { LoaderCircle, Stethoscope, Wifi, WifiOff } from 'lucide-svelte';
  import { reachabilityStore } from '$lib/stores/reachability.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';
  import { openDiagnosticsWindow } from '$lib/windows';

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

  // Host only — full URLs get long.
  const hostish = $derived.by(() => {
    if (!serverUrl) return null;
    try {
      const u = new URL(serverUrl);
      return u.host || serverUrl;
    } catch {
      return serverUrl;
    }
  });

  const reachTone = $derived.by(() => {
    if (phase.t === 'not_configured') return 'text-fg-subtle';
    if (reach.t === 'online') return 'text-emerald-300';
    if (reach.t === 'offline') return 'text-rose-300';
    return 'text-fg-muted';
  });

  const syncLine = $derived.by(() => {
    switch (phase.t) {
      case 'not_configured':
        return 'no server';
      case 'unknown':
        return 'starting…';
      case 'idle':
        return 'idle';
      case 'syncing':
        return `${phase.op}…`;
      case 'ok':
        return `${phase.op} ok — ${phase.summary}`;
      case 'err':
        return `${phase.op} failed`;
    }
  });
</script>

<footer
  class="flex h-7 shrink-0 items-center gap-3 border-t border-line bg-surface-1 px-3 text-[11px] text-fg-muted"
>
  <span class="inline-flex items-center gap-1 {reachTone}">
    {#if phase.t === 'not_configured'}
      <WifiOff size={11} />
      no server
    {:else if reach.t === 'online'}
      <Wifi size={11} /> online
    {:else if reach.t === 'offline'}
      <WifiOff size={11} />
      <span title={reach.reason}>offline</span>
    {:else}
      <LoaderCircle size={11} class="animate-spin" /> probing…
    {/if}
  </span>

  {#if hostish}
    <code class="truncate font-mono text-fg-muted" title={serverUrl ?? undefined}>
      {hostish}
    </code>
  {/if}

  <span class="h-3 w-px bg-line"></span>

  <span
    class={phase.t === 'err' ? 'text-rose-300' : phase.t === 'syncing' ? 'text-amber-300' : ''}
    title={phase.t === 'err' ? phase.message : undefined}
  >
    sync: {syncLine}
  </span>

  <button
    type="button"
    class="ml-auto inline-flex items-center gap-1 rounded-md border border-line px-2 py-0.5 text-fg hover:bg-surface-2"
    title="Diagnose"
    aria-label="Diagnose"
    onclick={() => void openDiagnosticsWindow()}
  >
    <Stethoscope size={11} />
    <span>Diagnose</span>
  </button>
</footer>
