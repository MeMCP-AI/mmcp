//! Conformance of served tool results to the output schema every tool
//! declares.
//!
//! `commands::serve` routes every `tools/call` through
//! [`conform_to_output_schema`]: the MCP specification requires
//! structured content on the result of a tool declaring an output
//! schema, and conforming clients refuse a result without it.

mod conform;
mod missing_structured_content;

pub(crate) use conform::conform_to_output_schema;
pub(crate) use missing_structured_content::MissingStructuredContent;
