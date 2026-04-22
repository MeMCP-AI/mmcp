<script lang="ts">
  import {
    AlertTriangle,
    ChevronDown,
    ChevronRight,
    Info,
    LoaderCircle,
    RefreshCcw,
    X,
    XCircle
  } from 'lucide-svelte';
  import type { DiagReport, GroupReport, Issue } from '$lib/types';
  import { severityTotals, type SeverityFilter } from '$lib/stores/diagnostics.svelte';

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

  const totals = $derived(severityTotals(report));

  function matchesFilter(severity: string): boolean {
    switch (filter) {
      case 'all':
        return true;
      case 'errors':
        return severity === 'error';
      case 'warnings':
        return severity === 'warn';
      case 'infos':
        return severity !== 'error' && severity !== 'warn';
    }
  }

  function groupAccent(issues: Issue[]): string {
    if (issues.some((i) => i.severity === 'error')) return 'border-l-rose-500';
    if (issues.some((i) => i.severity === 'warn')) return 'border-l-amber-500';
    if (issues.length === 0) return 'border-l-emerald-600';
    return 'border-l-sky-500';
  }

  function countIssues(issues: Issue[]): { err: number; warn: number; info: number } {
    const acc = { err: 0, warn: 0, info: 0 };
    for (const i of issues) {
      if (i.severity === 'error') acc.err++;
      else if (i.severity === 'warn') acc.warn++;
      else acc.info++;
    }
    return acc;
  }

  function issueIcon(severity: string) {
    if (severity === 'error') return { Icon: XCircle, cls: 'text-rose-400' };
    if (severity === 'warn') return { Icon: AlertTriangle, cls: 'text-amber-400' };
    return { Icon: Info, cls: 'text-sky-400' };
  }

  const filters: { value: SeverityFilter; label: string; count: number }[] = $derived([
    { value: 'all', label: 'All', count: totals.errors + totals.warnings + totals.infos },
    { value: 'errors', label: 'Errors', count: totals.errors },
    { value: 'warnings', label: 'Warnings', count: totals.warnings },
    { value: 'infos', label: 'Infos', count: totals.infos }
  ]);

  function visibleIssues(group: GroupReport): Issue[] {
    return group.issues.filter((i) => matchesFilter(i.severity));
  }

  function groupIsVisible(group: GroupReport): boolean {
    if (filter === 'all') return true;
    return group.issues.some((i) => matchesFilter(i.severity)) || !group.manifest_ok;
  }
</script>

