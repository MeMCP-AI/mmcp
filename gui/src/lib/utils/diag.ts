// Shared diagnostics helpers. Keeps severity parsing, counting, and
// per-severity presentation metadata in one place so every consumer
// (store, panel, primitives) agrees on names, colours, and icons —
// the "inconsistent" UX came from each call site redefining these.

import { AlertTriangle, Info, XCircle } from '@lucide/svelte';
import type { DiagReport, Finding } from '$lib/types';

export type Severity = 'error' | 'warning' | 'info';
export type SeverityFilter = 'all' | Severity;

export const SEVERITY_ORDER: Severity[] = ['error', 'warning', 'info'];

/** Coerce anything Rust (or a stale client) might emit into the
 * canonical three-way severity. `'warn'`, `'err'`, and unknown
 * strings fall back cleanly so a single typo server-side doesn't
 * silently drop findings from the UI. */
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
  Icon: typeof XCircle;
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
    tile: 'border-rose-500/40 bg-rose-500/15 text-sev-error',
    chip: 'bg-rose-500/20 text-sev-error ring-rose-500/40',
    accent: 'border-l-rose-500'
  },
  warning: {
    label: 'Warning',
    plural: 'Warnings',
    tone: 'amber',
    Icon: AlertTriangle,
    tile: 'border-amber-500/40 bg-amber-500/15 text-sev-warning',
    chip: 'bg-amber-500/20 text-sev-warning ring-amber-500/40',
    accent: 'border-l-amber-500'
  },
  info: {
    label: 'Info',
    plural: 'Infos',
    tone: 'sky',
    Icon: Info,
    tile: 'border-sky-500/40 bg-sky-500/15 text-sev-info',
    chip: 'bg-sky-500/20 text-sev-info ring-sky-500/40',
    accent: 'border-l-sky-500'
  }
};

export type SeverityCounts = Record<Severity, number>;

export function countBySeverity(findings: Finding[]): SeverityCounts {
  const acc: SeverityCounts = { error: 0, warning: 0, info: 0 };
  for (const f of findings) acc[normalizeSeverity(f.severity)]++;
  return acc;
}

export function reportTotals(report: DiagReport | null): SeverityCounts {
  if (!report) return { error: 0, warning: 0, info: 0 };
  const all = [...report.project_findings, ...report.groups.flatMap((g) => g.findings)];
  return countBySeverity(all);
}

export function matchesFilter(finding: Finding, filter: SeverityFilter): boolean {
  return filter === 'all' || normalizeSeverity(finding.severity) === filter;
}

/** Highest-severity accent a group should paint. Empty group →
 * emerald (clean bill of health), which lives outside the severity
 * scale so it isn't in SEVERITY_META. */
export function accentFor(findings: Finding[]): string {
  const counts = countBySeverity(findings);
  if (counts.error) return SEVERITY_META.error.accent;
  if (counts.warning) return SEVERITY_META.warning.accent;
  if (counts.info) return SEVERITY_META.info.accent;
  return 'border-l-emerald-500';
}

export function filterFindings(findings: Finding[], filter: SeverityFilter): Finding[] {
  if (filter === 'all') return findings;
  return findings.filter((f) => matchesFilter(f, filter));
}
