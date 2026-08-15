// Memory vs issue classification. Feature-request and issue-tracker
// memories (sister kinds, same underlying tracked-ticket shape) live
// side-by-side with plain knowledge memories in the mmcp mirror;
// every variant wants to surface them separately.
//
// `MEMORY_KIND_VALUES` is re-exported from `./memory_kind.generated`, the generated vocabulary.
// `KindStr` derives from it, so a kind added or removed server-side is a compile error here.

import { MEMORY_KIND_VALUES } from './memory_kind.generated';

export { MEMORY_KIND_VALUES };

export type KindStr = (typeof MEMORY_KIND_VALUES)[number];

export type MemoryClass = 'memory' | 'issue';

/// Feature and Issue are today's two tracked-ticket kinds, so both
/// bucket into the "issue" class. Milestone is a rollup container
/// over features, not a raw ticket (no status/depends_on/blocks of
/// its own), so it stays in the generic "memory" bucket until it
/// gets dedicated UI treatment.
export function classifyMemoryKind(kind: KindStr): MemoryClass {
  switch (kind) {
    case 'feature':
    case 'issue':
      return 'issue';
    default:
      return 'memory';
  }
}
