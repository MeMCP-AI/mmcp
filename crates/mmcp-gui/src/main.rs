//! `mmcp-gui` binary entry point.
//!
//! Boots a `tracing` subscriber, initialises `eframe`, and hands the
//! native window to [`app::MmcpGuiApp`]. Every piece of UI or I/O
//! lives in the library modules — `main.rs` stays a thin launcher so
//! integration tests can construct `MmcpGuiApp` directly without
//! going through the eframe event loop.

#![forbid(unsafe_code)]

mod app;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use crate::app::MmcpGuiApp;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

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
        Box::new(|_cc| Ok(Box::new(MmcpGuiApp))),
    )
    .map_err(|err| anyhow::anyhow!("eframe::run_native failed: {err}"))
}
