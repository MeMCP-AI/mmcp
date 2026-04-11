//! Session tracking for mmcp.
//!
//! Owns the per-session state that drives staleness warnings and mandatory
//! memory enforcement. Tracks turn counters produced by the
//! `UserPromptSubmit` hook, detects compaction by inspecting the
//! transcript file, and records which memories have been read or verified
//! in the current session.
