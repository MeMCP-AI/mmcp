//! mmcp home shim — re-exports `MmcpHome` / `ResolvedAuthor` from
//! `mmcp-store` and keeps the `init_backend` composition helper
//! here until `GroupIndex` follows them into the store (FR-020
//! commit 4). Once that move lands, the helper drops back into
//! `mmcp-store` and this file disappears.

use std::sync::Arc;

use anyhow::{Context, Result};

use mmcp_git::NativeBackend;

pub use mmcp_store::home::{MmcpHome, ResolvedAuthor, read_git_global};

use crate::state::GroupIndex;

/// Initialize a `NativeBackend` and `GroupIndex` from `home`.
///
/// Transitional free function — while `GroupIndex` still lives in
/// `mmcp-client`, the composition helper cannot live on `MmcpHome`
/// (which is now in `mmcp-store`). Callers update from
/// `home.init_backend()` to `crate::home::init_backend(&home)`;
/// the change reverses in commit 4 when `GroupIndex` moves and
/// `init_backend` becomes a method (or free function) on the
/// store side.
pub async fn init_backend(home: &MmcpHome) -> Result<(Arc<NativeBackend>, GroupIndex)> {
    let repos_root = home.repos_root();
    let backend = Arc::new(
        NativeBackend::new(&repos_root)
            .with_context(|| format!("initializing repo root {}", repos_root.display()))?,
    );
    let groups = GroupIndex::build(repos_root, backend.clone())
        .await
        .with_context(|| format!("building group index at {}", home.repos_root().display()))?;
    Ok((backend, groups))
}
