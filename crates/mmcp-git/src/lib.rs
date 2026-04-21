//! Git storage abstraction for mmcp.
//!
//! Defines the [`GitBackend`] trait and its implementations. The default
//! [`NativeBackend`] uses `gix` against bare repositories on disk.
//! Alternative backends (Forgejo, Gitea, GitHub, GitLab) talk to
//! external forges over REST and live behind the same trait.
//!
//! Consumers of this crate depend only on the trait, never on a
//! specific backend.

pub mod backend;
pub mod error;
pub mod native;
pub mod types;

pub use backend::GitBackend;
pub use error::GitError;
pub use native::NativeBackend;
pub use types::{
    CommitMeta, CommitSpec, Credentials, FastForwardOutcome, PushReport, RefSpec, RepoHandle, Rev,
};
