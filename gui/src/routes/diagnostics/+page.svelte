<script lang="ts">
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

<div class="h-full w-full">
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
