//! Passkey ceremony state kept in the caller's session.

mod error;
mod pending_ceremony;

pub(crate) use error::CeremonyRefusal;
pub(crate) use pending_ceremony::{PendingCeremony, accept_pending_ceremony};
