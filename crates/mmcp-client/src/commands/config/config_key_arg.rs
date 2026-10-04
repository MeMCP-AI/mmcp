//! [`ConfigKeyArg`], a setting as the `config` tool and CLI name it.

use clap::ValueEnum;
use mmcp_core::config::ConfigKey;
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

// The settings the `config` tool and `mmcp config` accept.
// Plain comments here: a type-level doc comment would be sent to every client as schema text.
// The variant docs below are the CLI help and the schema descriptions of each key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub enum ConfigKeyArg {
    /// Notices proposing the mmcp block for the project CLAUDE.md.
    #[serde(rename = "notice.md.project")]
    #[value(name = "notice.md.project")]
    NoticeMdProject,

    /// Notices proposing the mmcp block for ~/.claude/CLAUDE.md.
    #[serde(rename = "notice.md.user")]
    #[value(name = "notice.md.user")]
    NoticeMdUser,
}

impl From<ConfigKeyArg> for ConfigKey {
    fn from(arg: ConfigKeyArg) -> Self {
        match arg {
            ConfigKeyArg::NoticeMdProject => Self::NoticeMdProject,
            ConfigKeyArg::NoticeMdUser => Self::NoticeMdUser,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn every_key_has_an_argument_spelled_as_its_wire_string() {
        for key in ConfigKey::ALL {
            let by_serde: ConfigKeyArg =
                serde_json::from_value(serde_json::json!(key.as_str())).unwrap();
            let by_clap = ConfigKeyArg::from_str(key.as_str(), false).unwrap();
            assert_eq!(by_serde, by_clap);
            assert_eq!(ConfigKey::from(by_serde), key);
        }
    }

    #[test]
    fn the_arguments_cover_exactly_the_keys() {
        assert_eq!(ConfigKeyArg::value_variants().len(), ConfigKey::ALL.len());
    }

    #[test]
    fn every_variant_carries_its_description_in_the_clap_help() {
        for variant in ConfigKeyArg::value_variants() {
            let value = variant.to_possible_value().unwrap();
            assert!(
                value
                    .get_help()
                    .is_some_and(|help| !help.to_string().is_empty()),
                "{} needs a description",
                value.get_name()
            );
        }
    }
}
