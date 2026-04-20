<script lang="ts">
  import '../app.css';

  import StatusBar from '$lib/components/StatusBar.svelte';
  import TopNav from '$lib/components/TopNav.svelte';
  import { serverBaseUrl } from '$lib/api/client';
  import { authStore } from '$lib/stores/auth.svelte';
  import { healthStore } from '$lib/stores/health.svelte';

  let { children } = $props();

  // First paint on the client: rehydrate the bearer token from
  // localStorage, start the health pulse, and let children render.
  $effect(() => {
    authStore.hydrate();
    healthStore.mount();
    return () => healthStore.unmount();
  });

  const authed = $derived(authStore.token !== null);
  const serverUrl = serverBaseUrl();
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-zinc-950 text-zinc-100">
  <TopNav />

  <main class="min-h-0 flex-1 overflow-y-auto">
    {@render children()}
  </main>

  <StatusBar
    health={healthStore.state}
    probing={healthStore.probing}
    healthError={healthStore.error}
    {serverUrl}
    {authed}
  />
</div>
