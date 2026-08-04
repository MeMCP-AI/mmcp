<script lang="ts">
  import { page } from '$app/state';
  import { ArrowLeft, LoaderCircle } from '@lucide/svelte';
  import KindBadge from '$lib/components/KindBadge.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';

  const groupId = $derived(page.params.id ?? '');

  $effect(() => {
    const id = groupId;
    if (!id) return;
    if (!memoriesStore.byGroup[id] && !memoriesStore.loading[id]) {
      void memoriesStore.load(id);
    }
  });

  const mems = $derived(memoriesStore.byGroup[groupId]);
  const info = $derived(memoriesStore.info[groupId]);
  const loading = $derived(!!memoriesStore.loading[groupId]);
</script>

<div class="mx-auto flex w-full max-w-4xl flex-col gap-4 p-6">
  <div class="flex items-center gap-2 text-xs text-zinc-500">
    <a href="/" class="inline-flex items-center gap-1 hover:text-zinc-300">
      <ArrowLeft size={12} /> Groups
    </a>
  </div>

  <header class="flex flex-col gap-1">
    <h1 class="text-lg font-semibold text-zinc-100">
      {info?.display_name ?? info?.slug ?? groupId}
    </h1>
    <div class="flex flex-wrap items-center gap-2 text-xs text-zinc-500">
      <code title={groupId}>{groupId}</code>
      {#if info}
        <span>·</span>
        <span>owner: {info.owner}</span>
        <span>·</span>
        <span>role: {info.effective_role}</span>
        <span>·</span>
        <span>{info.memory_count} memories</span>
      {/if}
    </div>
  </header>

  {#if loading && !mems}
    <p class="inline-flex items-center gap-1 text-sm text-zinc-400">
      <LoaderCircle size={14} class="animate-spin" /> Loading memories…
    </p>
  {:else if memoriesStore.error}
    <p class="rounded-md border border-rose-500/30 bg-rose-500/10 p-3 text-sm text-rose-200">
      {memoriesStore.error}
    </p>
  {:else if mems && mems.length === 0}
    <p class="text-sm text-zinc-400">No memories in this group yet.</p>
  {:else if mems}
    <ul class="flex flex-col divide-y divide-zinc-800 rounded-md border border-zinc-800 bg-zinc-900/50">
      {#each mems as mem (mem.id)}
        <li class="flex flex-col gap-1 px-4 py-3 text-sm">
          <div class="flex items-center gap-2">
            <KindBadge kind={mem.kind} />
            <span class="font-medium text-zinc-100">{mem.name}</span>
            {#if mem.mandatory}
              <span
                class="rounded-sm bg-rose-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-rose-300 ring-1 ring-inset ring-rose-500/30"
              >
                mandatory
              </span>
            {/if}
            {#if mem.latest_version}
              <span class="text-xs text-zinc-500">{mem.latest_version}</span>
            {/if}
          </div>
          <p class="text-xs text-zinc-400">{mem.description}</p>
          <div class="flex gap-3 text-[11px] text-zinc-600">
            <span>slug: <code class="text-zinc-400">{mem.slug}</code></span>
            <span title={mem.id}>id: <code class="text-zinc-400">{mem.id.slice(0, 8)}…</code></span>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</div>
