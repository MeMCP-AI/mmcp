//! [`ConfigCommand`], one validated `config` operation.

use mmcp_core::config::{ConfigKey, ConfigScope, NoticeValue};

use super::{
    ConfigEnvironment, ConfigOpError, ConfigOutcome, ProjectLocation, get_key, set_key, unset_key,
};

/// A `config` operation whose arguments are all present and consistent.
/// The tool validates its optional arguments into one; the CLI builds one from its subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigCommand {
    /// Read `key` from every layer.
    Get {
        /// The key to read.
        key: ConfigKey,
    },
    /// Store `value` for `key` at `scope`.
    Set {
        /// The key to write.
        key: ConfigKey,
        /// The value to store.
        value: NoticeValue,
        /// The file to store it in.
        scope: ConfigScope,
    },
    /// Remove `key` at `scope`.
    Unset {
        /// The key to remove.
        key: ConfigKey,
        /// The file to remove it from.
        scope: ConfigScope,
    },
}

impl ConfigCommand {
    /// Run the command against `location`.
    ///
    /// # Errors
    /// The failure of the underlying get, set or unset.
    pub fn execute(
        self,
        environment: &ConfigEnvironment<'_>,
        location: &ProjectLocation,
    ) -> Result<ConfigOutcome, ConfigOpError> {
        match self {
            Self::Get { key } => Ok(ConfigOutcome::Read {
                key,
                resolution: get_key(environment, key, location)?,
            }),
            Self::Set { key, value, scope } => {
                set_key(environment, key, value, scope, location).map(ConfigOutcome::Written)
            }
            Self::Unset { key, scope } => {
                unset_key(environment, key, scope, location).map(ConfigOutcome::Written)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use mmcp_core::config::NoticeSource;

    use super::super::config_fixture::{ConfigFixture, already_excluded};
    use super::*;

    const KEY: ConfigKey = ConfigKey::NoticeMdProject;

    #[test]
    fn a_set_then_a_get_then_an_unset_round_trip_through_one_command_type() {
        let fixture = ConfigFixture::new();
        let environment = fixture.environment(already_excluded);
        let location = fixture.located();

        let set = ConfigCommand::Set {
            key: KEY,
            value: NoticeValue::Off,
            scope: ConfigScope::Project,
        }
        .execute(&environment, &location)
        .unwrap();
        let read = ConfigCommand::Get { key: KEY }
            .execute(&environment, &location)
            .unwrap();
        let unset = ConfigCommand::Unset {
            key: KEY,
            scope: ConfigScope::Project,
        }
        .execute(&environment, &location)
        .unwrap();

        assert!(matches!(set, ConfigOutcome::Written(outcome) if outcome.changed));
        assert!(matches!(
            read,
            ConfigOutcome::Read { key: KEY, resolution }
                if resolution.effective == NoticeValue::Off
                    && resolution.source == NoticeSource::Project
        ));
        assert!(matches!(
            unset,
            ConfigOutcome::Written(outcome)
                if outcome.changed && outcome.value.is_none()
                    && outcome.resolved().source == NoticeSource::Default
        ));
    }
}
