// Shared diagnostics helpers. Keeps severity parsing, counting, and
// per-severity presentation metadata in one place so every consumer
// (store, panel, primitives) agrees on names, colours, and icons —
// the "inconsistent" UX came from each call site redefining these.

import type { ComponentType } from 'svelte';
import { AlertTriangle, Info, XCircle, type IconProps } from 'lucide-svelte';
import type { DiagReport, GroupReport, Issue } from '$lib/types';

export type Severity = 'error' | 'warning' | 'info';
export type SeverityFilter = 'all' | Severity;

export const SEVERITY_ORDER: Severity[] = ['error', 'warning', 'info'];

/** Coerce anything Rust (or a stale client) might emit into the
 * canonical three-way severity. `'warn'`, `'err'`, and unknown
 * strings fall back cleanly so a single typo server-side doesn't
 * silently drop issues from the UI. */
export function normalizeSeverity(raw: string): Severity {
  const s = raw.toLowerCase();
  if (s === 'error' || s === 'err') return 'error';
  if (s === 'warning' || s === 'warn') return 'warning';
  return 'info';
}

export interface SeverityMeta {
  label: string;
  plural: string;
  tone: 'rose' | 'amber' | 'sky';
  Icon: ComponentType<IconProps>;
  /** Tailwind text/bg/ring tuple so every renderer paints the same
   * pill — no more ad-hoc hex picks. */
  tile: string;
  chip: string;
  accent: string; /* border-l-* colour for the group card */
}

export const SEVERITY_META: Record<Severity, SeverityMeta> = {
  error: {
    label: 'Error',
    plural: 'Errors',
    tone: 'rose',
    Icon: XCircle,
    tile: 'border-rose-500/30 bg-rose-500/10 text-rose-300',
    chip: 'bg-rose-500/15 text-rose-300 ring-rose-500/30',
    accent: 'border-l-rose-500'
  },
  warning: {
    label: 'Warning',
    plural: 'Warnings',
    tone: 'amber',
    Icon: AlertTriangle,
    tile: 'border-amber-500/30 bg-amber-500/10 text-amber-300',
    chip: 'bg-amber-500/15 text-amber-300 ring-amber-500/30',
    accent: 'border-l-amber-500'
  },
  info: {
    label: 'Info',
    plural: 'Infos',
    tone: 'sky',
    Icon: Info,
    tile: 'border-sky-500/30 bg-sky-500/10 text-sky-300',
    chip: 'bg-sky-500/15 text-sky-300 ring-sky-500/30',
    accent: 'border-l-sky-500'
  }
};

export type SeverityCounts = Record<Severity, number>;

export function countBySeverity(issues: Issue[]): SeverityCounts {
  const acc: SeverityCounts = { error: 0, warning: 0, info: 0 };
  for (const i of issues) acc[normalizeSeverity(i.severity)]++;
  return acc;
}

export function reportTotals(report: DiagReport | null): SeverityCounts {
  if (!report) return { error: 0, warning: 0, info: 0 };
  const all = [...report.project_issues, ...report.groups.flatMap((g) => g.issues)];
  return countBySeverity(all);
}

export function matchesFilter(issue: Issue, filter: SeverityFilter): boolean {
  return filter === 'all' || normalizeSeverity(issue.severity) === filter;
}

/** Highest-severity accent a group should paint. Empty group →
 * emerald (clean bill of health), which lives outside the severity
 * scale so it isn't in SEVERITY_META. */
export function accentFor(issues: Issue[]): string {
  const counts = countBySeverity(issues);
  if (counts.error) return SEVERITY_META.error.accent;
  if (counts.warning) return SEVERITY_META.warning.accent;
  if (counts.info) return SEVERITY_META.info.accent;
  return 'border-l-emerald-500';
}

export function filterIssues(issues: Issue[], filter: SeverityFilter): Issue[] {
  if (filter === 'all') return issues;
  return issues.filter((i) => matchesFilter(i, filter));
}

export function groupHasVisibleIssues(group: GroupReport, filter: SeverityFilter): boolean {
  return filterIssues(group.issues, filter).length > 0;
}
