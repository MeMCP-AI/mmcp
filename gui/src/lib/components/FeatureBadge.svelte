<script lang="ts">
  // Compact status pill for feature-request memories. Tint follows
  // the lifecycle state so a glance at a list conveys where each
  // FR stands without expanding it.

  import { Ban, CheckCircle2, Circle, CircleDashed, GitMerge, PauseCircle } from 'lucide-svelte';
  import type { FeatureStatus } from '$lib/types';

  interface Props {
    status: FeatureStatus;
    number?: number | null;
    /** Compact variant drops the label so it fits inside narrow
     * tree rows. `label: true` is the default for use inside
     * viewer headers and card rows. */
    label?: boolean;
  }

  let { status, number = null, label = true }: Props = $props();

  const META: Record<
    FeatureStatus,
    { label: string; Icon: typeof Circle; cls: string }
  > = {
    open: {
      label: 'Open',
      Icon: Circle,
      cls: 'bg-sky-500/15 text-sky-300 ring-sky-500/40'
    },
    resolved: {
      label: 'Resolved',
      Icon: CheckCircle2,
      cls: 'bg-emerald-500/15 text-emerald-300 ring-emerald-500/40'
    },
    blocked: {
      label: 'Blocked',
      Icon: Ban,
      cls: 'bg-rose-500/15 text-rose-300 ring-rose-500/40'
    },
    deferred: {
      label: 'Deferred',
      Icon: PauseCircle,
      cls: 'bg-amber-500/15 text-amber-300 ring-amber-500/40'
    },
    duplicate: {
      label: 'Duplicate',
      Icon: CircleDashed,
      cls: 'bg-zinc-500/15 text-fg-muted ring-zinc-500/40'
    },
    superseded: {
      label: 'Superseded',
      Icon: GitMerge,
      cls: 'bg-violet-500/15 text-violet-300 ring-violet-500/40'
    }
  };

  const meta = $derived(META[status]);
</script>

<span
  class="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] font-semibold uppercase ring-1 ring-inset {meta.cls}"
  title={`${meta.label}${number !== null ? ` — FR-${String(number).padStart(3, '0')}` : ''}`}
>
  <meta.Icon size={10} />
  {#if label}
    <span>{meta.label}</span>
  {/if}
  {#if number !== null}
    <span class="font-mono">#{number}</span>
  {/if}
</span>
