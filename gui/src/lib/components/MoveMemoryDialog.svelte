<script lang="ts">
  // Cross-group move confirmation. Keeps the memory's id, slug, and
  // body; the source group loses the memory (a normal delete commit),
  // the target group gains it with a history pointer back to the
  // source. Reports every `[[...]]` back-reference the caller may
  // want to fix by hand: this dialog never edits another memory.

  import { moveMemory } from '$lib/api/memory';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import { formatErr } from '$lib/utils/error';
  import type { MoveMemoryResult } from '$lib/types';

  let {
    groupId,
    slug,
    onClose,
    onMoved
  }: {
    groupId: string;
    slug: string;
    onClose: () => void;
    onMoved: (result: MoveMemoryResult) => void;
  } = $props();

  const candidateGroups = $derived(groupsStore.groups.filter((g) => g.group_id !== groupId));

  let targetGroupId = $state('');
  let renumber = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let result = $state<MoveMemoryResult | null>(null);

  async function submit() {
    if (!targetGroupId) return;
    busy = true;
    error = null;
    try {
      result = await moveMemory(groupId, slug, targetGroupId, renumber);
      onMoved(result);
    } catch (e) {
      error = formatErr(e);
    } finally {
      busy = false;
    }
  }
</script>

<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
  role="presentation"
  onclick={(e) => {
    if (e.target === e.currentTarget && !busy) onClose();
  }}
>
  <div
    class="flex max-h-[80vh] w-[28rem] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 text-sm text-fg shadow-xl"
    role="dialog"
    aria-modal="true"
    aria-label="Move memory to another group"
  >
    <header class="flex items-center justify-between border-b border-line px-4 py-2">
      <h2 class="text-sm font-medium text-fg">Move to another group</h2>
      <button type="button" class="text-fg-subtle hover:text-fg" onclick={onClose} aria-label="Close"
        >✕</button
      >
    </header>

    <div class="flex-1 overflow-y-auto px-4 py-3">
      {#if error}
        <p class="mb-3 rounded-md border border-red-700 bg-red-950/40 px-3 py-2 text-red-200">
          {error}
        </p>
      {/if}

      {#if result}
        <p class="mb-3 rounded-md border border-line bg-surface-2 px-3 py-2 text-fg">
          Moved <code class="font-mono text-xs">{result.slug}</code> to the target group.
          {#if result.renumbered}
            Tracker number renumbered {result.renumbered[0]} to {result.renumbered[1]}.
          {/if}
        </p>
        {#if result.back_references.length > 0}
          <div class="mb-3">
            <p class="mb-1 text-fg-muted">
              Links that may need a manual fix (never rewritten by this move):
            </p>
            <ul class="space-y-1">
              {#each result.back_references as ref (ref.group_slug + ':' + ref.memory_slug + ':' + ref.kind)}
                <li class="rounded-md border border-amber-700/40 bg-amber-950/20 px-2 py-1 text-xs text-amber-200">
                  {ref.group_slug}:{ref.memory_slug}
                  {ref.kind === 'dangling_same_group_link' ? '(now dangling)' : '(cross-group link)'}
                </li>
              {/each}
            </ul>
          </div>
        {/if}
        <p class="text-xs text-fg-subtle">{result.sync_push_note}</p>
      {:else}
        <label class="mb-3 flex flex-col gap-1 text-fg-muted">
          <span>Target group</span>
          <select
            name="move-target-group"
            bind:value={targetGroupId}
            class="rounded-md border border-line bg-surface-2 px-2 py-1.5 text-fg"
          >
            <option value="" disabled>Select a group…</option>
            {#each candidateGroups as g (g.group_id)}
              <option value={g.group_id}>{g.display_name ?? g.slug}</option>
            {/each}
          </select>
        </label>
        <label class="flex items-center gap-2 text-fg-muted">
          <input type="checkbox" name="move-renumber" bind:checked={renumber} />
          Mint a new tracker number if this one is already taken in the target group
        </label>
      {/if}
    </div>

    <footer class="flex items-center justify-end gap-2 border-t border-line px-4 py-2">
      <button
        type="button"
        class="rounded-md px-3 py-1.5 text-fg-muted hover:bg-surface-2"
        onclick={onClose}>{result ? 'Close' : 'Cancel'}</button
      >
      {#if !result}
        <button
          type="button"
          class="inline-flex items-center rounded-md bg-sky-600 px-3 py-1.5 font-medium text-white hover:bg-sky-500 disabled:cursor-not-allowed disabled:opacity-50"
          onclick={submit}
          disabled={busy || !targetGroupId}
        >
          Move
        </button>
      {/if}
    </footer>
  </div>
</div>
