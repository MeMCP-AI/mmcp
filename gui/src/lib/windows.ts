// Single-instance window helpers. Each dedicated child window
// (Settings, Diagnostics, …) picks a fixed label so the OS-level
// WebviewWindow.getByLabel() lookup returns the existing one
// instead of spawning a duplicate. Focusing the existing window is
// the expected behaviour when the user re-clicks the toolbar.
//
// Routes are SvelteKit routes served by adapter-static + Tauri's
// `frontendDist`. Passing the bare path ("/settings") resolves
// against the same origin the main window uses.

import { WebviewWindow } from '@tauri-apps/api/webviewWindow';

interface WindowSpec {
  label: string;
  url: string;
  title: string;
  width: number;
  height: number;
  minWidth?: number;
  minHeight?: number;
}

async function openSingleton(spec: WindowSpec): Promise<void> {
  const existing = await WebviewWindow.getByLabel(spec.label);
  if (existing) {
    try {
      await existing.unminimize();
      await existing.setFocus();
      return;
    } catch {
      // Window might have been closed between lookup and focus; fall
      // through to spawn a fresh one.
      const retry = await WebviewWindow.getByLabel(spec.label);
      if (retry) return;
    }
  }
  new WebviewWindow(spec.label, {
    url: spec.url,
    title: spec.title,
    width: spec.width,
    height: spec.height,
    minWidth: spec.minWidth,
    minHeight: spec.minHeight,
    resizable: true,
    focus: true,
    // Child windows share the main window's custom chrome. A
    // thin ChildTitleBar inside each route handles drag + close.
    decorations: false
  });
}

export function openSettingsWindow(): Promise<void> {
  return openSingleton({
    label: 'settings',
    url: '/settings',
    title: 'mmcp-gui · Settings',
    width: 920,
    height: 640,
    minWidth: 640,
    minHeight: 480
  });
}

export function openDiagnosticsWindow(): Promise<void> {
  return openSingleton({
    label: 'diagnostics',
    url: '/diagnostics',
    title: 'mmcp-gui · Diagnostics',
    width: 1000,
    height: 720,
    minWidth: 720,
    minHeight: 520
  });
}
