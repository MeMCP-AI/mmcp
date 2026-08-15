//! Default values and constants shared across `routes/` handlers.

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
