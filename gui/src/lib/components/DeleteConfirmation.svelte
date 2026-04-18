<script lang="ts">
  import Modal from './Modal.svelte';
  import { Trash2 } from 'lucide-svelte';

  interface Props {
    slug: string;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let { slug, onConfirm, onCancel }: Props = $props();
</script>

<Modal title="Delete memory?" onClose={onCancel} widthClass="max-w-md">
  <div class="flex flex-col gap-4 p-5">
    <p class="text-sm text-zinc-200">
      Delete memory <code class="rounded bg-zinc-800 px-1 py-0.5 text-xs">{slug}</code>?
    </p>
    <p class="text-xs text-zinc-500">
      A git commit records the deletion; the memory can be recovered from history.
    </p>
    <div class="flex justify-end gap-2">
      <button
        type="button"
        class="inline-flex items-center rounded-md border border-zinc-700 px-3 py-1.5 text-sm text-zinc-200 hover:bg-zinc-800"
        onclick={onCancel}
      >
        Cancel
      </button>
      <button
        type="button"
        class="inline-flex items-center gap-1.5 rounded-md bg-rose-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-rose-500"
        onclick={onConfirm}
      >
        <Trash2 size={14} /> Delete
      </button>
    </div>
  </div>
</Modal>
