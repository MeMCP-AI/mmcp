//! Identity layer for mmcp.
//!
//! Models users, orgs, groups, and the membership relations between
//! them. Mirrors the GitHub-style hierarchy used by the control layer:
//! users belong to zero or more orgs, and inside each org belong to
//! zero or more groups (teams).
//!
//! These types describe identity only. Permission resolution lives in
//! the future `acl` module and consumes these types.

mod group;
mod membership;
mod org;
mod role;
mod user;

pub use group::Group;
pub use membership::Membership;
pub use org::Org;
pub use role::Role;
pub use user::User;
