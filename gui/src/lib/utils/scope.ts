// Scope metadata lookup — labels + tint classes for every variant
// that buckets groups by scope. Keeping the single source of
// truth here means a rename / tint tweak lands everywhere at once.

import type { GroupScope } from '$lib/types';

export interface ScopeMeta {
  label: string;
  description: string;
  /** Tailwind-utility ring + bg tint used on scope tiles. */
  tileTint: string;
}

export const SCOPE_META: Record<GroupScope, ScopeMeta> = {
  project: {
    label: 'Project',
    description:
      "Memories scoped to the active project. Usually the bulk of day-to-day reads.",
    tileTint: 'bg-kind-feature/10 ring-kind-feature/30'
  },
  shared: {
    label: 'Shared',
    description: 'Memories shared across a team or working group.',
    tileTint: 'bg-kind-reference/10 ring-kind-reference/30'
  },
  global: {
    label: 'Global',
    description:
      'Cross-project globals — coding conventions, mandatory rules, reference docs.',
    tileTint: 'bg-kind-rule/10 ring-kind-rule/30'
  }
};

export const SCOPE_ORDER: GroupScope[] = ['project', 'shared', 'global'];
