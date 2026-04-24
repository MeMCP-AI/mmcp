<script lang="ts">
  // App shell. Renders the shared navbar, the active variant, and
  // the shared footer. Every bit of variant-specific logic lives
  // inside the variants themselves — this file just routes on
  // `settingsStore.values.ui_variant` and wires the background
  // mirror-refresh cascade that the stores need to stay fresh.

  import CommonFooter from '$lib/components/CommonFooter.svelte';
  import CommonNavbar from '$lib/components/CommonNavbar.svelte';
  import FeedView from '$lib/components/variants/FeedView.svelte';
  import HubView from '$lib/components/variants/HubView.svelte';
  import RepoView from '$lib/components/variants/RepoView.svelte';

  import { groupsStore } from '$lib/stores/groups.svelte';
  import { memoriesStore } from '$lib/stores/memories.svelte';
  import { reachabilityStore } from '$lib/stores/reachability.svelte';
  import { selectionStore } from '$lib/stores/selection.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { syncStore } from '$lib/stores/sync.svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';

  interface MirrorChangedPayload {
    group_id: string | null;
  }

  $effect(() => {
    // Auto-pull the moment the probe reports the server is back.
    // Silent: the pull's own `mirror:changed` broadcast drives the
    // refresh cascade; we don't need to surface the call.
    reachabilityStore.onRestore = () => {
      if (!syncStore.configured || syncStore.inFlight) return;
      void syncStore.pull();
    };
    (async () => {
      await Promise.all([
        groupsStore.load(),
        syncStore.refreshStatus(),
        reachabilityStore.mount(),
        settingsStore.mount()
      ]);
    })();
    return () => {
      reachabilityStore.onRestore = null;
      reachabilityStore.unmount();
    };
  });

  // Any change on disk — local writes, sync pulls, fs-watcher
  // events — refreshes the group list + active group silently.
  // Currently-viewed memory lands in `pendingBodies` for the
  // banner, so a refresh never yanks the reader off their page.
  $effect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;
    void (async () => {
      const off = await listen<MirrorChangedPayload>('mirror:changed', (e) => {
        handleMirrorChanged(e.payload?.group_id ?? null);
      });
      if (cancelled) off();
      else unlisten = off;
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });

  function handleMirrorChanged(groupId: string | null) {
    void groupsStore.refreshQuiet();
    const active = selectionStore.groupId;
    const currentlyViewed =
      selectionStore.groupId && selectionStore.slug
        ? { groupId: selectionStore.groupId, slug: selectionStore.slug }
        : null;
    if (active && (groupId === null || groupId === active)) {
      void memoriesStore.refreshGroup(active, currentlyViewed);
    }
  }
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-surface-0 text-fg">
  <CommonNavbar />

  <div class="relative flex min-h-0 flex-1 flex-col overflow-hidden">
    {#if settingsStore.values.ui_variant === 'repo'}
      <RepoView />
    {:else if settingsStore.values.ui_variant === 'feed'}
      <FeedView />
    {:else}
      <HubView />
    {/if}
  </div>

  <CommonFooter />
</div>
