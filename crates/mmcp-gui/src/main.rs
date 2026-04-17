//! `mmcp-gui` binary entry point.
//!
//! Boots a `tracing` subscriber, spins up a multi-thread tokio
//! runtime, applies the app's visual preset, spawns the background
//! worker that owns every `mmcp-store` call, and hands the native
//! window off to [`app::MmcpGuiApp`]. Every piece of UI or I/O
//! lives in the library modules — `main.rs` stays a thin launcher
//! so integration tests can construct `MmcpGuiApp` directly without
//! going through the eframe event loop.

#![forbid(unsafe_code)]

mod app;
mod error;
mod io;
mod runtime;
mod state;
mod style;
mod ui;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use crate::app::MmcpGuiApp;
use crate::runtime::BackgroundHandle;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("tokio runtime: {e}"))?;

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([600.0, 400.0])
            .with_title("mmcp-gui"),
        ..Default::default()
    };

    eframe::run_native(
        "mmcp-gui",
        native_options,
        Box::new(move |cc| {
            style::configure(&cc.egui_ctx);
            let background = BackgroundHandle::spawn(&runtime, cc.egui_ctx.clone());
            Ok(Box::new(MmcpGuiApp::new(background, runtime)))
        }),
    )
    .map_err(|err| anyhow::anyhow!("eframe::run_native failed: {err}"))
}
