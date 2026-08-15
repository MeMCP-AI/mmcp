//! mmcp client binary entry point.
//!
//! A single `[[bin]]` crate that dispatches clap subcommands across
//! four roles: MCP stdio server, project CLI, sync engine, hook
//! handler. The crate exposes no library target; the
//! reusable store logic lives in `mmcp-store` so other workspace
//! members (`mmcp-gui`, future third-party consumers) depend on
//! that instead.
//!
//! Integration tests under `tests/` drive the store directly via
//! `mmcp-store` for unit-level checks, and the binary via
//! `assert_cmd` for end-to-end CLI smoke.

#![forbid(unsafe_code)]

mod commands;
mod notes;
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

        /// Restrict the registered tool surface. `readonly` exposes
        /// only read-only tools; `edit` adds non-destructive
        /// mutators; `full` exposes every tool. Default `full`.
        #[arg(long, value_enum, default_value_t = commands::serve::ServeMode::Full)]
        mode: commands::serve::ServeMode,
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

    /// Import memories into the store. Three input shapes: a loose
    /// `--file`, a `--dir` of loose files, or a portable `--archive`
    /// (tar; gzip auto-detected) produced by `mmcp export`. Loose
    /// inputs target one `--group` and error on slug collision unless
    /// `--override`; archives carry their own groups and recreate
    /// them, optionally remapping into one group via `--into`. Writes
    /// into a protected group prompt for confirmation on a TTY;
    /// non-TTY invocations require `--force`.
    Import(commands::import::ImportArgs),

    /// Export one or more groups to a portable archive on disk. The
    /// batch counterpart to `mmcp import`. Select groups with
    /// repeated `--group`, with `--all`, or default to the current
    /// project's group. Pass `--gzip` to compress the tar.
    Export {
        /// Group to export (UUID or slug). Repeatable. All mirrored
        /// groups with `--all`; the current project group if neither
        /// is given.
        #[arg(long, conflicts_with = "all")]
        group: Vec<String>,

        /// Export every group in the local mirror.
        #[arg(long, default_value_t = false)]
        all: bool,

        /// Memory filter facets (slug / kind / tag / search / mandatory,
        /// include and exclude). Empty exports every memory.
        #[command(flatten)]
        filter: commands::archive_filter::MemoryFilterArgs,

        /// Destination archive path.
        #[arg(long)]
        output: std::path::PathBuf,

        /// gzip-compress the tar stream.
        #[arg(long, default_value_t = false)]
        gzip: bool,
    },

    /// Hook handler subcommands invoked by Claude Code hook entries.
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },

    /// Manage the project's feature requests.
    /// Subcommands: `add`, `read`, `update`, `delete`, `list`.
    /// Running `mmcp feature` with no subcommand prints this help.
    Feature(commands::feature::FeatureArgs),

    /// Manage the project's issue tracker (sister surface to
    /// `feature`). Subcommands: `add`, `read`, `update`,
    /// `delete`, `list`, `rename`. Running `mmcp issue` with no
    /// subcommand prints this help.
    Issue(commands::issue::IssueArgs),

    /// Manage the project's milestones: a grouping container over
    /// features, possibly spanning multiple project groups (D3).
    /// Reduced surface (M5 design), subcommands: `add`, `read`,
    /// `update`, `list`. Running `mmcp milestone` with no
    /// subcommand prints this help.
    Milestone(commands::milestone::MilestoneArgs),

    /// Read / list / search memories.
    /// Subcommands: `list`, `read`, `versions`, `sections`,
    /// `search`. Running `mmcp memory` with no subcommand prints
    /// this help.
    Memory(commands::memory::MemoryArgs),

    /// Manage groups in the local mirror.
    /// Subcommands: `list`, `info`, `create`. Running `mmcp
    /// group` with no subcommand prints this help.
    Group(commands::group::GroupArgs),

    /// List mandatory and project-scoped memories. Mirrors the
    /// `bootstrap_context` MCP tool. Use `--scope` to restrict
    /// to mandatory or project; `--project` to override the
    /// cwd-walked project group.
    Bootstrap(commands::bootstrap::BootstrapArgs),

    /// Subscribe the current project to a tag, memory, group, or
    /// language. Edits `[subscriptions]` in `.mmcp/config.toml`.
    /// Mirrors the `subscribe` MCP tool. Idempotent: re-subscribing
    /// to the same value is a no-op.
    Subscribe(commands::subscribe::SubscribeCliArgs),

    /// Remove a subscription previously written by `mmcp subscribe`
    /// or the `subscribe` MCP tool. Idempotent: unsubscribing from
    /// a value the project never subscribed to is a no-op.
    Unsubscribe(commands::subscribe::SubscribeCliArgs),

    /// Raw git access into a group's bare repository. Mirrors
    /// the `mcp:debug_*` MCP tools. Subcommands: `git-log`,
    /// `list-tree`, `read-file`, `write-file`. Use the typed
    /// `mmcp memory` / `mmcp group` surfaces first.
    Debug(commands::debug::DebugArgs),

    /// Print the registered MCP tool catalogue with annotation
    /// hints. Same data the `describe_tools` MCP tool returns, but
    /// callable without booting the stdio server. Useful for hook
    /// scripts and ad-hoc audits picking which tools to allow.
    Tools {
        /// Output format. `table` is the default; `json` feeds
        /// scripts piping into jq.
        #[arg(long, value_enum, default_value_t = commands::tools::ToolsFormat::Table)]
        format: commands::tools::ToolsFormat,
    },
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

    // Best-effort: start the local content cache's process-global
    // pool before dispatching to the resolved subcommand, so every
    // write path (CLI `mmcp memory` / `mmcp feature` / ... commands,
    // all of which eventually call `write_file_at_path`) gets the
    // write-trigger hook for free. A failure here (e.g. an unwritable
    // home directory) never blocks the CLI itself: the cache is a
    // derived artifact, not source-of-truth state. Runs after
    // `Cli::parse()` so `--help` / bad-arg invocations never touch
    // the filesystem at all.
    if let Ok(home) = mmcp_store::home::MmcpHome::discover()
        && let Err(err) = mmcp_store::cache::init_from_home(&home).await
    {
        tracing::warn!(error = %err, "failed to initialise local content cache");
    }

    match cli.command {
        Command::Serve { debug, mode } => commands::serve::run(debug, mode).await?,
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
        Command::Sync { selector } => commands::sync::run_sync(selector).await?,
        Command::Fetch { selector } => commands::sync::run_fetch(selector).await?,
        Command::Pull { selector } => commands::sync::run_pull(selector).await?,
        Command::Push { selector } => commands::sync::run_push(selector).await?,
        Command::Import(args) => commands::import::run(args).await?,
        Command::Export {
            group,
            all,
            filter,
            output,
            gzip,
        } => commands::export::run(group, all, filter, output, gzip).await?,
        Command::Hook { command } => match command {
            HookCommand::UserPrompt => commands::hook::user_prompt().await?,
        },
        Command::Feature(args) => commands::feature::run(args).await?,
        Command::Issue(args) => commands::issue::run(args).await?,
        Command::Milestone(args) => commands::milestone::run(args).await?,
        Command::Memory(args) => commands::memory::run(args).await?,
        Command::Group(args) => commands::group::run(args).await?,
        Command::Bootstrap(args) => commands::bootstrap::run(args).await?,
        Command::Subscribe(args) => commands::subscribe::run_subscribe(args).await?,
        Command::Unsubscribe(args) => commands::subscribe::run_unsubscribe(args).await?,
        Command::Debug(args) => commands::debug::run(args).await?,
        Command::Tools { format } => {
            commands::tools::run(format, commands::serve::registered_tool_attrs())?
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// clap's own graph self-check: catches a `conflicts_with` /
    /// `requires` reference to an unknown arg id, a duplicate flag,
    /// and similar wiring mistakes across the whole command tree.
    #[test]
    fn cli_graph_is_valid() {
        Cli::command().debug_assert();
    }

    /// `commands::tools::build_rows` against the real, live tool
    /// list returns one row per registered tool with the annotation
    /// hint columns wired up. Matches the MCP `describe_tools` shape
    /// so consumers can switch surfaces without re-parsing.
    ///
    /// Lives here, not in `commands::tools`' own test module: this
    /// binary's `main.rs` is the only place that legitimately holds
    /// both `commands::tools::build_rows` and
    /// `commands::serve::registered_tool_attrs()`.
    /// See commands::tools's module doc for why this never imports from commands::serve.
    #[test]
    fn tools_json_lists_every_registered_tool_with_hint_columns() {
        let rows = commands::tools::build_rows(commands::serve::registered_tool_attrs());
        assert!(!rows.is_empty());
        for row in &rows {
            assert!(!row.name.is_empty(), "tool name must not be empty");
            assert!(
                row.title.as_deref().map(|t| !t.is_empty()).unwrap_or(false),
                "{}: title must be non-empty",
                row.name,
            );
        }
        // Spot-check a known tool to lock the rendering of the
        // hint columns once.
        let read_memory = rows
            .iter()
            .find(|r| r.name == "read_memory")
            .expect("read_memory present");
        assert_eq!(read_memory.read_only, Some(true));
        assert_eq!(read_memory.idempotent, Some(true));
        assert_eq!(read_memory.open_world, Some(false));
        // read_memory has no risky args; write_memory has
        // an `override` hint.
        assert!(read_memory.arg_risk_hints.is_empty());
        let write_memory = rows
            .iter()
            .find(|r| r.name == "write_memory")
            .expect("write_memory present");
        assert!(
            write_memory
                .arg_risk_hints
                .iter()
                .any(|h| h.arg == "override"),
            "write_memory must surface an `override` arg_risk_hint",
        );
    }
}
