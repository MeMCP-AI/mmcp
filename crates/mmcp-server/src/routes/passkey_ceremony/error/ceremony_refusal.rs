//! Why a pending passkey ceremony was refused at finish.

use std::time::Duration;

use thiserror::Error;

/// A passkey ceremony the session holds could not be finished.
///
/// Every cause maps to the same external response of its route; each keeps its own log line.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum CeremonyRefusal {
    /// The session holds no ceremony of this kind: never started, already finished, or lost with the session.
    #[error("no pending ceremony in the session")]
    Absent,

    /// The ceremony was started longer ago than the ceremony window allows.
    #[error("ceremony started {age:?} ago, beyond the {ttl:?} window")]
    Expired { age: Duration, ttl: Duration },

    /// The ceremony was started for another user than the one the finish request resolves.
    #[error("ceremony was started for another user")]
    UserMismatch,
}
