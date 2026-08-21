//! `mmcp-gui-tauri` — Tauri 2 backend for the mmcp desktop client.
//!
//! Boots the app, constructs `AppState` (backend + group index +
//! optional sync bundle + resolved author), registers every
//! `#[tauri::command]` the frontend can invoke, and spawns the 15 s
//! reachability probe that emits `reachability:changed` events when
//! the configured sync server stops responding.

#![forbid(unsafe_code)]

pub mod commands;
pub mod error;
pub mod state;

use std::sync::Mutex;
use std::time::Duration;

use mmcp_sync::{SyncClient, SyncError};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::interval;
use tracing_subscriber::EnvFilter;

use crate::commands::settings::SettingsLock;
use crate::commands::workspace::{ProbeAction, probe_action_for};
use crate::state::AppState;

const PROBE_INTERVAL: Duration = Duration::from_secs(15);
const REACHABILITY_CHANGED_EVENT: &str = "reachability:changed";

/// Payload for the `reachability:changed` event, the frontend's only
/// source of the sync-server reachability badge.
///
/// Three states, not a `bool` plus `Option<String>`: `NotApplicable`
/// is distinct from `Offline`, not a degenerate case of it. Emitted
/// whenever the active default remote has no manifest endpoint to
/// probe (`direct-git`) or no sync is configured at all, so the
/// frontend's `ReachabilityStore` never keeps showing a PREVIOUS
/// workspace's online/offline reading for a remote nothing has
/// checked (frontend: `gui/src/lib/stores/reachability.svelte.ts`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
enum ReachabilityEvent {
    Online,
    Offline {
        reason: String,
    },
    /// No probe applies to the current default remote: either no
    /// sync is configured, or the default remote is `direct-git`
    /// (no `/sync/manifest` endpoint to poll).
    NotApplicable,
}

/// Emit [`ReachabilityEvent::NotApplicable`] on the shared
/// `reachability:changed` event. Called whenever a sync-bundle
/// rebuild (workspace switch) lands on a remote set with no
/// `probe_url`, so the badge resets instead of keeping the OLD
/// workspace's online/offline reading.
pub(crate) fn emit_reachability_not_applicable(handle: &AppHandle) {
    let _ = handle.emit(REACHABILITY_CHANGED_EVENT, ReachabilityEvent::NotApplicable);
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(SettingsLock(Mutex::new(())))
        .invoke_handler(tauri::generate_handler![
            commands::groups::list_groups,
            commands::groups::refresh_groups,
            commands::archive::export_archive,
            commands::archive::import_archive,
            commands::archive::pick_import_path,
            commands::archive::inspect_archive,
            commands::archive::local_tags,
            commands::memory::list_memory_slugs,
            commands::memory::list_memory_descriptors,
            commands::memory::load_memory,
            commands::memory::create_memory,
            commands::memory::update_memory,
            commands::memory::delete_memory,
            commands::sync::sync_status,
            commands::sync::sync_pull,
            commands::sync::sync_push,
            commands::diagnose::run_diagnose,
            commands::settings::load_settings,
            commands::settings::save_settings,
            commands::workspace::set_reference_point,
            commands::workspace::pick_directory,
            commands::config::load_user_config,
            commands::config::save_user_config,
            commands::config::load_project_config,
            commands::config::save_project_config,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let discover_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                match AppState::discover(&discover_handle).await {
                    Ok(state) => {
                        let probe_url = state
                            .sync
                            .read()
                            .await
                            .as_ref()
                            .and_then(|s| s.probe_url.clone());
                        // Reuse the same decision `set_reference_point`
                        // makes on a workspace switch: a `direct-git`
                        // default (or no sync at all) has no
                        // `probe_url`, and must reset the badge to the
                        // explicit not-applicable state rather than
                        // leave it stuck on "probing…" forever with no
                        // `reachability:changed` event ever emitted.
                        match probe_action_for(probe_url) {
                            ProbeAction::Spawn(url) => {
                                let probe =
                                    tauri::async_runtime::spawn(probe_loop(handle.clone(), url));
                                *state.probe.lock().await = Some(probe);
                            }
                            ProbeAction::Reset => emit_reachability_not_applicable(&handle),
                        }
                        handle.manage(state);
                        tracing::info!("AppState discovered, commands are live");
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "AppState discovery failed");
                        let _ = handle.emit(
                            "app:init-failed",
                            serde_json::json!({ "message": err.to_string() }),
                        );
                    }
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start mmcp-gui Tauri app");
}

/// Periodic (15 s) probe against the sync server's manifest
/// endpoint. Every flip from online → offline or back emits
/// `reachability:changed` so the frontend can gate Pull / Push
/// controls. Exposed `pub(crate)` so the workspace command can
/// restart a fresh probe when the reference point — and therefore
/// the active server URL — changes at runtime.
pub(crate) async fn probe_loop(handle: AppHandle, server_url: String) {
    let client = match SyncClient::new(&server_url) {
        Ok(c) => c,
        Err(err) => {
            let _ = handle.emit(
                REACHABILITY_CHANGED_EVENT,
                ReachabilityEvent::Offline {
                    reason: err.to_string(),
                },
            );
            return;
        }
    };
    let mut ticker = interval(PROBE_INTERVAL);
    loop {
        ticker.tick().await;
        let event = match client.get_manifest().await {
            Ok(_) => ReachabilityEvent::Online,
            // Only transport failures count as "offline". Remote 401 /
            // 500 responses mean the server is alive, the user should
            // still be allowed to click Pull / Push and see a clear
            // error.
            Err(SyncError::Transport(msg)) => ReachabilityEvent::Offline { reason: msg },
            Err(_) => ReachabilityEvent::Online,
        };
        if handle.emit(REACHABILITY_CHANGED_EVENT, event).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// The frontend's `ReachabilityEvent` TS union
    /// (`gui/src/lib/types.ts`) discriminates on a `status` field with
    /// exactly these three string values. A wire-shape drift here
    /// breaks that contract silently (both sides compile; the
    /// frontend just stops matching any arm), so pin it down as a
    /// structural test instead of relying on the two sides staying in
    /// sync by convention.
    #[test]
    fn reachability_event_wire_shape_tags_on_status() {
        assert_eq!(
            serde_json::to_value(ReachabilityEvent::Online).unwrap(),
            serde_json::json!({ "status": "online" })
        );
        assert_eq!(
            serde_json::to_value(ReachabilityEvent::Offline {
                reason: "connection refused".to_string()
            })
            .unwrap(),
            serde_json::json!({ "status": "offline", "reason": "connection refused" })
        );
        assert_eq!(
            serde_json::to_value(ReachabilityEvent::NotApplicable).unwrap(),
            serde_json::json!({ "status": "not_applicable" })
        );
    }
}
