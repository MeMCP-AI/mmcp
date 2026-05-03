<script lang="ts">
  // Unified card for one scope (a real group OR the synthetic
  // "Project" section). Both consumers want the same chrome:
  // left-accent stripe coloured by worst severity, collapsible
  // header with name + status pill + per-severity chip counts,
  // and a body that renders DiagFindingBlock for every visible
  // finding.

  import { AlertTriangle, ChevronDown, ChevronRight, Info, XCircle } from 'lucide-svelte';
  import type { Finding } from '$lib/types';
  import {
    accentFor,
    countBySeverity,
    filterFindings,
    SEVERITY_META,
    SEVERITY_ORDER,
    type SeverityFilter
  } from '$lib/utils/diag';
  import DiagFindingBlock from './DiagFindingBlock.svelte';

  interface Props {
    title: string;
    subtitle?: string;
    statusOk?: boolean;
    statusOkLabel?: string;
    statusBrokenLabel?: string;
    findings: Finding[];
    filter: SeverityFilter;
    open: boolean;
    onToggle: () => void;
  }

  let {
    title,
    subtitle,
    statusOk,
    statusOkLabel,
    statusBrokenLabel,
    findings,
    filter,
    open,
    onToggle
  }: Props = $props();

  const counts = $derived(countBySeverity(findings));
  const visible = $derived(filterFindings(findings, filter));
  const accent = $derived(accentFor(findings));
</script>

<section
  class="rounded-lg border border-l-4 border-line {accent} bg-surface-1/40"
>
  <header class="flex flex-wrap items-center gap-2 p-3">
    <button
      type="button"
      class="flex items-center gap-2 text-sm font-semibold text-fg"
      onclick={onToggle}
    >
      {#if open}
        <ChevronDown size={14} />
      {:else}
        <ChevronRight size={14} />
      {/if}
      {title}
    </button>
    {#if subtitle}
      <span class="text-xs text-fg-subtle">{subtitle}</span>
    {/if}
    {#if statusOk !== undefined && (statusOkLabel || statusBrokenLabel)}
      <span
        class="inline-flex items-center rounded-md px-1.5 py-0.5 text-[10px] font-semibold uppercase ring-1 ring-inset
          {statusOk
          ? 'bg-emerald-500/15 text-emerald-300 ring-emerald-500/30'
          : 'bg-rose-500/15 text-rose-300 ring-rose-500/30'}"
      >
        {statusOk ? statusOkLabel : statusBrokenLabel}
      </span>
    {/if}
    <span class="ml-auto flex items-center gap-1">
      {#each SEVERITY_ORDER as sev (sev)}
        {#if counts[sev] > 0}
          {@const meta = SEVERITY_META[sev]}
          <span
            class="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] font-semibold ring-1 ring-inset {meta.chip}"
            title={meta.plural}
          >
            {#if sev === 'error'}
              <XCircle size={10} />
            {:else if sev === 'warning'}
              <AlertTriangle size={10} />
            {:else}
              <Info size={10} />
            {/if}
            {counts[sev]}
          </span>
        {/if}
      {/each}
    </span>
  </header>
  {#if open}
    <div class="flex flex-col gap-2 px-3 pb-3">
      {#each visible as finding, idx (idx)}
        <DiagFindingBlock {finding} />
      {:else}
        <span class="text-xs text-fg-subtle">
          {findings.length === 0 ? 'No findings.' : 'No findings at current filter.'}
        </span>
      {/each}
    </div>
  {/if}
</section>
