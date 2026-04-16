//! Library surface of the mmcp client binary.
//!
//! The binary entry point in `main.rs` is a thin clap dispatcher
//! over the modules exported here. Exposing them through a
//! library target lets integration tests under `tests/` drive
//! the same state, commands, and configuration code the binary
//! runs.

pub mod commands;
pub mod config;
pub mod home;
pub mod state;
