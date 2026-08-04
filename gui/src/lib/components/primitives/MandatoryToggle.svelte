<script lang="ts">
  // "Mandatory only" filter toggle. Pair with `KindFilterRow` to
  // form every variant's filter bar.

  import { Pin } from '@lucide/svelte';

  interface Props {
    value: boolean;
    onChange: (v: boolean) => void;
  }

  let { value, onChange }: Props = $props();
</script>

<label
  class="inline-flex cursor-pointer items-center gap-1 rounded-full px-2 py-0.5 text-[11px] ring-1 ring-inset
    {value
    ? 'bg-amber-500/15 text-amber-300 ring-amber-500/40'
    : 'text-fg-muted ring-line-strong hover:bg-surface-2'}"
>
  <input
    type="checkbox"
    class="sr-only"
    checked={value}
    onchange={(e) => onChange((e.currentTarget as HTMLInputElement).checked)}
  />
  <Pin size={10} />
  Mandatory only
</label>
