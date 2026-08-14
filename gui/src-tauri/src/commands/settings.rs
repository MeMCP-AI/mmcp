//! Settings persistence — simple JSON file under the platform's
//! app-config dir. The frontend is the source of truth for settings
//! *shape*; this command just reads / writes the serialized blob.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::{GuiError, GuiResult};

/// Opaque settings blob. The frontend owns the schema; we just
/// round-trip serde_json::Value so adding a field on the frontend
/// needs no backend change.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SettingsBlob(pub serde_json::Value);

/// Lock around the on-disk settings file.
pub struct SettingsLock(pub Mutex<()>);

fn settings_path(app: &AppHandle) -> GuiResult<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| GuiError::Other(format!("app_config_dir: {e}")))?;
    fs::create_dir_all(&dir).map_err(|e| GuiError::Other(format!("create config dir: {e}")))?;
    Ok(dir.join("settings.json"))
}

#[tauri::command]
pub async fn load_settings(
    app: AppHandle,
    lock: tauri::State<'_, SettingsLock>,
) -> GuiResult<SettingsBlob> {
    let _guard = lock.0.lock().unwrap_or_else(|e| e.into_inner());
    let path = settings_path(&app)?;
    if !path.exists() {
        return Ok(SettingsBlob::default());
    }
    let text =
        fs::read_to_string(&path).map_err(|e| GuiError::Other(format!("read settings: {e}")))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| GuiError::Other(format!("parse settings: {e}")))?;
    Ok(SettingsBlob(value))
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    lock: tauri::State<'_, SettingsLock>,
    settings: SettingsBlob,
) -> GuiResult<()> {
    let _guard = lock.0.lock().unwrap_or_else(|e| e.into_inner());
    let path = settings_path(&app)?;
    let json = serde_json::to_string_pretty(&settings.0)
        .map_err(|e| GuiError::Other(format!("serialize settings: {e}")))?;
    fs::write(&path, json).map_err(|e| GuiError::Other(format!("write settings: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Backward-compat guard for issue #128: the frontend's
    /// `UiSettings` type dropped the dead `diff_view` / `ui_variant`
    /// fields, but `SettingsBlob` wraps `serde_json::Value` precisely
    /// so this backend never needs to know the frontend's schema.
    /// The exact `to_string_pretty` / `from_str` round trip
    /// `save_settings` / `load_settings` perform must preserve a
    /// legacy field verbatim rather than silently dropping it.
    #[test]
    fn legacy_fields_survive_the_save_load_round_trip() {
        let on_disk_blob = serde_json::json!({
            "kind_display": "icon_and_text",
            "reference_point": null,
            "layout_mode": "columns",
            "diff_view": "unified",
            "theme": "dark",
            "ui_variant": "hub",
            "pinned_groups": []
        });
        let blob = SettingsBlob(on_disk_blob.clone());

        // Mirrors save_settings's write path.
        let written = serde_json::to_string_pretty(&blob.0).expect("serialize settings");
        // Mirrors load_settings's read path.
        let restored: SettingsBlob = serde_json::from_str(&written).expect("parse settings");

        assert_eq!(restored.0, on_disk_blob, "round trip must be lossless");
        assert_eq!(restored.0["diff_view"], "unified");
        assert_eq!(restored.0["ui_variant"], "hub");
    }
}
