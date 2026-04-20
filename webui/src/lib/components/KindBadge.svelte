<script lang="ts">
  import type { KindStr } from '$lib/types';

  // Mirrored from `gui/src/lib/components/KindBadge.svelte`. The
  // gui version carries an extra `mode` prop tied to its settings
  // store; the webui has no such toggle yet, so we inline the
  // `icon_and_text` branch.

  let { kind }: { kind: KindStr } = $props();

  const LABELS: Record<KindStr, string> = {
    rule: 'RULE',
    snapshot: 'SNAP',
    log: 'LOG',
    reference: 'REF',
    scratch: 'SCR',
    feature: 'FEAT'
  };

  const COLORS: Record<KindStr, string> = {
    rule: 'bg-kind-rule/15 text-kind-rule ring-kind-rule/30',
    snapshot: 'bg-kind-snapshot/15 text-kind-snapshot ring-kind-snapshot/30',
    log: 'bg-kind-log/15 text-kind-log ring-kind-log/30',
    reference: 'bg-kind-reference/15 text-kind-reference ring-kind-reference/30',
    scratch: 'bg-kind-scratch/15 text-kind-scratch ring-kind-scratch/30',
    feature: 'bg-kind-feature/15 text-kind-feature ring-kind-feature/30'
  };
</script>

<span
  class="inline-flex items-center rounded-md px-1.5 py-0.5 text-[10px] font-semibold tracking-wide uppercase ring-1 ring-inset {COLORS[
    kind
  ]}"
>
  {LABELS[kind]}
</span>
