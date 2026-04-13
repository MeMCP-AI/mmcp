//! Load-set resolution for a project session.
//!
//! Given a parsed [`ProjectConfig`](crate::config::ProjectConfig) and
//! an optional list of auto-detected languages, compute the ordered
//! list of group references the client should pull before the
//! session starts.
//!
//! This module performs no I/O. The caller is responsible for running
//! the filesystem scan that feeds the language list (typically
//! `mmcp-client` at session startup) and for resolving the returned
//! references to concrete [`GroupId`](crate::id::GroupId) values via
//! the server.

mod reference;
mod resolver;

pub use reference::GroupRef;
pub use resolver::{LoadSet, resolve_load_set};
