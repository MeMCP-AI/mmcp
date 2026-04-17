//! Thin wrapper over `mmcp_store::diagnose_all`.
//!
//! `diagnose_all` is already a pure-Rust typed call that returns a
//! `DiagReport`; the only reason this module exists is to keep the
//! I/O boundary uniform — every background-worker call routes
//! through `io::*` so phase 5's write path follows the same shape.

use mmcp_git::NativeBackend;
use mmcp_store::{DiagReport, GroupIndex, diagnose_all};

pub async fn run_diagnose(backend: &NativeBackend, groups: &GroupIndex) -> DiagReport {
    diagnose_all(backend, groups).await
}
