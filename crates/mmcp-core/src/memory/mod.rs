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
mod limits;
mod milestone;
mod parser;
mod refs;
mod response_budget;
mod splice;
mod status;
mod version;
mod xrefs;

pub use body::{
    BodyParseError, Section, line_count, parse_sections, render_sections, slugify_heading,
};
pub use bump::BumpIntent;
pub use entry::Memory;
pub use feature::{FeatureMetadata, FeatureStatus, FeatureStatusParseError};
pub use frontmatter::MemoryFrontmatter;
pub use issue::{IssueMetadata, IssueStatus, IssueStatusParseError, IssueSupersedeInvariantError};
pub use kind::{MemoryKind, MemoryKindParseError};
pub use limits::{
    FieldLengthError, MAX_BODY_LENGTH, MAX_DESCRIPTION_LENGTH, MAX_MESSAGE_LENGTH, MAX_NAME_LENGTH,
    MAX_TAG_COUNT, MAX_TAG_LENGTH, MCP_CLIENT_RESULT_CEILING_BYTES, validate_body_length,
    validate_field_length, validate_frontmatter_lengths, validate_message_length, validate_tags,
};
pub use milestone::{MilestoneMetadata, MilestoneStatus, MilestoneStatusParseError};
pub use parser::{FrontmatterFormat, MemoryFile, MemoryParseError, parse_frontmatter};
pub use refs::{COMMIT_SHA_HEX_LEN, InvalidCommit, MemoryRef, looks_like_commit_sha};
pub use response_budget::{
    COMPACT_RECORD_ESTIMATED_BYTES, DEFAULT_LIST_MEMORIES_LIMIT, MAX_LIST_MEMORIES_LIMIT,
    ResponseEnvelope,
};
pub use splice::{SpliceError, line_terminator, splice};
pub use status::Status;
pub use version::Version;
pub use xrefs::{MemoryRefInput, XrefError, parse_cross_refs, parse_memory_refs};
