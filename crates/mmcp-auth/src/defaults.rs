//! Default values and derived bounds for account handle length,
//! used by [`crate::backend`]'s password and OAuth login flows.

/// Compiled-in default account handle length bound, in bytes.
/// Lowest-precedence tier: callers resolve the effective bound through `mmcp_server::config` and pass it in.
pub const MAX_HANDLE_LENGTH: usize = 64;

/// Maximum number of numeric-suffix retries when the preferred
/// OAuth handle is already taken by an unrelated account, before
/// giving up with [`crate::error::AuthError::HandleAllocationExhausted`].
pub(crate) const MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS: u32 = 20;

/// Number of decimal digits in `value`, computed at compile time so
/// [`SUFFIX_RESERVE_BYTES`] never drifts if
/// [`MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS`] changes.
const fn decimal_digit_count(mut value: u32) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

/// Bytes reserved out of the effective `max_handle_length` for a retry candidate's `-N` tail.
/// Separator byte plus the widest digit count [`MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS`] can produce.
///
/// The base handle is truncated to `max_handle_length - SUFFIX_RESERVE_BYTES` before a suffix is appended.
/// Without the reserve, every retry re-truncates back to `base` and collapses onto the handle already taken.
pub(crate) const SUFFIX_RESERVE_BYTES: usize =
    1 + decimal_digit_count(MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS);

/// Smallest `max_handle_length` that still leaves `SUFFIX_RESERVE_BYTES` of room after truncating the base handle.
/// Below it, the collision retry produces duplicate or empty-base candidates.
/// The config cascade rejects any tier below this minimum.
pub const MIN_VIABLE_MAX_HANDLE_LENGTH: usize = SUFFIX_RESERVE_BYTES + 1;
