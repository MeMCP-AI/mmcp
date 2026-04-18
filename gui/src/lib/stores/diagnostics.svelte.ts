import { runDiagnose } from '$lib/api/diagnose';
import type { DiagReport } from '$lib/types';

export type SeverityFilter = 'all' | 'errors' | 'warnings' | 'infos';

class DiagnosticsStore {
  report = $state<DiagReport | null>(null);
  loading = $state(false);
  error = $state<string | null>(null);
  filter = $state<SeverityFilter>('all');
  collapsed = $state<Record<string, boolean>>({});

  async run() {
    this.loading = true;
    this.error = null;
    this.report = null;
    try {
      this.report = await runDiagnose();
    } catch (err) {
      this.error = formatErr(err);
    } finally {
      this.loading = false;
    }
  }

  setFilter(f: SeverityFilter) {
    this.filter = f;
  }

  toggle(groupSlug: string) {
    this.collapsed[groupSlug] = !this.collapsed[groupSlug];
  }

  matches(severity: string): boolean {
    switch (this.filter) {
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
}

export function severityTotals(report: DiagReport | null): {
  errors: number;
  warnings: number;
  infos: number;
} {
  if (!report) return { errors: 0, warnings: 0, infos: 0 };
  const acc = { errors: 0, warnings: 0, infos: 0 };
  const all = [...report.project_issues, ...report.groups.flatMap((g) => g.issues)];
  for (const issue of all) {
    if (issue.severity === 'error') acc.errors++;
    else if (issue.severity === 'warn') acc.warnings++;
    else acc.infos++;
  }
  return acc;
}

function formatErr(err: unknown): string {
  if (err && typeof err === 'object' && 'message' in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

export const diagnosticsStore = new DiagnosticsStore();
