import { runDiagnose } from '$lib/api/diagnose';
import type { DiagReport } from '$lib/types';
import { formatErr } from '$lib/utils/error';
import type { SeverityFilter } from '$lib/utils/diag';

export type { SeverityFilter } from '$lib/utils/diag';

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
}

export const diagnosticsStore = new DiagnosticsStore();
