<script lang="ts">
  import GroupList from '$lib/components/GroupList.svelte';
  import MemoryList from '$lib/components/MemoryList.svelte';
  import MemoryViewer from '$lib/components/MemoryViewer.svelte';
  import MemoryEditor from '$lib/components/MemoryEditor.svelte';
  import StatusBar from '$lib/components/StatusBar.svelte';
  import Toolbar from '$lib/components/Toolbar.svelte';

  import { createMemory, deleteMemory, updateMemory } from '$lib/api/memory';
  import { groupsStore } from '$lib/stores/groups.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { reachabilityStore } from '$lib/stores/reachability.svelte';
  import { selectionStore } from '$lib/stores/selection.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';

  import type { MemoryFile } from '$lib/types';

  // Editor lives locally on the page: null = not editing, otherwise
  // carries the initial file + mode. Kept page-scoped until a
  // future commit pulls the editor into its own route.
  type EditorState = { mode: 'new' | 'edit'; initial: MemoryFile | null } | null;
  let editor = $state<EditorState>(null);

  // Kick off initial data loads + event listeners.
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

  // When the user picks a group, fetch its slugs.
  $effect(() => {
    const gid = selectionStore.groupId;
    if (gid && !memoriesStore.slugs[gid] && !memoriesStore.loadingSlugs[gid]) {
      memoriesStore.loadSlugs(gid);
    }
  });

  // When the user picks a memory, fetch its body.
  $effect(() => {
    const gid = selectionStore.groupId;
    const slug = selectionStore.slug;
    if (gid && slug && !memoriesStore.bodyFor(gid, slug) && !memoriesStore.isLoadingBody(gid, slug)) {
      memoriesStore.loadBody(gid, slug);
    }
  });

  // Eagerly pre-fetch each slug's body so the KindBadge prefix can
  // render for the whole list. Progressive; cheap.
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

  // Derived selections.
  const selectedGroup = $derived(
    groupsStore.groups.find((g) => g.group_id === selectionStore.groupId) ?? null
  );
  const slugs = $derived(
    selectionStore.groupId ? memoriesStore.slugs[selectionStore.groupId] : undefined
  );
  const slugsLoading = $derived(
    selectionStore.groupId
      ? !!memoriesStore.loadingSlugs[selectionStore.groupId]
      : false
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
  const canEditOrDelete = $derived(!!currentBody && editor === null);
  const syncReady = $derived(syncStore.configured && reachabilityStore.online && !syncStore.inFlight);

  function handleNew() {
    editor = { mode: 'new', initial: null };
  }

  function handleEdit() {
    if (!currentBody) return;
    editor = { mode: 'edit', initial: currentBody };
  }

  async function handleDelete() {
    const gid = selectionStore.groupId;
    const slug = selectionStore.slug;
    if (!gid || !slug) return;
    if (!confirm(`Delete memory '${slug}'? The deletion is a git commit, recoverable from history.`))
      return;
    try {
      await deleteMemory(gid, slug);
      memoriesStore.invalidate(gid, slug);
      selectionStore.clearMemory();
      await memoriesStore.loadSlugs(gid);
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

  function formatErr(err: unknown): string {
    if (err && typeof err === 'object' && 'message' in err) {
      return String((err as { message: unknown }).message);
    }
    return String(err);
  }
</script>

<div class="flex h-full w-full flex-col bg-zinc-950 text-zinc-100">
  <Toolbar
    {canCreate}
    {canEditOrDelete}
    {syncReady}
    onNew={handleNew}
    onEdit={handleEdit}
    onDelete={handleDelete}
    onPull={() => syncStore.pull()}
    onPush={() => syncStore.push()}
    onDiagnose={() => alert('Diagnose panel lands in the next commit.')}
    onSettings={() => alert('Settings panel lands in the next commit.')}
  />

  <div class="grid min-h-0 flex-1 grid-cols-[220px_280px_1fr]">
    <GroupList
      groups={groupsStore.groups}
      selectedId={selectionStore.groupId}
      onSelect={(id) => selectionStore.selectGroup(id)}
    />

    <MemoryList
      slugs={slugs}
      groupSelected={!!selectionStore.groupId}
      loading={slugsLoading}
      selectedSlug={selectionStore.slug}
      kindDisplay={settingsStore.values.kind_display}
      bodyFor={(slug) =>
        selectionStore.groupId
          ? memoriesStore.bodyFor(selectionStore.groupId, slug)
          : undefined}
      onSelect={(slug) => selectionStore.selectMemory(slug)}
    />

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

  <StatusBar
    reachability={reachabilityStore.state}
    sync={syncStore.phase}
    selectedGroupSlug={selectedGroup?.slug ?? null}
    selectedMemoryCount={slugs?.length ?? null}
  />
</div>
