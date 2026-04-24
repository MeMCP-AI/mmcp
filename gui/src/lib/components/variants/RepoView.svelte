<script lang="ts">
  // GitHub-style layout. Scope is the top-level — Global / Shared
  // / Project act as three "repo" tabs. Groups expand inline in a
  // tree sidebar to reveal their memories; the viewer sits in the
  // centre; a related panel on the right lists outgoing refs +
  // backlinks + feature relations for the active memory.
  //
  // Composition only — tree row / reader / related panel /
  // filters all come from $lib/components/primitives.

  import {
    ChevronDown,
    ChevronRight,
    FolderGit2,
    Hash,
    LoaderCircle,
    RefreshCw
  } from 'lucide-svelte';
  import FeatureBadge from '../FeatureBadge.svelte';
  import KindBadge from '../KindBadge.svelte';
  import MandatoryPill from '../primitives/MandatoryPill.svelte';
  import MemoryReader from '../primitives/MemoryReader.svelte';
  import RelatedPanel from '../primitives/RelatedPanel.svelte';
  import ScopeIcon from '../primitives/ScopeIcon.svelte';
  import SearchInput from '../primitives/SearchInput.svelte';

  import { selectionStore } from '$lib/stores/selection.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';
  import { reachabilityStore } from '$lib/stores/reachability.svelte';
  import { matchesMemoryFilter } from '$lib/utils/filter';
  import { classifyMemoryKind, type MemoryClass } from '$lib/utils/memory_kind';
  import { SCOPE_META, SCOPE_ORDER } from '$lib/utils/scope';
  import type { GroupEntry, GroupScope, KindStr, MemoryFile } from '$lib/types';

  let scope = $state<GroupScope>('project');

  // Auto-focus a scope with content on mount.
  $effect(() => {
    if (groupsStore.groups.length === 0) return;
    const counts = countByScope(groupsStore.groups);
    if (counts[scope] === 0) {
      const firstWithContent = SCOPE_ORDER.find((s) => counts[s] > 0);
      if (firstWithContent) scope = firstWithContent;
    }
  });

  function countByScope(groups: GroupEntry[]): Record<GroupScope, number> {
    const out: Record<GroupScope, number> = { global: 0, shared: 0, project: 0 };
    for (const g of groups) out[g.scope]++;
    return out;
  }

  const counts = $derived(countByScope(groupsStore.groups));
  const groupsInScope = $derived(groupsStore.groups.filter((g) => g.scope === scope));

  // Tree-expand state per group.
  let expanded = $state<Record<string, boolean>>({});
  $effect(() => {
    if (selectionStore.groupId) expanded[selectionStore.groupId] = true;
  });

  $effect(() => {
    for (const gid of Object.keys(expanded)) {
      if (
        expanded[gid] &&
        !memoriesStore.slugs[gid] &&
        !memoriesStore.loadingSlugs[gid]
      ) {
        void memoriesStore.loadSlugs(gid);
      }
    }
  });

  $effect(() => {
    const gid = selectionStore.groupId;
    const slug = selectionStore.slug;
    if (!gid || !slug) return;
    if (!memoriesStore.bodyFor(gid, slug) && !memoriesStore.isLoadingBody(gid, slug)) {
      void memoriesStore.loadBody(gid, slug);
    }
  });

  $effect(() => {
    for (const gid of Object.keys(expanded)) {
      if (!expanded[gid]) continue;
      const slugs = memoriesStore.slugs[gid];
      if (!slugs) continue;
      for (const s of slugs) {
        if (!memoriesStore.bodyFor(gid, s) && !memoriesStore.isLoadingBody(gid, s)) {
          void memoriesStore.loadBody(gid, s);
        }
      }
    }
  });

  let query = $state('');
  // Memory vs issue split — same toggle pattern as Hub/Feed.
  // Memories and issues are separate categories; the tree
  // narrows to one or the other, never unifies.
  let classFilter = $state<MemoryClass>('memory');

  function matchesTreeEntry(slug: string, groupId: string): boolean {
    const body = memoriesStore.bodyFor(groupId, slug);
    if (body && classifyMemoryKind(body.frontmatter.kind) !== classFilter) return false;
    return matchesMemoryFilter(slug, body, { query });
  }

  const currentBody = $derived(
    selectionStore.groupId && selectionStore.slug
      ? memoriesStore.bodyFor(selectionStore.groupId, selectionStore.slug)
      : undefined
  );
  const currentGroup = $derived(
    selectionStore.groupId
      ? groupsStore.groups.find((g) => g.group_id === selectionStore.groupId) ?? null
      : null
  );

  function pickMemory(groupId: string, slug: string) {
    if (selectionStore.groupId !== groupId) selectionStore.selectGroup(groupId);
    selectionStore.selectMemory(slug);
  }

  const CLASS_TABS: { id: MemoryClass; label: string }[] = [
    { id: 'memory', label: 'Memories' },
    { id: 'issue', label: 'Issues' }
  ];
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-surface-0 text-fg">
  <!-- Repo chrome: scope tabs + quick search + class toggle. -->
  <header class="flex h-11 shrink-0 items-center gap-1 border-b border-line bg-surface-1 px-3">
    <nav class="flex items-center gap-0.5">
      {#each SCOPE_ORDER as s (s)}
        {@const active = scope === s}
        <button
          type="button"
          class="inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors
            {active
            ? 'bg-sky-500/15 text-selected-fg'
            : 'text-fg-muted hover:bg-surface-2 hover:text-fg'}"
          onclick={() => (scope = s)}
          title={`${SCOPE_META[s].label} scope — ${counts[s]} group${counts[s] === 1 ? '' : 's'}`}
        >
          <ScopeIcon scope={s} size={13} />
          <span>{SCOPE_META[s].label}</span>
          <span
            class="rounded-full bg-surface-2 px-1.5 py-0.5 text-[10px] font-semibold text-fg-muted"
          >
            {counts[s]}
          </span>
        </button>
      {/each}
    </nav>

    <nav class="ml-3 flex items-center gap-0.5">
      {#each CLASS_TABS as tab (tab.id)}
        {@const active = classFilter === tab.id}
        <button
          type="button"
          class="inline-flex items-center rounded-md px-2 py-1 text-[11px] transition-colors
            {active
            ? 'bg-sky-500/15 text-selected-fg'
            : 'text-fg-muted hover:bg-surface-2 hover:text-fg'}"
          onclick={() => (classFilter = tab.id)}
        >
          {tab.label}
        </button>
      {/each}
    </nav>

    <div class="ml-auto flex items-center gap-2">
      <SearchInput
        value={query}
        onChange={(v) => (query = v)}
        placeholder="Find a memory…"
        widthClass="w-48"
      />
      <button
        type="button"
        class="inline-flex items-center gap-1 rounded-md border border-line px-2 py-1 text-xs text-fg hover:bg-surface-2 disabled:opacity-40"
        disabled={!reachabilityStore.online || !syncStore.configured || syncStore.inFlight}
        onclick={() => syncStore.pull()}
        title="Sync pull"
      >
        <RefreshCw size={11} class={syncStore.inFlight ? 'animate-spin' : ''} />
        Pull
      </button>
    </div>
  </header>

  <!-- Body: tree | viewer | related -->
  <div class="grid min-h-0 flex-1 grid-cols-[220px_1fr_260px] lg:grid-cols-[260px_1fr_300px]">
    <!-- Tree -->
    <aside class="flex min-h-0 flex-col overflow-hidden border-r border-line bg-surface-1/50">
      <div class="min-h-0 flex-1 overflow-y-auto p-1">
        {#if groupsInScope.length === 0}
          <div class="px-3 py-6 text-center text-xs text-fg-subtle">
            No groups in the {scope} scope yet.
          </div>
        {/if}
        {#each groupsInScope as group (group.group_id)}
          {@const isExpanded = !!expanded[group.group_id]}
          {@const slugs = memoriesStore.slugs[group.group_id]}
          <div class="mb-1">
            <button
              type="button"
              class="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-sm text-fg hover:bg-surface-2"
              onclick={() => (expanded[group.group_id] = !isExpanded)}
              title={group.slug}
            >
              {#if isExpanded}
                <ChevronDown size={12} class="shrink-0 text-fg-subtle" />
              {:else}
                <ChevronRight size={12} class="shrink-0 text-fg-subtle" />
              {/if}
              <FolderGit2 size={12} class="shrink-0 text-fg-muted" />
              <span class="truncate">{group.display_name ?? group.slug}</span>
            </button>
            {#if isExpanded}
              {#if !slugs}
                <div class="flex items-center gap-1.5 px-6 py-1 text-[11px] text-fg-subtle">
                  <LoaderCircle size={10} class="animate-spin" />
                  Loading…
                </div>
              {:else if slugs.length === 0}
                <div class="px-6 py-1 text-[11px] text-fg-subtle">Empty group</div>
              {:else}
                <ul class="flex flex-col pl-4">
                  {#each slugs.filter((s) => matchesTreeEntry(s, group.group_id)) as slug (slug)}
                    {@const active =
                      selectionStore.groupId === group.group_id &&
                      selectionStore.slug === slug}
                    {@const body = memoriesStore.bodyFor(group.group_id, slug)}
                    {@const kind = body?.frontmatter.kind as KindStr | undefined}
                    {@const mandatory = body?.frontmatter.mandatory === true}
                    {@const feature = body?.frontmatter.feature ?? null}
                    <li>
                      <button
                        type="button"
                        class="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs transition-colors
                          {active
                          ? 'bg-sky-500/15 text-selected-fg'
                          : 'text-fg hover:bg-surface-2'}"
                        onclick={() => pickMemory(group.group_id, slug)}
                        title={body?.frontmatter.name
                          ? `${slug} — ${body.frontmatter.name}`
                          : slug}
                      >
                        <Hash size={10} class="shrink-0 text-fg-subtle" />
                        <span class="truncate">{slug}</span>
                        {#if feature}
                          <span class="ml-auto shrink-0">
                            <FeatureBadge
                              status={feature.status}
                              number={feature.number}
                              label={false}
                            />
                          </span>
                        {/if}
                        {#if kind}
                          <span class="{feature ? '' : 'ml-auto'} shrink-0">
                            <KindBadge {kind} mode="icon" />
                          </span>
                        {/if}
                        {#if mandatory}
                          <MandatoryPill label={false} size={10} />
                        {/if}
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
            {/if}
          </div>
        {/each}
      </div>
    </aside>

    <!-- Viewer -->
    <section class="flex min-h-0 flex-col overflow-hidden">
      {#if !currentBody}
        <div class="m-auto flex flex-col items-center gap-2 text-sm text-fg-subtle">
          <span>Pick a memory from the tree on the left.</span>
        </div>
      {:else}
        <header
          class="flex shrink-0 flex-wrap items-start gap-3 border-b border-line bg-surface-1/40 px-6 py-3"
        >
          <div class="flex min-w-0 flex-1 flex-col gap-1">
            <div
              class="flex items-center gap-2 text-[11px] text-fg-subtle"
              title={currentGroup?.slug ?? ''}
            >
              <FolderGit2 size={11} />
              <span>{currentGroup?.display_name ?? currentGroup?.slug ?? '—'}</span>
              <span>/</span>
              <code class="truncate font-mono text-fg-muted">{selectionStore.slug}</code>
            </div>
          </div>
        </header>
        <div class="min-h-0 flex-1 overflow-y-auto">
          <MemoryReader memory={currentBody} />
        </div>
      {/if}
    </section>

    <!-- Related -->
    <aside class="flex min-h-0 flex-col overflow-hidden border-l border-line bg-surface-1/30">
      <div class="min-h-0 flex-1 overflow-y-auto p-3 text-sm">
        {#if !currentBody}
          <div class="text-xs text-fg-subtle">Select a memory to see related items.</div>
        {:else}
          <RelatedPanel
            memory={currentBody}
            selfGroupId={selectionStore.groupId}
            selfSlug={selectionStore.slug}
            onNavigate={pickMemory}
          />
        {/if}
      </div>
    </aside>
  </div>
</div>
