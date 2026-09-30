//! The `notice.*` configuration keys and their resolution across configuration layers.
//!
//! One `[notice]` table type is reused in `~/.mmcp/config.toml`, `.mmcp.toml` and `.mmcp.local.toml`.
//! [`NoticeLayers`] resolves the per-layer values of one key into its effective value.

mod launch_values;
mod md_notice_config;
mod notice_config;
mod notice_launch;
mod notice_layers;
mod notice_resolution;
mod notice_source;
mod notice_value;
mod notice_value_parse_error;

pub use launch_values::LaunchValues;
pub use md_notice_config::MdNoticeConfig;
pub use notice_config::NoticeConfig;
pub use notice_launch::NoticeLaunch;
pub use notice_layers::NoticeLayers;
pub use notice_resolution::NoticeResolution;
pub use notice_source::NoticeSource;
pub use notice_value::NoticeValue;
pub use notice_value_parse_error::NoticeValueParseError;
