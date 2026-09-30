//! [`UserProjectConfig`], the user's own settings for one project.

use serde::{Deserialize, Serialize};

use super::ClaudeMdTable;

/// One entry of the `projects` table of `~/.mmcp/config.toml`, keyed by the project UUID.
/// Lenient: a key the loader does not know is reported as a diagnostic, never a load failure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserProjectConfig {
    /// The user's CLAUDE.md suggestion setting for this project.
    #[serde(skip_serializing_if = "ClaudeMdTable::is_empty")]
    pub claude_md: ClaudeMdTable,
}

impl UserProjectConfig {
    /// Whether the entry carries nothing, so it is left out of the file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.claude_md.is_empty()
    }
}
