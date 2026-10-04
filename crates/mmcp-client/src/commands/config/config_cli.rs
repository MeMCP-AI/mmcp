//! The `mmcp config` CLI: arguments, execution and the output lines.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use mmcp_core::config::{LOCAL_CONFIG_EXCLUDE_PATTERN, NoticeLaunch, NoticeValue};
use mmcp_store::home::MmcpHome;

use super::error_chain::message_with_causes;
use super::{
    ConfigCommand, ConfigEnvironment, ConfigKeyArg, ConfigOutcome, ConfigScopeArg,
    ConfigWriteOutcome, NoticeValueArg, ProjectLocation,
};

/// Arguments of `mmcp config`.
#[derive(Debug, Args)]
pub struct ConfigCliArgs {
    #[command(subcommand)]
    pub command: ConfigCliCommand,

    /// Project root, defaulting to the one found from the current directory.
    #[arg(long, global = true)]
    pub path: Option<PathBuf>,
}

/// The three operations of `mmcp config`.
#[derive(Debug, Subcommand)]
pub enum ConfigCliCommand {
    /// Show the local, project and user layers of a setting and its effective value.
    Get {
        /// The setting.
        #[arg(value_enum)]
        key: ConfigKeyArg,
    },

    /// Set a setting at a scope.
    Set {
        /// The setting.
        #[arg(value_enum)]
        key: ConfigKeyArg,

        /// The value to store.
        #[arg(value_enum)]
        value: NoticeValueArg,

        /// The file to store it in.
        #[arg(long, value_enum)]
        scope: ConfigScopeArg,
    },

    /// Remove a setting from a scope.
    Unset {
        /// The setting.
        #[arg(value_enum)]
        key: ConfigKeyArg,

        /// The file to remove it from.
        #[arg(long, value_enum)]
        scope: ConfigScopeArg,
    },
}

impl From<ConfigCliCommand> for ConfigCommand {
    fn from(command: ConfigCliCommand) -> Self {
        match command {
            ConfigCliCommand::Get { key } => Self::Get { key: key.into() },
            ConfigCliCommand::Set { key, value, scope } => Self::Set {
                key: key.into(),
                value: value.into(),
                scope: scope.into(),
            },
            ConfigCliCommand::Unset { key, scope } => Self::Unset {
                key: key.into(),
                scope: scope.into(),
            },
        }
    }
}

