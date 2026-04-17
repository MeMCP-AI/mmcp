//! Self-describing manifest committed at the root of every group
//! git repository.
//!
//! Each group repo carries a `.mmcp.toml` file in the `main`
//! branch whose contents mirror the server-side group metadata.
//! The manifest serves three purposes:
//!
//! 1. **Disaster recovery**: if the server database is lost, the
//!    bare repos on disk still describe themselves. Rebuilding the
//!    index is a matter of reading every `.mmcp.toml`.
//! 2. **Forks and exports**: a group cloned to another server or
//!    to a forge-backed backend keeps its identity intact because
//!    the manifest travels with the content.
//! 3. **Tooling**: anyone inspecting a repo outside mmcp can see
//!    what it is without consulting a separate index.
//!
//! The manifest format is intentionally minimal: a stable UUID, a
//! slug, an owner hint, timestamps, and an explicit schema version.
//! Unknown fields are preserved verbatim by the parser so future
//! mmcp versions can add fields without breaking older clients.

mod error;
mod group_manifest;

pub use error::ManifestError;
pub use group_manifest::{
    GroupManifest, GroupOwnerHint, GroupScope, MANIFEST_FILENAME, MANIFEST_SCHEMA_VERSION,
};
