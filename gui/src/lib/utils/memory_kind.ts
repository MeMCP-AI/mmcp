// Memory vs issue classification. Feature-request and issue-tracker
// memories (sister kinds, same underlying tracked-ticket shape) live
// side-by-side with plain knowledge memories in the mmcp mirror;
// every variant wants to surface them separately.
//
// This is also the single source of truth for the kind vocabulary:
// MEMORY_KIND_VALUES mirrors `MemoryKind` in
// crates/mmcp-core/src/memory/kind.rs verbatim (8 variants), and
// `KindStr` is derived from it so every consumer (types.ts and every
// Record<KindStr, ...> site) gets a compile error, not a runtime
// lookup failure, the day a 9th kind is added server-side.

export const MEMORY_KIND_VALUES = [
  'rule',
  'snapshot',
  'log',
  'reference',
  'scratch',
  'feature',
  'issue',
  'milestone'
] as const;

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
