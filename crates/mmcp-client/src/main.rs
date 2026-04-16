//! mmcp client binary entry point.
//!
//! A single binary with multiple entry points dispatched via clap
//! subcommands. Each subcommand corresponds to one hat the client
//! wears: MCP stdio server, project CLI, sync engine, or hook
//! handler. The command implementations themselves live in the
//! library target of this crate so integration tests can exercise
//! them directly.

use anyhow::Result;
use clap::{Parser, Subcommand};

use mmcp_client::commands;

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

    /// Initialize mmcp-managed files for the current directory. With
    /// no subcommand, writes `.mmcp.toml`. Subcommands manage other
    /// project-level files (currently: `claude`).
    Init(InitArgs),

    /// Show local mmcp state for the current project.
    Status,

    /// Pull then push against the configured remote server.
    Sync,

    /// Pull updates from the remote server.
    Pull,

    /// Push pending local edits to the remote server.
    Push,

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
    },

    /// Hook handler subcommands invoked by Claude Code hook entries.
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
}

#[derive(Subcommand)]
enum HookCommand {
    /// Handle a Claude Code UserPromptSubmit hook invocation.
    UserPrompt,
}

#[derive(clap::Args)]
struct InitArgs {
    #[command(subcommand)]
    cmd: Option<InitCommand>,
}

#[derive(Subcommand)]
enum InitCommand {
    /// Generate, append to, or convert the project's CLAUDE.md.
    Claude(commands::claude::ClaudeArgs),
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
        Command::Init(InitArgs { cmd: None }) => commands::init::run().await?,
        Command::Init(InitArgs {
            cmd: Some(InitCommand::Claude(args)),
        }) => commands::claude::run(args).await?,
        Command::Status => commands::status::run().await?,
        Command::Sync => commands::sync::run(true, true).await?,
        Command::Pull => commands::sync::run(true, false).await?,
        Command::Push => commands::sync::run(false, true).await?,
        Command::Import {
            group,
            file,
            dir,
            slug,
            name,
            description,
            kind,
        } => commands::import::run(group, file, dir, slug, name, description, kind).await?,
        Command::Hook { command } => match command {
            HookCommand::UserPrompt => commands::hook::user_prompt().await?,
        },
    }

    Ok(())
}
