<script lang="ts">
  // Diagnostics panel. Composes DiagSeverityTile + DiagGroupCard +
  // DiagIssueRow primitives; this file is just routing & layout.
  // Every severity colour / count / icon / filter check lives in
  // $lib/utils/diag so nothing here can drift out of sync with
  // the store or the primitives.

  import { LoaderCircle, RefreshCcw, X } from 'lucide-svelte';
  import type { DiagReport } from '$lib/types';
  import {
    reportTotals,
    SEVERITY_ORDER,
    type SeverityFilter
  } from '$lib/utils/diag';
  import DiagSeverityTile from './primitives/DiagSeverityTile.svelte';
  import DiagGroupCard from './primitives/DiagGroupCard.svelte';

  interface Props {
    report: DiagReport | null;
    loading: boolean;
    error: string | null;
    filter: SeverityFilter;
    collapsed: Record<string, boolean>;
    onClose: () => void;
    onRefresh: () => void;
    onFilterChange: (filter: SeverityFilter) => void;
    onToggleGroup: (slug: string) => void;
  }

  let {
    report,
    loading,
    error,
    filter,
    collapsed,
    onClose,
    onRefresh,
    onFilterChange,
    onToggleGroup
  }: Props = $props();

  const totals = $derived(reportTotals(report));

  // Click a tile to filter; click again to clear. One control
  // surface for counts + filtering instead of the old tiles + pill
  // row pair that drifted from each other.
  function toggleFilter(target: SeverityFilter) {
    onFilterChange(filter === target ? 'all' : target);
  }

  const PROJECT_KEY = '__project__';
</script>

<section class="flex h-full min-h-0 flex-col overflow-hidden bg-surface-0 text-fg">
  <header
    class="flex shrink-0 items-center gap-3 border-b border-line bg-surface-1/40 px-4 py-2 sm:px-6"
  >
    <h1 class="text-sm font-semibold text-fg">Diagnostics</h1>
    {#if filter !== 'all'}
      <button
        type="button"
        class="rounded-full bg-surface-2 px-2 py-0.5 text-[10px] uppercase tracking-wide text-fg-muted hover:text-fg"
        onclick={() => onFilterChange('all')}
        title="Clear filter"
      >
        Filter: {filter} ✕
      </button>
    {/if}
    <button
      type="button"
      class="ml-auto inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs font-medium text-fg-muted hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-50"
      disabled={loading}
      onclick={onRefresh}
      title="Re-run diagnose_all"
    >
      <RefreshCcw size={12} class={loading ? 'animate-spin' : ''} />
      Refresh
    </button>
    <button
      type="button"
      class="rounded-md p-1 text-fg-muted hover:bg-surface-2 hover:text-fg"
      aria-label="Close"
      title="Close"
      onclick={onClose}
    >
      <X size={14} />
    </button>
  </header>

  <div class="min-h-0 flex-1 overflow-y-auto">
    <div class="flex flex-col gap-4 p-5 sm:p-6">
      {#if loading}
        <div class="flex items-center gap-2 py-8 text-sm text-fg-muted">
          <LoaderCircle size={14} class="animate-spin" /> Running diagnose_all…
        </div>
      {:else if error}
        <div class="rounded-md border border-rose-900/60 bg-rose-950/40 p-3 text-sm text-rose-200">
          {error}
        </div>
      {:else if report}
        <div class="grid grid-cols-3 gap-3">
          {#each SEVERITY_ORDER as sev (sev)}
            <DiagSeverityTile
              severity={sev}
              count={totals[sev]}
              {filter}
              onToggle={() => toggleFilter(sev)}
            />
          {/each}
        </div>

        {#if totals.error + totals.warning + totals.info === 0}
          <div
            class="rounded-md border border-emerald-500/30 bg-emerald-500/10 p-3 text-sm text-emerald-300"
          >
            All clear — nothing to report.
          </div>
        {/if}

        {#if report.project_issues.length > 0}
          <DiagGroupCard
            title="Project"
            issues={report.project_issues}
            {filter}
            open={!collapsed[PROJECT_KEY]}
            onToggle={() => onToggleGroup(PROJECT_KEY)}
          />
        {/if}

        <div class="flex flex-col gap-2">
          {#each report.groups as group (group.slug)}
            <DiagGroupCard
              title={group.slug}
              subtitle={`${group.memory_count} mem`}
              statusOk={group.manifest_ok}
              statusOkLabel="manifest ok"
              statusBrokenLabel="manifest broken"
              issues={group.issues}
              {filter}
              open={!collapsed[group.slug]}
              onToggle={() => onToggleGroup(group.slug)}
            />
          {/each}
        </div>
      {:else}
        <div class="py-8 text-center text-sm text-fg-muted">
          Click Refresh to run a report.
        </div>
      {/if}
    </div>
  </div>
</section>
