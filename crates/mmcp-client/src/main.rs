//! mmcp client binary entry point.
//!
//! A single binary with multiple entry points dispatched via clap
//! subcommands. Each subcommand corresponds to one hat the client
//! wears: MCP stdio server, project CLI, sync engine, or hook handler.
//! All subcommands are scaffold stubs for now.

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
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // TODO(GGLinnk 2026-04-11): replace stubs with real implementations
    match cli.command {
        Command::Serve => tracing::info!("serve: not yet implemented"),
        Command::Init => tracing::info!("init: not yet implemented"),
        Command::Status => tracing::info!("status: not yet implemented"),
        Command::Sync => tracing::info!("sync: not yet implemented"),
        Command::Pull => tracing::info!("pull: not yet implemented"),
        Command::Push => tracing::info!("push: not yet implemented"),
        Command::Hook { command } => match command {
            HookCommand::UserPrompt => {
                tracing::info!("hook user-prompt: not yet implemented");
            }
        },
    }

    Ok(())
}
