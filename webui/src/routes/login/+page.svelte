<script lang="ts">
  import { goto } from '$app/navigation';
  import { LoaderCircle } from '@lucide/svelte';
  import { login } from '$lib/api/auth';
  import { formatErr } from '$lib/format';
  import { authStore } from '$lib/stores/auth.svelte';

  let handle = $state('');
  let password = $state('');
  let pending = $state(false);
  let errorMsg = $state<string | null>(null);

  async function onSubmit(ev: SubmitEvent) {
    ev.preventDefault();
    pending = true;
    errorMsg = null;
    try {
      const ok = await login(handle, password);
      authStore.set({ token: ok.token, userId: ok.user_id, expiresAt: ok.expires_at });
      await goto('/');
    } catch (err) {
      errorMsg = formatErr(err);
    } finally {
      pending = false;
    }
  }
</script>

<div class="mx-auto flex w-full max-w-sm flex-col gap-4 p-6">
  <header>
    <h1 class="text-lg font-semibold text-zinc-100">Sign in</h1>
    <p class="text-xs text-zinc-500">
      Authenticate against the upstream mmcp-server. The bearer token
      is stored in this browser's localStorage.
    </p>
  </header>

  <form onsubmit={onSubmit} class="flex flex-col gap-3">
    <label class="flex flex-col gap-1 text-xs text-zinc-400">
      Handle
      <input
        type="text"
        autocomplete="username"
        bind:value={handle}
        class="rounded-md border border-zinc-800 bg-zinc-900 px-3 py-2 text-sm text-zinc-100 focus:border-sky-500 focus:outline-none"
        required
      />
    </label>
    <label class="flex flex-col gap-1 text-xs text-zinc-400">
      Password
      <input
        type="password"
        autocomplete="current-password"
        bind:value={password}
        class="rounded-md border border-zinc-800 bg-zinc-900 px-3 py-2 text-sm text-zinc-100 focus:border-sky-500 focus:outline-none"
        required
      />
    </label>
    <button
      type="submit"
      disabled={pending}
      class="mt-2 inline-flex items-center justify-center gap-2 rounded-md bg-sky-500 px-3 py-2 text-sm font-medium text-white hover:bg-sky-400 disabled:cursor-not-allowed disabled:opacity-60"
    >
      {#if pending}
        <LoaderCircle size={14} class="animate-spin" />
        Signing in…
      {:else}
        Sign in
      {/if}
    </button>
  </form>

  {#if errorMsg}
    <p class="rounded-md border border-rose-500/30 bg-rose-500/10 p-3 text-sm text-rose-200">
      {errorMsg}
    </p>
  {/if}
</div>
