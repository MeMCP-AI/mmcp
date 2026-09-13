//! Sync engine wiring and effective-remote-set resolution.
//!
//! `engine_wiring` holds `build_engine`.
//! `build_engine` takes a resolved [`remotes::EffectiveRemotes`] and a live backend/group index.
//! It returns a ready-to-use [`mmcp_sync::SyncEngine`].
//! `engine_wiring` also holds [`engine_wiring::IndexResolver`].
//! It is the `GroupHandleResolver` / `ScopeIndex` impl every sync surface passes to `build_engine`.
//! `remotes` holds [`remotes::resolve_effective_remotes`].
//! It merges a loaded `UserConfig` and `ProjectConfig` into that `EffectiveRemotes`.
//! This is a genuinely separate concern: pure config resolution, no backend, no network.
//! `engine_wiring` does the engine construction with that output.

mod engine_wiring;
mod remotes;

pub use engine_wiring::{IndexResolver, build_engine, build_engine_with_env};
pub use remotes::{EffectiveRemotes, RemoteLevel, ResolvedRemote, resolve_effective_remotes};
