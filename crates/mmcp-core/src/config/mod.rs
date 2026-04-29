//! Project configuration loaded from `.mmcp/config.toml`.
//!
//! This module defines the typed schema for the TOML file that lives
//! at the root of each mmcp-managed project. The file is committed to
//! the project's own version control so every contributor resolves
//! the same project identity and group load set.
//!
//! Parsing is done through `serde` + the `toml` crate. Rendering back
//! out is symmetric so round-tripping preserves unknown fields.

mod error;
mod project;
pub mod user;

pub use error::ConfigError;
pub use project::{ProjectConfig, SubscriptionsConfig, SyncConfig};
pub use user::{AuthorConfig, DefaultsConfig, UserConfig};
