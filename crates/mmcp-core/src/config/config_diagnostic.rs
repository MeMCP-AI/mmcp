//! [`ConfigDiagnostic`], a finding of a configuration load that does not fail the load.

use thiserror::Error;

use super::ClaudeMdSettingError;

/// A mistake in a configuration file that the loader tolerates.
/// The loader returns these beside the loaded config; the path of the file is attached by the caller that read it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigDiagnostic {
    /// A key the loader does not know and ignores.
    #[error("unknown key {key_path} is ignored")]
    UnknownKey {
        /// Dotted key path of the ignored key.
        key_path: String,
    },

    /// A `claude_md` table holds an invalid setting and counts as unset.
    #[error("the claude_md table {table_path} holds an invalid setting: {error}")]
    ClaudeMdSettingInvalid {
        /// Dotted key path of the table.
        table_path: String,
        /// Why the table holds no usable setting.
        #[source]
        error: ClaudeMdSettingError,
    },

    /// An entry of the `projects` table is keyed by something that is not a project UUID, so no project reads it.
    #[error("projects entry {key} is not keyed by a project UUID and is ignored")]
    ProjectKeyNotUuid {
        /// The key of the entry as written.
        key: String,
    },
}