<section class="flex h-full min-h-0 flex-col overflow-hidden bg-surface-0 text-fg">
  <header
    class="flex shrink-0 items-center gap-3 border-b border-line bg-surface-1/40 px-4 py-2 sm:px-6"
  >
    <h1 class="text-sm font-semibold text-fg">Diagnostics</h1>
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
          <div class="rounded-lg border border-rose-500/30 bg-rose-500/10 p-3">
            <div class="text-2xl font-semibold text-rose-300">{totals.errors}</div>
            <div class="text-[11px] uppercase tracking-wide text-rose-300/80">Errors</div>
          </div>
          <div class="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3">
            <div class="text-2xl font-semibold text-amber-300">{totals.warnings}</div>
            <div class="text-[11px] uppercase tracking-wide text-amber-300/80">Warnings</div>
          </div>
          <div class="rounded-lg border border-sky-500/30 bg-sky-500/10 p-3">
            <div class="text-2xl font-semibold text-sky-300">{totals.infos}</div>
            <div class="text-[11px] uppercase tracking-wide text-sky-300/80">Infos</div>
          </div>
        </div>

        <div class="flex flex-wrap gap-1.5">
          {#each filters as f (f.value)}
            {@const active = filter === f.value}
            <button
              type="button"
              class="rounded-full px-3 py-1 text-xs font-medium ring-1 ring-inset transition-colors
                {active
                ? 'bg-sky-500/20 text-selected-fg ring-sky-500/50'
                : 'text-fg-muted ring-line-strong hover:bg-surface-2/60'}"
              onclick={() => onFilterChange(f.value)}
            >
              {f.label} ({f.count})
            </button>
          {/each}
        </div>

        {#if report.project_issues.length > 0}
          <section
            class="rounded-lg border border-l-4 border-line {groupAccent(report.project_issues)} bg-surface-1/40 p-3"
          >
            <h3 class="text-sm font-semibold text-fg">Project</h3>
            <div class="mt-2 flex flex-col gap-1.5">
              {#each report.project_issues.filter((i) => matchesFilter(i.severity)) as issue, idx (idx)}
                {@const icon = issueIcon(issue.severity)}
                <div class="flex items-start gap-2 text-sm">
                  <icon.Icon size={14} class="mt-0.5 {icon.cls}" />
                  <span class="text-fg">{issue.message}</span>
                </div>
              {/each}
            </div>
          </section>
        {/if}

        <div class="flex flex-col gap-2">
          {#each report.groups as group (group.slug)}
            {#if groupIsVisible(group)}
              {@const open = !collapsed[group.slug]}
              {@const counts = countIssues(group.issues)}
              <section
                class="rounded-lg border border-l-4 border-line {groupAccent(group.issues)} bg-surface-1/40"
              >
                <header class="flex flex-wrap items-center gap-2 p-3">
                  <button
                    type="button"
                    class="flex items-center gap-2 text-sm font-semibold text-fg"
                    onclick={() => onToggleGroup(group.slug)}
                  >
                    {#if open}
                      <ChevronDown size={14} />
                    {:else}
                      <ChevronRight size={14} />
                    {/if}
                    {group.slug}
                  </button>
                  <span
                    class="ml-2 inline-flex items-center rounded-md px-1.5 py-0.5 text-[10px] font-semibold uppercase ring-1 ring-inset
                      {group.manifest_ok
                      ? 'bg-emerald-500/15 text-emerald-300 ring-emerald-500/30'
                      : 'bg-rose-500/15 text-rose-300 ring-rose-500/30'}"
                  >
                    {group.manifest_ok ? 'manifest ok' : 'manifest broken'}
                  </span>
                  <span
                    class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] text-fg-muted"
                  >
                    {group.memory_count} mem
                  </span>
                  {#if counts.err > 0}
                    <span
                      class="inline-flex items-center gap-1 rounded-md bg-rose-500/15 px-1.5 py-0.5 text-[10px] font-semibold text-rose-300 ring-1 ring-inset ring-rose-500/30"
                    >
                      <XCircle size={10} />
                      {counts.err}
                    </span>
                  {/if}
                  {#if counts.warn > 0}
                    <span
                      class="inline-flex items-center gap-1 rounded-md bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold text-amber-300 ring-1 ring-inset ring-amber-500/30"
                    >
                      <AlertTriangle size={10} />
                      {counts.warn}
                    </span>
                  {/if}
                  {#if counts.info > 0}
                    <span
                      class="inline-flex items-center gap-1 rounded-md bg-sky-500/15 px-1.5 py-0.5 text-[10px] font-semibold text-sky-300 ring-1 ring-inset ring-sky-500/30"
                    >
                      <Info size={10} />
                      {counts.info}
                    </span>
                  {/if}
                </header>
                {#if open}
                  <div class="flex flex-col gap-1.5 px-5 pb-3">
                    {#each visibleIssues(group) as issue, idx (idx)}
                      {@const icon = issueIcon(issue.severity)}
                      <div class="flex items-start gap-2 text-sm">
                        <icon.Icon size={14} class="mt-0.5 {icon.cls}" />
                        {#if issue.slug}
                          <code
                            class="rounded bg-surface-2 px-1 py-0.5 text-[11px] text-fg-muted"
                          >
                            {issue.slug}
                          </code>
                        {/if}
                        <span class="text-fg">{issue.message}</span>
                      </div>
                    {:else}
                      <span class="text-xs text-fg-subtle">(no issues at current filter)</span>
                    {/each}
                  </div>
                {/if}
              </section>
            {/if}
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
