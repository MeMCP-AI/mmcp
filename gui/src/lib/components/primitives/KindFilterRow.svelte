<script lang="ts">
  // Toggleable kind pill-row. Every filter UI has the same bank:
  // six opacity-faded kind badges that brighten when selected.
  // Caller owns the `Set<KindStr>`, we just render + call back.

  import KindBadge from '../KindBadge.svelte';
  import type { KindStr } from '$lib/types';

  interface Props {
    selected: Set<KindStr>;
    kinds?: KindStr[];
    onToggle: (kind: KindStr) => void;
  }

  const DEFAULT_KINDS: KindStr[] = [
    'rule',
    'snapshot',
    'log',
    'reference',
    'scratch',
    'feature'
  ];

  let { selected, kinds = DEFAULT_KINDS, onToggle }: Props = $props();
</script>

<div class="flex flex-wrap items-center gap-1">
  {#each kinds as k (k)}
    {@const active = selected.has(k)}
    <button
      type="button"
      class="rounded-md transition-opacity {active ? '' : 'opacity-55 hover:opacity-100'}"
      onclick={() => onToggle(k)}
      aria-pressed={active}
      title={active ? `Don't filter on ${k}` : `Filter on ${k}`}
    >
      <KindBadge kind={k} mode="icon_and_text" />
    </button>
  {/each}
</div>
