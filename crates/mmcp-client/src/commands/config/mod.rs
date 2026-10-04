//! Reading and writing configuration keys through the user, project and local files.
//!
//! The shared owner of the `config` operations: the MCP tool and the `mmcp config` CLI are adapters over it.
//! [`NoticeSources`] is also what `bootstrap_context` resolves the notice keys from.

mod config_environment;
#[cfg(test)]
mod config_fixture;
mod config_get;
mod config_op_error;
mod config_write;
mod config_write_outcome;
mod notice_sources;

pub use config_environment::{ConfigEnvironment, LocalExclusion};
pub use config_get::get_key;
pub use config_op_error::ConfigOpError;
pub use config_write::{set_key, unset_key};
pub use config_write_outcome::ConfigWriteOutcome;
pub use notice_sources::NoticeSources;
