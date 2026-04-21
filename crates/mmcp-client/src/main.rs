//! mmcp client binary entry point.
//!
//! A single `[[bin]]` crate that dispatches clap subcommands across
//! four roles: MCP stdio server, project CLI, sync engine, hook
//! handler. Per FR-020 the crate exposes no library target; the
//! reusable store logic lives in `mmcp-store` so other workspace
//! members (`mmcp-gui`, future third-party consumers) depend on
//! that instead.
//!
//! Integration tests under `tests/` drive the store directly via
//! `mmcp-store` for unit-level checks, and the binary via
//! `assert_cmd` for end-to-end CLI smoke.

#![forbid(unsafe_code)]

mod commands;
mod state;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mmcp", version, about = "mmcp memory client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP stdio server for AI clients to connect to.
    Serve {
        /// Enable debug tools for raw git access.
        #[arg(long, default_value_t = false)]
        debug: bool,
    },

    /// Initialize mmcp-managed files for the current directory.
    /// Takes a subcommand naming which surface to bootstrap
    /// (`project` for `.mmcp.toml` + group repo, `claude` for
    /// CLAUDE.md). Running `mmcp init` with no subcommand prints
    /// this help text.
    Init(InitArgs),

    /// Show local mmcp state for the current project.
    Status,

    /// Pull then push against the configured remote server.
    Sync {
        #[command(flatten)]
        selector: commands::sync::SyncSelector,
    },

    /// Read remote heads into local tracking refs without
    /// advancing the group's main branch. Mirrors `git fetch`;
    /// use `mmcp pull` to fast-forward.
    Fetch {
        #[command(flatten)]
        selector: commands::sync::SyncSelector,
    },

    /// Pull updates from the remote server.
    Pull {
        #[command(flatten)]
        selector: commands::sync::SyncSelector,
    },

    /// Push pending local edits to the remote server.
    Push {
        #[command(flatten)]
        selector: commands::sync::SyncSelector,
    },

    /// Quick health check: manifests parse, memories parse, no errors.
    Check {
        /// Check only this group (UUID or slug). All groups if omitted.
        #[arg(long)]
        group: Option<String>,
    },

    /// Deep diagnostic analysis: everything health checks plus
    /// missing fields, naming drift, empty groups, cross-group
    /// duplicates, and structural hints.
    Diagnose {
        /// Diagnose only this group (UUID or slug). All groups if omitted.
        #[arg(long)]
        group: Option<String>,
    },

    /// Import memories from external markdown files into a group.
    /// Errors on slug collision by default — pass `--override` to
    /// replace the existing memory, mirroring the MCP
    /// `write_memory` contract. Writes into a protected group
    /// prompt for confirmation on a TTY; non-TTY invocations
    /// require `--force`.
    Import {
        /// Target group (UUID or slug).
        #[arg(long)]
        group: String,

        /// Path to a single .md file to import.
        #[arg(long, conflicts_with = "dir")]
        file: Option<std::path::PathBuf>,

        /// Import all .md files from this directory.
        #[arg(long, conflicts_with = "file")]
        dir: Option<std::path::PathBuf>,

        /// Override the slug (only valid with --file).
        #[arg(long, conflicts_with = "dir")]
        slug: Option<String>,

        /// Memory name (required if file has no +++ frontmatter).
        #[arg(long)]
        name: Option<String>,

        /// Memory description (required if file has no +++ frontmatter).
        #[arg(long)]
        description: Option<String>,

        /// Memory kind: rule, snapshot, log, reference, scratch
        /// (required if file has no +++ frontmatter).
        #[arg(long)]
        kind: Option<String>,

        /// Replace an existing memory instead of erroring. Default
        /// is strict create — prefer editing memories via the MCP
        /// `edit_memory` tool or a direct file edit in the bare
        /// repo, and reach for `--override` only when replacing
        /// the whole file is the intent.
        #[arg(long, default_value_t = false)]
        r#override: bool,

        /// Skip the protected-group confirmation prompt. Required
        /// on non-TTY stdin when the target group is marked
        /// protected; otherwise the command errors rather than
        /// silently writing.
        #[arg(long, default_value_t = false)]
        force: bool,
    },

    /// Hook handler subcommands invoked by Claude Code hook entries.
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },

    /// Manage the project's feature requests (FR-007).
    /// Subcommands: `add`, `read`, `update`, `delete`, `list`.
    /// Running `mmcp feature` with no subcommand prints this help.
    Feature(commands::feature::FeatureArgs),
}

#[derive(Subcommand)]
enum HookCommand {
    /// Handle a Claude Code UserPromptSubmit hook invocation.
    UserPrompt,
}

#[derive(clap::Args)]
#[command(arg_required_else_help = true)]
struct InitArgs {
    #[command(subcommand)]
    cmd: Option<InitCommand>,
}

#[derive(Subcommand)]
enum InitCommand {
    /// Generate, append to, or convert the project's CLAUDE.md.
    Claude(commands::claude::ClaudeArgs),
    /// Create the project's backing group repo keyed on
    /// `.mmcp.toml`'s `project_uuid`. Requires `mmcp init` to have
    /// run first so that UUID exists.
    Project(commands::init::ProjectArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,mmcp=info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Serve { debug } => commands::serve::run(debug).await?,
        Command::Check { group } => commands::health::run_check(group).await?,
        Command::Diagnose { group } => commands::health::run_diagnose(group).await?,
        Command::Init(InitArgs { cmd }) => match cmd {
            // `arg_required_else_help = true` on `InitArgs` makes
            // clap surface help before we ever get here when no
            // subcommand is passed. This branch only fires when a
            // future variant is added without a dispatch arm.
            None => unreachable!("clap enforces subcommand presence"),
            Some(InitCommand::Claude(args)) => commands::claude::run(args).await?,
            Some(InitCommand::Project(args)) => commands::init::run_project(args).await?,
        },
        Command::Status => commands::status::run().await?,
        Command::Sync { selector } => commands::sync::run(true, true, selector).await?,
        Command::Fetch { selector } => commands::sync::run_fetch(selector).await?,
        Command::Pull { selector } => commands::sync::run(true, false, selector).await?,
        Command::Push { selector } => commands::sync::run(false, true, selector).await?,
        Command::Import {
            group,
            file,
            dir,
            slug,
            name,
            description,
            kind,
            r#override,
            force,
        } => {
            commands::import::run(
                group,
                file,
                dir,
                slug,
                name,
                description,
                kind,
                r#override,
                force,
            )
            .await?
        }
        Command::Hook { command } => match command {
            HookCommand::UserPrompt => commands::hook::user_prompt().await?,
        },
        Command::Feature(args) => commands::feature::run(args).await?,
    }

    Ok(())
}
