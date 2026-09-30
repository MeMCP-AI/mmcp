//! Project configuration loaded from `.mmcp/config.toml`.
//!
//! This module defines the typed schema for the TOML file that lives
//! at the root of each mmcp-managed project. The file is committed to
//! the project's own version control so every contributor resolves
//! the same project identity and group load set.
//!
//! Parsing is done through `serde` + the `toml` crate. Rendering back
//! out is symmetric so round-tripping preserves unknown fields.

mod config_key;
mod config_key_parse_error;
mod config_scope;
mod env_vars;
mod error;
mod local_config;
mod notice;
mod project;
mod remote;
mod remote_auth;
mod sync_config;
pub mod user;

pub use config_key::ConfigKey;
pub use config_key_parse_error::ConfigKeyParseError;
pub use config_scope::ConfigScope;
pub use env_vars::{SYNC_PUSH_TOKEN_ENV, SYNC_TOKEN_ENV};
pub use error::ConfigError;
pub use local_config::{LOCAL_CONFIG_EXCLUDE_PATTERN, LOCAL_CONFIG_FILENAME, LocalConfig};
pub use notice::{
    LaunchValues, MdNoticeConfig, NoticeConfig, NoticeLaunch, NoticeLayers, NoticeResolution,
    NoticeSource, NoticeValue, NoticeValueParseError,
};
pub use project::{ProjectConfig, SubscriptionsConfig, is_group_adopted};
pub use remote::Remote;
pub use remote_auth::RemoteAuth;
pub use sync_config::SyncConfig;
pub use user::{AuthorConfig, DefaultsConfig, LimitsConfig, UserConfig};
