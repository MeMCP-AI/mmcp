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

/// Byte length of the random OAuth CSRF `state` token
/// [`oauth2::CsrfToken::new_random_len`] mints, before base64url
/// (no-padding) encoding.
pub(crate) const OAUTH_STATE_TOKEN_BYTES: usize = 32;

/// Encoded length of the OAuth CSRF `state` token `oauth2` produces.
/// `CsrfToken::new_random_len` base64url-encodes (no padding) the
/// raw [`OAUTH_STATE_TOKEN_BYTES`]: every 3 raw bytes become 4
/// characters, with the trailing partial group rounded up.
/// The callback handler rejects a mismatched length before comparing values.
pub(crate) const OAUTH_STATE_TOKEN_LENGTH: usize = (OAUTH_STATE_TOKEN_BYTES * 4).div_ceil(3);

/// Per-provider session key prefix for the OAuth CSRF `state` token
/// [`crate::routes::auth`]'s authorize and callback handlers exchange
/// through the session store.
pub(crate) const OAUTH_STATE_SESSION_KEY_PREFIX: &str = "oauth_csrf_state:";

/// Per-provider session key prefix for the OAuth PKCE code verifier
/// [`crate::routes::auth`]'s authorize and callback handlers exchange
/// through the session store, mirroring
/// [`OAUTH_STATE_SESSION_KEY_PREFIX`] but under its own namespace so
/// the two values never collide.
pub(crate) const OAUTH_PKCE_VERIFIER_SESSION_KEY_PREFIX: &str = "oauth_pkce_verifier:";

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

/// Maximum accepted length of an email address, in bytes. 254 is
/// the maximum length an RFC 5321 compliant email address can have
/// (the `MAIL FROM` reverse-path limit), so it is a real protocol
/// bound rather than an arbitrary pick.
pub(crate) const MAX_EMAIL_LENGTH: usize = 254;

/// Maximum accepted length of a display name, in bytes. Display
/// names are shown in WebUI listings; 128 stays far above any real
/// name while bounding pathological input.
pub(crate) const MAX_DISPLAY_NAME_LENGTH: usize = 128;

/// Header carrying the shared push-token credential `POST /sync/push`
/// requires in addition to the caller's own per-user bearer session
/// token (mmcp issue #190). Deliberately distinct from
/// `Authorization`, which `AuthenticatedUser` already owns for
/// per-user session verification on this same route: reusing
/// `Authorization` for the push token would make it collide with the
/// session token on the one header a request carries.
pub(crate) const PUSH_TOKEN_HEADER: &str = "x-mmcp-push-token";

/// Passkey ceremonies (registration or authentication) must complete
/// within this window; a real browser round-trip takes seconds, not
/// minutes. An entry older than this is stale and is purged on the
/// next insert into the same map, bounding memory growth from
/// ceremonies an authenticated user started but never finished.
pub(crate) const PASSKEY_CEREMONY_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);
