<script lang="ts">
  // App shell. Renders the shared navbar, Hub (the only remaining
  // variant), and the shared footer, and wires the background
  // mirror-refresh cascade that the stores need to stay fresh.

  import CommonFooter from '$lib/components/CommonFooter.svelte';
  import TitleBar from '$lib/components/TitleBar.svelte';
  import HubView from '$lib/components/variants/HubView.svelte';

  import { onAppInitFailed } from '$lib/api/events';
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

  // Fatal `AppState::discover` failure reported by the backend; every command needs `AppState`.
  let initFailedMessage = $state<string | null>(null);

  $effect(() => {
    // Auto-pull the moment the probe reports the server is back.
    // Silent: the pull's own `mirror:changed` broadcast drives the
    // refresh cascade; we don't need to surface the call.
    reachabilityStore.onRestore = () => {
      if (!syncStore.configured || syncStore.inFlight) return;
      void syncStore.pull();
    };
    let unlistenInitFailed: UnlistenFn | null = null;
    let cancelled = false;
    void onAppInitFailed((message) => {
      initFailedMessage = message;
    }).then((off) => {
      if (cancelled) off();
      else unlistenInitFailed = off;
    });
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
      cancelled = true;
      unlistenInitFailed?.();
    };
  });

  // Any change on disk — local writes, sync pulls, fs-watcher
  // events — refreshes the group list + active group silently.
  // Currently-viewed memory lands in `pendingBodies` for the
  // banner, so a refresh never yanks the reader off their page.
  //
  // Coalescing window for `mirror:changed`: one sync pull emits one event per group.
  // A batch flushes as one `refreshQuiet()` plus at most one `refreshGroup()` for the active group.
  const MIRROR_EVENT_COALESCE_MS = 200;

  $effect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;
    const pendingGroupIds = new Set<string>();
    let pendingRootChange = false;
    let flushTimer: ReturnType<typeof setTimeout> | null = null;

    const flush = () => {
      flushTimer = null;
      const groupIds = Array.from(pendingGroupIds);
      const rootChanged = pendingRootChange;
      pendingGroupIds.clear();
      pendingRootChange = false;
      void groupsStore.refreshQuiet();
      const active = selectionStore.groupId;
      if (!active) return;
      const currentlyViewed =
        active && selectionStore.slug ? { groupId: active, slug: selectionStore.slug } : null;
      if (rootChanged || groupIds.includes(active)) {
        void memoriesStore.refreshGroup(active, currentlyViewed);
      }
    };

    void (async () => {
      const off = await listen<MirrorChangedPayload>('mirror:changed', (e) => {
        const groupId = e.payload?.group_id ?? null;
        if (groupId === null) pendingRootChange = true;
        else pendingGroupIds.add(groupId);
        if (flushTimer === null) {
          flushTimer = setTimeout(flush, MIRROR_EVENT_COALESCE_MS);
        }
      });
      if (cancelled) off();
      else unlisten = off;
    })();
    return () => {
      cancelled = true;
      unlisten?.();
      if (flushTimer !== null) clearTimeout(flushTimer);
    };
  });
</script>

<div class="flex h-full w-full flex-col overflow-hidden bg-surface-0 text-fg">
  <TitleBar />

  {#if initFailedMessage}
    <div
      class="m-3 shrink-0 rounded-md border border-rose-900/60 bg-rose-950/40 p-3 text-sm text-rose-200"
      role="alert"
    >
      <p class="font-semibold">The app failed to start correctly.</p>
      <p class="mt-1 text-rose-200/90">{initFailedMessage}</p>
    </div>
  {/if}

  <div class="relative flex min-h-0 flex-1 flex-col overflow-hidden">
    <HubView />
  </div>

  <CommonFooter />
</div>
