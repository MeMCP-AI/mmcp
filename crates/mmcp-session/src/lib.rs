//! Session-related primitives for mmcp.
//!
//! This crate deliberately ships only the pure, reusable pieces of
//! session tracking: the transcript signature used to detect
//! Claude Code conversation compactions, and the comparison
//! function that decides whether a new signature represents a
//! compaction event.
//!
//! Persistence and higher-level session state live in the crates
//! that actually need them. `mmcp-client` stores per-session state
//! in flat TOML files under `~/.mmcp/sessions/` via its own
//! `SessionStore`, and consumes the compaction primitives from
//! here. Anything that needs the same logic on the server side
//! can also consume these functions without pulling in file I/O
//! or database concerns.

pub mod compaction;
pub mod error;

pub use compaction::{TranscriptSignature, compute_signature, detect_compaction};
pub use error::SessionError;
