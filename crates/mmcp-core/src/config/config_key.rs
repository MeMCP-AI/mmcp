//! [`ConfigKey`], a setting the `config` tool reads and writes.

use super::ConfigKeyParseError;
use super::NoticeValue;

/// A typed configuration key.
/// A future setting is a new variant, reachable through the same tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigKey {
    /// `notice.md.project`: the notices proposing the mmcp block for the project's CLAUDE.md.
    NoticeMdProject,
    /// `notice.md.user`: the notices proposing the mmcp block for the user-level `~/.claude/CLAUDE.md`.
    NoticeMdUser,
}

impl ConfigKey {
    /// Every key, in declaration order.
    pub const ALL: [Self; 2] = [Self::NoticeMdProject, Self::NoticeMdUser];

    /// Wire spelling of the key: its dotted key path in a configuration file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoticeMdProject => "notice.md.project",
            Self::NoticeMdUser => "notice.md.user",
        }
    }

    /// Value the key has when no layer sets it.
    #[must_use]
    pub const fn default_value(self) -> NoticeValue {
        match self {
            Self::NoticeMdProject | Self::NoticeMdUser => NoticeValue::On,
        }
    }

    /// Long name of the `mmcp serve` flag that sets the key for one server run.
    #[must_use]
    pub const fn launch_flag(self) -> &'static str {
        match self {
            Self::NoticeMdProject => "notice-md-project",
            Self::NoticeMdUser => "notice-md-user",
        }
    }

    /// Environment variable that sets the key for one server run.
    #[must_use]
    pub const fn launch_variable(self) -> &'static str {
        match self {
            Self::NoticeMdProject => "MMCP_NOTICE_MD_PROJECT",
            Self::NoticeMdUser => "MMCP_NOTICE_MD_USER",
        }
    }
}

impl std::str::FromStr for ConfigKey {
    type Err = ConfigKeyParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|key| key.as_str() == raw)
            .ok_or_else(|| ConfigKeyParseError {
                input: raw.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_key_wire_strings_are_the_dotted_toml_paths() {
        assert_eq!(ConfigKey::NoticeMdProject.as_str(), "notice.md.project");
        assert_eq!(ConfigKey::NoticeMdUser.as_str(), "notice.md.user");
    }

    #[test]
    fn every_key_parses_back_from_its_wire_string() {
        for key in ConfigKey::ALL {
            assert_eq!(key.as_str().parse(), Ok(key));
        }
    }

    #[test]
    fn an_unknown_key_is_a_typed_error_carrying_the_input() {
        assert_eq!(
            "notice.md.projet".parse::<ConfigKey>(),
            Err(ConfigKeyParseError {
                input: "notice.md.projet".to_string()
            })
        );
    }

    #[test]
    fn every_key_defaults_to_on() {
        for key in ConfigKey::ALL {
            assert_eq!(key.default_value(), NoticeValue::On);
        }
    }

    #[test]
    fn launch_names_derive_from_the_key_path() {
        for key in ConfigKey::ALL {
            let dashed = key.as_str().replace('.', "-");
            assert_eq!(key.launch_flag(), dashed);
            assert_eq!(
                key.launch_variable(),
                format!("MMCP_{}", dashed.to_uppercase().replace('-', "_"))
            );
        }
    }
}
