<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { authStore } from '$lib/stores/auth.svelte';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import { LogIn, LogOut, Server } from '@lucide/svelte';

  const authed = $derived(authStore.token !== null);

  function handleLogout() {
    authStore.clear();
    groupsStore.reset();
    void goto('/login');
  }

  const links: { href: string; label: string }[] = [
    { href: '/', label: 'Groups' }
  ];
</script>

<header
  class="flex h-11 shrink-0 items-center gap-4 border-b border-zinc-800 bg-zinc-900/70 px-3 text-sm"
>
  <a href="/" class="inline-flex items-center gap-2 font-semibold text-zinc-100">
    <Server size={16} /> mmcp
    <span class="rounded-sm bg-zinc-800 px-1 py-0.5 text-[10px] font-medium uppercase text-zinc-400">
      webui
    </span>
  </a>

  <nav class="flex items-center gap-1">
    {#each links as link (link.href)}
      {@const active = page.url.pathname === link.href}
      <a
        href={link.href}
        class="rounded-md px-3 py-1 text-xs font-medium transition-colors
          {active
          ? 'bg-sky-500/15 text-sky-100'
          : 'text-zinc-400 hover:bg-zinc-800/70 hover:text-zinc-200'}"
      >
        {link.label}
      </a>
    {/each}
  </nav>

  <div class="ml-auto flex items-center gap-2">
    {#if authed}
      <button
        type="button"
        onclick={handleLogout}
        class="inline-flex items-center gap-1 rounded-md px-3 py-1 text-xs font-medium text-zinc-300 hover:bg-zinc-800/70 hover:text-zinc-100"
      >
        <LogOut size={14} /> Logout
      </button>
    {:else}
      <a
        href="/login"
        class="inline-flex items-center gap-1 rounded-md bg-sky-500/15 px-3 py-1 text-xs font-medium text-sky-100 hover:bg-sky-500/25"
      >
        <LogIn size={14} /> Login
      </a>
    {/if}
  </div>
</header>
