<script lang="ts">
  // A single diagnostic rendered as a self-contained block with a
  // title row (severity icon + slug badge) and a body paragraph
  // (the message). Block layout beats the old inline row because
  // long messages always wrap under a consistent left edge and the
  // slug is always in the same spot regardless of length.

  import { AlertTriangle, Info, XCircle } from 'lucide-svelte';
  import type { Issue } from '$lib/types';
  import { normalizeSeverity, SEVERITY_META } from '$lib/utils/diag';

  interface Props {
    issue: Issue;
  }

  let { issue }: Props = $props();
  const sev = $derived(normalizeSeverity(issue.severity));
  const meta = $derived(SEVERITY_META[sev]);
  const iconColor = $derived(
    meta.tile.split(' ').find((c) => c.startsWith('text-')) ?? ''
  );
</script>

<article
  class="rounded-md border-l-2 bg-surface-2/40 px-3 py-2 {meta.accent.replace('border-l-', 'border-l-')}"
>
  <header class="flex items-center gap-2 text-[11px] font-semibold uppercase tracking-wide {iconColor}">
    {#if sev === 'error'}
      <XCircle size={13} />
    {:else if sev === 'warning'}
      <AlertTriangle size={13} />
    {:else}
      <Info size={13} />
    {/if}
    <span>{meta.label}</span>
    {#if issue.slug}
      <code class="ml-1 rounded bg-surface-3 px-1.5 py-0.5 text-[10px] font-normal normal-case text-fg-muted">
        {issue.slug}
      </code>
    {/if}
  </header>
  <p class="mt-1 break-words text-[13px] leading-snug text-fg">
    {issue.message}
  </p>
</article>
