<script lang="ts">
  import { X } from '@lucide/svelte';

  interface Props {
    title: string;
    onClose: () => void;
    widthClass?: string;
    children?: import('svelte').Snippet;
  }

  let { title, onClose, widthClass = 'max-w-lg', children }: Props = $props();

  function handleKey(e: KeyboardEvent) {
    if (e.key === 'Escape') onClose();
  }
</script>

<svelte:window onkeydown={handleKey} />

<div
  class="fixed inset-0 z-50 flex items-start justify-center bg-black/60 p-6 backdrop-blur-sm"
  onclick={onClose}
  role="presentation"
>
  <div
    class="relative mt-16 w-full {widthClass} overflow-hidden rounded-xl border border-line bg-surface-1 shadow-2xl"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.stopPropagation()}
    role="dialog"
    aria-modal="true"
    aria-label={title}
    tabindex={-1}
  >
    <header class="flex items-center justify-between border-b border-line px-5 py-3">
      <h2 class="text-sm font-semibold text-fg">{title}</h2>
      <button
        type="button"
        class="rounded-md p-1 text-fg-muted hover:bg-surface-2 hover:text-fg"
        aria-label="Close"
        onclick={onClose}
      >
        <X size={14} />
      </button>
    </header>
    <div class="max-h-[70vh] overflow-y-auto">
      {@render children?.()}
    </div>
  </div>
</div>
