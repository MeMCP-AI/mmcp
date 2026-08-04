<script lang="ts">
  // Thin search-styled input. Every variant's filter / global
  // search bar uses the same shape — magnifier icon on the left,
  // optional clear button on the right. Consolidated so a visual
  // tweak applies across every call site.

  import { Search, X } from '@lucide/svelte';

  interface Props {
    value: string;
    onChange: (v: string) => void;
    placeholder?: string;
    /** Width helper; default matches the compact navbar search. */
    widthClass?: string;
    onFocus?: () => void;
    onBlur?: () => void;
  }

  let {
    value,
    onChange,
    placeholder = 'Search…',
    widthClass = 'w-full',
    onFocus,
    onBlur
  }: Props = $props();
</script>

<div class="relative {widthClass}">
  <Search
    size={12}
    class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-fg-subtle"
  />
  <input
    type="text"
    class="w-full rounded-md border border-line bg-surface-0 py-1 pl-7 pr-7 text-xs text-fg placeholder:text-fg-subtle focus:border-line-strong focus:outline-none"
    {placeholder}
    {value}
    oninput={(e) => onChange((e.currentTarget as HTMLInputElement).value)}
    onfocus={() => onFocus?.()}
    onblur={() => onBlur?.()}
  />
  {#if value}
    <button
      type="button"
      class="absolute right-1 top-1/2 -translate-y-1/2 rounded-sm p-0.5 text-fg-subtle hover:bg-surface-2 hover:text-fg"
      onclick={() => onChange('')}
      aria-label="Clear search"
      onmousedown={(e) => e.preventDefault()}
    >
      <X size={11} />
    </button>
  {/if}
</div>
