<script lang="ts">
  // One tile per severity bucket. Doubles as both a count display
  // and a filter toggle — tapping it flips the current filter
  // between "that severity" and "all", so the same row shows the
  // bucket sizes *and* the control that narrows the view. Avoids
  // the old duplication of separate tiles + pill row.

  import { AlertTriangle, Info, XCircle } from '@lucide/svelte';
  import { SEVERITY_META, type Severity, type SeverityFilter } from '$lib/utils/diag';

  interface Props {
    severity: Severity;
    count: number;
    filter: SeverityFilter;
    onToggle: () => void;
  }

  let { severity, count, filter, onToggle }: Props = $props();

  const meta = $derived(SEVERITY_META[severity]);
  const active = $derived(filter === severity);
</script>

<button
  type="button"
  aria-pressed={active}
  onclick={onToggle}
  class="flex items-center gap-2 rounded-lg border p-3 text-left transition-colors {meta.tile} {active ? 'ring-2 ring-current' : 'hover:brightness-110'}"
>
  {#if severity === 'error'}
    <XCircle size={16} />
  {:else if severity === 'warning'}
    <AlertTriangle size={16} />
  {:else}
    <Info size={16} />
  {/if}
  <span class="flex flex-col leading-tight">
    <span class="text-xl font-semibold">{count}</span>
    <span class="text-[10px] uppercase tracking-wide opacity-80">{meta.plural}</span>
  </span>
</button>
