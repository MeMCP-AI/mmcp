import { loadSettings, saveSettings, type SettingsBlob } from '$lib/api/settings';
import { emit, listen } from '@tauri-apps/api/event';

// Cross-window change notification. Settings live in a dedicated
// child window, so when the user edits them there the main window
// has to be told. `emit` broadcasts to every webview (including the
// emitter); the receiving listener just re-applies the payload.
const SETTINGS_CHANGED_EVENT = 'settings:changed';

export type KindDisplay = 'off' | 'icon' | 'text' | 'icon_and_text';
export type LayoutMode = 'columns' | 'stacked';
export type ThemeMode = 'dark' | 'light' | 'system' | 'oled' | 'dim' | 'dim-dark';

export interface UiSettings {
  kind_display: KindDisplay;
  reference_point: string | null;
  layout_mode: LayoutMode;
  theme: ThemeMode;
  /** UUIDs of groups the user has pinned on the home dashboard.
   * GitHub-style "starred repos" — scales with thousands of groups
   * because the long tail never makes it into the dashboard unless
   * the user opts in. */
  pinned_groups: string[];
}

// `diff_view` / `ui_variant` used to live here (a `repo`/`feed`
// variant switcher and a diff-view-mode picker, both dead code —
// see issue #128). Dropping them from `UiSettings` does not drop
// them from an on-disk blob written by an older build: `mount()`
// spreads the parsed JSON over `DEFAULT_SETTINGS`, and a plain
// object spread keeps every key the parsed JSON actually has,
// typed or not, so `save()` round-trips them unchanged.
const DEFAULT_SETTINGS: UiSettings = {
  kind_display: 'icon_and_text',
  reference_point: null,
  layout_mode: 'columns',
  theme: 'dark',
  pinned_groups: []
};

class SettingsStore {
  values = $state<UiSettings>({ ...DEFAULT_SETTINGS });
  loaded = $state(false);

  async mount() {
    if (this.loaded) return;
    try {
      const blob = await loadSettings<UiSettings>();
      const raw = (blob as unknown as UiSettings) ?? DEFAULT_SETTINGS;
      this.values = { ...DEFAULT_SETTINGS, ...raw };
    } catch {
      this.values = { ...DEFAULT_SETTINGS };
    } finally {
      this.loaded = true;
    }
    void listen<UiSettings>(SETTINGS_CHANGED_EVENT, (e) => {
      if (e.payload) this.values = { ...DEFAULT_SETTINGS, ...e.payload };
    });
  }

  async save() {
    try {
      await saveSettings<UiSettings>(this.values as unknown as SettingsBlob<UiSettings>);
      await emit(SETTINGS_CHANGED_EVENT, this.values);
    } catch (err) {
      console.warn('save settings failed', err);
    }
  }

  setKindDisplay(mode: KindDisplay) {
    this.values.kind_display = mode;
    void this.save();
  }

  setReferencePoint(path: string | null) {
    this.values.reference_point = path && path.trim().length > 0 ? path : null;
    void this.save();
  }

  setLayoutMode(mode: LayoutMode) {
    this.values.layout_mode = mode;
    void this.save();
  }

  toggleLayoutMode() {
    this.setLayoutMode(this.values.layout_mode === 'columns' ? 'stacked' : 'columns');
  }

  setTheme(mode: ThemeMode) {
    this.values.theme = mode;
    void this.save();
  }

  /// Advance through the theme modes in a fixed order. Backs the
  /// toolbar's quick-switch button, which doesn't expose a full
  /// picker — cycling keeps the affordance to a single click
  /// while still reaching all three options.
  cycleTheme() {
    const order: ThemeMode[] = ['oled', 'dark', 'dim-dark', 'dim', 'light', 'system'];
    const next = order[(order.indexOf(this.values.theme) + 1) % order.length];
    this.setTheme(next);
  }

  togglePinnedGroup(groupId: string) {
    const set = new Set(this.values.pinned_groups);
    if (set.has(groupId)) set.delete(groupId);
    else set.add(groupId);
    this.values.pinned_groups = Array.from(set);
    void this.save();
  }

  isGroupPinned(groupId: string): boolean {
    return this.values.pinned_groups.includes(groupId);
  }

  reset() {
    this.values = { ...DEFAULT_SETTINGS };
    void this.save();
  }
}

export const settingsStore = new SettingsStore();
