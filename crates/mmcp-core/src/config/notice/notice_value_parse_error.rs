//! [`NoticeValueParseError`], a spelling that is not a notice value.

use thiserror::Error;

/// The text is neither `on` nor `off`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid notice value {input}: accepted values are on and off")]
pub struct NoticeValueParseError {
    /// The text that failed to parse.
    pub input: String,
}
