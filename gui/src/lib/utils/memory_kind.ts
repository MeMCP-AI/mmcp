// Memory vs issue classification. Feature-request and issue-tracker
// memories (sister kinds, same underlying tracked-ticket shape) live
// side-by-side with plain knowledge memories in the mmcp mirror;
// every variant wants to surface them separately.
//
// This is the hand-written companion to the generated kind
// vocabulary, not the source of truth itself: MEMORY_KIND_VALUES is
// re-exported from `./memory_kind.generated`, which a Rust test in
// crates/mmcp-core/src/memory/kind.rs (MemoryKind::ALL) asserts
// against on every `cargo test -p mmcp-core` run, so a 9th kind
// added server-side either regenerates this file or fails CI, never
// drifts silently. `KindStr` is derived from that re-export so every
// consumer (types.ts and every Record<KindStr, ...> site) gets a
// compile error, not a runtime lookup failure, the day a kind is
// added or removed.

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
