//! Sync engine wiring and effective-remote-set resolution.
//!
//! `engine_wiring` holds `build_engine`, which turns a resolved
//! [`remotes::EffectiveRemotes`] plus a live backend/group index
//! into a ready-to-use [`mmcp_sync::SyncEngine`], and
//! [`engine_wiring::IndexResolver`], the `GroupHandleResolver` /
//! `ScopeIndex` impl every sync surface passes to it.
//! `remotes` holds [`remotes::resolve_effective_remotes`], which
//! merges a loaded `UserConfig` and `ProjectConfig` into that
//! `EffectiveRemotes` in the first place - a genuinely separate
//! concern (pure config resolution, no backend, no network) from the
//! engine construction `engine_wiring` does with its output.

mod engine_wiring;
mod remotes;

pub use engine_wiring::{IndexResolver, build_engine, build_engine_with_env};
pub use remotes::{EffectiveRemotes, RemoteLevel, ResolvedRemote, resolve_effective_remotes};
