<script lang="ts">
  // Minimalist feed. Groups blur into the background; memories
  // scroll as a flat, filterable list of cards regardless of which
  // repo they belong to. A card click opens a full reader overlay
  // so the feed doesn't fight with the content for attention.
  //
  // Shares stores with the other variants — switching away and
  // back keeps the selection so the user can resume reading.

  import { marked } from 'marked';
  import {
    ChevronLeft,
    FolderGit2,
    Globe,
    Layers,
    LoaderCircle,
    Pin,
    Search,
    X
  } from 'lucide-svelte';
  import ChromeTools from '../ChromeTools.svelte';
  import FeatureBadge from '../FeatureBadge.svelte';
  import FeatureRelations from '../FeatureRelations.svelte';
  import KindBadge from '../KindBadge.svelte';
  import { selectionStore } from '$lib/stores/selection.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import type { GroupEntry, GroupScope, KindStr, MemoryFile } from '$lib/types';

  const KIND_ACCENT: Record<KindStr, string> = {
    rule: 'border-l-kind-rule',
    snapshot: 'border-l-kind-snapshot',
    log: 'border-l-kind-log',
    reference: 'border-l-kind-reference',
    scratch: 'border-l-kind-scratch',
    feature: 'border-l-kind-feature'
  };

  const SCOPE_META: Record<GroupScope, { label: string; Icon: typeof Globe }> = {
    project: { label: 'Project', Icon: FolderGit2 },
    shared: { label: 'Shared', Icon: Layers },
    global: { label: 'Global', Icon: Globe }
  };

  // Filter chips: scope + kind. Empty set = match all.
  let scopeFilter = $state<Set<GroupScope>>(new Set());
  let kindFilter = $state<Set<KindStr>>(new Set());
  let mandatoryOnly = $state(false);
  let query = $state('');

  // Load every group's slug list + bodies up front so the feed is
  // populated without the user having to click into groups first.
  // Memory bodies are small and the store caches per-pair, so this
  // is cheap.
  $effect(() => {
    for (const g of groupsStore.groups) {
      if (!memoriesStore.slugs[g.group_id] && !memoriesStore.loadingSlugs[g.group_id]) {
        void memoriesStore.loadSlugs(g.group_id);
      }
    }
  });

  $effect(() => {
    for (const g of groupsStore.groups) {
      const slugs = memoriesStore.slugs[g.group_id];
      if (!slugs) continue;
      for (const slug of slugs) {
        if (
          !memoriesStore.bodyFor(g.group_id, slug) &&
          !memoriesStore.isLoadingBody(g.group_id, slug)
        ) {
          void memoriesStore.loadBody(g.group_id, slug);
        }
      }
    }
  });

  interface Entry {
    groupId: string;
    group: GroupEntry;
    slug: string;
    body: MemoryFile;
  }

  const entries = $derived.by<Entry[]>(() => {
    const out: Entry[] = [];
    for (const group of groupsStore.groups) {
      const slugs = memoriesStore.slugs[group.group_id] ?? [];
      for (const slug of slugs) {
        const body = memoriesStore.bodyFor(group.group_id, slug);
        if (!body) continue;
        out.push({ groupId: group.group_id, group, slug, body });
      }
    }
    return out;
  });

  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return entries.filter(({ group, slug, body }) => {
      if (scopeFilter.size > 0 && !scopeFilter.has(group.scope)) return false;
      if (kindFilter.size > 0 && !kindFilter.has(body.frontmatter.kind)) return false;
      if (mandatoryOnly && !body.frontmatter.mandatory) return false;
      if (!q) return true;
      if (slug.toLowerCase().includes(q)) return true;
      if (body.frontmatter.name.toLowerCase().includes(q)) return true;
      if (body.frontmatter.description.toLowerCase().includes(q)) return true;
      return body.frontmatter.tags.some((t) => t.toLowerCase().includes(q));
    });
  });

  function toggleScope(s: GroupScope) {
    const next = new Set(scopeFilter);
    if (next.has(s)) next.delete(s);
    else next.add(s);
    scopeFilter = next;
  }

  function toggleKind(k: KindStr) {
    const next = new Set(kindFilter);
    if (next.has(k)) next.delete(k);
    else next.add(k);
    kindFilter = next;
  }

  const KINDS: KindStr[] = ['rule', 'snapshot', 'log', 'reference', 'scratch', 'feature'];
  const SCOPES: GroupScope[] = ['project', 'shared', 'global'];

  function openReader(groupId: string, slug: string) {
    if (selectionStore.groupId !== groupId) selectionStore.selectGroup(groupId);
    selectionStore.selectMemory(slug);
  }
  function closeReader() {
    selectionStore.clearMemory();
  }

  const readerBody = $derived(
    selectionStore.groupId && selectionStore.slug
      ? memoriesStore.bodyFor(selectionStore.groupId, selectionStore.slug)
      : undefined
  );
  const readerGroup = $derived(
    selectionStore.groupId
      ? groupsStore.groups.find((g) => g.group_id === selectionStore.groupId) ?? null
      : null
  );

  marked.setOptions({ breaks: false, gfm: true });
  const readerHtml = $derived(
    readerBody ? (marked.parse(readerBody.body) as string) : ''
  );

  const anyLoading = $derived(
    Object.values(memoriesStore.loadingSlugs).some(Boolean) ||
      Object.values(memoriesStore.loadingBody).some(Boolean)
  );
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-surface-0 text-fg">
  <header
    class="flex h-11 shrink-0 items-center gap-2 border-b border-line bg-surface-1 px-4"
  >
    <h1 class="text-sm font-semibold tracking-tight">Feed</h1>
    <span class="text-[11px] text-fg-subtle">
      {filtered.length} / {entries.length}
    </span>
    {#if anyLoading}
      <LoaderCircle size={12} class="animate-spin text-fg-subtle" />
    {/if}
    <div class="relative ml-auto">
      <Search
        size={12}
        class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-fg-subtle"
      />
      <input
        type="text"
        placeholder="Search memories…"
        class="w-64 rounded-md border border-line bg-surface-0 py-1 pl-7 pr-6 text-xs text-fg placeholder:text-fg-subtle focus:border-line-strong focus:outline-none"
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
    <ChromeTools />
  </header>

  <div class="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-line bg-surface-1/40 px-4 py-2">
    <span class="text-[10px] font-semibold uppercase tracking-wide text-fg-subtle">
      Scopes
    </span>
    {#each SCOPES as s (s)}
      {@const active = scopeFilter.has(s)}
      {@const meta = SCOPE_META[s]}
      <button
        type="button"
        class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] ring-1 ring-inset transition-colors
          {active
          ? 'bg-sky-500/15 text-selected-fg ring-sky-500/40'
          : 'text-fg-muted ring-line-strong hover:bg-surface-2'}"
        onclick={() => toggleScope(s)}
      >
        <meta.Icon size={10} />
        {meta.label}
      </button>
    {/each}
    <span class="mx-2 h-4 w-px bg-line"></span>
    <span class="text-[10px] font-semibold uppercase tracking-wide text-fg-subtle">
      Kinds
    </span>
    {#each KINDS as k (k)}
      {@const active = kindFilter.has(k)}
      <button
        type="button"
        class="rounded-md transition-opacity {active ? '' : 'opacity-55 hover:opacity-100'}"
        onclick={() => toggleKind(k)}
        aria-pressed={active}
        title={active ? `Don't filter on ${k}` : `Filter on ${k}`}
      >
        <KindBadge kind={k} mode="icon_and_text" />
      </button>
    {/each}
    <span class="mx-2 h-4 w-px bg-line"></span>
    <label
      class="inline-flex cursor-pointer items-center gap-1 rounded-full px-2 py-0.5 text-[11px] ring-1 ring-inset
        {mandatoryOnly
        ? 'bg-amber-500/15 text-amber-300 ring-amber-500/40'
        : 'text-fg-muted ring-line-strong hover:bg-surface-2'}"
    >
      <input
        type="checkbox"
        class="sr-only"
        bind:checked={mandatoryOnly}
      />
      <Pin size={10} />
      Mandatory only
    </label>
    {#if scopeFilter.size > 0 || kindFilter.size > 0 || mandatoryOnly || query}
      <button
        type="button"
        class="ml-auto text-[11px] text-fg-subtle hover:text-fg"
        onclick={() => {
          scopeFilter = new Set();
          kindFilter = new Set();
          mandatoryOnly = false;
          query = '';
        }}
      >
        Reset filters
      </button>
    {/if}
  </div>

  <div class="min-h-0 flex-1 overflow-y-auto">
    <ul class="mx-auto flex max-w-3xl flex-col gap-3 p-4 sm:p-6">
      {#if filtered.length === 0}
        <li class="py-16 text-center text-sm text-fg-subtle">
          {#if entries.length === 0 && anyLoading}
            Loading memories across every group…
          {:else if entries.length === 0}
            No memories loaded yet.
          {:else}
            No memories match the current filters.
          {/if}
        </li>
      {/if}
      {#each filtered as entry (entry.groupId + ':' + entry.slug)}
        {@const fm = entry.body.frontmatter}
        {@const scopeMeta = SCOPE_META[entry.group.scope]}
        <li>
          <button
            type="button"
            class="group flex w-full flex-col gap-2 rounded-lg border border-l-4 border-line bg-surface-1 p-4 text-left transition-colors hover:border-line-strong hover:bg-surface-2 {KIND_ACCENT[fm.kind]}"
            onclick={() => openReader(entry.groupId, entry.slug)}
          >
            <div class="flex flex-wrap items-start gap-2">
              <div class="min-w-0 flex-1">
                <h2 class="truncate text-sm font-semibold text-fg" title={fm.name}>
                  {fm.name}
                </h2>
                <p class="mt-0.5 line-clamp-2 text-xs text-fg-muted">{fm.description}</p>
              </div>
              <div class="flex shrink-0 flex-col items-end gap-1 text-[10px] text-fg-subtle">
                <span class="inline-flex items-center gap-1">
                  <scopeMeta.Icon size={10} />
                  {scopeMeta.label}
                </span>
                <span class="font-mono" title={entry.group.slug}>
                  {entry.group.display_name ?? entry.group.slug}
                </span>
              </div>
            </div>
            <div class="flex flex-wrap items-center gap-1.5">
              <KindBadge kind={fm.kind} mode="icon_and_text" />
              {#if fm.feature}
                <FeatureBadge status={fm.feature.status} number={fm.feature.number} />
              {/if}
              <code class="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[10px] text-fg-muted">
                {entry.slug}
              </code>
              {#if fm.mandatory}
                <span
                  class="inline-flex items-center gap-1 rounded-md bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase text-amber-300 ring-1 ring-inset ring-amber-500/30"
                >
                  <Pin size={9} />
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
          </button>
        </li>
      {/each}
    </ul>
  </div>

  {#if readerBody}
    <!-- Reader overlay. Takes over the whole variant area so the
         feed's filter bar doesn't compete with reading. -->
    <div class="absolute inset-0 flex flex-col bg-surface-0">
      <header
        class="flex shrink-0 items-center gap-3 border-b border-line bg-surface-1/40 px-4 py-2 sm:px-6"
      >
        <button
          type="button"
          class="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-fg hover:bg-surface-2"
          onclick={closeReader}
          title="Back to feed"
        >
          <ChevronLeft size={13} />
          Back
        </button>
        <div class="min-w-0 flex-1">
          <div
            class="flex items-center gap-2 text-[11px] text-fg-subtle"
            title={readerGroup?.slug ?? ''}
          >
            <FolderGit2 size={11} />
            <span>{readerGroup?.display_name ?? readerGroup?.slug ?? '—'}</span>
            <span>/</span>
            <code class="truncate font-mono text-fg-muted">{selectionStore.slug}</code>
          </div>
          <h1
            class="truncate text-base font-semibold text-fg"
            title={readerBody.frontmatter.name}
          >
            {readerBody.frontmatter.name}
          </h1>
        </div>
        <KindBadge kind={readerBody.frontmatter.kind} mode="icon_and_text" />
      </header>
      <div class="min-h-0 flex-1 overflow-y-auto">
        <article class="mx-auto max-w-3xl px-6 py-6">
          <p class="text-sm text-fg-muted">{readerBody.frontmatter.description}</p>
          {#if readerBody.frontmatter.feature}
            <div class="mt-3 flex flex-wrap items-center gap-1.5">
              <FeatureBadge
                status={readerBody.frontmatter.feature.status}
                number={readerBody.frontmatter.feature.number}
              />
            </div>
            <div class="mt-4 rounded-lg border border-line bg-surface-1/40 p-4">
              <h3
                class="mb-2 text-[10px] font-semibold uppercase tracking-wide text-fg-muted"
              >
                Feature relations
              </h3>
              <FeatureRelations
                feature={readerBody.frontmatter.feature}
                onNavigate={openReader}
              />
            </div>
          {/if}
          <div
            class="prose prose-zinc prose-sm mt-4 max-w-none prose-pre:bg-surface-1 prose-pre:ring-1 prose-pre:ring-line prose-headings:tracking-tight"
          >
            {#if readerBody.body.trim()}
              {@html readerHtml}
            {:else}
              <p class="italic text-fg-subtle">(empty body)</p>
            {/if}
          </div>
        </article>
      </div>
    </div>
  {/if}
</div>
