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
