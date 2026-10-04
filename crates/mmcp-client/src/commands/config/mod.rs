//! Reading and writing configuration keys through the user, project and local files.
//!
//! The shared owner of the `config` operations: the MCP tool and the `mmcp config` CLI are adapters over it.
//! [`NoticeSources`] is also what `bootstrap_context` resolves the notice keys from.

mod config_action;
mod config_args_error;
mod config_cli;
mod config_command;
mod config_environment;
#[cfg(test)]
mod config_fixture;
mod config_get;
mod config_key_arg;
mod config_op_error;
mod config_outcome;
mod config_scope_arg;
mod config_tool;
mod config_tool_args;
mod config_write;
mod config_write_outcome;
mod error_chain;
mod notice_launch_args;
mod notice_sources;
mod notice_value_arg;
mod project_location;

pub use config_action::ConfigAction;
pub use config_args_error::ConfigArgsError;
pub use config_cli::{ConfigCliArgs, run};
pub use config_command::ConfigCommand;
pub use config_environment::ConfigEnvironment;
pub use config_get::get_key;
pub use config_key_arg::ConfigKeyArg;
pub use config_op_error::ConfigOpError;
pub use config_outcome::ConfigOutcome;
pub use config_scope_arg::ConfigScopeArg;
pub use config_tool::call_config_tool;
pub use config_tool_args::ConfigToolArgs;
pub use config_write::{set_key, unset_key};
pub use config_write_outcome::ConfigWriteOutcome;
pub use notice_launch_args::{notice_launch_from, with_notice_flags};
pub use notice_sources::NoticeSources;
pub use notice_value_arg::NoticeValueArg;
pub use project_location::ProjectLocation;
