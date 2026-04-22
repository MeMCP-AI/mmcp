<script lang="ts">
  import '../app.css';
  import { settingsStore } from '$lib/stores/settings.svelte';
  import { applyTheme } from '$lib/theme';

  let { children } = $props();

  // Each webview (main, settings, diagnostics) boots this layout,
  // mounts the settings store, and mirrors the chosen theme onto
  // `<html data-theme>`. When mode is `system`, the $effect also
  // tracks OS-level pref changes so a laptop flipping between
  // light / dark during use updates live.
  $effect(() => {
    void settingsStore.mount();
  });

  $effect(() => {
    const mode = settingsStore.values.theme;
    applyTheme(mode);
    if (mode !== 'system') return;
    if (typeof window === 'undefined' || !window.matchMedia) return;
    const mq = window.matchMedia('(prefers-color-scheme: light)');
    const handler = () => applyTheme('system');
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  });
</script>

<div class="flex h-full w-full flex-col">
  {@render children()}
</div>
