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
    Serve,

    /// Initialize a new mmcp project in the current directory.
    Init,

    /// Show local mmcp state for the current project.
    Status,

    /// Pull then push against the configured remote server.
    Sync,

    /// Pull updates from the remote server.
    Pull,

    /// Push pending local edits to the remote server.
    Push,

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
        Command::Serve => commands::serve::run().await?,
        Command::Init => commands::init::run().await?,
        Command::Status => commands::status::run().await?,
        Command::Sync => commands::sync::run(true, true).await?,
        Command::Pull => commands::sync::run(true, false).await?,
        Command::Push => commands::sync::run(false, true).await?,
        Command::Hook { command } => match command {
            HookCommand::UserPrompt => commands::hook::user_prompt().await?,
        },
    }

    Ok(())
}
