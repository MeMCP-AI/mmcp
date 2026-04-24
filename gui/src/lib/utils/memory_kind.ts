// Memory vs issue classification. Feature-request memories (and
// later on, bug / support tickets — same underlying kind space)
// live side-by-side with plain knowledge memories in the mmcp
// mirror; every variant wants to surface them separately.

import type { KindStr } from '$lib/types';

export type MemoryClass = 'memory' | 'issue';

/// FR kind is the only "issue"-class today. Bugs, support, and
/// other trackable kinds can fold into this switch as they get
/// added to the core enum.
export function classifyMemoryKind(kind: KindStr): MemoryClass {
  switch (kind) {
    case 'feature':
      return 'issue';
    default:
      return 'memory';
  }
}

export const MEMORY_KINDS: KindStr[] = [
  'rule',
  'snapshot',
  'log',
  'reference',
  'scratch'
];

export const ISSUE_KINDS: KindStr[] = ['feature'];
