<script lang="ts">
  import { LoaderCircle, Wifi, WifiOff } from '@lucide/svelte';
  import { shortCommit } from '$lib/format';
  import type { HealthInfo } from '$lib/types';

  interface Props {
    health: HealthInfo | null;
    probing: boolean;
    healthError: string | null;
    serverUrl: string;
    authed: boolean;
  }

  let { health, probing, healthError, serverUrl, authed }: Props = $props();
</script>

<footer
  class="flex h-7 shrink-0 items-center gap-3 border-t border-zinc-800 bg-zinc-950 px-3 text-xs text-zinc-500"
>
  {#if probing && !health}
    <span class="inline-flex items-center gap-1">
      <LoaderCircle size={12} class="animate-spin" /> probing…
    </span>
  {:else if health}
    <span class="inline-flex items-center gap-1 text-emerald-400" title={health.name}>
      <Wifi size={12} /> {health.status}
    </span>
  {:else}
    <span class="inline-flex items-center gap-1 text-rose-400" title={healthError ?? ''}>
      <WifiOff size={12} /> unreachable
    </span>
  {/if}

  <span class="h-3 w-px bg-zinc-800"></span>

  <span>server: {serverUrl}</span>

  {#if health}
    <span class="h-3 w-px bg-zinc-800"></span>
    <span title="Server binary version">v{shortCommit(health.version, 12)}</span>
  {/if}

  <span class="ml-auto">
    {#if authed}
      <span class="text-emerald-400">authenticated</span>
    {:else}
      <span class="text-amber-400">anonymous</span>
    {/if}
  </span>
</footer>
