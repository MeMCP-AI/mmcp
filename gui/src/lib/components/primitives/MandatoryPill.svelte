<script lang="ts">
  // The amber "mandatory" pill that decorates memories the AI is
  // expected to read every session. Compact mode renders only the
  // pin icon for narrow rows; full mode adds the label + optional
  // icon sizing.

  import { Pin } from '@lucide/svelte';

  interface Props {
    /** `label: false` renders the pin glyph alone for row / card
     * slots where horizontal space is scarce. */
    label?: boolean;
    size?: number;
  }

  let { label = true, size = 10 }: Props = $props();
</script>

{#if label}
  <span
    class="inline-flex items-center gap-1 rounded-md bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-amber-300 ring-1 ring-inset ring-amber-500/30"
    title="Mandatory memory — always read at session start"
  >
    <Pin {size} />
    mandatory
  </span>
{:else}
  <Pin {size} class="text-amber-400" aria-label="Mandatory memory" />
{/if}
