<script lang="ts">
  // GitHub-scale navigation. Thousands of groups make expand-
  // everything-inline unusable — so the Hub variant routes through
  // discrete screens instead: Home → Scope → Group → Memory.
  // Every screen shares a persistent top bar with global search,
  // breadcrumbs, and the app-level chrome tools.
  //
  //   home    — landing. Scope tiles (Global / Shared / Project),
  //             pinned groups, mandatory memories, recent cached
  //             activity. No tree, no long list.
  //   scope   — paginated / filterable list of groups within one
  //             scope. User drills in.
  //   group   — one group's memories, kind/mandatory filtered,
  //             searchable by slug / name / tag.
  //   memory  — the reader + related panel (siblings, refs,
  //             backlinks via cached memories).
  //
  // Global search sweeps every cached frontmatter in one pass and
  // surfaces the top 12 hits regardless of which scope / group
  // they belong to, so the user can leap straight to a memory
  // without remembering where it lives.

  import { marked } from 'marked';
  import {
    ArrowLeft,
    ChevronRight,
    Compass,
    FolderGit2,
    Globe,
    Hash,
    Home,
    Layers,
    LoaderCircle,
    Pin,
    PinOff,
    Search,
    Star,
    X
  } from 'lucide-svelte';
  import ChromeTools from '../ChromeTools.svelte';
  import FeatureBadge from '../FeatureBadge.svelte';
  import FeatureRelations from '../FeatureRelations.svelte';
  import KindBadge from '../KindBadge.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { selectionStore } from '$lib/stores/selection.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import type { GroupEntry, GroupScope, KindStr, MemoryFile } from '$lib/types';

  type Route =
    | { t: 'home' }
    | { t: 'scope'; scope: GroupScope }
    | { t: 'group'; groupId: string }
    | { t: 'memory'; groupId: string; slug: string };

  let route = $state<Route>({ t: 'home' });

  // Hop to the right screen when the shared selection changes
  // from outside (e.g. the user switched variants mid-read). No-ops
  // when we're already on the matching route.
  $effect(() => {
    const gid = selectionStore.groupId;
    const slug = selectionStore.slug;
    if (gid && slug) {
      if (route.t !== 'memory' || route.groupId !== gid || route.slug !== slug) {
        route = { t: 'memory', groupId: gid, slug };
      }
    }
  });

  const SCOPE_META: Record<GroupScope, { label: string; Icon: typeof Globe; tint: string }> = {
    project: {
      label: 'Project',
      Icon: FolderGit2,
      tint: 'bg-kind-feature/10 ring-kind-feature/30'
    },
    shared: {
      label: 'Shared',
      Icon: Layers,
      tint: 'bg-kind-reference/10 ring-kind-reference/30'
    },
    global: {
      label: 'Global',
      Icon: Globe,
      tint: 'bg-kind-rule/10 ring-kind-rule/30'
    }
  };

  // Pre-warm every group's slug list so global search + related
  // resolution can reach into memories the user hasn't clicked
  // into yet. Bodies load lazily per-screen to stay cheap at the
  // thousands-of-groups end of the scale curve.
  $effect(() => {
    for (const g of groupsStore.groups) {
      if (!memoriesStore.slugs[g.group_id] && !memoriesStore.loadingSlugs[g.group_id]) {
        void memoriesStore.loadSlugs(g.group_id);
      }
    }
  });

  // Bodies only load eagerly for the current screen's scope —
  // Home warms pinned + mandatory-candidate groups; Scope warms
  // the visible scope only; Group loads its own; Memory already
  // loads the selected body via the shared store.
  $effect(() => {
    if (route.t === 'home') {
      for (const gid of settingsStore.values.pinned_groups) {
        loadBodiesInGroup(gid);
      }
    } else if (route.t === 'scope') {
      for (const g of groupsByScope(route.scope)) {
        loadBodiesInGroup(g.group_id);
      }
    } else if (route.t === 'group') {
      loadBodiesInGroup(route.groupId);
    } else if (route.t === 'memory') {
      if (
        !memoriesStore.bodyFor(route.groupId, route.slug) &&
        !memoriesStore.isLoadingBody(route.groupId, route.slug)
      ) {
        void memoriesStore.loadBody(route.groupId, route.slug);
      }
    }
  });

  function loadBodiesInGroup(gid: string) {
    const slugs = memoriesStore.slugs[gid];
    if (!slugs) return;
    for (const slug of slugs) {
      if (!memoriesStore.bodyFor(gid, slug) && !memoriesStore.isLoadingBody(gid, slug)) {
        void memoriesStore.loadBody(gid, slug);
      }
    }
  }

  function groupsByScope(scope: GroupScope): GroupEntry[] {
    return groupsStore.groups.filter((g) => g.scope === scope);
  }

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

  // Global search. Scans every cached frontmatter across every
  // group — slug, name, description, tags. Capped at 12 hits so
  // the dropdown stays scannable; the "See more" tail hints at the
  // filtered group screen for deeper digs.
  let globalQuery = $state('');
  let globalOpen = $state(false);

  interface GlobalHit {
    groupId: string;
    slug: string;
    name: string;
    group: GroupEntry;
    body: MemoryFile;
  }

  const globalHits = $derived.by<GlobalHit[]>(() => {
    const q = globalQuery.trim().toLowerCase();
    if (q.length < 2) return [];
    const out: GlobalHit[] = [];
    for (const g of groupsStore.groups) {
      const slugs = memoriesStore.slugs[g.group_id];
      if (!slugs) continue;
      for (const slug of slugs) {
        const body = memoriesStore.bodyFor(g.group_id, slug);
        if (!body) continue;
        const haystacks = [
          slug.toLowerCase(),
          body.frontmatter.name.toLowerCase(),
          body.frontmatter.description.toLowerCase(),
          ...body.frontmatter.tags.map((t) => t.toLowerCase())
        ];
        if (haystacks.some((h) => h.includes(q))) {
          out.push({ groupId: g.group_id, slug, name: body.frontmatter.name, group: g, body });
          if (out.length >= 12) return out;
        }
      }
    }
    return out;
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

  // Home-screen signal: cached memories that surface on the
  // dashboard. Only pulls from already-cached bodies so the home
  // stays fast even with ten-thousand groups in the mirror.
  const mandatoryAcrossMirror = $derived.by<GlobalHit[]>(() => {
    const out: GlobalHit[] = [];
    for (const g of groupsStore.groups) {
      const slugs = memoriesStore.slugs[g.group_id] ?? [];
      for (const slug of slugs) {
        const body = memoriesStore.bodyFor(g.group_id, slug);
        if (!body) continue;
        if (!body.frontmatter.mandatory) continue;
        out.push({ groupId: g.group_id, slug, name: body.frontmatter.name, group: g, body });
      }
    }
    return out;
  });

  // Group-screen state.
  let groupQuery = $state('');
  let groupKindFilter = $state<Set<KindStr>>(new Set());
  let groupMandatoryOnly = $state(false);

  $effect(() => {
    // Reset per-group filters when the active group changes so a
    // stale kind filter from another group doesn't hide the
    // contents of a new one.
    if (route.t === 'group') {
      groupQuery = '';
      groupKindFilter = new Set();
      groupMandatoryOnly = false;
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
    return slugs.map((slug) => ({
      slug,
      body: memoriesStore.bodyFor(gid, slug)
    }));
  });

  const filteredGroupEntries = $derived.by(() => {
    const q = groupQuery.trim().toLowerCase();
    return groupEntries.filter(({ slug, body }) => {
      if (groupMandatoryOnly && body?.frontmatter.mandatory !== true) return false;
      if (groupKindFilter.size > 0) {
        const k = body?.frontmatter.kind as KindStr | undefined;
        if (!k || !groupKindFilter.has(k)) return false;
      }
      if (!q) return true;
      if (slug.toLowerCase().includes(q)) return true;
      if (body && body.frontmatter.name.toLowerCase().includes(q)) return true;
      if (body && body.frontmatter.description.toLowerCase().includes(q)) return true;
      if (body && body.frontmatter.tags.some((t) => t.toLowerCase().includes(q))) return true;
      return false;
    });
  });

  // Scope-screen state.
  let scopeQuery = $state('');
  const scopeEntries = $derived.by(() => {
    if (route.t !== 'scope') return [] as GroupEntry[];
    const q = scopeQuery.trim().toLowerCase();
    return groupsByScope(route.scope).filter((g) => {
      if (!q) return true;
      return (
        g.slug.toLowerCase().includes(q) ||
        (g.display_name?.toLowerCase().includes(q) ?? false)
      );
    });
  });

  // Memory-screen: active memory body + group context + related.
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

  marked.setOptions({ breaks: false, gfm: true });
  const activeHtml = $derived(activeBody ? (marked.parse(activeBody.body) as string) : '');

  const activeSiblings = $derived.by(() => {
    if (route.t !== 'memory') return [] as string[];
    const { groupId, slug: self } = route;
    return (memoriesStore.slugs[groupId] ?? []).filter((s) => s !== self);
  });

  interface ResolvedRef {
    target: string;
    slug: string | null;
    groupId: string | null;
    name: string | null;
  }
  const outgoingRefs = $derived.by<ResolvedRef[]>(() => {
    if (!activeBody) return [];
    return activeBody.frontmatter.refs.map((r) => {
      for (const gid of Object.keys(memoriesStore.slugs)) {
        for (const slug of memoriesStore.slugs[gid] ?? []) {
          const body = memoriesStore.bodyFor(gid, slug);
          if (body && body.frontmatter.id === r.target) {
            return { target: r.target, slug, groupId: gid, name: body.frontmatter.name };
          }
        }
      }
      return { target: r.target, slug: null, groupId: null, name: null };
    });
  });

  const backlinks = $derived.by(() => {
    if (!activeBody?.frontmatter.id) return [] as { groupId: string; slug: string; name: string }[];
    const target = activeBody.frontmatter.id;
    const out: { groupId: string; slug: string; name: string }[] = [];
    for (const gid of Object.keys(memoriesStore.slugs)) {
      for (const slug of memoriesStore.slugs[gid] ?? []) {
        if (
          route.t === 'memory' &&
          gid === route.groupId &&
          slug === route.slug
        ) {
          continue;
        }
        const body = memoriesStore.bodyFor(gid, slug);
        if (!body) continue;
        if (body.frontmatter.refs.some((r) => r.target === target)) {
          out.push({ groupId: gid, slug, name: body.frontmatter.name });
        }
      }
    }
    return out;
  });

  const KINDS: KindStr[] = ['rule', 'snapshot', 'log', 'reference', 'scratch', 'feature'];
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-surface-0 text-fg">
  <!-- Persistent top bar: breadcrumbs + global search + chrome. -->
  <header
    class="flex h-12 shrink-0 items-center gap-3 border-b border-line bg-surface-1 px-4"
  >
    <!-- Home + breadcrumb trail. -->
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
          onclick={() => {
            if (route.t !== 'group' || !activeGroup) return;
            route = { t: 'scope', scope: activeGroup.scope };
          }}
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

    <!-- Global search. -->
    <div class="relative ml-auto w-72">
      <Search
        size={12}
        class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-fg-subtle"
      />
      <input
        type="text"
        class="w-full rounded-md border border-line bg-surface-0 py-1 pl-7 pr-7 text-xs text-fg placeholder:text-fg-subtle focus:border-line-strong focus:outline-none"
        placeholder="Search memories, groups, tags…"
        bind:value={globalQuery}
        onfocus={() => (globalOpen = true)}
        onblur={() => setTimeout(() => (globalOpen = false), 120)}
      />
      {#if globalQuery}
        <button
          type="button"
          class="absolute right-1 top-1/2 -translate-y-1/2 rounded-sm p-0.5 text-fg-subtle hover:bg-surface-2 hover:text-fg"
          onclick={() => {
            globalQuery = '';
            globalOpen = false;
          }}
          aria-label="Clear search"
        >
          <X size={11} />
        </button>
      {/if}
      {#if globalOpen && globalQuery.trim().length >= 2}
        <div
          class="absolute left-0 right-0 top-full z-10 mt-1 max-h-80 overflow-y-auto rounded-md border border-line bg-surface-1 shadow-lg"
        >
          {#if globalHits.length === 0}
            <div class="px-3 py-2 text-xs text-fg-subtle">No matches in cached memories.</div>
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

    <ChromeTools />
  </header>

  <!-- Route-driven body. -->
  <div class="min-h-0 flex-1 overflow-y-auto">
    {#if route.t === 'home'}
      <div class="mx-auto max-w-5xl p-6 sm:p-8">
        <section class="mb-8">
          <h2 class="mb-3 text-sm font-semibold uppercase tracking-wide text-fg-muted">
            Scopes
          </h2>
          <div class="grid gap-3 sm:grid-cols-3">
            {#each (['project', 'shared', 'global'] as GroupScope[]) as s (s)}
              {@const meta = SCOPE_META[s]}
              <button
                type="button"
                class="rounded-lg border border-line p-4 text-left transition-colors hover:border-line-strong hover:bg-surface-1 {meta.tint} ring-1 ring-inset"
                onclick={() => (route = { t: 'scope', scope: s })}
              >
                <div class="flex items-center justify-between">
                  <div class="inline-flex items-center gap-2 text-sm font-semibold text-fg">
                    <meta.Icon size={14} />
                    {meta.label}
                  </div>
                  <span class="text-[11px] text-fg-muted">
                    {scopeCounts[s]} group{scopeCounts[s] === 1 ? '' : 's'}
                  </span>
                </div>
                <p class="mt-1 text-xs text-fg-muted">
                  {#if s === 'project'}
                    Memories scoped to the active project. Usually the bulk of day-to-day
                    reads.
                  {:else if s === 'shared'}
                    Memories shared across a team or working group.
                  {:else}
                    Cross-project globals — coding conventions, mandatory rules, reference docs.
                  {/if}
                </p>
              </button>
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
              No groups pinned yet. Open a group and hit <Pin class="inline" size={11} /> to keep it here.
            </div>
          {:else}
            <ul class="flex flex-col gap-1.5">
              {#each pinnedGroups as g (g.group_id)}
                {@const slugs = memoriesStore.slugs[g.group_id]}
                {@const meta = SCOPE_META[g.scope]}
                <li>
                  <button
                    type="button"
                    class="flex w-full items-center gap-3 rounded-lg border border-line bg-surface-1 p-3 text-left hover:border-line-strong hover:bg-surface-2"
                    onclick={() => gotoGroup(g.group_id)}
                  >
                    <meta.Icon size={13} class="shrink-0 text-fg-muted" />
                    <div class="min-w-0 flex-1">
                      <div class="truncate text-sm text-fg">
                        {g.display_name ?? g.slug}
                      </div>
                      <div class="truncate text-[11px] text-fg-subtle">
                        {meta.label} · {g.slug}
                      </div>
                    </div>
                    <span class="text-[11px] text-fg-subtle">
                      {slugs ? `${slugs.length} memories` : '…'}
                    </span>
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <section>
          <h2 class="mb-3 flex items-center gap-2 text-sm font-semibold uppercase tracking-wide text-fg-muted">
            <Pin size={12} /> Mandatory memories
          </h2>
          {#if mandatoryAcrossMirror.length === 0}
            <div class="text-xs text-fg-subtle">
              None cached yet. Mandatory memories will appear once their groups load.
            </div>
          {:else}
            <ul class="flex flex-col gap-1.5">
              {#each mandatoryAcrossMirror.slice(0, 10) as hit (hit.groupId + ':' + hit.slug)}
                <li>
                  <button
                    type="button"
                    class="flex w-full items-center gap-2 rounded-md border border-line bg-surface-1 p-2 text-left text-xs hover:border-line-strong hover:bg-surface-2"
                    onclick={() => gotoMemory(hit.groupId, hit.slug)}
                  >
                    <Pin size={11} class="shrink-0 text-amber-400" />
                    <KindBadge kind={hit.body.frontmatter.kind} mode="icon" />
                    <div class="min-w-0 flex-1">
                      <div class="truncate text-fg">{hit.body.frontmatter.name}</div>
                      <div class="truncate text-[10px] text-fg-subtle">
                        {SCOPE_META[hit.group.scope].label} · {hit.group.slug}
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
        </section>
      </div>
    {:else if route.t === 'scope'}
      {@const meta = SCOPE_META[route.scope]}
      <div class="mx-auto max-w-4xl p-6 sm:p-8">
        <div class="mb-6 flex items-center gap-3">
          <button
            type="button"
            class="inline-flex items-center gap-1 rounded-md border border-line px-2 py-1 text-xs text-fg hover:bg-surface-2"
            onclick={() => (route = { t: 'home' })}
          >
            <ArrowLeft size={11} /> Home
          </button>
          <meta.Icon size={18} class="text-fg-muted" />
          <h2 class="text-lg font-semibold text-fg">{meta.label} scope</h2>
          <span class="text-xs text-fg-subtle">
            {scopeEntries.length} / {scopeCounts[route.scope]} group{scopeCounts[route.scope] === 1 ? '' : 's'}
          </span>
        </div>

        <div class="relative mb-4">
          <Search
            size={12}
            class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-fg-subtle"
          />
          <input
            type="text"
            class="w-full rounded-md border border-line bg-surface-0 py-1.5 pl-7 pr-7 text-xs text-fg placeholder:text-fg-subtle focus:border-line-strong focus:outline-none"
            placeholder="Filter groups by slug or display name…"
            bind:value={scopeQuery}
          />
          {#if scopeQuery}
            <button
              type="button"
              class="absolute right-1 top-1/2 -translate-y-1/2 rounded-sm p-1 text-fg-subtle hover:bg-surface-2 hover:text-fg"
              onclick={() => (scopeQuery = '')}
            >
              <X size={11} />
            </button>
          {/if}
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
              {@const pinned = settingsStore.isGroupPinned(g.group_id)}
              {@const slugs = memoriesStore.slugs[g.group_id]}
              <li
                class="flex items-center gap-2 rounded-lg border border-line bg-surface-1 px-3 py-2 hover:border-line-strong"
              >
                <button
                  type="button"
                  class="flex min-w-0 flex-1 items-center gap-3 text-left"
                  onclick={() => gotoGroup(g.group_id)}
                >
                  <FolderGit2 size={13} class="shrink-0 text-fg-muted" />
                  <div class="min-w-0 flex-1">
                    <div class="truncate text-sm text-fg">
                      {g.display_name ?? g.slug}
                    </div>
                    <div class="truncate text-[11px] text-fg-subtle">{g.slug}</div>
                  </div>
                  <span class="shrink-0 text-[11px] text-fg-subtle">
                    {slugs ? `${slugs.length} memories` : '…'}
                  </span>
                </button>
                <button
                  type="button"
                  class="rounded-md p-1 text-fg-muted hover:bg-surface-2 hover:text-amber-300"
                  title={pinned ? 'Unpin from home' : 'Pin on home'}
                  aria-label={pinned ? 'Unpin group' : 'Pin group'}
                  onclick={() => settingsStore.togglePinnedGroup(g.group_id)}
                >
                  {#if pinned}
                    <Star size={13} class="fill-amber-300 text-amber-300" />
                  {:else}
                    <Star size={13} />
                  {/if}
                </button>
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

        <div class="mb-3 flex flex-wrap items-center gap-2">
          <div class="relative flex-1 min-w-[200px]">
            <Search
              size={12}
              class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-fg-subtle"
            />
            <input
              type="text"
              class="w-full rounded-md border border-line bg-surface-0 py-1.5 pl-7 pr-7 text-xs text-fg placeholder:text-fg-subtle focus:border-line-strong focus:outline-none"
              placeholder="Filter by slug, name, tag…"
              bind:value={groupQuery}
            />
            {#if groupQuery}
              <button
                type="button"
                class="absolute right-1 top-1/2 -translate-y-1/2 rounded-sm p-1 text-fg-subtle hover:bg-surface-2 hover:text-fg"
                onclick={() => (groupQuery = '')}
              >
                <X size={11} />
              </button>
            {/if}
          </div>
          {#each KINDS as k (k)}
            {@const active = groupKindFilter.has(k)}
            <button
              type="button"
              class="rounded-md transition-opacity {active ? '' : 'opacity-55 hover:opacity-100'}"
              onclick={() => toggleGroupKind(k)}
              aria-pressed={active}
            >
              <KindBadge kind={k} mode="icon_and_text" />
            </button>
          {/each}
          <label
            class="inline-flex cursor-pointer items-center gap-1 rounded-full px-2 py-0.5 text-[11px] ring-1 ring-inset
              {groupMandatoryOnly
              ? 'bg-amber-500/15 text-amber-300 ring-amber-500/40'
              : 'text-fg-muted ring-line-strong hover:bg-surface-2'}"
          >
            <input type="checkbox" class="sr-only" bind:checked={groupMandatoryOnly} />
            <Pin size={10} /> Mandatory
          </label>
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
          <ul class="flex flex-col gap-1">
            {#each filteredGroupEntries as entry (entry.slug)}
              {@const body = entry.body}
              {@const kind = body?.frontmatter.kind as KindStr | undefined}
              <li>
                <button
                  type="button"
                  class="flex w-full items-center gap-2 rounded-md border border-line bg-surface-1 px-3 py-2 text-left hover:border-line-strong hover:bg-surface-2"
                  onclick={() =>
                    route = {
                      t: 'memory',
                      groupId: (activeGroup as GroupEntry).group_id,
                      slug: entry.slug
                    } satisfies Route}
                >
                  {#if kind}
                    <KindBadge {kind} mode="icon" />
                  {/if}
                  <div class="min-w-0 flex-1">
                    <div class="truncate text-sm text-fg">
                      {body?.frontmatter.name ?? entry.slug}
                    </div>
                    <div class="truncate text-[11px] text-fg-subtle">
                      <Hash size={10} class="inline" />
                      {entry.slug}
                    </div>
                  </div>
                  {#if body?.frontmatter.feature}
                    <FeatureBadge
                      status={body.frontmatter.feature.status}
                      number={body.frontmatter.feature.number}
                      label={false}
                    />
                  {/if}
                  {#if body?.frontmatter.mandatory}
                    <Pin size={11} class="shrink-0 text-amber-400" />
                  {/if}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {:else if route.t === 'memory'}
      {#if !activeBody || !activeGroup}
        <div class="m-auto mt-12 flex items-center justify-center gap-2 text-sm text-fg-subtle">
          <LoaderCircle size={14} class="animate-spin" />
          Loading memory…
        </div>
      {:else}
        {@const fm = activeBody.frontmatter}
        <div
          class="mx-auto grid max-w-6xl grid-cols-1 gap-6 p-6 sm:p-8 lg:grid-cols-[1fr_280px]"
        >
          <article>
            <h1 class="text-xl font-semibold text-fg" title={fm.name}>{fm.name}</h1>
            <p class="mt-1 text-sm text-fg-muted">{fm.description}</p>
            <div class="mt-3 flex flex-wrap items-center gap-1.5">
              <KindBadge kind={fm.kind} mode="icon_and_text" />
              {#if fm.feature}
                <FeatureBadge status={fm.feature.status} number={fm.feature.number} />
              {/if}
              {#if fm.mandatory}
                <span
                  class="inline-flex items-center gap-1 rounded-md bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-amber-300 ring-1 ring-inset ring-amber-500/30"
                >
                  <Pin size={10} />
                  mandatory
                </span>
              {/if}
              {#if fm.version}
                <span
                  class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] font-semibold text-fg-muted"
                >
                  v{fm.version}
                </span>
              {/if}
              {#each fm.tags as tag (tag)}
                <span
                  class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] text-fg-muted"
                >
                  #{tag}
                </span>
              {/each}
            </div>
            <div
              class="prose prose-zinc prose-sm mt-6 max-w-none prose-pre:bg-surface-1 prose-pre:ring-1 prose-pre:ring-line prose-headings:tracking-tight"
            >
              {#if activeBody.body.trim()}
                {@html activeHtml}
              {:else}
                <p class="italic text-fg-subtle">(empty body)</p>
              {/if}
            </div>
          </article>

          <aside class="flex flex-col gap-4 text-sm">
            {#if fm.feature}
              <section class="rounded-lg border border-line bg-surface-1/40 p-3">
                <h3
                  class="mb-2 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
                >
                  Feature relations
                </h3>
                <FeatureRelations feature={fm.feature} onNavigate={gotoMemory} />
              </section>
            {/if}
            <section class="rounded-lg border border-line bg-surface-1/40 p-3">
              <h3
                class="mb-2 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
              >
                Siblings
              </h3>
              {#if activeSiblings.length === 0}
                <p class="text-[11px] text-fg-subtle">No other memories in this group.</p>
              {:else}
                <ul class="flex flex-col gap-0.5">
                  {#each activeSiblings.slice(0, 8) as slug (slug)}
                    {@const body =
                      route.t === 'memory'
                        ? memoriesStore.bodyFor(route.groupId, slug)
                        : undefined}
                    <li>
                      <button
                        type="button"
                        class="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs text-fg hover:bg-surface-2"
                        onclick={() =>
                          route.t === 'memory' && gotoMemory(route.groupId, slug)}
                      >
                        {#if body?.frontmatter.kind}
                          <KindBadge kind={body.frontmatter.kind} mode="icon" />
                        {/if}
                        <span class="truncate">{body?.frontmatter.name ?? slug}</span>
                      </button>
                    </li>
                  {/each}
                </ul>
                {#if activeSiblings.length > 8}
                  <button
                    type="button"
                    class="mt-1 text-[11px] text-fg-muted hover:text-fg"
                    onclick={() =>
                      route.t === 'memory' && (route = { t: 'group', groupId: route.groupId })}
                  >
                    See all {activeSiblings.length + 1} in group…
                  </button>
                {/if}
              {/if}
            </section>
            {#if outgoingRefs.length > 0}
              <section class="rounded-lg border border-line bg-surface-1/40 p-3">
                <h3
                  class="mb-2 flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
                >
                  <Compass size={10} /> References
                </h3>
                <ul class="flex flex-col gap-1">
                  {#each outgoingRefs as r (r.target)}
                    <li
                      class="rounded-md border border-line bg-surface-0 px-2 py-1.5 text-xs"
                    >
                      {#if r.slug && r.groupId}
                        <button
                          type="button"
                          class="block w-full truncate text-left text-fg hover:underline"
                          onclick={() => gotoMemory(r.groupId!, r.slug!)}
                          title={r.name ?? r.slug}
                        >
                          {r.slug}
                        </button>
                      {:else}
                        <div class="truncate font-mono text-[10px] text-fg-muted">
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
                        onclick={() => gotoMemory(link.groupId, link.slug)}
                      >
                        <Hash size={10} class="shrink-0 text-fg-subtle" />
                        <span class="truncate">{link.slug}</span>
                      </button>
                    </li>
                  {/each}
                </ul>
              </section>
            {/if}
          </aside>
        </div>
      {/if}
    {/if}
  </div>
</div>
