<script lang="ts">
  // =====================================================================
  // PENDING REMOVAL — do not build on this file.
  //
  // User (2026-04-22) picked Hub as the live UI and flagged Repo +
  // Feed variants for deletion. Kept on disk as reference only; no
  // import wires them into the app anymore. Any AI touching this
  // file: confirm with the user before investing work here — the
  // default should be "delete", not "extend".
  // =====================================================================
  //
  // Minimalist feed. Groups blur into the background; memories
  // scroll as a flat, filterable stream of cards regardless of
  // which repo they belong to. A card click opens a full reader
  // overlay so the feed doesn't fight with the content.
  //
  // Composition-only — every piece of list / card / reader markup
  // lives in primitives; this component just orchestrates state.

  import { ChevronLeft, LoaderCircle } from '@lucide/svelte';
  import FeatureBadge from '../FeatureBadge.svelte';
  import KindFilterRow from '../primitives/KindFilterRow.svelte';
  import KindBadge from '../KindBadge.svelte';
  import MandatoryToggle from '../primitives/MandatoryToggle.svelte';
  import MemoryFeedCard from '../primitives/MemoryFeedCard.svelte';
  import MemoryReader from '../primitives/MemoryReader.svelte';
  import RelatedPanel from '../primitives/RelatedPanel.svelte';
  import SearchInput from '../primitives/SearchInput.svelte';
  import ScopeIcon from '../primitives/ScopeIcon.svelte';

  import { selectionStore } from '$lib/stores/selection.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import { matchesMemoryFilter } from '$lib/utils/filter';
  import { classifyMemoryKind, type MemoryClass } from '$lib/utils/memory_kind';
  import { SCOPE_META, SCOPE_ORDER } from '$lib/utils/scope';
  import type { GroupEntry, GroupScope, KindStr, MemoryFile } from '$lib/types';

  // Filter state.
  let scopeFilter = $state<Set<GroupScope>>(new Set());
  let kindFilter = $state<Set<KindStr>>(new Set());
  let mandatoryOnly = $state(false);
  // Memories and issues are separate categories; the feed always
  // narrows to one or the other.
  let classFilter = $state<MemoryClass>('memory');
  let query = $state('');

  // Load every group's slug list + bodies so the feed is
  // populated without clicking.
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
    return entries.filter(({ group, slug, body }) => {
      if (scopeFilter.size > 0 && !scopeFilter.has(group.scope)) return false;
      if (classifyMemoryKind(body.frontmatter.kind) !== classFilter) return false;
      return matchesMemoryFilter(slug, body, {
        query,
        kinds: kindFilter,
        mandatoryOnly
      });
    });
  });

  const counts = $derived.by(() => {
    let memories = 0;
    let issues = 0;
    for (const e of entries) {
      if (classifyMemoryKind(e.body.frontmatter.kind) === 'issue') issues++;
      else memories++;
    }
    return { memories, issues };
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

  const anyLoading = $derived(
    Object.values(memoriesStore.loadingSlugs).some(Boolean) ||
      Object.values(memoriesStore.loadingBody).some(Boolean)
  );

  const CLASS_TABS: { id: MemoryClass; label: string; count: (c: typeof counts) => number }[] = [
    { id: 'memory', label: 'Memories', count: (c) => c.memories },
    { id: 'issue', label: 'Issues', count: (c) => c.issues }
  ];
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-surface-0 text-fg">
  <header class="flex h-11 shrink-0 items-center gap-2 border-b border-line bg-surface-1 px-4">
    <h1 class="text-sm font-semibold tracking-tight">Feed</h1>
    <nav class="flex items-center gap-0.5">
      {#each CLASS_TABS as tab (tab.id)}
        {@const active = classFilter === tab.id}
        <button
          type="button"
          class="inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-[11px] transition-colors
            {active
            ? 'bg-sky-500/15 text-selected-fg'
            : 'text-fg-muted hover:bg-surface-2 hover:text-fg'}"
          onclick={() => (classFilter = tab.id)}
        >
          {tab.label}
          <span class="rounded-full bg-surface-2 px-1 py-0.5 text-[10px]">
            {tab.count(counts)}
          </span>
        </button>
      {/each}
    </nav>
    <span class="text-[11px] text-fg-subtle">
      {filtered.length} visible
    </span>
    {#if anyLoading}
      <LoaderCircle size={12} class="animate-spin text-fg-subtle" />
    {/if}
    <div class="ml-auto">
      <SearchInput
        value={query}
        onChange={(v) => (query = v)}
        placeholder="Search memories…"
        widthClass="w-64"
      />
    </div>
  </header>

  <div
    class="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-line bg-surface-1/40 px-4 py-2"
  >
    <span class="text-[10px] font-semibold uppercase tracking-wide text-fg-subtle">
      Scopes
    </span>
    {#each SCOPE_ORDER as s (s)}
      {@const active = scopeFilter.has(s)}
      <button
        type="button"
        class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] ring-1 ring-inset transition-colors
          {active
          ? 'bg-sky-500/15 text-selected-fg ring-sky-500/40'
          : 'text-fg-muted ring-line-strong hover:bg-surface-2'}"
        onclick={() => toggleScope(s)}
      >
        <ScopeIcon scope={s} size={10} />
        {SCOPE_META[s].label}
      </button>
    {/each}
    <span class="mx-2 h-4 w-px bg-line"></span>
    <span class="text-[10px] font-semibold uppercase tracking-wide text-fg-subtle">Kinds</span>
    <KindFilterRow selected={kindFilter} onToggle={toggleKind} />
    <span class="mx-2 h-4 w-px bg-line"></span>
    <MandatoryToggle value={mandatoryOnly} onChange={(v) => (mandatoryOnly = v)} />
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
        <li>
          <MemoryFeedCard
            slug={entry.slug}
            body={entry.body}
            group={entry.group}
            onOpen={() => openReader(entry.groupId, entry.slug)}
          />
        </li>
      {/each}
    </ul>
  </div>

  {#if readerBody}
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
          <div class="flex items-center gap-2 text-[11px] text-fg-subtle" title={readerGroup?.slug ?? ''}>
            <ScopeIcon scope={readerGroup?.scope ?? 'project'} size={11} />
            <span>{readerGroup?.display_name ?? readerGroup?.slug ?? '—'}</span>
            <span>/</span>
            <code class="truncate font-mono text-fg-muted">{selectionStore.slug}</code>
          </div>
        </div>
        <KindBadge kind={readerBody.frontmatter.kind} mode="icon_and_text" />
        {#if readerBody.frontmatter.feature}
          <FeatureBadge
            status={readerBody.frontmatter.feature.status}
            number={readerBody.frontmatter.feature.number}
          />
        {/if}
      </header>
      <div class="min-h-0 flex-1 overflow-y-auto">
        <MemoryReader memory={readerBody}>
          {#snippet sidebar()}
            <RelatedPanel
              memory={readerBody}
              selfGroupId={selectionStore.groupId}
              selfSlug={selectionStore.slug}
              onNavigate={openReader}
            />
          {/snippet}
        </MemoryReader>
      </div>
    </div>
  {/if}
</div>
