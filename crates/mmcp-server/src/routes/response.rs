//! Shared conversion from an internal (server-side) error into a
//! generic HTTP error response.
//!
//! `global-security-rules` forbids HTTP error responses from carrying
//! internal exception text (SQL detail, server filesystem paths, and
//! so on): the full error is logged server-side and callers only ever
//! see a fixed, generic message. Every route module used to duplicate
//! this exact shape (`internal()` / `internal_error()`); this module
//! is the single owner so the log-then-generalize behavior exists in
//! exactly one place.

use tracing::error;

/// Fixed message returned to callers for any internal failure.
///
/// Never derived from the triggering error: the caller-visible text
/// must not vary with what actually went wrong server-side.
pub const GENERIC_INTERNAL_ERROR_MESSAGE: &str = "internal server error";

/// A route's own error-response type, constructible from nothing but
/// the fact that an internal error occurred.
///
/// Each route module implements this for its own error type so
/// [`into_generic_response`] can build the right wire shape (status
/// code, wrapped protocol error, and so on) while the logging and
/// message text stay shared.
pub trait FromInternalError {
    /// Build the generic, caller-visible representation of an
    /// internal failure. Implementations must not embed any detail
    /// from the error that triggered it.
    fn from_internal_error() -> Self;
}

/// Log the full error server-side, then return a generic error
/// response of type `R`.
///
/// This is the single seam every route handler's `.map_err(...)`
/// passes an internal failure through, so `err`'s `Display` output
/// (which can carry SQL detail or absolute server paths) never
/// reaches the HTTP response body.
pub fn into_generic_response<E: std::fmt::Display, R: FromInternalError>(err: E) -> R {
    error!(error = %err, "internal error handling request");
    R::from_internal_error()
}
