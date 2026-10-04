//! [`ConfigToolReply`], what a successful `config` tool call returns.

use serde_json::Value;

/// The result object of a `config` tool call and the advisories that ride beside it.
#[derive(Debug)]
pub struct ConfigToolReply {
    /// The result object.
    pub result: Value,
    /// Advisories about a call that succeeded, such as a write whose effective value could not be read back.
    pub notes: Vec<mmcp_proto::Note>,
}
