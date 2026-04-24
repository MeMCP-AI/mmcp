<script lang="ts">
  // GitHub-style layout. Scope becomes the top-level concept:
  // Global / Shared / Project are the three "repos" the user
  // navigates between via a tab bar. Inside a repo, groups expand
  // inline to reveal their memories — tree nav. The viewer sits in
  // the centre; a related-memories panel on the right lists
  // siblings, outgoing refs, and naive backlinks.
  //
  // Interactions push through the same stores the classic layout
  // uses, so selection state persists when the user flips variants
  // via the switcher.

  import { marked } from 'marked';
  import {
    ChevronDown,
    ChevronRight,
    FolderGit2,
    GitCompare,
    Globe,
    Hash,
    Layers,
    LoaderCircle,
    Pin,
    RefreshCw,
    Search,
    X
  } from 'lucide-svelte';
  import FeatureBadge from '../FeatureBadge.svelte';
  import FeatureRelations from '../FeatureRelations.svelte';
  import KindBadge from '../KindBadge.svelte';
  import { selectionStore } from '$lib/stores/selection.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';
  import { reachabilityStore } from '$lib/stores/reachability.svelte';
  import type { GroupEntry, GroupScope, KindStr, MemoryFile } from '$lib/types';

  type ScopeId = GroupScope;

  let scope = $state<ScopeId>('project');

  // Auto-focus a scope that actually has groups on first mount, so
  // the user isn't greeted with an empty Global tab when everything
  // they have is in Project.
  $effect(() => {
    if (groupsStore.groups.length === 0) return;
    const counts = countByScope(groupsStore.groups);
    if (counts[scope] === 0) {
      const firstWithContent = (['project', 'shared', 'global'] as ScopeId[]).find(
        (s) => counts[s] > 0
      );
      if (firstWithContent) scope = firstWithContent;
    }
  });

  function countByScope(groups: GroupEntry[]): Record<ScopeId, number> {
    const out: Record<ScopeId, number> = { global: 0, shared: 0, project: 0 };
    for (const g of groups) out[g.scope]++;
    return out;
  }

  const counts = $derived(countByScope(groupsStore.groups));

  const groupsInScope = $derived(
    groupsStore.groups.filter((g) => g.scope === scope)
  );

  // Expanded state for each group's tree node. Default-collapses
  // groups the user hasn't interacted with to keep the sidebar
  // scannable.
  let expanded = $state<Record<string, boolean>>({});
  $effect(() => {
    // Auto-expand the active selection so the list row is visible.
    if (selectionStore.groupId) expanded[selectionStore.groupId] = true;
  });

  $effect(() => {
    // Lazy-load slug lists once a group is expanded.
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

  // Keep the viewer body loaded for the current selection.
  $effect(() => {
    const gid = selectionStore.groupId;
    const slug = selectionStore.slug;
    if (!gid || !slug) return;
    if (!memoriesStore.bodyFor(gid, slug) && !memoriesStore.isLoadingBody(gid, slug)) {
      void memoriesStore.loadBody(gid, slug);
    }
  });

  // Eagerly pre-load every slug body in groups the user has
  // expanded, so the tree surfaces mandatory pins and the related
  // panel can resolve refs / backlinks without awaiting click.
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

  function matchesQuery(slug: string, groupId: string, q: string): boolean {
    if (!q) return true;
    const needle = q.toLowerCase();
    if (slug.toLowerCase().includes(needle)) return true;
    const body = memoriesStore.bodyFor(groupId, slug);
    if (!body) return false;
    if (body.frontmatter.name.toLowerCase().includes(needle)) return true;
    return body.frontmatter.tags.some((t) => t.toLowerCase().includes(needle));
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

  marked.setOptions({ breaks: false, gfm: true });
  const previewHtml = $derived(
    currentBody ? (marked.parse(currentBody.body) as string) : ''
  );

  // Outgoing refs — the frontmatter `refs` list pins target UUIDs +
  // commits. We resolve each target by scanning every cached body
  // across every group for a matching id, so the chip can render a
  // human-friendly slug instead of a raw UUID when possible.
  interface ResolvedRef {
    target: string;
    commit: string;
    slug: string | null;
    groupId: string | null;
    name: string | null;
  }
  const outgoingRefs = $derived.by<ResolvedRef[]>(() => {
    if (!currentBody) return [];
    return currentBody.frontmatter.refs.map((r) => {
      for (const gid of Object.keys(memoriesStore.slugs)) {
        for (const slug of memoriesStore.slugs[gid] ?? []) {
          const body = memoriesStore.bodyFor(gid, slug);
          if (body && body.frontmatter.id === r.target) {
            return {
              target: r.target,
              commit: r.commit,
              slug,
              groupId: gid,
              name: body.frontmatter.name
            };
          }
        }
      }
      return { target: r.target, commit: r.commit, slug: null, groupId: null, name: null };
    });
  });

  // Backlinks — naive scan of every cached body for refs pointing
  // at the current memory's id. Only cached bodies participate, so
  // the list is "best-effort" until every group has been expanded.
  interface Backlink {
    groupId: string;
    slug: string;
    name: string;
  }
  const backlinks = $derived.by<Backlink[]>(() => {
    if (!currentBody?.frontmatter.id) return [];
    const target = currentBody.frontmatter.id;
    const out: Backlink[] = [];
    for (const gid of Object.keys(memoriesStore.slugs)) {
      for (const slug of memoriesStore.slugs[gid] ?? []) {
        if (
          gid === selectionStore.groupId &&
          slug === selectionStore.slug
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

  const SCOPE_META: { id: ScopeId; label: string; Icon: typeof Globe }[] = [
    { id: 'project', label: 'Project', Icon: FolderGit2 },
    { id: 'shared', label: 'Shared', Icon: Layers },
    { id: 'global', label: 'Global', Icon: Globe }
  ];

  function pickMemory(groupId: string, slug: string) {
    if (selectionStore.groupId !== groupId) selectionStore.selectGroup(groupId);
    selectionStore.selectMemory(slug);
  }
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-surface-0 text-fg">
  <!-- Repo chrome: scope tabs + quick search + variant switch. -->
  <header
    class="flex h-11 shrink-0 items-center gap-1 border-b border-line bg-surface-1 px-3"
  >
    <nav class="flex items-center gap-0.5">
      {#each SCOPE_META as s (s.id)}
        {@const active = scope === s.id}
        <button
          type="button"
          class="inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors
            {active
            ? 'bg-sky-500/15 text-selected-fg'
            : 'text-fg-muted hover:bg-surface-2 hover:text-fg'}"
          onclick={() => (scope = s.id)}
          title={`${s.label} scope — ${counts[s.id]} group${counts[s.id] === 1 ? '' : 's'}`}
        >
          <s.Icon size={13} />
          <span>{s.label}</span>
          <span
            class="rounded-full bg-surface-2 px-1.5 py-0.5 text-[10px] font-semibold text-fg-muted"
          >
            {counts[s.id]}
          </span>
        </button>
      {/each}
    </nav>

    <div class="ml-auto flex items-center gap-2">
      <div class="relative">
        <Search
          size={12}
          class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-fg-subtle"
        />
        <input
          type="text"
          placeholder="Find a memory…"
          class="w-48 rounded-md border border-line bg-surface-0 py-1 pl-7 pr-6 text-xs text-fg placeholder:text-fg-subtle focus:border-line-strong focus:outline-none"
          bind:value={query}
        />
        {#if query}
          <button
            type="button"
            class="absolute right-1 top-1/2 -translate-y-1/2 rounded-sm p-0.5 text-fg-subtle hover:bg-surface-2 hover:text-fg"
            aria-label="Clear search"
            onclick={() => (query = '')}
          >
            <X size={11} />
          </button>
        {/if}
      </div>
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

  <!-- Body: tree | viewer | related. -->
  <div class="grid min-h-0 flex-1 grid-cols-[220px_1fr_260px] lg:grid-cols-[260px_1fr_300px]">
    <!-- Tree sidebar. -->
    <aside
      class="flex min-h-0 flex-col overflow-hidden border-r border-line bg-surface-1/50"
    >
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
                  {#each slugs.filter((s) => matchesQuery(s, group.group_id, query.trim())) as slug (slug)}
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
                          <Pin size={10} class="shrink-0 text-amber-400" />
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

    <!-- Viewer. -->
    <section class="flex min-h-0 flex-col overflow-hidden">
      {#if !currentBody}
        <div class="m-auto flex flex-col items-center gap-2 text-sm text-fg-subtle">
          <span>Pick a memory from the tree on the left.</span>
        </div>
      {:else}
        {@const fm = currentBody.frontmatter}
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
            <h1 class="truncate text-lg font-semibold text-fg" title={fm.name}>
              {fm.name}
            </h1>
            <p class="truncate text-xs text-fg-muted" title={fm.description}>
              {fm.description}
            </p>
          </div>
          <div class="flex flex-wrap items-center gap-1.5">
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
          </div>
        </header>
        <div class="min-h-0 flex-1 overflow-y-auto">
          <article class="mx-auto max-w-3xl px-6 py-6">
            <div
              class="prose prose-zinc prose-sm max-w-none prose-pre:bg-surface-1 prose-pre:ring-1 prose-pre:ring-line prose-headings:tracking-tight"
            >
              {#if currentBody.body.trim()}
                {@html previewHtml}
              {:else}
                <p class="italic text-fg-subtle">(empty body)</p>
              {/if}
            </div>
            {#if fm.tags.length > 0}
              <div class="mt-6 flex flex-wrap items-center gap-1.5">
                {#each fm.tags as tag (tag)}
                  <span
                    class="inline-flex items-center rounded-md bg-surface-2 px-1.5 py-0.5 text-[10px] text-fg-muted"
                  >
                    #{tag}
                  </span>
                {/each}
              </div>
            {/if}
          </article>
        </div>
      {/if}
    </section>

    <!-- Related panel. -->
    <aside
      class="flex min-h-0 flex-col overflow-hidden border-l border-line bg-surface-1/30"
    >
      <div class="min-h-0 flex-1 overflow-y-auto p-3 text-sm">
        {#if !currentBody}
          <div class="text-xs text-fg-subtle">Select a memory to see related items.</div>
        {:else}
          {#if currentBody.frontmatter.feature}
            <section class="mb-4">
              <h3
                class="mb-1.5 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
              >
                Feature relations
              </h3>
              <FeatureRelations
                feature={currentBody.frontmatter.feature}
                onNavigate={pickMemory}
              />
            </section>
          {/if}

          <section class="mb-4">
            <h3
              class="mb-1.5 flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
            >
              <GitCompare size={10} /> References
            </h3>
            {#if outgoingRefs.length === 0}
              <p class="text-[11px] text-fg-subtle">This memory references no others.</p>
            {:else}
              <ul class="flex flex-col gap-1">
                {#each outgoingRefs as ref (ref.target + ref.commit)}
                  <li
                    class="rounded-md border border-line bg-surface-0 px-2 py-1.5 text-xs"
                  >
                    {#if ref.slug && ref.groupId}
                      <button
                        type="button"
                        class="block w-full truncate text-left text-fg hover:underline"
                        onclick={() => pickMemory(ref.groupId!, ref.slug!)}
                        title={ref.name ?? ref.slug}
                      >
                        {ref.slug}
                      </button>
                    {:else}
                      <div class="truncate font-mono text-[10px] text-fg-muted" title={ref.target}>
                        {ref.target.slice(0, 8)}…
                      </div>
                    {/if}
                    <div
                      class="truncate font-mono text-[10px] text-fg-subtle"
                      title={ref.commit}
                    >
                      @{ref.commit.slice(0, 7)}
                    </div>
                  </li>
                {/each}
              </ul>
            {/if}
          </section>

          <section>
            <h3 class="mb-1.5 text-[10px] font-semibold uppercase tracking-wide text-fg-muted">
              Referenced by
            </h3>
            {#if backlinks.length === 0}
              <p class="text-[11px] text-fg-subtle">
                No cached memory references this one. Expand more groups on the left to
                widen the scan.
              </p>
            {:else}
              <ul class="flex flex-col gap-0.5">
                {#each backlinks as link (link.groupId + link.slug)}
                  <li>
                    <button
                      type="button"
                      class="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs text-fg hover:bg-surface-2"
                      onclick={() => pickMemory(link.groupId, link.slug)}
                      title={link.name}
                    >
                      <Hash size={10} class="shrink-0 text-fg-subtle" />
                      <span class="truncate">{link.slug}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          </section>
        {/if}
      </div>
    </aside>
  </div>
</div>
