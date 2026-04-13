//! Strongly-typed identifiers for mmcp domain entities.
//!
//! Each domain concept gets its own newtype wrapping a UUIDv7. The
//! newtypes prevent accidental cross-use in function signatures and
//! make the data model self-documenting at call sites.
//!
//! UUIDv7 is chosen because it embeds a millisecond timestamp in the
//! high bits, which gives chronological ordering, database-friendly
//! index locality, and a stable global unique value without a
//! coordinator.

mod group_id;
mod memory_id;
mod org_id;
mod project_uuid;
mod user_id;

pub use group_id::GroupId;
pub use memory_id::MemoryId;
pub use org_id::OrgId;
pub use project_uuid::ProjectUuid;
pub use user_id::UserId;
