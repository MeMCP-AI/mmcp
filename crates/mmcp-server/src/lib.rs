//! mmcp server library.
//!
//! Re-exports the modules that integration tests and the binary
//! entry point both need.

pub mod app;
pub mod config;
mod defaults;
mod oauth_client;
pub mod routes;
pub mod state;
