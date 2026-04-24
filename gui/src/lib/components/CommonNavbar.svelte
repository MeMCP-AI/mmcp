<script lang="ts">
  // App-shell top bar. Sits above every variant's own chrome so
  // theme / settings / variant-switch are reachable from any
  // layout without each variant re-implementing them.

  import { Monitor, Moon, Settings as SettingsIcon, Sun } from 'lucide-svelte';
  import VariantSwitcher from './variants/VariantSwitcher.svelte';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { openSettingsWindow } from '$lib/windows';

  const theme = $derived(settingsStore.values.theme);
  const themeLabel = $derived(
    theme === 'dark' ? 'Dark' : theme === 'light' ? 'Light' : 'System'
  );
  const themeNext = $derived(
    theme === 'dark' ? 'Light' : theme === 'light' ? 'System' : 'Dark'
  );
</script>

<header
  class="flex h-10 shrink-0 items-center gap-3 border-b border-line bg-surface-1 px-3 sm:px-4"
>
  <span class="select-none text-sm font-semibold tracking-tight text-fg">mmcp</span>

  <div class="ml-auto flex items-center gap-2">
    <VariantSwitcher
      current={settingsStore.values.ui_variant}
      onSelect={(v) => settingsStore.setUiVariant(v)}
    />

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
      title="Settings"
      aria-label="Settings"
      onclick={() => void openSettingsWindow()}
    >
      <SettingsIcon size={12} />
    </button>
  </div>
</header>
