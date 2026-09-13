//! Flat per-session state storage under `~/.mmcp/sessions/`.
//!
//! Each Claude Code session has its own TOML file named after the session id.
//! Every write goes through an atomic temp-file-rename.
//! That lets the hook process and the serve process share the file without tearing each other's edits.
//!
//! The store intentionally does not take a file lock.
//! In practice the hook process runs only while Claude Code is waiting for the prompt to be accepted.
//! The serve process is idle during that window.
//! So the two never overlap on the same session file.
//!
//! The store uses [`StoreError`].
//! It is the consolidated error type covering sessions, groups, memory, sync, and diagnostics.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use mmcp_session::{TranscriptSignature, compute_signature, detect_compaction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{FileOperation, StoreError};

/// Full session state serialised to TOML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    /// Stable session identifier from the AI client.
    pub session_id: String,

    /// User that owns this session, if authenticated.
    /// Local-only clients leave this `None`.
    #[serde(default)]
    pub user_id: Option<Uuid>,

    /// Project UUID the session is operating inside, if any.
    #[serde(default)]
    pub project_uuid: Option<Uuid>,

    /// Turn counter bumped by the `UserPromptSubmit` hook.
    #[serde(default)]
    pub turn_counter: i32,

    /// Path to the transcript file the AI client is writing.
    #[serde(default)]
    pub transcript_path: Option<String>,

    /// Signature of the transcript at last check.
    #[serde(default)]
    pub transcript_signature: Option<TranscriptSignature>,

    /// Set when a compaction has been detected and the session has
    /// not yet acknowledged it by re-reading its mandatory memories.
    #[serde(default)]
    pub post_compaction: bool,

    /// When the session file was first created, ms since epoch.
    pub started_at: i64,

    /// When the session file was most recently touched.
    pub last_seen_at: i64,

    /// Every memory read recorded for the session so far.
    #[serde(default)]
    pub reads: Vec<MemoryRead>,
}

impl SessionState {
    fn new(session_id: String, now: i64) -> Self {
        Self {
            session_id,
            user_id: None,
            project_uuid: None,
            turn_counter: 0,
            transcript_path: None,
            transcript_signature: None,
            post_compaction: false,
            started_at: now,
            last_seen_at: now,
            reads: Vec::new(),
        }
    }
}

/// A single memory-read entry recorded for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRead {
    pub memory_id: Uuid,
    pub turn: i32,
    #[serde(default)]
    pub version: Option<String>,
    pub verified: bool,
    pub read_at: i64,
}

