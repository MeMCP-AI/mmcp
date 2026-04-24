<script lang="ts">
  // Shared chrome strip — dropped into every variant's top nav so
  // the user never loses reach to the app-level controls (server
  // status, theme, settings, diagnostics, variant switch) even in
  // layouts that strip the classic toolbar.

  import { Monitor, Moon, Settings as SettingsIcon, Stethoscope, Sun } from 'lucide-svelte';
  import SyncBadge from './SyncBadge.svelte';
  import VariantSwitcher from './variants/VariantSwitcher.svelte';
  import { settingsStore, type ThemeMode, type UiVariant } from '$lib/stores/settings.svelte';
  import { openDiagnosticsWindow, openSettingsWindow } from '$lib/windows';

  const theme = $derived(settingsStore.values.theme);

  const themeIcon = $derived(theme === 'dark' ? Moon : theme === 'light' ? Sun : Monitor);
  const themeLabel = $derived(theme === 'dark' ? 'Dark' : theme === 'light' ? 'Light' : 'System');
  const themeNext = $derived(theme === 'dark' ? 'Light' : theme === 'light' ? 'System' : 'Dark');

  function chooseVariant(v: UiVariant) {
    settingsStore.setUiVariant(v);
  }
</script>

<div class="flex items-center gap-2">
  <SyncBadge />

  <button
    type="button"
    class="inline-flex items-center gap-1 rounded-md border border-line bg-surface-0 px-2 py-0.5 text-[11px] text-fg hover:bg-surface-2"
    title={`Theme: ${themeLabel} — click to switch to ${themeNext}`}
    aria-label={`Theme: ${themeLabel}`}
    onclick={() => settingsStore.cycleTheme()}
  >
    {#if theme === 'dark'}<Moon size={11} />{/if}
    {#if theme === 'light'}<Sun size={11} />{/if}
    {#if theme === 'system'}<Monitor size={11} />{/if}
    <span>{themeLabel}</span>
  </button>

  <button
    type="button"
    class="inline-flex items-center rounded-md border border-line bg-surface-0 p-1 text-fg hover:bg-surface-2"
    title="Diagnose"
    aria-label="Diagnose"
    onclick={() => void openDiagnosticsWindow()}
  >
    <Stethoscope size={12} />
  </button>

  <button
    type="button"
    class="inline-flex items-center rounded-md border border-line bg-surface-0 p-1 text-fg hover:bg-surface-2"
    title="Settings"
    aria-label="Settings"
    onclick={() => void openSettingsWindow()}
  >
    <SettingsIcon size={12} />
  </button>

  <VariantSwitcher
    current={settingsStore.values.ui_variant}
    onSelect={chooseVariant}
  />
</div>
