//! Session tracking for mmcp.
//!
//! Owns the per-session state that drives staleness warnings and
//! mandatory memory enforcement. Tracks turn counters produced by
//! the `UserPromptSubmit` hook, detects compaction by inspecting
//! the transcript file, and records which memories have been read
//! or verified in the current session.
//!
//! This crate does not open its own database connection: callers
//! pass in a [`SeaORM connection`](sea_orm::DatabaseConnection) from
//! `mmcp-db` so the same connection pool backs both the control
//! plane and the session tracker.

pub mod compaction;
pub mod error;
pub mod tracker;

pub use compaction::{TranscriptSignature, compute_signature, detect_compaction};
pub use error::SessionError;
pub use tracker::{SessionTracker, StartSession};
