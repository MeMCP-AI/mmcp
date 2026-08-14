<script lang="ts">
  // App shell. Renders the shared navbar, the active variant, and
  // the shared footer. Every bit of variant-specific logic lives
  // inside the variants themselves — this file just routes on
  // `settingsStore.values.ui_variant` and wires the background
  // mirror-refresh cascade that the stores need to stay fresh.

  import CommonFooter from '$lib/components/CommonFooter.svelte';
  import TitleBar from '$lib/components/TitleBar.svelte';
  import HubView from '$lib/components/variants/HubView.svelte';

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
  //
  // Events are coalesced (issue #126): a single sync pull touching
  // several groups fires one `mirror:changed` event PER group, each
  // in its own debounce window on the Rust side. Handling each event
  // independently meant one pull of 8 groups triggered 8 full
  // `refreshQuiet()` rescans. Every event arriving within
  // `MIRROR_EVENT_COALESCE_MS` of the first is instead folded into
  // one pending batch; the batch flushes as a single `refreshQuiet()`
  // call plus at most one `refreshGroup()` call for the active group.
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

  <!-- Hub is the only live variant; `settingsStore.values.ui_variant`
       stays in case a future experiment re-introduces the
       switcher. -->
  <div class="relative flex min-h-0 flex-1 flex-col overflow-hidden">
    <HubView />
  </div>

  <CommonFooter />
</div>
