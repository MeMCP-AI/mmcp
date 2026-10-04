//! [`ConfigArgsError`], why the arguments of a `config` tool call are not an operation.

use super::ConfigAction;

/// A combination of `action`, `value` and `scope` the tool refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigArgsError {
    /// A set without a value.
    #[error("set needs a value.")]
    ValueRequired,

    /// A get or an unset with a value.
    #[error("{} takes no value.", action.as_str())]
    ValueNotAllowed {
        /// The action that took no value.
        action: ConfigAction,
    },

    /// A set or an unset without a scope.
    #[error("{} needs a scope.", action.as_str())]
    ScopeRequired {
        /// The action that needs a scope.
        action: ConfigAction,
    },

    /// A get with a scope.
    #[error("get takes no scope.")]
    ScopeNotAllowed,
}

impl ConfigArgsError {
    /// Stable wire code of the error.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ValueRequired => "value_required",
            Self::ValueNotAllowed { .. } => "value_not_allowed",
            Self::ScopeRequired { .. } => "scope_required",
            Self::ScopeNotAllowed => "scope_not_allowed",
        }
    }
}
