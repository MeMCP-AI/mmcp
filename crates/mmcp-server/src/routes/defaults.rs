//! Default values and constants shared across `routes/` handlers.

/// Maximum accepted body size for every `/auth/*` request, in bytes.
/// A coarse defense-in-depth backstop against oversized bodies, not a
/// proven bound: 16 KiB comfortably covers every field's fixed
/// maximum (email, display name, password) plus JSON overhead at the
/// compiled-in `max_handle_length` default, but the cascade in
/// [`crate::config::ServerConfig::max_handle_length`] has no upper
/// bound of its own, so an operator-configured value large enough can
/// still make this limit reject a request before
/// `validate_max_length` gets a chance to return its field-specific
/// error. That failure mode is a coarser 413 instead of a 400, never
/// a validation bypass.
pub(crate) const AUTH_REQUEST_BODY_LIMIT_BYTES: usize = 16 * 1024;

/// Byte length of the OS-CSPRNG-derived OAuth CSRF `state` token before hex encoding.
pub(crate) const OAUTH_STATE_TOKEN_BYTES: usize = 32;

/// Hex-encoded length of the OAuth CSRF `state` token.
/// Each byte of [`OAUTH_STATE_TOKEN_BYTES`] renders as exactly two hex digits.
/// The callback handler rejects a mismatched length before comparing values.
pub(crate) const OAUTH_STATE_HEX_LENGTH: usize = OAUTH_STATE_TOKEN_BYTES * 2;

/// Per-provider session key prefix for the OAuth CSRF `state` token
/// [`crate::routes::auth`]'s authorize and callback handlers exchange
/// through the session store.
pub(crate) const OAUTH_STATE_SESSION_KEY_PREFIX: &str = "oauth_csrf_state:";

/// Challenge header value advertised on every bearer-auth rejection,
/// matching the scheme `crate::routes::bearer_auth`'s extractor
/// actually accepts.
pub(crate) const WWW_AUTHENTICATE_BEARER: &str = r#"Bearer realm="mmcp""#;

/// Caller-visible body for a bearer-auth rejection. Carries no detail
/// beyond "authenticate": the specific cause (missing header versus a
/// verification failure) is logged server-side only, per
/// `global-security-rules`'s error-message hygiene.
pub(crate) const REJECTION_MESSAGE: &str = "missing or invalid bearer token";

/// Cap on simultaneous per-group `read_manifest`/`walk_history` calls
/// while building the `/sync/manifest` response. Each row's git reads
/// are independent; bounding concurrency here avoids opening every
/// group's bare repo at once on a server with hundreds of groups,
/// while still running far faster than one row after another.
pub(crate) const MAX_CONCURRENT_MANIFEST_LOOKUPS: usize = 8;
