<script lang="ts">
  import DeleteConfirmation from '$lib/components/DeleteConfirmation.svelte';
  import DiagnosticsPanel from '$lib/components/DiagnosticsPanel.svelte';
  import GroupList from '$lib/components/GroupList.svelte';
  import MemoryEditor from '$lib/components/MemoryEditor.svelte';
  import MemoryList from '$lib/components/MemoryList.svelte';
  import MemoryViewer from '$lib/components/MemoryViewer.svelte';
  import SettingsPanel from '$lib/components/SettingsPanel.svelte';
  import StatusBar from '$lib/components/StatusBar.svelte';
  import Toolbar from '$lib/components/Toolbar.svelte';

  import { createMemory, deleteMemory, updateMemory } from '$lib/api/memory';
  import { diagnosticsStore, type SeverityFilter } from '$lib/stores/diagnostics.svelte';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { reachabilityStore } from '$lib/stores/reachability.svelte';
  import { selectionStore } from '$lib/stores/selection.svelte';
  import { settingsStore, type KindDisplay } from '$lib/stores/settings.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';

  import type { MemoryFile } from '$lib/types';

  type EditorState = { mode: 'new' | 'edit'; initial: MemoryFile | null } | null;
  let editor = $state<EditorState>(null);
  let pendingDelete = $state<{ groupId: string; slug: string } | null>(null);
  let settingsOpen = $state(false);
  let diagnosticsOpen = $state(false);

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
  const canEditOrDelete = $derived(
    !!currentBody && editor === null && pendingDelete === null
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

  function handleDeleteRequest() {
    const gid = selectionStore.groupId;
    const slug = selectionStore.slug;
    if (!gid || !slug) return;
    pendingDelete = { groupId: gid, slug };
  }

  async function confirmDelete() {
    if (!pendingDelete) return;
    const { groupId, slug } = pendingDelete;
    pendingDelete = null;
    try {
      await deleteMemory(groupId, slug);
      memoriesStore.invalidate(groupId, slug);
      selectionStore.clearMemory();
      await memoriesStore.loadSlugs(groupId);
    } catch (err) {
      alert(formatErr(err));
    }
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

  function openDiagnostics() {
    diagnosticsOpen = true;
    void diagnosticsStore.run();
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
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-zinc-950 text-zinc-100">
  <Toolbar
    {canCreate}
    {canEditOrDelete}
    {syncReady}
    onNew={handleNew}
    onEdit={handleEdit}
    onDelete={handleDeleteRequest}
    onPull={() => syncStore.pull()}
    onPush={() => syncStore.push()}
    onDiagnose={openDiagnostics}
    onSettings={() => (settingsOpen = true)}
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

  <!-- Main body: stacked below md, three-pane grid above. Each pane
       root pins `h-full min-h-0 overflow-hidden` so scroll lives
       inside the pane instead of bubbling to the page. -->
  <div
    class="min-h-0 flex-1 md:grid md:grid-cols-[180px_240px_1fr] lg:grid-cols-[220px_280px_1fr]"
  >
    <div class="{paneVisibility('groups')} h-full min-h-0 flex-col overflow-hidden">
      <GroupList
        groups={groupsStore.groups}
        selectedId={selectionStore.groupId}
        onSelect={(id: string) => selectionStore.selectGroup(id)}
      />
    </div>

    <div class="{paneVisibility('memories')} h-full min-h-0 flex-col overflow-hidden">
      <MemoryList
        {slugs}
        groupSelected={!!selectionStore.groupId}
        loading={slugsLoading}
        selectedSlug={selectionStore.slug}
        kindDisplay={settingsStore.values.kind_display}
        bodyFor={(slug: string) =>
          selectionStore.groupId
            ? memoriesStore.bodyFor(selectionStore.groupId, slug)
            : undefined}
        onSelect={(slug: string) => selectionStore.selectMemory(slug)}
      />
    </div>

    <div class="{paneVisibility('viewer')} h-full min-h-0 flex-col overflow-hidden">
      {#if editor}
        <MemoryEditor
          initial={editor.initial}
          mode={editor.mode}
          onSave={handleSave}
          onCancel={handleCancel}
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

  <StatusBar
    reachability={reachabilityStore.state}
    sync={syncStore.phase}
    selectedGroupSlug={selectedGroup?.slug ?? null}
    selectedMemoryCount={slugs?.length ?? null}
  />

  {#if pendingDelete}
    <DeleteConfirmation
      slug={pendingDelete.slug}
      onConfirm={confirmDelete}
      onCancel={() => (pendingDelete = null)}
    />
  {/if}

  {#if settingsOpen}
    <SettingsPanel
      value={settingsStore.values.kind_display}
      onChange={(mode: KindDisplay) => settingsStore.setKindDisplay(mode)}
      onReset={() => settingsStore.reset()}
      onClose={() => (settingsOpen = false)}
    />
  {/if}

  {#if diagnosticsOpen}
    <DiagnosticsPanel
      report={diagnosticsStore.report}
      loading={diagnosticsStore.loading}
      error={diagnosticsStore.error}
      filter={diagnosticsStore.filter}
      collapsed={diagnosticsStore.collapsed}
      onClose={() => (diagnosticsOpen = false)}
      onFilterChange={(f: SeverityFilter) => diagnosticsStore.setFilter(f)}
      onToggleGroup={(slug: string) => diagnosticsStore.toggle(slug)}
    />
  {/if}
</div>
