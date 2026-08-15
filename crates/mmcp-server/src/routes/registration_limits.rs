//! Bounds [`validate_register_request`](super::auth) checks a new
//! account's fields against.

/// Resolved length bounds for `POST /auth/register`, grouped so
/// three same-typed `usize` values cannot transpose silently at a
/// call site: passing three bare `usize` arguments compiles cleanly
/// even when two of them are swapped, silently applying the wrong
/// bound to the wrong field.
#[derive(Debug, Clone, Copy)]
pub struct RegistrationLimits {
    pub min_password_length: usize,
    pub max_password_length: usize,
    pub max_handle_length: usize,
}
