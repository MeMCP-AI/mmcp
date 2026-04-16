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

use anyhow::{Context, Result};
use serde::Deserialize;
use uuid::Uuid;

use crate::home::MmcpHome;
use crate::state::SessionStore;

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

    let mmcp_home = MmcpHome::discover()?;
    let sessions_root = mmcp_home.sessions_root();
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
