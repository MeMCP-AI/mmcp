//! [`ConfigAction`], what the `config` tool does.

use rmcp::schemars::JsonSchema;
use serde::Deserialize;

// The three operations of the `config` tool.
// A plain comment: a type-level doc comment would be sent to every client as schema text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum ConfigAction {
    Get,
    Set,
    Unset,
}

impl ConfigAction {
    /// Wire spelling of the action.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Set => "set",
            Self::Unset => "unset",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn wire_spelling_matches_the_serde_form() {
        for action in [ConfigAction::Get, ConfigAction::Set, ConfigAction::Unset] {
            let parsed: ConfigAction =
                serde_json::from_value(serde_json::json!(action.as_str())).unwrap();
            assert_eq!(parsed, action);
        }
    }
}
