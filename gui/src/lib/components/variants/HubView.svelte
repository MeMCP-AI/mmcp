<script lang="ts">
  // GitHub-scale navigation: routed screens Home → Scope → Group
  // → Memory. Every screen reaches into shared primitives
  // (MemoryRow, GroupRow, ScopeTile, MemoryReader, RelatedPanel,
  // SearchInput, KindFilterRow, MandatoryToggle); this component
  // owns the routing state + data-loading effects only.

  import {
    ArrowLeft,
    ChevronRight,
    FolderGit2,
    Home,
    LoaderCircle,
    Pin,
    PinOff,
    Star
  } from 'lucide-svelte';
  import FeatureBadge from '../FeatureBadge.svelte';
  import GroupRow from '../primitives/GroupRow.svelte';
  import KindBadge from '../KindBadge.svelte';
  import KindFilterRow from '../primitives/KindFilterRow.svelte';
  import MandatoryToggle from '../primitives/MandatoryToggle.svelte';
  import MemoryReader from '../primitives/MemoryReader.svelte';
  import MemoryRow from '../primitives/MemoryRow.svelte';
  import RelatedPanel from '../primitives/RelatedPanel.svelte';
  import ScopeIcon from '../primitives/ScopeIcon.svelte';
  import ScopeTile from '../primitives/ScopeTile.svelte';
  import SearchInput from '../primitives/SearchInput.svelte';
  import ThemeSelector from '../ThemeSelector.svelte';

  import { settingsStore } from '$lib/stores/settings.svelte';
  import { selectionStore } from '$lib/stores/selection.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import { matchesMemoryFilter } from '$lib/utils/filter';
  import { classifyMemoryKind, type MemoryClass } from '$lib/utils/memory_kind';
  import { SCOPE_META, SCOPE_ORDER } from '$lib/utils/scope';
  import type { GroupEntry, GroupScope, KindStr, MemoryFile } from '$lib/types';

  // ---------------------------------------------------------------
  //  Routing
  // ---------------------------------------------------------------

  type Route =
    | { t: 'home' }
    | { t: 'scope'; scope: GroupScope }
    | { t: 'group'; groupId: string }
    | { t: 'memory'; groupId: string; slug: string };

  let route = $state<Route>({ t: 'home' });

  // Sync with the shared selection so switching variants mid-read
  // resumes on the same memory.
  $effect(() => {
    const gid = selectionStore.groupId;
    const slug = selectionStore.slug;
    if (gid && slug) {
      if (route.t !== 'memory' || route.groupId !== gid || route.slug !== slug) {
        route = { t: 'memory', groupId: gid, slug };
      }
    }
  });

  function gotoMemory(groupId: string, slug: string) {
    if (selectionStore.groupId !== groupId) selectionStore.selectGroup(groupId);
    selectionStore.selectMemory(slug);
    route = { t: 'memory', groupId, slug };
    globalQuery = '';
    globalOpen = false;
  }

  function gotoGroup(groupId: string) {
    if (selectionStore.groupId !== groupId) selectionStore.selectGroup(groupId);
    route = { t: 'group', groupId };
  }

  // ---------------------------------------------------------------
  //  Data loading — pre-warm slug lists for every group; load
  //  bodies eagerly on screens that need them.
  // ---------------------------------------------------------------

  $effect(() => {
    for (const g of groupsStore.groups) {
      if (!memoriesStore.slugs[g.group_id] && !memoriesStore.loadingSlugs[g.group_id]) {
        void memoriesStore.loadSlugs(g.group_id);
      }
    }
  });

  $effect(() => {
    const loadBodies = (gid: string) => {
      const slugs = memoriesStore.slugs[gid];
      if (!slugs) return;
      for (const slug of slugs) {
        if (!memoriesStore.bodyFor(gid, slug) && !memoriesStore.isLoadingBody(gid, slug)) {
          void memoriesStore.loadBody(gid, slug);
        }
      }
    };
    if (route.t === 'home') {
      for (const g of groupsStore.groups) loadBodies(g.group_id);
    } else if (route.t === 'scope') {
      const scope = route.scope;
      for (const g of groupsStore.groups.filter((x) => x.scope === scope)) {
        loadBodies(g.group_id);
      }
    } else if (route.t === 'group') {
      loadBodies(route.groupId);
    } else if (route.t === 'memory') {
      if (
        !memoriesStore.bodyFor(route.groupId, route.slug) &&
        !memoriesStore.isLoadingBody(route.groupId, route.slug)
      ) {
        void memoriesStore.loadBody(route.groupId, route.slug);
      }
    }
  });

  // ---------------------------------------------------------------
  //  Counts
  // ---------------------------------------------------------------

  const scopeCounts = $derived.by(() => {
    const out: Record<GroupScope, number> = { global: 0, shared: 0, project: 0 };
    for (const g of groupsStore.groups) out[g.scope]++;
    return out;
  });

  const pinnedGroups = $derived(
    settingsStore.values.pinned_groups
      .map((gid) => groupsStore.groups.find((g) => g.group_id === gid))
      .filter((g): g is GroupEntry => g !== undefined)
  );

  // ---------------------------------------------------------------
  //  Global search
  // ---------------------------------------------------------------

  let globalQuery = $state('');
  let globalOpen = $state(false);

  interface GlobalHit {
    groupId: string;
    slug: string;
    group: GroupEntry;
    body: MemoryFile;
  }

  const globalHits = $derived.by<GlobalHit[]>(() => {
    const q = globalQuery.trim();
    if (q.length < 2) return [];
    const out: GlobalHit[] = [];
    for (const g of groupsStore.groups) {
      const slugs = memoriesStore.slugs[g.group_id] ?? [];
      for (const slug of slugs) {
        const body = memoriesStore.bodyFor(g.group_id, slug);
        if (!body) continue;
        if (matchesMemoryFilter(slug, body, { query: q })) {
          out.push({ groupId: g.group_id, slug, group: g, body });
          if (out.length >= 12) return out;
        }
      }
    }
    return out;
  });

  // ---------------------------------------------------------------
  //  Home-dashboard signals
  // ---------------------------------------------------------------

  interface ClassifiedEntry {
    groupId: string;
    slug: string;
    group: GroupEntry;
    body: MemoryFile;
  }

  const allCachedEntries = $derived.by<ClassifiedEntry[]>(() => {
    const out: ClassifiedEntry[] = [];
    for (const g of groupsStore.groups) {
      for (const slug of memoriesStore.slugs[g.group_id] ?? []) {
        const body = memoriesStore.bodyFor(g.group_id, slug);
        if (body) out.push({ groupId: g.group_id, slug, group: g, body });
      }
    }
    return out;
  });

  const mandatoryMemories = $derived(
    allCachedEntries.filter((e) => e.body.frontmatter.mandatory)
  );

  const openIssues = $derived(
    allCachedEntries.filter(
      (e) =>
        classifyMemoryKind(e.body.frontmatter.kind) === 'issue' &&
        e.body.frontmatter.feature?.status === 'open'
    )
  );

  // ---------------------------------------------------------------
  //  Group-screen filters
  // ---------------------------------------------------------------

  let groupQuery = $state('');
  let groupKindFilter = $state<Set<KindStr>>(new Set());
  let groupMandatoryOnly = $state(false);
  // Memories and issues are separate categories — the tab always
  // narrows to one or the other, never both.
  let groupClassFilter = $state<MemoryClass>('memory');

  $effect(() => {
    if (route.t === 'group') {
      groupQuery = '';
      groupKindFilter = new Set();
      groupMandatoryOnly = false;
      groupClassFilter = 'memory';
    }
  });

  function toggleGroupKind(k: KindStr) {
    const next = new Set(groupKindFilter);
    if (next.has(k)) next.delete(k);
    else next.add(k);
    groupKindFilter = next;
  }

  const groupEntries = $derived.by(() => {
    if (route.t !== 'group') return [] as { slug: string; body: MemoryFile | undefined }[];
    const gid = route.groupId;
    const slugs = memoriesStore.slugs[gid] ?? [];
    return slugs.map((slug) => ({ slug, body: memoriesStore.bodyFor(gid, slug) }));
  });

  const filteredGroupEntries = $derived.by(() =>
    groupEntries.filter(({ slug, body }) => {
      if (body && classifyMemoryKind(body.frontmatter.kind) !== groupClassFilter) return false;
      return matchesMemoryFilter(slug, body, {
        query: groupQuery,
        kinds: groupKindFilter,
        mandatoryOnly: groupMandatoryOnly
      });
    })
  );

  // ---------------------------------------------------------------
  //  Scope-screen filter
  // ---------------------------------------------------------------

  let scopeQuery = $state('');

  const scopeEntries = $derived.by(() => {
    if (route.t !== 'scope') return [] as GroupEntry[];
    const scope = route.scope;
    const q = scopeQuery.trim().toLowerCase();
    return groupsStore.groups
      .filter((g) => g.scope === scope)
      .filter((g) =>
        q.length === 0
          ? true
          : g.slug.toLowerCase().includes(q) ||
            (g.display_name?.toLowerCase().includes(q) ?? false)
      );
  });

  // ---------------------------------------------------------------
  //  Memory screen data
  // ---------------------------------------------------------------

  const activeBody = $derived(
    route.t === 'memory'
      ? memoriesStore.bodyFor(route.groupId, route.slug)
      : undefined
  );
  const activeGroup = $derived.by(() => {
    if (route.t !== 'memory' && route.t !== 'group') return null;
    const gid = route.groupId;
    return groupsStore.groups.find((g) => g.group_id === gid) ?? null;
  });

  // Memories and issues are separate categories; the tab flips
  // between them, never unifies.
  const CLASS_TABS: { id: MemoryClass; label: string }[] = [
    { id: 'memory', label: 'Memories' },
    { id: 'issue', label: 'Issues' }
  ];
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-surface-0 text-fg">
  <!-- Breadcrumbs + global search -->
  <header
    class="flex h-12 shrink-0 items-center gap-3 border-b border-line bg-surface-1 px-4"
  >
    <nav class="flex min-w-0 items-center gap-1 text-sm">
      <button
        type="button"
        class="inline-flex items-center gap-1 rounded-md px-2 py-1 text-fg-muted hover:bg-surface-2 hover:text-fg"
        onclick={() => (route = { t: 'home' })}
        title="Home"
      >
        <Home size={13} />
        <span class="hidden sm:inline">Home</span>
      </button>
      {#if route.t === 'scope'}
        <ChevronRight size={12} class="text-fg-subtle" />
        <span class="text-fg">{SCOPE_META[route.scope].label}</span>
      {:else if route.t === 'group' && activeGroup}
        <ChevronRight size={12} class="text-fg-subtle" />
        <button
          type="button"
          class="rounded-md px-1.5 py-0.5 text-fg-muted hover:bg-surface-2 hover:text-fg"
          onclick={() =>
            activeGroup && (route = { t: 'scope', scope: activeGroup.scope })}
        >
          {SCOPE_META[activeGroup.scope].label}
        </button>
        <ChevronRight size={12} class="text-fg-subtle" />
        <span class="truncate text-fg" title={activeGroup.slug}>
          {activeGroup.display_name ?? activeGroup.slug}
        </span>
      {:else if route.t === 'memory' && activeGroup}
        <ChevronRight size={12} class="text-fg-subtle" />
        <button
          type="button"
          class="rounded-md px-1.5 py-0.5 text-fg-muted hover:bg-surface-2 hover:text-fg"
          onclick={() =>
            activeGroup && (route = { t: 'scope', scope: activeGroup.scope })}
        >
          {SCOPE_META[activeGroup.scope].label}
        </button>
        <ChevronRight size={12} class="text-fg-subtle" />
        <button
          type="button"
          class="rounded-md px-1.5 py-0.5 text-fg-muted hover:bg-surface-2 hover:text-fg"
          onclick={() =>
            activeGroup && (route = { t: 'group', groupId: activeGroup.group_id })}
          title={activeGroup.slug}
        >
          {activeGroup.display_name ?? activeGroup.slug}
        </button>
        <ChevronRight size={12} class="text-fg-subtle" />
        <code class="truncate font-mono text-[12px] text-fg">{route.slug}</code>
      {/if}
    </nav>

    <div class="ml-auto relative">
      <SearchInput
        value={globalQuery}
        onChange={(v) => (globalQuery = v)}
        placeholder="Search memories, groups, tags…"
        widthClass="w-72"
        onFocus={() => (globalOpen = true)}
        onBlur={() => setTimeout(() => (globalOpen = false), 120)}
      />
      {#if globalOpen && globalQuery.trim().length >= 2}
        <div
          class="absolute left-0 right-0 top-full z-10 mt-1 max-h-80 overflow-y-auto rounded-md border border-line bg-surface-1 shadow-lg"
        >
          {#if globalHits.length === 0}
            <div class="px-3 py-2 text-xs text-fg-subtle">
              No matches in cached memories.
            </div>
          {:else}
            <ul>
              {#each globalHits as hit (hit.groupId + ':' + hit.slug)}
                <li>
                  <button
                    type="button"
                    class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs hover:bg-surface-2"
                    onmousedown={(e) => e.preventDefault()}
                    onclick={() => gotoMemory(hit.groupId, hit.slug)}
                  >
                    <KindBadge kind={hit.body.frontmatter.kind} mode="icon" />
                    <div class="min-w-0 flex-1">
                      <div class="truncate text-fg">{hit.body.frontmatter.name}</div>
                      <div class="truncate text-[10px] text-fg-subtle">
                        {hit.group.slug} / {hit.slug}
                      </div>
                    </div>
                    {#if hit.body.frontmatter.feature}
                      <FeatureBadge
                        status={hit.body.frontmatter.feature.status}
                        number={hit.body.frontmatter.feature.number}
                        label={false}
                      />
                    {/if}
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
    </div>

    <ThemeSelector />
  </header>

  <!-- Routed body -->
  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if route.t === 'home'}
      <div class="mx-auto max-w-5xl p-6 sm:p-8">
        <section class="mb-8">
          <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-fg-muted">
            Scopes
          </h2>
          <div class="grid gap-3 sm:grid-cols-3">
            {#each SCOPE_ORDER as s (s)}
              <ScopeTile
                scope={s}
                groupCount={scopeCounts[s]}
                onOpen={() => (route = { t: 'scope', scope: s })}
              />
            {/each}
          </div>
        </section>

        <section class="mb-8">
          <h2 class="mb-3 flex items-center gap-2 text-sm font-semibold uppercase tracking-wide text-fg-muted">
            <Star size={12} /> Pinned groups
          </h2>
          {#if pinnedGroups.length === 0}
            <div
              class="rounded-lg border border-dashed border-line bg-surface-1/40 p-6 text-center text-xs text-fg-subtle"
            >
              No groups pinned yet. Open a group and hit
              <Pin class="inline" size={11} /> to keep it here.
            </div>
          {:else}
            <ul class="flex flex-col gap-1.5">
              {#each pinnedGroups as g (g.group_id)}
                <li>
                  <GroupRow
                    group={g}
                    memoryCount={memoriesStore.slugs[g.group_id]?.length ?? null}
                    onOpen={() => gotoGroup(g.group_id)}
                  />
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <section class="mb-8">
          <h2
            class="mb-3 flex items-center gap-2 text-sm font-semibold uppercase tracking-wide text-fg-muted"
          >
            <Pin size={12} /> Mandatory memories
          </h2>
          {#if mandatoryMemories.length === 0}
            <div class="text-xs text-fg-subtle">
              None cached yet. Mandatory memories will appear once their groups load.
            </div>
          {:else}
            <ul class="flex flex-col gap-1.5">
              {#each mandatoryMemories.slice(0, 10) as hit (hit.groupId + ':' + hit.slug)}
                <li>
                  <MemoryRow
                    slug={hit.slug}
                    body={hit.body}
                    subtitle={`${SCOPE_META[hit.group.scope].label} · ${hit.group.slug}`}
                    onSelect={() => gotoMemory(hit.groupId, hit.slug)}
                  />
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <section>
          <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-fg-muted">
            Open issues
          </h2>
          {#if openIssues.length === 0}
            <div class="text-xs text-fg-subtle">
              No open feature requests cached. They'll appear as their groups load.
            </div>
          {:else}
            <ul class="flex flex-col gap-1.5">
              {#each openIssues.slice(0, 10) as hit (hit.groupId + ':' + hit.slug)}
                <li>
                  <MemoryRow
                    slug={hit.slug}
                    body={hit.body}
                    subtitle={`${SCOPE_META[hit.group.scope].label} · ${hit.group.slug}`}
                    onSelect={() => gotoMemory(hit.groupId, hit.slug)}
                  />
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      </div>
    {:else if route.t === 'scope'}
      <div class="mx-auto max-w-4xl p-6 sm:p-8">
        <div class="mb-6 flex items-center gap-3">
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-md border border-line px-2 py-1 text-xs text-fg hover:bg-surface-2"
            onclick={() => (route = { t: 'home' })}
          >
            <ArrowLeft size={11} /> Home
          </button>
          <ScopeIcon scope={route.scope} size={18} extraClass="text-fg-muted" />
          <h2 class="text-lg font-semibold text-fg">
            {SCOPE_META[route.scope].label} scope
          </h2>
          <span class="text-xs text-fg-subtle">
            {scopeEntries.length} / {scopeCounts[route.scope]} groups
          </span>
        </div>
        <div class="mb-4">
          <SearchInput
            value={scopeQuery}
            onChange={(v) => (scopeQuery = v)}
            placeholder="Filter groups by slug or display name…"
          />
        </div>
        {#if scopeEntries.length === 0}
          <div
            class="rounded-lg border border-dashed border-line bg-surface-1/40 p-8 text-center text-sm text-fg-subtle"
          >
            No groups match.
          </div>
        {:else}
          <ul class="flex flex-col gap-1.5">
            {#each scopeEntries as g (g.group_id)}
              <li>
                <GroupRow
                  group={g}
                  memoryCount={memoriesStore.slugs[g.group_id]?.length ?? null}
                  pinned={settingsStore.isGroupPinned(g.group_id)}
                  showPinToggle
                  onOpen={() => gotoGroup(g.group_id)}
                  onTogglePin={() => settingsStore.togglePinnedGroup(g.group_id)}
                />
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {:else if route.t === 'group' && activeGroup}
      <div class="mx-auto max-w-4xl p-6 sm:p-8">
        <div class="mb-5 flex flex-wrap items-center gap-3">
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-md border border-line px-2 py-1 text-xs text-fg hover:bg-surface-2"
            onclick={() => {
              if (activeGroup) route = { t: 'scope', scope: activeGroup.scope };
            }}
          >
            <ArrowLeft size={11} /> {SCOPE_META[activeGroup.scope].label}
          </button>
          <FolderGit2 size={18} class="text-fg-muted" />
          <div class="min-w-0 flex-1">
            <h2 class="truncate text-lg font-semibold text-fg" title={activeGroup.slug}>
              {activeGroup.display_name ?? activeGroup.slug}
            </h2>
            <div class="text-[11px] text-fg-subtle">
              {SCOPE_META[activeGroup.scope].label} · {activeGroup.slug}
            </div>
          </div>
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-md border border-line px-2 py-1 text-xs text-fg hover:bg-surface-2"
            onclick={() => settingsStore.togglePinnedGroup(activeGroup!.group_id)}
            title={settingsStore.isGroupPinned(activeGroup.group_id)
              ? 'Unpin from home'
              : 'Pin to home'}
          >
            {#if settingsStore.isGroupPinned(activeGroup.group_id)}
              <PinOff size={11} />
              Unpin
            {:else}
              <Pin size={11} />
              Pin
            {/if}
          </button>
        </div>

        <!-- Memory vs Issue tabs -->
        <nav class="mb-3 flex items-center gap-0.5">
          {#each CLASS_TABS as tab (tab.id)}
            {@const active = groupClassFilter === tab.id}
            <button
              type="button"
              class="inline-flex items-center rounded-md px-2 py-0.5 text-[11px] transition-colors
                {active
                ? 'bg-sky-500/15 text-selected-fg'
                : 'text-fg-muted hover:bg-surface-2 hover:text-fg'}"
              onclick={() => (groupClassFilter = tab.id)}
            >
              {tab.label}
            </button>
          {/each}
        </nav>

        <div class="mb-3 flex flex-wrap items-center gap-2">
          <div class="min-w-[200px] flex-1">
            <SearchInput
              value={groupQuery}
              onChange={(v) => (groupQuery = v)}
              placeholder="Filter by slug, name, tag…"
            />
          </div>
          <KindFilterRow selected={groupKindFilter} onToggle={toggleGroupKind} />
          <MandatoryToggle
            value={groupMandatoryOnly}
            onChange={(v) => (groupMandatoryOnly = v)}
          />
        </div>

        {#if !memoriesStore.slugs[activeGroup.group_id]}
          <div class="flex items-center gap-2 py-6 text-xs text-fg-subtle">
            <LoaderCircle size={12} class="animate-spin" /> Loading…
          </div>
        {:else if filteredGroupEntries.length === 0}
          <div
            class="rounded-lg border border-dashed border-line bg-surface-1/40 p-8 text-center text-sm text-fg-subtle"
          >
            {groupEntries.length === 0
              ? 'No memories in this group yet.'
              : 'No memories match the current filters.'}
          </div>
        {:else}
          <ul class="flex flex-col gap-1.5">
            {#each filteredGroupEntries as entry (entry.slug)}
              <li>
                <MemoryRow
                  slug={entry.slug}
                  body={entry.body}
                  onSelect={() =>
                    (route = {
                      t: 'memory',
                      groupId: (activeGroup as GroupEntry).group_id,
                      slug: entry.slug
                    })}
                />
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {:else if route.t === 'memory'}
      {#if !activeBody}
        <div class="m-auto mt-12 flex items-center justify-center gap-2 text-sm text-fg-subtle">
          <LoaderCircle size={14} class="animate-spin" />
          Loading memory…
        </div>
      {:else}
        <MemoryReader memory={activeBody}>
          {#snippet sidebar()}
            <RelatedPanel
              memory={activeBody}
              selfGroupId={route.t === 'memory' ? route.groupId : null}
              selfSlug={route.t === 'memory' ? route.slug : null}
              onNavigate={gotoMemory}
            />
          {/snippet}
        </MemoryReader>
      {/if}
    {/if}
  </div>
</div>
