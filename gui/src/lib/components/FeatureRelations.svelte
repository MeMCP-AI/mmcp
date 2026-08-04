<script lang="ts">
  // Dependency / supersession panel for feature-request memories.
  // Renders three typed lists — depends_on, blocks, superseded_by —
  // resolved against whatever the memories store has cached so
  // each UUID becomes a clickable slug when possible.

  import { ArrowDownCircle, ArrowUpCircle, GitMerge } from '@lucide/svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import type { FeatureMetadata, MemoryFile } from '$lib/types';

  interface Props {
    feature: FeatureMetadata;
    /** `(groupId, slug)` → navigate the host layout's viewer to
     * that memory. Caller decides what "navigate" means in its
     * variant (classic/repo set selection, feed opens reader). */
    onNavigate: (groupId: string, slug: string) => void;
  }

  let { feature, onNavigate }: Props = $props();

  interface Resolved {
    target: string;
    slug: string | null;
    groupId: string | null;
    body: MemoryFile | null;
  }

  function resolve(target: string): Resolved {
    for (const gid of Object.keys(memoriesStore.slugs)) {
      for (const slug of memoriesStore.slugs[gid] ?? []) {
        const body = memoriesStore.bodyFor(gid, slug);
        if (body && body.frontmatter.id === target) {
          return { target, slug, groupId: gid, body };
        }
      }
    }
    return { target, slug: null, groupId: null, body: null };
  }

  const depends = $derived(feature.depends_on.map(resolve));
  const blocks = $derived(feature.blocks.map(resolve));
  const supersede = $derived(
    feature.superseded_by ? resolve(feature.superseded_by.target) : null
  );

  function label(r: Resolved): string {
    if (r.body) {
      const n = r.body.frontmatter.feature?.number;
      const prefix = n !== undefined && n !== null ? `FR-${String(n).padStart(3, '0')} · ` : '';
      return `${prefix}${r.slug ?? r.body.frontmatter.name}`;
    }
    return r.target.slice(0, 8) + '…';
  }
</script>

<section class="flex flex-col gap-2 text-xs">
  {#if supersede}
    <div class="rounded-md border border-violet-500/40 bg-violet-500/10 px-2 py-1.5">
      <div
        class="flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide text-violet-300"
      >
        <GitMerge size={10} /> Superseded by
      </div>
      <button
        type="button"
        class="mt-1 block w-full truncate text-left text-fg hover:underline disabled:cursor-default disabled:no-underline"
        disabled={!supersede.slug}
        onclick={() =>
          supersede.groupId && supersede.slug && onNavigate(supersede.groupId, supersede.slug)}
        title={supersede.body?.frontmatter.name ?? supersede.target}
      >
        {label(supersede)}
      </button>
    </div>
  {/if}

  {#if depends.length > 0}
    <div>
      <h4
        class="mb-1 flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
      >
        <ArrowUpCircle size={10} /> Depends on
      </h4>
      <ul class="flex flex-col gap-0.5">
        {#each depends as r (r.target)}
          <li>
            <button
              type="button"
              class="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-fg hover:bg-surface-2 disabled:cursor-default disabled:text-fg-subtle disabled:hover:bg-transparent"
              disabled={!r.slug}
              onclick={() => r.groupId && r.slug && onNavigate(r.groupId, r.slug)}
              title={r.body?.frontmatter.name ?? r.target}
            >
              <span class="truncate">{label(r)}</span>
              {#if r.body?.frontmatter.feature}
                <span class="ml-auto shrink-0 text-[10px] text-fg-subtle">
                  {r.body.frontmatter.feature.status}
                </span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if blocks.length > 0}
    <div>
      <h4
        class="mb-1 flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
      >
        <ArrowDownCircle size={10} /> Blocks
      </h4>
      <ul class="flex flex-col gap-0.5">
        {#each blocks as r (r.target)}
          <li>
            <button
              type="button"
              class="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-fg hover:bg-surface-2 disabled:cursor-default disabled:text-fg-subtle disabled:hover:bg-transparent"
              disabled={!r.slug}
              onclick={() => r.groupId && r.slug && onNavigate(r.groupId, r.slug)}
              title={r.body?.frontmatter.name ?? r.target}
            >
              <span class="truncate">{label(r)}</span>
              {#if r.body?.frontmatter.feature}
                <span class="ml-auto shrink-0 text-[10px] text-fg-subtle">
                  {r.body.frontmatter.feature.status}
                </span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if !supersede && depends.length === 0 && blocks.length === 0}
    <p class="text-fg-subtle italic">No dependencies tracked yet.</p>
  {/if}
</section>
