import { loadSettings, saveSettings, type SettingsBlob } from '$lib/api/settings';

export type KindDisplay = 'off' | 'icon' | 'text' | 'icon_and_text';

export interface UiSettings {
  kind_display: KindDisplay;
  reference_point: string | null;
}

const DEFAULT_SETTINGS: UiSettings = {
  kind_display: 'icon_and_text',
  reference_point: null
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
  }

  async save() {
    try {
      await saveSettings<UiSettings>(this.values as unknown as SettingsBlob<UiSettings>);
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

  reset() {
    this.values = { ...DEFAULT_SETTINGS };
    void this.save();
  }
}

export const settingsStore = new SettingsStore();
