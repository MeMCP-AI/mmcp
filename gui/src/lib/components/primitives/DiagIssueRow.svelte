<script lang="ts">
  // A single diagnostic line: severity icon, optional slug chip,
  // message. Every issue — project-scoped or group-scoped — renders
  // through this primitive so colour, spacing, and icon sizing
  // never drift between sections.

  import type { Issue } from '$lib/types';
  import { normalizeSeverity, SEVERITY_META } from '$lib/utils/diag';

  interface Props {
    issue: Issue;
  }

  let { issue }: Props = $props();
  const sev = $derived(normalizeSeverity(issue.severity));
  const meta = $derived(SEVERITY_META[sev]);
</script>

<div class="flex items-start gap-2 text-sm">
  <meta.Icon size={14} class="mt-0.5 shrink-0 {meta.tile.split(' ').find((c) => c.startsWith('text-')) ?? ''}" />
  {#if issue.slug}
    <code class="shrink-0 rounded bg-surface-2 px-1 py-0.5 text-[11px] text-fg-muted">
      {issue.slug}
    </code>
  {/if}
  <span class="min-w-0 flex-1 text-fg">{issue.message}</span>
</div>
