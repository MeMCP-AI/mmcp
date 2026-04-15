//! High-level session tracker.
//!
//! Thin layer over `mmcp-db` repository calls that applies the
//! session state machine (start, bump turn, detect compaction,
//! record read, verify read, clear post-compaction flag).

use jiff::Timestamp;
use mmcp_db::Connection;
use mmcp_db::entities::memory_read;
use mmcp_db::entities::session::Model as SessionModel;
use mmcp_db::repository::session_repo;
use uuid::Uuid;

use crate::compaction::{TranscriptSignature, compute_signature, detect_compaction};
use crate::error::SessionError;

/// Parameters for opening or refreshing a session.
#[derive(Debug, Clone)]
pub struct StartSession {
    pub session_id: String,
    pub user_id: Option<Uuid>,
    pub project_uuid: Option<Uuid>,
    pub transcript_path: Option<String>,
}

/// Session tracker bound to a database connection.
#[derive(Debug, Clone)]
pub struct SessionTracker {
    conn: Connection,
}

impl SessionTracker {
    #[must_use]
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// Open a session (or refresh an existing one) with the given
    /// parameters. Idempotent.
    pub async fn start(&self, start: StartSession) -> Result<SessionModel, SessionError> {
        let now = now_ms();
        let row = session_repo::upsert(
            &self.conn,
            session_repo::NewSession {
                session_id: start.session_id,
                user_id: start.user_id,
                project_uuid: start.project_uuid,
                transcript_path: start.transcript_path,
                started_at: now,
            },
        )
        .await?;
        Ok(row)
    }

    /// Look up an existing session row.
    pub async fn find(&self, session_id: &str) -> Result<Option<SessionModel>, SessionError> {
        Ok(session_repo::find(&self.conn, session_id).await?)
    }

    /// Increment the session's turn counter and return the new value.
    pub async fn bump_turn(&self, session_id: &str) -> Result<i32, SessionError> {
        let now = now_ms();
        Ok(session_repo::bump_turn(&self.conn, session_id, now).await?)
    }

    /// Inspect the transcript file on disk and, if a compaction has
    /// occurred since the previously stored signature, mark the
    /// session as post-compaction and persist the new signature.
    ///
    /// Returns `true` if a compaction was detected and the flag was
    /// just set.
    pub async fn check_transcript(&self, session_id: &str) -> Result<bool, SessionError> {
        let session = self
            .find(session_id)
            .await?
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        let Some(path) = session.transcript_path.as_ref() else {
            return Ok(false);
        };
        let current = match compute_signature(std::path::Path::new(path))? {
            Some(sig) => sig,
            None => return Ok(false),
        };

        let previous = session
            .transcript_signature
            .as_deref()
            .and_then(TranscriptSignature::decode);

        let compacted = detect_compaction(previous.as_ref(), &current);
        let encoded = current.encode();
        let now = now_ms();
        if compacted {
            session_repo::mark_post_compaction(&self.conn, session_id, Some(encoded), now).await?;
            Ok(true)
        } else {
            // Silent update of the stored signature even when there
            // is no compaction so the next call compares against the
            // most recent state.
            session_repo::mark_post_compaction(&self.conn, session_id, Some(encoded), now).await?;
            session_repo::clear_post_compaction(&self.conn, session_id).await?;
            Ok(false)
        }
    }

    /// Clear the post-compaction flag explicitly. Used after the
    /// caller has re-read every mandatory memory.
    pub async fn acknowledge_compaction(&self, session_id: &str) -> Result<(), SessionError> {
        session_repo::clear_post_compaction(&self.conn, session_id).await?;
        Ok(())
    }

    /// Record that a session read a memory during a specific turn.
    /// `verified = true` also flags the row as an explicit
    /// verification.
    pub async fn record_read(
        &self,
        session_id: &str,
        memory: Uuid,
        turn: i32,
        version: Option<String>,
        verified: bool,
    ) -> Result<(), SessionError> {
        session_repo::record_read(
            &self.conn,
            memory_read::Model {
                id: Uuid::now_v7(),
                session_id: session_id.to_string(),
                memory_id: memory,
                turn,
                version,
                verified,
                read_at: now_ms(),
            },
        )
        .await?;
        Ok(())
    }

    /// True if the session has read the memory at least once.
    pub async fn has_read(&self, session_id: &str, memory: Uuid) -> Result<bool, SessionError> {
        Ok(session_repo::has_read(&self.conn, session_id, memory).await?)
    }

    /// True if the session is currently flagged as post-compaction.
    pub async fn is_post_compaction(&self, session_id: &str) -> Result<bool, SessionError> {
        let session = self
            .find(session_id)
            .await?
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        Ok(session.post_compaction)
    }
}

fn now_ms() -> i64 {
    Timestamp::now().as_millisecond()
}
