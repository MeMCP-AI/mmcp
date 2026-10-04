//! Crate-level default values not owned by a more specific submodule.

/// Inactivity window before the shared session-store cookie
/// (see [`crate::app::build_router`]) expires.
///
/// Applies to every session the single shared store issues: the
/// anonymous CSRF-state session `oauth_authorize` mints, and every
/// authenticated `axum-login` session alike, since one
/// `SessionManagerLayer` covers the whole router.
/// [`tower_sessions::Expiry::OnInactivity`] resets on each session
/// write, so an actively used session keeps renewing itself.
/// Only a session nobody touches again, most concretely an abandoned
/// OAuth round trip, actually expires.
/// Minutes-scale: long enough that a slow real OAuth or password
/// login round trip never lapses, short enough that an anonymous
/// caller cannot accumulate session records in the in-memory store
/// indefinitely.
pub(crate) const SESSION_INACTIVITY_EXPIRY: time::Duration = time::Duration::minutes(15);

/// Period of the expired-session sweep (see [`crate::session_store::spawn_expired_session_sweeper`]).
/// An expired record is never served, so the period only bounds how long a dead row stays in the table.
pub(crate) const EXPIRED_SESSION_SWEEP_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(60);

/// Rows one expired-session delete removes at most.
/// Bounds how long a sweep holds the database connection, whatever the number of expired rows.
pub(crate) const EXPIRED_SESSION_SWEEP_BATCH_SIZE: u64 = 1000;

/// Fresh session ids a `create` draws before reporting an id-collision failure.
/// A 128-bit random id makes the first retry astronomically unlikely, so exhaustion signals a broken id source.
pub(crate) const SESSION_ID_COLLISION_MAX_ATTEMPTS: u32 = 8;

/// Nanoseconds in one millisecond, converting a session expiry to the stored epoch milliseconds.
pub(crate) const NANOSECONDS_PER_MILLISECOND: i128 = 1_000_000;
