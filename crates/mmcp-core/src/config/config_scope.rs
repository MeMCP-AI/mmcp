//! [`ConfigScope`], the file a configuration key is stored in.

use serde::{Deserialize, Serialize};

/// Where the `config` tool stores a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigScope {
    /// `~/.mmcp/config.toml`, for every project of the user.
    User,
    /// `.mmcp.toml`, shared with every contributor of the project.
    Project,
    /// `.mmcp.local.toml`, personal to the user and never committed.
    Local,
}

impl ConfigScope {
    /// Wire spelling of the scope.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn wire_spelling_matches_the_serde_form() {
        for scope in [ConfigScope::User, ConfigScope::Project, ConfigScope::Local] {
            let json = serde_json::to_string(&scope).expect("serialize");
            assert_eq!(json, format!("\"{}\"", scope.as_str()));
        }
    }
}
