//! Git storage abstraction for mmcp.
//!
//! Defines the `GitBackend` trait and its implementations. The default
//! `NativeBackend` uses `gix` against bare repositories on disk. Alternative
//! backends (Forgejo, Gitea, GitHub, GitLab) talk to external forges over
//! REST and live behind the same trait.
//!
//! Consumers of this crate depend only on the trait, never on a specific
//! backend.
