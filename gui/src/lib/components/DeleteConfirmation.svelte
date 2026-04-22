<script lang="ts">
  import Modal from './Modal.svelte';
  import { Trash2 } from 'lucide-svelte';

  interface Props {
    slugs: string[];
    onConfirm: () => void;
    onCancel: () => void;
  }

  let { slugs, onConfirm, onCancel }: Props = $props();
  const multi = $derived(slugs.length > 1);
</script>

<Modal
  title={multi ? `Delete ${slugs.length} memories?` : 'Delete memory?'}
  onClose={onCancel}
  widthClass="max-w-md"
>
  <div class="flex flex-col gap-4 p-5">
    {#if multi}
      <p class="text-sm text-fg">
        Delete {slugs.length} memories?
      </p>
      <ul class="max-h-48 overflow-y-auto rounded-md border border-line bg-surface-0 p-2 text-xs">
        {#each slugs as slug (slug)}
          <li class="truncate px-1 py-0.5 font-mono text-fg-muted" title={slug}>{slug}</li>
        {/each}
      </ul>
    {:else}
      <p class="text-sm text-fg">
        Delete memory <code class="rounded bg-surface-2 px-1 py-0.5 text-xs">{slugs[0]}</code>?
      </p>
    {/if}
    <p class="text-xs text-fg-subtle">
      A git commit records the deletion; the memor{multi ? 'ies' : 'y'} can be recovered from history.
    </p>
    <div class="flex justify-end gap-2">
      <button
        type="button"
        class="inline-flex items-center rounded-md border border-line-strong px-3 py-1.5 text-sm text-fg hover:bg-surface-2"
        onclick={onCancel}
      >
        Cancel
      </button>
      <button
        type="button"
        class="inline-flex items-center gap-1.5 rounded-md bg-rose-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-rose-500"
        onclick={onConfirm}
      >
        <Trash2 size={14} />
        {multi ? `Delete ${slugs.length}` : 'Delete'}
      </button>
    </div>
  </div>
</Modal>
