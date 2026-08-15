//! Unified error surface for `mmcp-store`.
//!
//! - `store_error`: [`StoreError`], the enum every consumer-facing function returns.
//! - `file_operation`: [`FileOperation`], the filesystem-action tag carried by [`StoreError::Io`].

mod file_operation;
mod store_error;

pub use file_operation::FileOperation;
pub use store_error::StoreError;
