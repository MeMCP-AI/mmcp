import { loadSettings, saveSettings, type SettingsBlob } from '$lib/api/settings';
import { emit, listen } from '@tauri-apps/api/event';

// Cross-window change notification. Settings live in a dedicated
// child window, so when the user edits them there the main window
// has to be told. `emit` broadcasts to every webview (including the
// emitter); the receiving listener just re-applies the payload.
const SETTINGS_CHANGED_EVENT = 'settings:changed';

export type KindDisplay = 'off' | 'icon' | 'text' | 'icon_and_text';
export type LayoutMode = 'columns' | 'stacked';
export type DiffViewMode = 'unified' | 'side_by_side' | 'inline_word';

export interface UiSettings {
  kind_display: KindDisplay;
  reference_point: string | null;
  layout_mode: LayoutMode;
  diff_view: DiffViewMode;
}

const DEFAULT_SETTINGS: UiSettings = {
  kind_display: 'icon_and_text',
  reference_point: null,
  layout_mode: 'columns',
  diff_view: 'unified'
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

  setDiffView(mode: DiffViewMode) {
    this.values.diff_view = mode;
    void this.save();
  }

  reset() {
    this.values = { ...DEFAULT_SETTINGS };
    void this.save();
  }
}

export const settingsStore = new SettingsStore();
