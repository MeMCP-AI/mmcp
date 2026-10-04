//! [`ConfigScopeArg`], a configuration file as the `config` tool and CLI name it.

use clap::ValueEnum;
use mmcp_core::config::ConfigScope;
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

// The scopes the `config` tool and `mmcp config` accept.
// Plain comments here: a type-level doc comment would be sent to every client as schema text.
// The variant docs below are the CLI help and the schema descriptions of each scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
#[schemars(crate = "rmcp::schemars")]
pub enum ConfigScopeArg {
    /// ~/.mmcp/config.toml.
    User,

    /// .mmcp.toml, shared, unreadable by older mmcp.
    Project,

    /// .mmcp.local.toml, personal, kept out of git.
    Local,
}

impl From<ConfigScopeArg> for ConfigScope {
    fn from(arg: ConfigScopeArg) -> Self {
        match arg {
            ConfigScopeArg::User => Self::User,
            ConfigScopeArg::Project => Self::Project,
            ConfigScopeArg::Local => Self::Local,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn every_scope_has_an_argument_spelled_as_its_wire_string() {
        for scope in [ConfigScope::User, ConfigScope::Project, ConfigScope::Local] {
            let by_serde: ConfigScopeArg =
                serde_json::from_value(serde_json::json!(scope.as_str())).unwrap();
            let by_clap = ConfigScopeArg::from_str(scope.as_str(), false).unwrap();
            assert_eq!(by_serde, by_clap);
            assert_eq!(ConfigScope::from(by_serde), scope);
        }
    }

    #[test]
    fn the_arguments_cover_exactly_the_scopes() {
        assert_eq!(ConfigScopeArg::value_variants().len(), 3);
    }
}
