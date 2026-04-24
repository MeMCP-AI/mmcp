import type { ThemeMode } from '$lib/stores/settings.svelte';

/// Resolve the effective theme, expanding `'system'` to the
/// browser's `prefers-color-scheme` hint. Falls back to dark when
/// the matchMedia API is unavailable (older webviews) so the app
/// keeps its pre-light-mode appearance.
export type EffectiveTheme = 'dark' | 'light' | 'oled' | 'dim' | 'dim-dark';

export function resolveTheme(mode: ThemeMode): EffectiveTheme {
  if (
    mode === 'dark' ||
    mode === 'light' ||
    mode === 'oled' ||
    mode === 'dim' ||
    mode === 'dim-dark'
  )
    return mode;
  if (typeof window === 'undefined') return 'dark';
  if (!window.matchMedia) return 'dark';
  return window.matchMedia('(prefers-color-scheme: light)').matches
    ? 'light'
    : 'dark';
}

/// Apply the effective theme to `<html>` by setting the
/// `data-theme` attribute the app.css tokens switch on.
export function applyTheme(mode: ThemeMode): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.theme = resolveTheme(mode);
}
