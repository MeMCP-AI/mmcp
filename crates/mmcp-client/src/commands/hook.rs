//! `mmcp hook user-prompt` implementation.
//!
//! Reads the Claude Code `UserPromptSubmit` hook JSON from stdin,
//! updates the session's state in the flat TOML store under
//! `~/.mmcp/sessions/`, and prints a short context block on stdout
//! that Claude Code injects into the user prompt. The context block
//! carries the session id, the new turn number, and a marker when
//! a compaction has been detected so the model can observe its
//! own session state.

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use uuid::Uuid;

use crate::state::SessionStore;

/// Directory name under the user's home that holds mmcp state.
const MMCP_HOME_DIR: &str = ".mmcp";
/// Subdirectory holding per-session TOML state files.
const MMCP_SESSIONS_SUBDIR: &str = "sessions";

#[derive(Deserialize)]
struct HookPayload {
    session_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

/// Process a single hook invocation.
pub async fn user_prompt() -> Result<()> {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("reading hook payload from stdin")?;
    let payload: HookPayload = serde_json::from_str(&raw).context("parsing hook payload")?;

    let sessions_root = sessions_dir()?;
    let store = SessionStore::open(&sessions_root)
        .with_context(|| format!("opening session store at {}", sessions_root.display()))?;

    store
        .upsert_session(
            &payload.session_id,
            None,
            project_uuid_from_cwd(payload.cwd.as_deref()),
            payload.transcript_path.clone(),
        )
        .context("upserting session state")?;

    let compacted = store
        .check_transcript(&payload.session_id)
        .context("checking transcript signature")?;
    let turn = store
        .bump_turn(&payload.session_id)
        .context("bumping turn counter")?;
    let message_id = Uuid::now_v7();

    // Printed to stdout so Claude Code injects it into the prompt
    // the model sees on its next turn. The session id is included
    // explicitly so tools that need per-session state can pull it
    // out of the visible context without guessing.
    println!(
        "[mmcp session={session} turn=#{turn} id={message_id}{compaction_marker}]",
        session = payload.session_id,
        compaction_marker = if compacted {
            " post-compaction"
        } else {
            ""
        }
    );
    Ok(())
}

fn project_uuid_from_cwd(cwd: Option<&str>) -> Option<Uuid> {
    let cwd = std::path::Path::new(cwd?);
    let root = crate::config::find_project_root(cwd)?;
    let cfg = crate::config::load(&root).ok()?;
    Some(*cfg.project_uuid.as_uuid())
}

fn sessions_dir() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home.join(MMCP_HOME_DIR).join(MMCP_SESSIONS_SUBDIR))
}

fn home_dir() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }
    bail!("cannot determine home directory: set HOME or USERPROFILE");
}
