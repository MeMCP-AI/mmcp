<script lang="ts">
  import SettingsPanel from '$lib/components/SettingsPanel.svelte';
  import {
    loadProjectConfig,
    loadUserConfig,
    saveProjectConfig,
    saveUserConfig
  } from '$lib/api/config';
  import { setReferencePoint } from '$lib/api/workspace';
  import { settingsStore, type KindDisplay } from '$lib/stores/settings.svelte';
  import type { LoadedProjectConfig, ProjectConfig, UserConfig } from '$lib/types';
  import { emit } from '@tauri-apps/api/event';
  import { open } from '@tauri-apps/plugin-dialog';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  // Each Tauri webview runs its own JS context — settings loaded in
  // the main window don't reach this one, so we mount our own copy
  // on entry. Changes are persisted through save_settings, which
  // the main window picks up via the `settings:changed` event.
  let userConfig = $state<UserConfig | null>(null);
  let userPath = $state<string | null>(null);
  let projectConfig = $state<LoadedProjectConfig | null>(null);
  let saving = $state(false);
  let lastError = $state<string | null>(null);

  $effect(() => {
    void settingsStore.mount();
    void refreshConfigs();
  });

  async function refreshConfigs() {
    lastError = null;
    try {
      const [u, p] = await Promise.all([
        loadUserConfig(),
        loadProjectConfig(settingsStore.values.reference_point)
      ]);
      userConfig = u.config;
      userPath = u.path;
      projectConfig = p;
    } catch (err) {
      lastError = formatErr(err);
    }
  }

  async function pickReferencePoint() {
    lastError = null;
    const chosen = await open({
      directory: true,
      multiple: false,
      title: 'Choose reference point',
      defaultPath: settingsStore.values.reference_point ?? undefined
    });
    if (typeof chosen !== 'string') return;
    await applyReferencePoint(chosen);
  }

  async function clearReferencePoint() {
    await applyReferencePoint(null);
  }

  async function applyReferencePoint(path: string | null) {
    saving = true;
    lastError = null;
    try {
      settingsStore.setReferencePoint(path);
      await setReferencePoint(path);
      await emit('workspace:changed');
      projectConfig = await loadProjectConfig(path);
    } catch (err) {
      lastError = formatErr(err);
    } finally {
      saving = false;
    }
  }

  async function onSaveUser(cfg: UserConfig) {
    saving = true;
    lastError = null;
    try {
      await saveUserConfig(cfg);
      const refreshed = await loadUserConfig();
      userConfig = refreshed.config;
      userPath = refreshed.path;
    } catch (err) {
      lastError = formatErr(err);
    } finally {
      saving = false;
    }
  }

  async function onSaveProject(root: string, cfg: ProjectConfig) {
    saving = true;
    lastError = null;
    try {
      await saveProjectConfig(root, cfg);
      projectConfig = await loadProjectConfig(settingsStore.values.reference_point);
    } catch (err) {
      lastError = formatErr(err);
    } finally {
      saving = false;
    }
  }

  function close() {
    void getCurrentWindow().close();
  }

  function formatErr(err: unknown): string {
    if (err && typeof err === 'object' && 'message' in err) {
      return String((err as { message: unknown }).message);
    }
    return String(err);
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
    referencePoint={settingsStore.values.reference_point}
    onPickReferencePoint={pickReferencePoint}
    onClearReferencePoint={clearReferencePoint}
    {userConfig}
    {userPath}
    {projectConfig}
    {onSaveUser}
    {onSaveProject}
    {saving}
    {lastError}
  />
</div>
