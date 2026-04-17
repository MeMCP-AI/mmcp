//! mmcp home shim — everything of substance now lives in
//! `mmcp_store::home`. Kept as a re-export so existing
//! `crate::home::{MmcpHome, ResolvedAuthor, read_git_global}`
//! imports keep compiling until commit 8 deletes
//! `mmcp-client/src/lib.rs` and every consumer rebinds to
//! `mmcp_store::home` directly.
//!
//! `init_backend`, which briefly lived here as a free function
//! during commit 2 (when `GroupIndex` still lived in
//! `mmcp-client`), is back on `MmcpHome` as an inherent method
//! now that the index follows into `mmcp-store`.

pub use mmcp_store::home::{MmcpHome, ResolvedAuthor, read_git_global};
