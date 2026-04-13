//! Access control resolution for mmcp.
//!
//! Takes the raw membership relations from the identity layer and
//! computes effective roles: given a principal and a group, which
//! [`Role`](crate::identity::Role) does the principal actually hold
//! once every applicable path through user, group, and org
//! memberships has been considered.
//!
//! The resolver has no I/O. Callers load the relevant identity
//! records from the database, hand them to the resolver as slices or
//! iterators, and receive a decision. This keeps ACL logic unit
//! testable without spinning up a database.

mod decision;
mod resolver;

pub use decision::EffectiveRole;
pub use resolver::resolve_effective_role;
