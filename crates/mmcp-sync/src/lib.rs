//! Sync engine for mmcp.
//!
//! Drives push, pull, diff, and merge operations over the `GitBackend`
//! trait. Manages the pending-push queue stored in `mmcp-db`, requests
//! server-assigned version numbers at push time, and surfaces conflicts
//! back to the caller for AI + user resolution.
