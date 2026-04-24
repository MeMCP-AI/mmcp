<script lang="ts">
  // Right-rail "related" block for a memory: feature relations
  // (depends_on / blocks / superseded_by), outgoing references,
  // incoming backlinks. Centralised so layouts don't re-derive
  // the ref/backlink scan per call site.

  import { Hash } from 'lucide-svelte';
  import FeatureRelations from '../FeatureRelations.svelte';
  import type { MemoryFile } from '$lib/types';
  import { findBacklinks, resolveRef, type ResolvedRef } from '$lib/utils/graph';

  interface Props {
    /** The memory we're building the panel for. Caller decides
     * what scope the panel applies to — usually the viewer's
     * active memory. */
    memory: MemoryFile;
    /** (groupId, slug) currently being viewed, used only to
     * skip self when scanning for backlinks. */
    selfGroupId: string | null;
    selfSlug: string | null;
    onNavigate: (groupId: string, slug: string) => void;
  }

  let { memory, selfGroupId, selfSlug, onNavigate }: Props = $props();

  const fm = $derived(memory.frontmatter);

  const outgoing = $derived.by<ResolvedRef[]>(() => fm.refs.map(r => resolveRef(r.target)));

  const backlinks = $derived.by(() => {
    if (!fm.id) return [];
    return findBacklinks(fm.id, selfGroupId, selfSlug);
  });
</script>

<div class="flex flex-col gap-4 text-sm">
  {#if fm.feature}
    <section class="rounded-lg border border-line bg-surface-1/40 p-3">
      <h3
        class="mb-2 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
      >
        Feature relations
      </h3>
      <FeatureRelations feature={fm.feature} {onNavigate} />
    </section>
  {/if}

  {#if outgoing.length > 0}
    <section class="rounded-lg border border-line bg-surface-1/40 p-3">
      <h3
        class="mb-2 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
      >
        References
      </h3>
      <ul class="flex flex-col gap-1">
        {#each outgoing as r (r.target)}
          <li class="rounded-md border border-line bg-surface-0 px-2 py-1.5 text-xs">
            {#if r.slug && r.groupId}
              <button
                type="button"
                class="block w-full truncate text-left text-fg hover:underline"
                onclick={() => onNavigate(r.groupId!, r.slug!)}
                title={r.body?.frontmatter.name ?? r.slug}
              >
                {r.slug}
              </button>
            {:else}
              <div
                class="truncate font-mono text-[10px] text-fg-muted"
                title={r.target}
              >
                {r.target.slice(0, 8)}…
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if backlinks.length > 0}
    <section class="rounded-lg border border-line bg-surface-1/40 p-3">
      <h3
        class="mb-2 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
      >
        Referenced by
      </h3>
      <ul class="flex flex-col gap-0.5">
        {#each backlinks as link (link.groupId + link.slug)}
          <li>
            <button
              type="button"
              class="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs text-fg hover:bg-surface-2"
              onclick={() => onNavigate(link.groupId, link.slug)}
              title={link.body.frontmatter.name}
            >
              <Hash size={10} class="shrink-0 text-fg-subtle" />
              <span class="truncate">{link.slug}</span>
            </button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if !fm.feature && outgoing.length === 0 && backlinks.length === 0}
    <p class="text-[11px] text-fg-subtle italic">
      No relations cached yet. Open more groups to widen the scan.
    </p>
  {/if}
</div>
