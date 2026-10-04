//! A passkey ceremony in flight, stored in the caller's session.
//!
//! The ceremony state must stay server-side, and the session is the caller's own record of it.
//! Binding it to the user it was started for keeps a login on the same session from finishing another user's ceremony.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::routes::defaults::PASSKEY_CEREMONY_TTL;
use crate::routes::passkey_ceremony::error::CeremonyRefusal;

/// The WebAuthn state of one ceremony, the user it was started for and when.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PendingCeremony<T> {
    state: T,
    user_id: Uuid,
    /// Start instant in epoch milliseconds; a monotonic clock reading does not survive serialization or a restart.
    started_at_ms: i64,
}

impl<T> PendingCeremony<T> {
    pub(crate) fn new(state: T, user_id: Uuid, started_at_ms: i64) -> Self {
        Self {
            state,
            user_id,
            started_at_ms,
        }
    }
}

/// The ceremony state when the session's pending ceremony may finish for `user_id` at `now_ms`.
///
/// A ceremony for another user is refused before its age is considered.
/// A start instant in the future, from clock skew between instances, counts as age zero.
pub(crate) fn accept_pending_ceremony<T>(
    pending: Option<PendingCeremony<T>>,
    user_id: Uuid,
    now_ms: i64,
) -> Result<T, CeremonyRefusal> {
    let pending = pending.ok_or(CeremonyRefusal::Absent)?;
    if pending.user_id != user_id {
        return Err(CeremonyRefusal::UserMismatch);
    }
    let age = Duration::from_millis(u64::try_from(now_ms - pending.started_at_ms).unwrap_or(0));
    if age >= PASSKEY_CEREMONY_TTL {
        return Err(CeremonyRefusal::Expired {
            age,
            ttl: PASSKEY_CEREMONY_TTL,
        });
    }
    Ok(pending.state)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const NOW_MS: i64 = 1_000_000_000;

    fn ttl_ms() -> i64 {
        i64::try_from(PASSKEY_CEREMONY_TTL.as_millis()).unwrap()
    }

    #[test]
    fn pending_ceremony_of_the_same_user_within_the_ttl_is_accepted() {
        let user = Uuid::now_v7();
        let pending = PendingCeremony::new("state", user, NOW_MS - ttl_ms() + 1);
        assert_eq!(
            accept_pending_ceremony(Some(pending), user, NOW_MS),
            Ok("state")
        );
    }

    #[test]
    fn pending_ceremony_for_another_user_is_refused() {
        let pending = PendingCeremony::new("state", Uuid::now_v7(), NOW_MS);
        assert_eq!(
            accept_pending_ceremony(Some(pending), Uuid::now_v7(), NOW_MS),
            Err(CeremonyRefusal::UserMismatch)
        );
    }

    #[test]
    fn pending_ceremony_older_than_the_ttl_is_refused() {
        let user = Uuid::now_v7();
        let pending = PendingCeremony::new("state", user, NOW_MS - ttl_ms());
        assert_eq!(
            accept_pending_ceremony(Some(pending), user, NOW_MS),
            Err(CeremonyRefusal::Expired {
                age: PASSKEY_CEREMONY_TTL,
                ttl: PASSKEY_CEREMONY_TTL,
            })
        );
    }

    #[test]
    fn a_missing_pending_ceremony_is_refused() {
        assert_eq!(
            accept_pending_ceremony::<&str>(None, Uuid::now_v7(), NOW_MS),
            Err(CeremonyRefusal::Absent)
        );
    }

    #[test]
    fn a_start_instant_ahead_of_the_clock_counts_as_age_zero() {
        let user = Uuid::now_v7();
        let pending = PendingCeremony::new("state", user, NOW_MS + ttl_ms());
        assert_eq!(
            accept_pending_ceremony(Some(pending), user, NOW_MS),
            Ok("state")
        );
    }

    #[test]
    fn pending_ceremony_survives_a_session_round_trip() {
        let user = Uuid::now_v7();
        let stored = serde_json::to_value(PendingCeremony::new("state", user, NOW_MS)).unwrap();
        let loaded: PendingCeremony<String> = serde_json::from_value(stored).unwrap();
        assert_eq!(
            accept_pending_ceremony(Some(loaded), user, NOW_MS),
            Ok("state".to_owned())
        );
    }
}
