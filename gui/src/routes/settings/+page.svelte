<script lang="ts">
  import SettingsPanel from '$lib/components/SettingsPanel.svelte';
  import { settingsStore, type KindDisplay } from '$lib/stores/settings.svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  // Each Tauri webview runs its own JS context — settings loaded in
  // the main window don't reach this one, so we mount our own copy
  // on entry. Changes are persisted through save_settings, which the
  // main window will pick up via the `settings:changed` event (wired
  // in `+page.svelte`).
  $effect(() => {
    void settingsStore.mount();
  });

  function close() {
    void getCurrentWindow().close();
  }
</script>

<svelte:head>
  <title>mmcp-gui · Settings</title>
</svelte:head>

<div class="h-full w-full">
  <SettingsPanel
    value={settingsStore.values.kind_display}
    onChange={(mode: KindDisplay) => settingsStore.setKindDisplay(mode)}
    onReset={() => settingsStore.reset()}
    onClose={close}
  />
</div>
