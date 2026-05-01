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

mod body;
mod bump;
mod entry;
mod feature;
mod frontmatter;
mod issue;
mod kind;
mod parser;
mod refs;
mod status;
mod version;
mod xrefs;

pub use body::{BodyParseError, Section, parse_sections, render_sections, slugify_heading};
pub use bump::BumpIntent;
pub use entry::Memory;
pub use feature::{FeatureMetadata, FeatureStatus, FeatureStatusParseError};
pub use frontmatter::MemoryFrontmatter;
pub use issue::{
    IssueMetadata, IssueStatus, IssueStatusParseError, IssueSupersedeInvariantError,
};
pub use kind::MemoryKind;
pub use parser::{FrontmatterFormat, MemoryFile, MemoryParseError};
pub use refs::{InvalidCommit, MemoryRef};
pub use status::Status;
pub use version::Version;
pub use xrefs::{MemoryRefInput, XrefError, parse_cross_refs, parse_memory_refs};
