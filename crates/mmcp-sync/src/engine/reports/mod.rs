//! Outcome types returned by `push`, `pull`, `fetch`, and `sync`.
//!
//! One file per type, or per tightly-coupled report/item pair
//! (`PushReport` + `PushedGroup`, `FetchReport` + `FetchedGroup`):
//! the item type only ever appears inside its own report's `Vec`
//! field, so keeping the pair together keeps the report's field
//! docs next to the shape they describe.

mod fetch;
mod group_sync_failure;
mod pull;
mod push;
mod sync;

pub use fetch::{FetchReport, FetchedGroup};
pub use group_sync_failure::GroupSyncFailure;
pub use pull::PullReport;
pub use push::{PushReport, PushedGroup, RemotePushOutcome};
pub use sync::SyncReport;
