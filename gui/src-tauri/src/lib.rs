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
use crate::state::AppState;

const PROBE_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct ReachabilityEvent {
    online: bool,
    reason: Option<String>,
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .manage(SettingsLock(Mutex::new(())))
        .invoke_handler(tauri::generate_handler![
            commands::groups::list_groups,
            commands::groups::refresh_groups,
            commands::memory::list_memory_slugs,
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
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match AppState::discover().await {
                    Ok(state) => {
                        let server_url = state.sync.as_ref().map(|s| s.server_url.clone());
                        handle.manage(state);
                        tracing::info!("AppState discovered, commands are live");
                        if let Some(url) = server_url {
                            tauri::async_runtime::spawn(probe_loop(handle.clone(), url));
                        }
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
/// controls.
async fn probe_loop(handle: AppHandle, server_url: String) {
    let client = match SyncClient::new(&server_url) {
        Ok(c) => c,
        Err(err) => {
            let _ = handle.emit(
                "reachability:changed",
                ReachabilityEvent {
                    online: false,
                    reason: Some(err.to_string()),
                },
            );
            return;
        }
    };
    let mut ticker = interval(PROBE_INTERVAL);
    loop {
        ticker.tick().await;
        let event = match client.get_manifest().await {
            Ok(_) => ReachabilityEvent {
                online: true,
                reason: None,
            },
            // Only transport failures count as "offline". Remote 401 /
            // 500 responses mean the server is alive — the user
            // should still be allowed to click Pull / Push and see a
            // clear error.
            Err(SyncError::Transport(msg)) => ReachabilityEvent {
                online: false,
                reason: Some(msg),
            },
            Err(_) => ReachabilityEvent {
                online: true,
                reason: None,
            },
        };
        if handle.emit("reachability:changed", event).is_err() {
            break;
        }
    }
}