/// Store backed by a directory of TOML files.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    /// Open the store at `root`, creating the directory if it does
    /// not already exist.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|source| StoreError::io(root.clone(), FileOperation::CreateDir, source))?;
        Ok(Self { root })
    }

    /// Root directory used by this store.
    #[must_use]
    #[allow(dead_code)] // consumed by test helpers and `mmcp status` in later phases.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Filesystem path for the given session id.
    fn path_for(&self, session_id: &str) -> PathBuf {
        self.root.join(format!("{session_id}.toml"))
    }

    /// Load an existing session file, returning `None` if the file
    /// does not exist.
    pub fn load(&self, session_id: &str) -> Result<Option<SessionState>, StoreError> {
        let path = self.path_for(session_id);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let state: SessionState = toml::from_str(&text)
                    .map_err(|source| StoreError::TomlParse { path, source })?;
                Ok(Some(state))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StoreError::io(path, FileOperation::Read, source)),
        }
    }

    /// Atomically write `state` to disk.
    /// The write goes to a sibling `.tmp` file which is then renamed over the final path
    /// so concurrent readers never observe a partial file.
    pub fn save(&self, state: &SessionState) -> Result<(), StoreError> {
        let path = self.path_for(&state.session_id);
        let text = toml::to_string_pretty(state).map_err(|source| StoreError::TomlSerialize {
            path: path.clone(),
            source,
        })?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text.as_bytes())
            .map_err(|source| StoreError::io(tmp.clone(), FileOperation::Write, source))?;
        std::fs::rename(&tmp, &path)
            .inspect_err(|_e| {
                let _ = std::fs::remove_file(&tmp);
            })
            .map_err(|source| StoreError::io(path, FileOperation::Rename, source))?;
        Ok(())
    }

    /// Load the session file if it exists, otherwise initialise a
    /// fresh one with `now` as the creation timestamp.
    fn load_or_init(&self, session_id: &str, now: i64) -> Result<SessionState, StoreError> {
        Ok(self
            .load(session_id)?
            .unwrap_or_else(|| SessionState::new(session_id.to_string(), now)))
    }

    /// Create or refresh a session, setting the auth and project context and the transcript path.
    /// Never bumps the turn counter.
    /// Returns the persisted state.
    pub fn upsert_session(
        &self,
        session_id: &str,
        user_id: Option<Uuid>,
        project_uuid: Option<Uuid>,
        transcript_path: Option<String>,
    ) -> Result<SessionState, StoreError> {
        let now = now_ms();
        let mut state = self.load_or_init(session_id, now)?;
        state.user_id = user_id;
        state.project_uuid = project_uuid;
        state.transcript_path = transcript_path;
        state.last_seen_at = now;
        self.save(&state)?;
        Ok(state)
    }

    /// Increment the session's turn counter and return the new value.
    pub fn bump_turn(&self, session_id: &str) -> Result<i32, StoreError> {
        let now = now_ms();
        let mut state = self.load_or_init(session_id, now)?;
        state.turn_counter += 1;
        state.last_seen_at = now;
        let next = state.turn_counter;
        self.save(&state)?;
        Ok(next)
    }

    /// Inspect the transcript file (if any) and, on compaction, set the session's `post_compaction` flag.
    /// The transcript signature is always updated to the current file contents.
    ///
    /// Returns `true` when a compaction was detected on this call.
    pub fn check_transcript(&self, session_id: &str) -> Result<bool, StoreError> {
        let now = now_ms();
        let mut state = self.load_or_init(session_id, now)?;

        let Some(path) = state.transcript_path.as_ref() else {
            state.last_seen_at = now;
            self.save(&state)?;
            return Ok(false);
        };
        let current = match compute_signature(Path::new(path))? {
            Some(sig) => sig,
            None => {
                state.last_seen_at = now;
                self.save(&state)?;
                return Ok(false);
            }
        };

        let compacted = detect_compaction(state.transcript_signature.as_ref(), &current);
        state.transcript_signature = Some(current);
        if compacted {
            state.post_compaction = true;
        }
        state.last_seen_at = now;
        self.save(&state)?;
        Ok(compacted)
    }

    /// Clear the post-compaction flag.
    /// Called by the caller once it has re-read every mandatory memory.
    // NOTE: part of the session-scoped tool surface (verify, refresh).
    // Kept public so callers can depend on a stable API before those
    // tools are wired onto the MCP router.
    #[allow(dead_code)]
    pub fn acknowledge_compaction(&self, session_id: &str) -> Result<(), StoreError> {
        let now = now_ms();
        let mut state = self.load_or_init(session_id, now)?;
        state.post_compaction = false;
        state.last_seen_at = now;
        self.save(&state)?;
        Ok(())
    }

    /// Record that the session read (and optionally verified) a
    /// memory on a given turn.
    #[allow(dead_code)] // Session-scoped write tool will call this once wired.
    pub fn record_read(
        &self,
        session_id: &str,
        memory_id: Uuid,
        turn: i32,
        version: Option<String>,
        verified: bool,
    ) -> Result<(), StoreError> {
        let now = now_ms();
        let mut state = self.load_or_init(session_id, now)?;
        state.reads.push(MemoryRead {
            memory_id,
            turn,
            version,
            verified,
            read_at: now,
        });
        state.last_seen_at = now;
        self.save(&state)?;
        Ok(())
    }

    /// True if the session has recorded at least one read for the
    /// given memory.
    #[allow(dead_code)] // Session-scoped query tool will call this once wired.
    pub fn has_read(&self, session_id: &str, memory_id: Uuid) -> Result<bool, StoreError> {
        let Some(state) = self.load(session_id)? else {
            return Ok(false);
        };
        Ok(state.reads.iter().any(|r| r.memory_id == memory_id))
    }

    /// True if the session is currently flagged as post-compaction.
    #[allow(dead_code)] // Session-scoped query tool will call this once wired.
    pub fn is_post_compaction(&self, session_id: &str) -> Result<bool, StoreError> {
        let Some(state) = self.load(session_id)? else {
            return Ok(false);
        };
        Ok(state.post_compaction)
    }
}

fn now_ms() -> i64 {
    Timestamp::now().as_millisecond()
}
