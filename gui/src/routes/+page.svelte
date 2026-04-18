<script lang="ts">
  import DeleteConfirmation from '$lib/components/DeleteConfirmation.svelte';
  import GroupList from '$lib/components/GroupList.svelte';
  import HistoryPanel from '$lib/components/HistoryPanel.svelte';
  import MemoryEditor from '$lib/components/MemoryEditor.svelte';
  import MemoryList from '$lib/components/MemoryList.svelte';
  import MemoryViewer from '$lib/components/MemoryViewer.svelte';
  import Splitter from '$lib/components/Splitter.svelte';
  import StatusBar from '$lib/components/StatusBar.svelte';
  import Toolbar from '$lib/components/Toolbar.svelte';

  import { createMemory, deleteMemory, updateMemory } from '$lib/api/memory';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { reachabilityStore } from '$lib/stores/reachability.svelte';
  import { selectionStore } from '$lib/stores/selection.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';
  import { openDiagnosticsWindow, openSettingsWindow } from '$lib/windows';

  import type { KindStr, MemoryFile } from '$lib/types';

  type EditorState = { mode: 'new' | 'edit'; initial: MemoryFile | null } | null;
  let editor = $state<EditorState>(null);
  // `pendingDelete.slugs` is always a non-empty list — a single
  // selection becomes `[slug]`, a multi-select pulls the set.
  let pendingDelete = $state<{ groupId: string; slugs: string[] } | null>(null);
  // History view toggle. Sticky across group/memory changes — once
  // the user opts into browsing history they probably want to do
  // the same for the next memory they click, so we follow the
  // current selection instead of pinning to a single slug. The
  // MemoryViewer empty state still shows when there's no slug
  // selected, which keeps the pane useful at the between-memory
  // boundary.
  let historyMode = $state(false);

  // Mobile single-pane state. Auto-advances as the selection deepens
  // so a tap on a group jumps to the memories pane, a tap on a
  // memory jumps to the viewer. The dedicated tab bar (shown at
  // <md only) lets the user walk back up.
  type MobilePane = 'groups' | 'memories' | 'viewer';
  let mobilePane = $state<MobilePane>('groups');

  $effect(() => {
    if (selectionStore.slug) mobilePane = 'viewer';
    else if (selectionStore.groupId) mobilePane = 'memories';
  });

  $effect(() => {
    (async () => {
      await Promise.all([
        groupsStore.load(),
        syncStore.refreshStatus(),
        reachabilityStore.mount(),
        settingsStore.mount()
      ]);
    })();
    return () => reachabilityStore.unmount();
  });

  $effect(() => {
    const gid = selectionStore.groupId;
    if (gid && !memoriesStore.slugs[gid] && !memoriesStore.loadingSlugs[gid]) {
      memoriesStore.loadSlugs(gid);
    }
  });

  $effect(() => {
    const gid = selectionStore.groupId;
    const slug = selectionStore.slug;
    if (
      gid &&
      slug &&
      !memoriesStore.bodyFor(gid, slug) &&
      !memoriesStore.isLoadingBody(gid, slug)
    ) {
      memoriesStore.loadBody(gid, slug);
    }
  });

  $effect(() => {
    const gid = selectionStore.groupId;
    if (!gid) return;
    const slugs = memoriesStore.slugs[gid];
    if (!slugs) return;
    for (const s of slugs) {
      if (!memoriesStore.bodyFor(gid, s) && !memoriesStore.isLoadingBody(gid, s)) {
        void memoriesStore.loadBody(gid, s);
      }
    }
  });

  const selectedGroup = $derived(
    groupsStore.groups.find((g: { group_id: string }) => g.group_id === selectionStore.groupId) ??
      null
  );
  const slugs = $derived(
    selectionStore.groupId ? memoriesStore.slugs[selectionStore.groupId] : undefined
  );
  const slugsLoading = $derived(
    selectionStore.groupId ? !!memoriesStore.loadingSlugs[selectionStore.groupId] : false
  );
  const currentBody = $derived(
    selectionStore.groupId && selectionStore.slug
      ? memoriesStore.bodyFor(selectionStore.groupId, selectionStore.slug)
      : undefined
  );
  const currentBodyLoading = $derived(
    selectionStore.groupId && selectionStore.slug
      ? memoriesStore.isLoadingBody(selectionStore.groupId, selectionStore.slug)
      : false
  );

  const canCreate = $derived(!!selectionStore.groupId && editor === null);
  const canEdit = $derived(!!currentBody && editor === null && pendingDelete === null);
  const canDelete = $derived(
    (!!currentBody || selectionStore.multi.size > 0) &&
      editor === null &&
      pendingDelete === null
  );
  const canViewHistory = $derived(
    !!selectionStore.groupId && !!selectionStore.slug && editor === null
  );
  const syncReady = $derived(
    syncStore.configured && reachabilityStore.online && !syncStore.inFlight
  );

  function handleNew() {
    editor = { mode: 'new', initial: null };
    mobilePane = 'viewer';
  }

  function handleEdit() {
    if (!currentBody) return;
    editor = { mode: 'edit', initial: currentBody };
    mobilePane = 'viewer';
  }

  function handleHistory() {
    if (!selectionStore.groupId || !selectionStore.slug) return;
    historyMode = true;
    mobilePane = 'viewer';
  }

  function handleDeleteRequest() {
    const gid = selectionStore.groupId;
    if (!gid) return;
    // Batch intent wins when a multi-select exists; otherwise fall
    // back to the current single-viewer selection.
    const batch = Array.from(selectionStore.multi);
    const slugs = batch.length > 0 ? batch : selectionStore.slug ? [selectionStore.slug] : [];
    if (slugs.length === 0) return;
    pendingDelete = { groupId: gid, slugs };
  }

  async function confirmDelete() {
    if (!pendingDelete) return;
    const { groupId, slugs } = pendingDelete;
    pendingDelete = null;
    const errors: string[] = [];
    for (const s of slugs) {
      try {
        await deleteMemory(groupId, s);
        memoriesStore.invalidate(groupId, s);
      } catch (err) {
        errors.push(`${s}: ${formatErr(err)}`);
      }
    }
    if (slugs.includes(selectionStore.slug ?? '')) selectionStore.clearMemory();
    selectionStore.clearMulti();
    await memoriesStore.loadSlugs(groupId);
    if (errors.length > 0) alert(errors.join('\n'));
  }

  async function handleSave(memory: MemoryFile, newSlug: string) {
    const gid = selectionStore.groupId;
    if (!gid || !editor) return;
    try {
      if (editor.mode === 'new') {
        await createMemory(gid, newSlug, memory);
        memoriesStore.invalidate(gid);
        await memoriesStore.loadSlugs(gid);
        selectionStore.selectMemory(newSlug);
      } else if (selectionStore.slug) {
        await updateMemory(gid, selectionStore.slug, memory);
        memoriesStore.invalidate(gid, selectionStore.slug);
        await memoriesStore.loadBody(gid, selectionStore.slug);
      }
      editor = null;
    } catch (err) {
      alert(formatErr(err));
    }
  }

  function handleCancel() {
    editor = null;
  }

  function formatErr(err: unknown): string {
    if (err && typeof err === 'object' && 'message' in err) {
      return String((err as { message: unknown }).message);
    }
    return String(err);
  }

  // Tailwind-friendly visibility helpers for the three panes.
  // `hidden md:block` means: hidden at <md, block at md+. We toggle
  // the `hidden` vs `block` half per-pane based on mobilePane.
  const paneVisibility = (pane: MobilePane) =>
    mobilePane === pane ? 'flex md:flex' : 'hidden md:flex';

  // User-resizable sidebar dimensions. Kept in-memory only — the
  // layout mode itself persists through `settingsStore` but the
  // pane sizes reset per session. Reasonable defaults chosen to
  // match the pre-splitter grid columns so the first frame looks
  // identical to the old `grid-cols-[220px_280px_1fr]` layout.
  let groupsWidth = $state(220);
  let memoriesWidth = $state(280);
  let sidebarWidth = $state(300);
  let groupsHeight = $state(220);
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-zinc-950 text-zinc-100">
  <Toolbar
    {canCreate}
    {canEdit}
    {canDelete}
    {canViewHistory}
    {syncReady}
    layout={settingsStore.values.layout_mode}
    onNew={handleNew}
    onEdit={handleEdit}
    onDelete={handleDeleteRequest}
    onHistory={handleHistory}
    onPull={() => syncStore.pull()}
    onPush={() => syncStore.push()}
    onDiagnose={() => void openDiagnosticsWindow()}
    onSettings={() => void openSettingsWindow()}
    onToggleLayout={() => settingsStore.toggleLayoutMode()}
  />

  <!-- Mobile-only pane tabs. Hidden at md+ where all three panes are
       visible simultaneously in the grid. -->
  <nav
    class="flex h-9 shrink-0 items-center gap-1 overflow-x-auto border-b border-zinc-800 bg-zinc-900/70 px-2 text-sm md:hidden"
  >
    {#each [
      { id: 'groups' as const, label: 'Groups', disabled: false },
      {
        id: 'memories' as const,
        label: 'Memories',
        disabled: !selectionStore.groupId
      },
      {
        id: 'viewer' as const,
        label: editor ? (editor.mode === 'new' ? 'New' : 'Edit') : 'Viewer',
        disabled: !selectionStore.slug && !editor
      }
    ] as tab (tab.id)}
      {@const active = mobilePane === tab.id}
      <button
        type="button"
        class="rounded-md px-3 py-1 text-xs font-medium transition-colors
          {active
          ? 'bg-sky-500/15 text-sky-100'
          : 'text-zinc-400 hover:bg-zinc-800/70 hover:text-zinc-200'}
          disabled:cursor-not-allowed disabled:opacity-40"
        disabled={tab.disabled}
        onclick={() => (mobilePane = tab.id)}
      >
        {tab.label}
      </button>
    {/each}
  </nav>

  <!-- Main body. Below md the three panes stack and the tab bar
       above governs which is visible (paneVisibility). At md+ the
       layout forks on `layout_mode`:
         * columns  — groups | memories | viewer, two horizontal
                      resize handles.
         * stacked  — sidebar (groups on top / memories on bottom
                      with a vertical splitter) | viewer, one
                      horizontal splitter between sidebar + viewer.
       Each pane root keeps `h-full min-h-0 overflow-hidden` so
       scroll stays contained. -->
  <div class="flex min-h-0 flex-1 flex-col md:flex-row">
    <!-- Below md: linear stack, tab-gated. Above md: hidden so the
         branched layouts below take over. -->
    <div class="{paneVisibility('groups')} h-full min-h-0 flex-col overflow-hidden md:hidden">
      <GroupList
        groups={groupsStore.groups}
        selectedId={selectionStore.groupId}
        onSelect={(id: string) => selectionStore.selectGroup(id)}
      />
    </div>
    <div class="{paneVisibility('memories')} h-full min-h-0 flex-col overflow-hidden md:hidden">
      <MemoryList
        {slugs}
        groupSelected={!!selectionStore.groupId}
        loading={slugsLoading}
        selectedSlug={selectionStore.slug}
        kindDisplay={settingsStore.values.kind_display}
        filter={selectionStore.filter}
        kindFilter={selectionStore.kindFilter}
        mandatoryOnly={selectionStore.mandatoryOnly}
        multi={selectionStore.multi}
        bodyFor={(slug: string) =>
          selectionStore.groupId
            ? memoriesStore.bodyFor(selectionStore.groupId, slug)
            : undefined}
        onSelect={(slug: string) => selectionStore.selectMemory(slug)}
        onFilterChange={(q: string) => selectionStore.setFilter(q)}
        onToggleMulti={(slug: string) => selectionStore.toggleMulti(slug)}
        onExtendMulti={(slug: string, visible: string[]) =>
          selectionStore.extendMulti(slug, visible)}
        onToggleKind={(kind: KindStr) => selectionStore.toggleKindFilter(kind)}
        onToggleMandatoryOnly={() =>
          selectionStore.setMandatoryOnly(!selectionStore.mandatoryOnly)}
        onClearFilters={() => selectionStore.clearAllFilters()}
        onSelectAll={(visible: string[]) => selectionStore.selectMultiAll(visible)}
        onClearMulti={() => selectionStore.clearMulti()}
      />
    </div>
    <div class="{paneVisibility('viewer')} h-full min-h-0 flex-col overflow-hidden md:hidden">
      {#if editor}
        <MemoryEditor
          initial={editor.initial}
          mode={editor.mode}
          onSave={handleSave}
          onCancel={handleCancel}
        />
      {:else if historyMode && selectionStore.groupId && selectionStore.slug}
        <HistoryPanel
          groupId={selectionStore.groupId}
          slug={selectionStore.slug}
          onClose={() => (historyMode = false)}
        />
      {:else}
        <MemoryViewer
          memory={currentBody}
          slug={selectionStore.slug}
          loading={currentBodyLoading}
          kindDisplay={settingsStore.values.kind_display}
        />
      {/if}
    </div>

    <!-- Desktop layouts (md+). Only one branch ever mounts so the
         splitters tracked in each are deterministic. -->
    {#if settingsStore.values.layout_mode === 'columns'}
      <div class="hidden h-full min-h-0 w-full flex-row md:flex">
        <div
          class="h-full min-h-0 shrink-0 overflow-hidden"
          style="width: {groupsWidth}px"
        >
          <GroupList
            groups={groupsStore.groups}
            selectedId={selectionStore.groupId}
            onSelect={(id: string) => selectionStore.selectGroup(id)}
          />
        </div>
        <Splitter
          orientation="horizontal"
          size={groupsWidth}
          min={140}
          max={500}
          onResize={(v) => (groupsWidth = v)}
        />
        <div
          class="h-full min-h-0 shrink-0 overflow-hidden"
          style="width: {memoriesWidth}px"
        >
          <MemoryList
            {slugs}
            groupSelected={!!selectionStore.groupId}
            loading={slugsLoading}
            selectedSlug={selectionStore.slug}
            kindDisplay={settingsStore.values.kind_display}
            filter={selectionStore.filter}
            kindFilter={selectionStore.kindFilter}
            mandatoryOnly={selectionStore.mandatoryOnly}
            multi={selectionStore.multi}
            bodyFor={(slug: string) =>
              selectionStore.groupId
                ? memoriesStore.bodyFor(selectionStore.groupId, slug)
                : undefined}
            onSelect={(slug: string) => selectionStore.selectMemory(slug)}
            onFilterChange={(q: string) => selectionStore.setFilter(q)}
            onToggleMulti={(slug: string) => selectionStore.toggleMulti(slug)}
            onExtendMulti={(slug: string, visible: string[]) =>
              selectionStore.extendMulti(slug, visible)}
            onToggleKind={(kind: KindStr) => selectionStore.toggleKindFilter(kind)}
            onToggleMandatoryOnly={() =>
              selectionStore.setMandatoryOnly(!selectionStore.mandatoryOnly)}
            onClearFilters={() => selectionStore.clearAllFilters()}
            onSelectAll={(visible: string[]) => selectionStore.selectMultiAll(visible)}
            onClearMulti={() => selectionStore.clearMulti()}
          />
        </div>
        <Splitter
          orientation="horizontal"
          size={memoriesWidth}
          min={180}
          max={600}
          onResize={(v) => (memoriesWidth = v)}
        />
        <div class="h-full min-h-0 flex-1 overflow-hidden">
          {#if editor}
            <MemoryEditor
              initial={editor.initial}
              mode={editor.mode}
              onSave={handleSave}
              onCancel={handleCancel}
            />
          {:else if historyMode && selectionStore.groupId && selectionStore.slug}
            <HistoryPanel
              groupId={selectionStore.groupId}
              slug={selectionStore.slug}
              onClose={() => (historyMode = false)}
            />
          {:else}
            <MemoryViewer
              memory={currentBody}
              slug={selectionStore.slug}
              loading={currentBodyLoading}
              kindDisplay={settingsStore.values.kind_display}
            />
          {/if}
        </div>
      </div>
    {:else}
      <div class="hidden h-full min-h-0 w-full flex-row md:flex">
        <div
          class="flex h-full min-h-0 shrink-0 flex-col overflow-hidden"
          style="width: {sidebarWidth}px"
        >
          <div
            class="shrink-0 overflow-hidden"
            style="height: {groupsHeight}px"
          >
            <GroupList
              groups={groupsStore.groups}
              selectedId={selectionStore.groupId}
              onSelect={(id: string) => selectionStore.selectGroup(id)}
            />
          </div>
          <Splitter
            orientation="vertical"
            size={groupsHeight}
            min={100}
            max={600}
            onResize={(v) => (groupsHeight = v)}
          />
          <div class="min-h-0 flex-1 overflow-hidden">
            <MemoryList
              {slugs}
              groupSelected={!!selectionStore.groupId}
              loading={slugsLoading}
              selectedSlug={selectionStore.slug}
              kindDisplay={settingsStore.values.kind_display}
              filter={selectionStore.filter}
              kindFilter={selectionStore.kindFilter}
              mandatoryOnly={selectionStore.mandatoryOnly}
              multi={selectionStore.multi}
              bodyFor={(slug: string) =>
                selectionStore.groupId
                  ? memoriesStore.bodyFor(selectionStore.groupId, slug)
                  : undefined}
              onSelect={(slug: string) => selectionStore.selectMemory(slug)}
              onFilterChange={(q: string) => selectionStore.setFilter(q)}
              onToggleMulti={(slug: string) => selectionStore.toggleMulti(slug)}
              onExtendMulti={(slug: string, visible: string[]) =>
                selectionStore.extendMulti(slug, visible)}
              onToggleKind={(kind: KindStr) => selectionStore.toggleKindFilter(kind)}
              onToggleMandatoryOnly={() =>
                selectionStore.setMandatoryOnly(!selectionStore.mandatoryOnly)}
              onClearFilters={() => selectionStore.clearAllFilters()}
              onSelectAll={(visible: string[]) => selectionStore.selectMultiAll(visible)}
              onClearMulti={() => selectionStore.clearMulti()}
            />
          </div>
        </div>
        <Splitter
          orientation="horizontal"
          size={sidebarWidth}
          min={200}
          max={700}
          onResize={(v) => (sidebarWidth = v)}
        />
        <div class="h-full min-h-0 flex-1 overflow-hidden">
          {#if editor}
            <MemoryEditor
              initial={editor.initial}
              mode={editor.mode}
              onSave={handleSave}
              onCancel={handleCancel}
            />
          {:else if historyMode && selectionStore.groupId && selectionStore.slug}
            <HistoryPanel
              groupId={selectionStore.groupId}
              slug={selectionStore.slug}
              onClose={() => (historyMode = false)}
            />
          {:else}
            <MemoryViewer
              memory={currentBody}
              slug={selectionStore.slug}
              loading={currentBodyLoading}
              kindDisplay={settingsStore.values.kind_display}
            />
          {/if}
        </div>
      </div>
    {/if}
  </div>

  <StatusBar
    reachability={reachabilityStore.state}
    sync={syncStore.phase}
    selectedGroupSlug={selectedGroup?.slug ?? null}
    selectedMemoryCount={slugs?.length ?? null}
  />

  {#if pendingDelete}
    <DeleteConfirmation
      slugs={pendingDelete.slugs}
      onConfirm={confirmDelete}
      onCancel={() => (pendingDelete = null)}
    />
  {/if}
</div>
