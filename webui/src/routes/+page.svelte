<script lang="ts">
  import { LoaderCircle } from '@lucide/svelte';
  import { shortCommit } from '$lib/format';
  import { groupsStore } from '$lib/stores/groups.svelte';

  $effect(() => {
    void groupsStore.load();
  });
</script>

<div class="mx-auto flex w-full max-w-4xl flex-col gap-4 p-6">
  <header class="flex items-end justify-between">
    <div>
      <h1 class="text-lg font-semibold text-zinc-100">Groups</h1>
      <p class="text-xs text-zinc-500">
        Every group served by this mmcp-server instance, as seen from
        the server's own manifest.
      </p>
    </div>
    <button
      type="button"
      class="rounded-md border border-zinc-800 bg-zinc-900 px-3 py-1 text-xs text-zinc-300 hover:bg-zinc-800/70"
      onclick={() => void groupsStore.load()}
      disabled={groupsStore.loading}
    >
      Refresh
    </button>
  </header>

  {#if groupsStore.loading && groupsStore.groups.length === 0}
    <p class="inline-flex items-center gap-1 text-sm text-zinc-400">
      <LoaderCircle size={14} class="animate-spin" /> Loading groups…
    </p>
  {:else if groupsStore.error}
    <p class="rounded-md border border-rose-500/30 bg-rose-500/10 p-3 text-sm text-rose-200">
      {groupsStore.error}
    </p>
  {:else if groupsStore.groups.length === 0}
    <p class="text-sm text-zinc-400">No groups visible on this server.</p>
  {:else}
    <ul class="flex flex-col divide-y divide-zinc-800 rounded-md border border-zinc-800 bg-zinc-900/50">
      {#each groupsStore.groups as group (group.group_id)}
        <li>
          <a
            href="/groups/{group.group_id}"
            class="flex items-center gap-3 px-4 py-3 text-sm transition-colors hover:bg-zinc-800/70"
          >
            <div class="min-w-0 flex-1">
              <div class="truncate font-medium text-zinc-100">{group.slug}</div>
              <div class="truncate text-xs text-zinc-500" title={group.group_id}>
                {group.group_id}
              </div>
            </div>
            <code class="rounded bg-zinc-800/70 px-1.5 py-0.5 text-[11px] text-zinc-400">
              {shortCommit(group.head_commit)}
            </code>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</div>
