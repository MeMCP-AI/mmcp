<script lang="ts">
  import type { KindStr } from '$lib/types';
  import type { KindDisplay } from '$lib/stores/settings.svelte';

  let { kind, mode = 'icon_and_text' }: { kind: KindStr; mode?: KindDisplay } = $props();

  const LABELS: Record<KindStr, { short: string; long: string }> = {
    rule: { short: 'ru', long: 'RULE' },
    snapshot: { short: 'sn', long: 'SNAP' },
    log: { short: 'lg', long: 'LOG' },
    reference: { short: 'rf', long: 'REF' },
    scratch: { short: 'sc', long: 'SCR' },
    feature: { short: 'ft', long: 'FEAT' }
  };

  const COLORS: Record<KindStr, string> = {
    rule: 'bg-kind-rule/15 text-kind-rule ring-kind-rule/30',
    snapshot: 'bg-kind-snapshot/15 text-kind-snapshot ring-kind-snapshot/30',
    log: 'bg-kind-log/15 text-kind-log ring-kind-log/30',
    reference: 'bg-kind-reference/15 text-kind-reference ring-kind-reference/30',
    scratch: 'bg-kind-scratch/15 text-kind-scratch ring-kind-scratch/30',
    feature: 'bg-kind-feature/15 text-kind-feature ring-kind-feature/30'
  };

  const label = $derived(
    mode === 'icon'
      ? LABELS[kind].short
      : mode === 'text'
        ? LABELS[kind].long
        : mode === 'icon_and_text'
          ? LABELS[kind].long
          : ''
  );
</script>

{#if mode !== 'off' && label}
  <span
    class="inline-flex items-center rounded-md px-1.5 py-0.5 text-[10px] font-semibold tracking-wide uppercase ring-1 ring-inset {COLORS[
      kind
    ]}"
  >
    {label}
  </span>
{/if}
