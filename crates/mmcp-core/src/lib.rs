//! Pure data model for mmcp.
//!
//! Defines the shared types used across the workspace: users, orgs, groups,
//! memories, versions, ACL rules, and the resolver that turns a project
//! configuration into an effective group load set.
//!
//! This crate performs no I/O and has no async entry points. Everything
//! below it in the dependency tree can be unit-tested without a runtime.

pub mod acl;
pub mod config;
pub mod conventions;
pub mod id;
pub mod identity;
pub mod loadset;
pub mod manifest;
pub mod memory;
