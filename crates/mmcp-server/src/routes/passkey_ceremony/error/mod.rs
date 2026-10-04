//! Error types of the passkey ceremony module.

mod ceremony_refusal;
mod ceremony_take_error;

pub(crate) use ceremony_refusal::CeremonyRefusal;
pub(crate) use ceremony_take_error::CeremonyTakeError;
