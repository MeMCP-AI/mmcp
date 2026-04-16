//! Memory model for mmcp.
//!
//! A memory is a single Markdown file with TOML frontmatter stored
//! inside a group's git repository. This module defines the typed
//! representation of that file and the auxiliary types used to manage
//! versions and kinds.
//!
//! Parsing and rendering of the `+++`-delimited frontmatter itself
//! lives in a forthcoming `frontmatter` submodule which consumes these
//! types.

mod bump;
mod entry;
mod frontmatter;
mod kind;
mod parser;
mod version;

pub use bump::BumpIntent;
pub use entry::Memory;
pub use frontmatter::MemoryFrontmatter;
pub use kind::MemoryKind;
pub use parser::{FrontmatterFormat, MemoryFile, MemoryParseError};
pub use version::Version;
