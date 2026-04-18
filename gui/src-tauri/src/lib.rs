//! `mmcp-gui-tauri` — Tauri 2 backend for the mmcp desktop client.
//!
//! This commit is scaffolding only: the builder boots with zero
//! commands registered. The next commit wires command modules for
//! groups / memory / sync / diagnose / settings plus the
//! reachability probe loop.

#![forbid(unsafe_code)]

use tracing_subscriber::EnvFilter;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .setup(|_app| {
            tracing::info!("mmcp-gui scaffolding — backend ready, no commands wired yet");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start mmcp-gui Tauri app");
}
