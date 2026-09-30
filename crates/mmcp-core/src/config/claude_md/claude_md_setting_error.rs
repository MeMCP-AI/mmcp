//! [`ClaudeMdSettingError`], why a `claude_md` table holds no usable setting.

use thiserror::Error;

/// A `claude_md` table whose content is not a valid setting.
/// The table counts as unset, and the rest of its file loads.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ClaudeMdSettingError {
    /// The `claude_md` key holds a value that is not a table.
    #[error("claude_md must be a table, found {found}")]
    NotATable {
        /// TOML type name of the value found.
        found: &'static str,
    },

    /// `project_file_suggestion` holds a value outside the accepted spellings.
    #[error("invalid value {value} for project_file_suggestion")]
    InvalidValue {
        /// Raw TOML rendering of the value found.
        value: String,
    },
}
