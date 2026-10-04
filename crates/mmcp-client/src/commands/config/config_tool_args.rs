//! [`ConfigToolArgs`], the wire form of the `config` tool.

use rmcp::schemars::JsonSchema;
use serde::Deserialize;

use super::{
    ConfigAction, ConfigArgsError, ConfigCommand, ConfigKeyArg, ConfigScopeArg, NoticeValueArg,
};

// Arguments of the `config` tool.
// A plain comment: a type-level doc comment would be sent to every client as schema text.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct ConfigToolArgs {
    pub action: ConfigAction,

    pub key: ConfigKeyArg,

    /// For set only.
    #[serde(default)]
    pub value: Option<NoticeValueArg>,

    /// For set and unset only.
    #[serde(default)]
    pub scope: Option<ConfigScopeArg>,

    /// Project root, defaulting to the one found from the server's working directory.
    #[serde(default)]
    pub path: Option<String>,
}

impl ConfigToolArgs {
    /// The operation these arguments ask for.
    ///
    /// # Errors
    /// [`ConfigArgsError`] when `value` or `scope` is missing where the action needs it, or present where it takes none.
    pub fn command(&self) -> Result<ConfigCommand, ConfigArgsError> {
        let key = self.key.into();
        match (self.action, self.value, self.scope) {
            (ConfigAction::Get, Some(_), _) | (ConfigAction::Unset, Some(_), _) => {
                Err(ConfigArgsError::ValueNotAllowed {
                    action: self.action,
                })
            }
            (ConfigAction::Set, None, _) => Err(ConfigArgsError::ValueRequired),
            (ConfigAction::Get, None, Some(_)) => Err(ConfigArgsError::ScopeNotAllowed),
            (ConfigAction::Set | ConfigAction::Unset, _, None) => {
                Err(ConfigArgsError::ScopeRequired {
                    action: self.action,
                })
            }
            (ConfigAction::Get, None, None) => Ok(ConfigCommand::Get { key }),
            (ConfigAction::Set, Some(value), Some(scope)) => Ok(ConfigCommand::Set {
                key,
                value: value.into(),
                scope: scope.into(),
            }),
            (ConfigAction::Unset, None, Some(scope)) => Ok(ConfigCommand::Unset {
                key,
                scope: scope.into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use mmcp_core::config::{ConfigKey, ConfigScope, NoticeValue};
    use serde_json::json;

    use super::*;

    fn args(value: serde_json::Value) -> ConfigToolArgs {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_get_without_value_or_scope_is_a_get() {
        let parsed = args(json!({"action": "get", "key": "notice.md.user"}));
        assert_eq!(
            parsed.command(),
            Ok(ConfigCommand::Get {
                key: ConfigKey::NoticeMdUser
            })
        );
    }

    #[test]
    fn a_set_with_value_and_scope_is_a_set() {
        let parsed = args(json!({
            "action": "set", "key": "notice.md.project", "value": "off", "scope": "local"
        }));
        assert_eq!(
            parsed.command(),
            Ok(ConfigCommand::Set {
                key: ConfigKey::NoticeMdProject,
                value: NoticeValue::Off,
                scope: ConfigScope::Local,
            })
        );
    }

    #[test]
    fn an_unset_with_a_scope_is_an_unset() {
        let parsed = args(json!({
            "action": "unset", "key": "notice.md.project", "scope": "project"
        }));
        assert_eq!(
            parsed.command(),
            Ok(ConfigCommand::Unset {
                key: ConfigKey::NoticeMdProject,
                scope: ConfigScope::Project,
            })
        );
    }

    #[test]
    fn a_set_without_a_value_is_refused_before_its_scope_is_checked() {
        let parsed = args(json!({"action": "set", "key": "notice.md.project"}));
        assert_eq!(parsed.command(), Err(ConfigArgsError::ValueRequired));
    }

    #[test]
    fn a_get_or_an_unset_with_a_value_is_refused_with_its_own_action() {
        for action in ["get", "unset"] {
            let parsed = args(json!({
                "action": action, "key": "notice.md.project", "value": "on", "scope": "user"
            }));
            let expected = if action == "get" {
                ConfigAction::Get
            } else {
                ConfigAction::Unset
            };
            assert_eq!(
                parsed.command(),
                Err(ConfigArgsError::ValueNotAllowed { action: expected })
            );
        }
    }

    #[test]
    fn a_set_or_an_unset_without_a_scope_is_refused_with_its_own_action() {
        let set = args(json!({"action": "set", "key": "notice.md.project", "value": "on"}));
        let unset = args(json!({"action": "unset", "key": "notice.md.project"}));
        assert_eq!(
            set.command(),
            Err(ConfigArgsError::ScopeRequired {
                action: ConfigAction::Set
            })
        );
        assert_eq!(
            unset.command(),
            Err(ConfigArgsError::ScopeRequired {
                action: ConfigAction::Unset
            })
        );
    }

    #[test]
    fn a_get_with_a_scope_is_refused() {
        let parsed = args(json!({"action": "get", "key": "notice.md.project", "scope": "user"}));
        assert_eq!(parsed.command(), Err(ConfigArgsError::ScopeNotAllowed));
    }

    #[test]
    fn an_unknown_key_value_scope_or_field_fails_to_deserialize() {
        for bad in [
            json!({"action": "get", "key": "notice.md.projet"}),
            json!({"action": "set", "key": "notice.md.user", "value": "maybe", "scope": "user"}),
            json!({"action": "get", "key": "notice.md.user", "scope": "global"}),
            json!({"action": "get", "key": "notice.md.user", "extra": 1}),
            json!({"action": "toggle", "key": "notice.md.user"}),
        ] {
            assert!(
                serde_json::from_value::<ConfigToolArgs>(bad.clone()).is_err(),
                "{bad} must be refused"
            );
        }
    }
}
