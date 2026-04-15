//! `mmcp hook user-prompt` implementation.
//!
//! Reads the Claude Code `UserPromptSubmit` hook JSON from stdin,
//! updates the session's turn counter, and prints a short context
//! block on stdout that Claude Code injects into the user prompt.

use std::io::Read;

use anyhow::{Context, Result};
use mmcp_db::connect;
use mmcp_session::{SessionTracker, StartSession};
use serde::Deserialize;
use uuid::Uuid;

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

    let db_url = local_database_url()?;
    let db = connect(&db_url).await.context("opening local mmcp database")?;
    db.migrate().await.context("running local migrations")?;

    let tracker = SessionTracker::new(db.connection().clone());
    tracker
        .start(StartSession {
            session_id: payload.session_id.clone(),
            user_id: None,
            project_uuid: project_uuid_from_cwd(payload.cwd.as_deref()),
            transcript_path: payload.transcript_path.clone(),
        })
        .await
        .context("upserting session row")?;

    let compacted = tracker
        .check_transcript(&payload.session_id)
        .await
        .context("checking transcript signature")?;
    let turn = tracker
        .bump_turn(&payload.session_id)
        .await
        .context("bumping turn counter")?;
    let message_id = Uuid::now_v7();

    println!(
        "[mmcp turn #{turn} id={message_id}{compaction_marker}]",
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

fn local_database_url() -> Result<String> {
    let home = dirs_home()?;
    let dir = home.join(".mmcp");
    std::fs::create_dir_all(&dir).context("creating local mmcp directory")?;
    let path = dir.join("local.db");
    Ok(format!("sqlite://{}?mode=rwc", path.display()))
}

fn dirs_home() -> Result<std::path::PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(std::path::PathBuf::from(home));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Ok(std::path::PathBuf::from(profile));
    }
    anyhow::bail!("cannot determine home directory: set HOME or USERPROFILE");
}
