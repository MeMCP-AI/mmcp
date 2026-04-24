<script lang="ts">
  import ChildTitleBar from '$lib/components/ChildTitleBar.svelte';
  import DiagnosticsPanel from '$lib/components/DiagnosticsPanel.svelte';
  import { diagnosticsStore, type SeverityFilter } from '$lib/stores/diagnostics.svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  // Fire the first diagnose_all on open. The user can re-run from
  // the Refresh button in the panel header.
  $effect(() => {
    void diagnosticsStore.run();
  });

  function close() {
    void getCurrentWindow().close();
  }
</script>

<svelte:head>
  <title>mmcp-gui · Diagnostics</title>
</svelte:head>

<div class="flex h-full w-full flex-col bg-surface-0 text-fg">
  <ChildTitleBar title="mmcp-gui · Diagnostics" />
  <div class="min-h-0 flex-1">
    <DiagnosticsPanel
      report={diagnosticsStore.report}
      loading={diagnosticsStore.loading}
      error={diagnosticsStore.error}
      filter={diagnosticsStore.filter}
      collapsed={diagnosticsStore.collapsed}
      onClose={close}
      onRefresh={() => diagnosticsStore.run()}
      onFilterChange={(f: SeverityFilter) => diagnosticsStore.setFilter(f)}
      onToggleGroup={(slug: string) => diagnosticsStore.toggle(slug)}
    />
  </div>
</div>
