//! The launch flags and variables of the notice keys, added to `mmcp serve`.

use clap::parser::ValueSource;
use clap::{Arg, ArgMatches, Command, value_parser};
use mmcp_core::config::{ConfigKey, LaunchValues, NoticeLaunch, NoticeValue};

use super::NoticeValueArg;

/// The help of the flag that sets `key` for one server run.
const fn flag_help(key: ConfigKey) -> &'static str {
    match key {
        ConfigKey::NoticeMdProject => {
            "Show or hide the project CLAUDE.md block notices for this run, unless a local or project setting decides"
        }
        ConfigKey::NoticeMdUser => {
            "Show or hide the ~/.claude/CLAUDE.md block notice for this run, unless a local or project setting decides"
        }
    }
}

/// `serve` with one flag per key, each also settable through its environment variable.
/// The names come from [`ConfigKey::launch_flag`] and [`ConfigKey::launch_variable`].
#[must_use]
pub fn with_notice_flags(serve: Command) -> Command {
    ConfigKey::ALL.into_iter().fold(serve, |serve, key| {
        serve.arg(
            Arg::new(key.launch_flag())
                .long(key.launch_flag())
                .env(key.launch_variable())
                .value_name("VALUE")
                .value_parser(value_parser!(NoticeValueArg))
                .help(flag_help(key)),
        )
    })
}

/// The launch values the parsed `serve` arguments carry.
/// A value given on the command line is the flag layer, one read from the environment is the environment layer.
#[must_use]
pub fn notice_launch_from(serve: &ArgMatches) -> NoticeLaunch {
    let values = |key: ConfigKey| {
        launch_values(
            serve.value_source(key.launch_flag()),
            serve
                .get_one::<NoticeValueArg>(key.launch_flag())
                .copied()
                .map(NoticeValue::from),
        )
    };
    NoticeLaunch {
        md_project: values(ConfigKey::NoticeMdProject),
        md_user: values(ConfigKey::NoticeMdUser),
    }
}

/// The layer a parsed value belongs to: the command line is the flag layer, the environment the environment layer.
/// A default or an absent value belongs to neither.
fn launch_values(source: Option<ValueSource>, value: Option<NoticeValue>) -> LaunchValues {
    match source {
        Some(ValueSource::CommandLine) => LaunchValues {
            flag: value,
            environment: None,
        },
        Some(ValueSource::EnvVariable) => LaunchValues {
            flag: None,
            environment: value,
        },
        _ => LaunchValues::default(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn serve() -> Command {
        with_notice_flags(Command::new("serve"))
    }

    fn launch(arguments: &[&str]) -> NoticeLaunch {
        let matches = serve()
            .try_get_matches_from(std::iter::once("serve").chain(arguments.iter().copied()))
            .unwrap();
        notice_launch_from(&matches)
    }

    #[test]
    fn a_flag_on_the_command_line_is_the_flag_layer_of_its_own_key_only() {
        let launched = launch(&["--notice-md-project", "off"]);
        assert_eq!(
            launched.md_project,
            LaunchValues {
                flag: Some(NoticeValue::Off),
                environment: None
            }
        );
        assert_eq!(launched.md_user, LaunchValues::default());
    }

    #[test]
    fn both_flags_are_independent() {
        let launched = launch(&["--notice-md-user", "on", "--notice-md-project", "off"]);
        assert_eq!(launched.md_project.flag, Some(NoticeValue::Off));
        assert_eq!(launched.md_user.flag, Some(NoticeValue::On));
    }

    #[test]
    fn a_value_read_from_the_environment_is_the_environment_layer_never_the_flag_layer() {
        assert_eq!(
            launch_values(Some(ValueSource::EnvVariable), Some(NoticeValue::On)),
            LaunchValues {
                flag: None,
                environment: Some(NoticeValue::On)
            }
        );
    }

    #[test]
    fn a_value_read_from_the_command_line_is_the_flag_layer_never_the_environment_layer() {
        assert_eq!(
            launch_values(Some(ValueSource::CommandLine), Some(NoticeValue::Off)),
            LaunchValues {
                flag: Some(NoticeValue::Off),
                environment: None
            }
        );
    }

    #[test]
    fn a_default_or_an_absent_value_belongs_to_no_launch_layer() {
        assert_eq!(
            launch_values(Some(ValueSource::DefaultValue), Some(NoticeValue::On)),
            LaunchValues::default()
        );
        assert_eq!(launch_values(None, None), LaunchValues::default());
    }

    #[test]
    fn a_value_other_than_on_or_off_is_refused() {
        let refused = serve().try_get_matches_from(["serve", "--notice-md-project", "maybe"]);
        assert!(refused.is_err());
    }

    #[test]
    fn the_flags_and_variables_are_the_ones_the_key_names() {
        let command = serve();
        for key in ConfigKey::ALL {
            let argument = command
                .get_arguments()
                .find(|argument| argument.get_id() == key.launch_flag())
                .unwrap();
            assert_eq!(argument.get_long(), Some(key.launch_flag()));
            assert_eq!(
                argument.get_env().and_then(|name| name.to_str()),
                Some(key.launch_variable())
            );
        }
    }

    #[test]
    fn each_flag_help_says_what_it_shows_or_hides_and_what_outranks_it() {
        let command = serve();
        for key in ConfigKey::ALL {
            let help = command
                .get_arguments()
                .find(|argument| argument.get_id() == key.launch_flag())
                .unwrap()
                .get_help()
                .unwrap()
                .to_string();
            assert_eq!(help, flag_help(key));
            assert!(help.ends_with("unless a local or project setting decides"));
        }
    }
}