/// CLI entry for `mmcp config`.
/// The launch flag and variable values belong to a serving process, which a CLI run is not, so they are never shown.
pub fn run(args: ConfigCliArgs) -> Result<()> {
    let working_directory = std::env::current_dir().context("reading current working directory")?;
    let home = MmcpHome::discover()?;
    let launch = NoticeLaunch::default();
    let environment = ConfigEnvironment::new(&home, &launch);
    run_in(
        &environment,
        &working_directory,
        args,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

/// [`run`] against an explicit environment and outputs.
fn run_in(
    environment: &ConfigEnvironment<'_>,
    working_directory: &std::path::Path,
    args: ConfigCliArgs,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> Result<()> {
    let location = ProjectLocation::find(args.path.as_deref(), working_directory)?;
    let outcome = ConfigCommand::from(args.command).execute(environment, &location)?;
    for line in output_lines(&outcome) {
        writeln!(output, "{line}").context("writing the result")?;
    }
    for line in diagnostic_lines(&outcome) {
        writeln!(diagnostics, "{line}").context("writing the diagnostics")?;
    }
    Ok(())
}

/// What a write cannot tell about the key once it is done, for the diagnostics stream.
fn diagnostic_lines(outcome: &ConfigOutcome) -> Vec<String> {
    match outcome {
        ConfigOutcome::Written(ConfigWriteOutcome {
            resolution: Err(error),
            ..
        }) => vec![format!(
            "The effective value is unknown: {}",
            message_with_causes(error)
        )],
        ConfigOutcome::Read { .. } | ConfigOutcome::Written(_) => Vec::new(),
    }
}

/// The output lines of one outcome.
fn output_lines(outcome: &ConfigOutcome) -> Vec<String> {
    match outcome {
        ConfigOutcome::Read { key, resolution } => {
            let mut lines: Vec<String> = resolution
                .layers
                .entries()
                .into_iter()
                .filter(|(source, _)| !source.is_launch())
                .map(|(source, value)| {
                    format!(
                        "{}: {}",
                        source.as_str(),
                        value.map_or("unset", NoticeValue::as_str)
                    )
                })
                .collect();
            let effective = format!(
                "effective: {} ({})",
                resolution.effective.as_str(),
                resolution.source.as_str()
            );
            // A serving process started with a flag or variable decides below the local and project files only.
            lines.push(if resolution.source.outranks_launch() {
                effective
            } else {
                format!(
                    "{effective}, unless the server was started with --{} or {}",
                    key.launch_flag(),
                    key.launch_variable()
                )
            });
            lines
        }
        ConfigOutcome::Written(written) => {
            let key = written.key.as_str();
            let scope = written.scope.as_str();
            let file = written.file.display();
            let mut lines = vec![match (written.value, written.changed) {
                (Some(value), true) => {
                    format!("{key} = {} at {scope} in {file}.", value.as_str())
                }
                (Some(value), false) => {
                    format!("{key} already {} at {scope} in {file}.", value.as_str())
                }
                (None, true) => format!("{key} removed from {scope} in {file}."),
                (None, false) => format!("{key} not set at {scope} in {file}."),
            }];
            if let Some(excludes) = &written.excluded_in {
                lines.push(format!(
                    "Added {LOCAL_CONFIG_EXCLUDE_PATTERN} to {}.",
                    excludes.display()
                ));
            }
            lines
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use clap::{CommandFactory, Parser};

    use super::super::config_fixture::{
        ConfigFixture, SCRATCH_EXCLUDES_FILE, already_excluded, appends_to_scratch_excludes,
    };
    use super::*;

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        args: ConfigCliArgs,
    }

    fn parse(arguments: &[&str]) -> Result<ConfigCliArgs, clap::Error> {
        Harness::try_parse_from(std::iter::once("config").chain(arguments.iter().copied()))
            .map(|harness| harness.args)
    }

    fn lines_of(stream: Vec<u8>) -> Vec<String> {
        String::from_utf8(stream)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The lines one run writes to its output and to its diagnostics.
    fn run_streams(
        fixture: &ConfigFixture,
        exclusion: fn(
            &std::path::Path,
        ) -> Result<mmcp_git::checkout::Exclusion, mmcp_git::GitError>,
        arguments: &[&str],
    ) -> (Vec<String>, Vec<String>) {
        let mut args = parse(arguments).unwrap();
        args.path = Some(fixture.project.clone());
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        run_in(
            &fixture.environment(exclusion),
            &fixture.project,
            args,
            &mut output,
            &mut diagnostics,
        )
        .unwrap();
        (lines_of(output), lines_of(diagnostics))
    }

    fn run_lines(
        fixture: &ConfigFixture,
        exclusion: fn(
            &std::path::Path,
        ) -> Result<mmcp_git::checkout::Exclusion, mmcp_git::GitError>,
        arguments: &[&str],
    ) -> Vec<String> {
        run_streams(fixture, exclusion, arguments).0
    }

    #[test]
    fn set_prints_the_value_the_scope_and_the_file() {
        let fixture = ConfigFixture::new();
        let lines = run_lines(
            &fixture,
            already_excluded,
            &["set", "notice.md.project", "off", "--scope", "project"],
        );
        assert_eq!(
            lines,
            [format!(
                "notice.md.project = off at project in {}.",
                fixture.project.join(".mmcp.toml").display()
            )]
        );
    }

    #[test]
    fn a_repeated_set_prints_already_and_an_unset_prints_removed_then_not_set() {
        let fixture = ConfigFixture::new();
        let file = fixture.project.join(".mmcp.toml");
        let set = ["set", "notice.md.user", "off", "--scope", "project"];
        run_lines(&fixture, already_excluded, &set);

        assert_eq!(
            run_lines(&fixture, already_excluded, &set),
            [format!(
                "notice.md.user already off at project in {}.",
                file.display()
            )]
        );
        let unset = ["unset", "notice.md.user", "--scope", "project"];
        assert_eq!(
            run_lines(&fixture, already_excluded, &unset),
            [format!(
                "notice.md.user removed from project in {}.",
                file.display()
            )]
        );
        assert_eq!(
            run_lines(&fixture, already_excluded, &unset),
            [format!(
                "notice.md.user not set at project in {}.",
                file.display()
            )]
        );
    }

    #[test]
    fn the_first_local_set_also_prints_the_excludes_file_it_appended_to() {
        let fixture = ConfigFixture::new();
        let lines = run_lines(
            &fixture,
            appends_to_scratch_excludes,
            &["set", "notice.md.project", "off", "--scope", "local"],
        );
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(
            lines[1],
            format!("Added **/.mmcp.local.toml to {SCRATCH_EXCLUDES_FILE}.")
        );
    }

    #[test]
    fn get_prints_the_file_layers_then_the_effective_value_and_its_source() {
        let fixture = ConfigFixture::new();
        run_lines(
            &fixture,
            already_excluded,
            &["set", "notice.md.project", "off", "--scope", "user"],
        );
        run_lines(
            &fixture,
            already_excluded,
            &["set", "notice.md.project", "on", "--scope", "project"],
        );

        let lines = run_lines(&fixture, already_excluded, &["get", "notice.md.project"]);

        assert_eq!(
            lines,
            [
                "local: unset",
                "project: on",
                "user: off",
                "effective: on (project)",
            ]
        );
    }

    #[test]
    fn get_never_shows_a_launch_layer_it_cannot_know() {
        let fixture = ConfigFixture::new();
        let lines = run_lines(&fixture, already_excluded, &["get", "notice.md.user"]);
        assert!(
            lines
                .iter()
                .all(|line| !line.starts_with("flag:") && !line.starts_with("environment:")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_value_decided_below_the_launch_layers_says_a_serving_process_may_differ() {
        let fixture = ConfigFixture::new();
        run_lines(
            &fixture,
            already_excluded,
            &["set", "notice.md.user", "off", "--scope", "user"],
        );

        let from_the_user_file = run_lines(&fixture, already_excluded, &["get", "notice.md.user"]);
        let from_the_default = run_lines(&fixture, already_excluded, &["get", "notice.md.project"]);

        assert_eq!(
            from_the_user_file.last().unwrap(),
            "effective: off (user), unless the server was started with --notice-md-user or MMCP_NOTICE_MD_USER"
        );
        assert_eq!(
            from_the_default.last().unwrap(),
            "effective: on (default), unless the server was started with --notice-md-project or MMCP_NOTICE_MD_PROJECT"
        );
    }

    #[test]
    fn a_value_decided_above_the_launch_layers_is_stated_without_a_caveat() {
        let fixture = ConfigFixture::new();
        for scope in ["local", "project"] {
            run_lines(
                &fixture,
                already_excluded,
                &["set", "notice.md.project", "off", "--scope", scope],
            );
            let lines = run_lines(&fixture, already_excluded, &["get", "notice.md.project"]);
            assert_eq!(
                lines.last().unwrap(),
                &format!("effective: off ({scope})"),
                "{lines:?}"
            );
            run_lines(
                &fixture,
                already_excluded,
                &["unset", "notice.md.project", "--scope", scope],
            );
        }
    }

    #[test]
    fn a_write_whose_effective_value_is_unknown_succeeds_and_says_so_on_the_diagnostics() {
        let fixture = ConfigFixture::new();
        std::fs::write(
            fixture.project.join(".mmcp.local.toml"),
            "[notice.md]\nprojet = \"off\"\n",
        )
        .unwrap();

        let (output, diagnostics) = run_streams(
            &fixture,
            already_excluded,
            &["set", "notice.md.user", "off", "--scope", "user"],
        );

        assert_eq!(
            output,
            [format!(
                "notice.md.user = off at user in {}.",
                fixture.home.user_config_path().display()
            )]
        );
        assert!(
            diagnostics[0].starts_with(
                "The effective value is unknown: The local config could not be loaded: "
            ),
            "{}",
            diagnostics[0]
        );
        assert_eq!(
            diagnostics
                .join("\n")
                .matches("unknown field `projet`")
                .count(),
            1,
            "the parse error is told once: {diagnostics:?}"
        );
    }

    #[test]
    fn a_write_that_resolves_writes_nothing_to_the_diagnostics() {
        let fixture = ConfigFixture::new();
        let (_, diagnostics) = run_streams(
            &fixture,
            already_excluded,
            &["set", "notice.md.user", "off", "--scope", "user"],
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn set_and_unset_require_a_scope_and_set_requires_a_value() {
        assert!(parse(&["set", "notice.md.project", "off"]).is_err());
        assert!(parse(&["unset", "notice.md.project"]).is_err());
        assert!(parse(&["set", "notice.md.project", "--scope", "local"]).is_err());
    }

    #[test]
    fn a_get_takes_neither_a_value_nor_a_scope() {
        assert!(parse(&["get", "notice.md.project", "off"]).is_err());
        assert!(parse(&["get", "notice.md.project", "--scope", "local"]).is_err());
    }

    #[test]
    fn an_unknown_key_value_or_scope_is_refused() {
        assert!(parse(&["get", "notice.md.projet"]).is_err());
        assert!(parse(&["set", "notice.md.user", "maybe", "--scope", "user"]).is_err());
        assert!(parse(&["unset", "notice.md.user", "--scope", "global"]).is_err());
    }

    #[test]
    fn the_path_applies_before_or_after_the_subcommand() {
        let before = parse(&["--path", "/p", "get", "notice.md.user"]).unwrap();
        let after = parse(&["get", "notice.md.user", "--path", "/p"]).unwrap();
        assert_eq!(before.path, Some(PathBuf::from("/p")));
        assert_eq!(after.path, Some(PathBuf::from("/p")));
    }

    #[test]
    fn every_command_converts_to_the_shared_operation() {
        let set = parse(&["set", "notice.md.user", "on", "--scope", "local"]).unwrap();
        assert_eq!(
            ConfigCommand::from(set.command),
            ConfigCommand::Set {
                key: mmcp_core::config::ConfigKey::NoticeMdUser,
                value: NoticeValue::On,
                scope: mmcp_core::config::ConfigScope::Local,
            }
        );
    }

    #[test]
    fn help_texts_are_the_approved_ones() {
        let mut command = Harness::command();
        let subcommand_help = |command: &mut clap::Command, name: &str| {
            command
                .find_subcommand_mut(name)
                .unwrap()
                .get_about()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            subcommand_help(&mut command, "get"),
            "Show the local, project and user layers of a setting and its effective value"
        );
        assert_eq!(
            subcommand_help(&mut command, "set"),
            "Set a setting at a scope"
        );
        assert_eq!(
            subcommand_help(&mut command, "unset"),
            "Remove a setting from a scope"
        );
        let path_help = command
            .get_arguments()
            .find(|argument| argument.get_id() == "path")
            .unwrap()
            .get_help()
            .unwrap()
            .to_string();
        assert_eq!(
            path_help,
            "Project root, defaulting to the one found from the current directory"
        );
    }
}
