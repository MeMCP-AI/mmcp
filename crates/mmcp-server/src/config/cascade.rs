//! Tunable length-limit tier cascade shared by [`super::ServerConfig`]'s
//! min/max password length and max handle length fields.
//!
//! Mirrors `mmcp_store::memory::resolve_max_auto_slug_length`'s
//! override -> env -> user-config-file -> compiled-default cascade,
//! applied to three independent `usize` tunables (minimum password
//! length, maximum password length, maximum handle length). The pure
//! resolution logic (`resolve_usize_from_tiers`) stays separate from
//! the I/O-performing wrappers (env var read, config file read) so the
//! precedence rules are unit-testable without mutating process-global
//! environment state or touching the filesystem.
//!
//! `resolve_usize_from_tiers` / `parse_usize_env` / `user_config_length_limit`
//! carry no "password" in their names (unlike the field-specific
//! callers below) precisely because a third, non-password tunable
//! (`max_handle_length`) now shares them: a name claiming
//! password-specificity would lie about a mechanism that is, and
//! always was, generic over any `usize`-valued tier (see
//! feedback-names-must-not-lie).

use super::defaults::{
    MAX_HANDLE_LENGTH_ENV, MAX_PASSWORD_LENGTH_ENV, MIN_PASSWORD_LENGTH_ENV, MIN_VALID_LENGTH_LIMIT,
};

/// Resolve the effective minimum password length.
/// Highest-precedence source wins:
/// 1. `override_len`, the `--min-password-length` CLI flag.
/// 2. [`MIN_PASSWORD_LENGTH_ENV`] environment variable.
/// 3. `~/.mmcp/config.toml` `[limits] min_password_length`
///    ([`mmcp_core::config::UserConfig`]).
/// 4. [`mmcp_auth::MIN_PASSWORD_LENGTH`], the compiled-in fallback.
pub(crate) fn resolve_min_password_length<F>(get: &F, override_len: Option<usize>) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    const FIELD: &str = "minimum password length";
    let env_len = parse_usize_env(
        FIELD,
        MIN_PASSWORD_LENGTH_ENV,
        get(MIN_PASSWORD_LENGTH_ENV).as_deref(),
    );
    let config_len = user_config_length_limit(|limits| limits.min_password_length);
    resolve_usize_from_tiers(
        FIELD,
        override_len,
        env_len,
        config_len,
        mmcp_auth::MIN_PASSWORD_LENGTH,
        MIN_VALID_LENGTH_LIMIT,
    )
}

/// Resolve the effective maximum password length. Same cascade as
/// [`resolve_min_password_length`], over [`MAX_PASSWORD_LENGTH_ENV`],
/// `[limits] max_password_length`, and [`mmcp_auth::MAX_PASSWORD_LENGTH`].
pub(crate) fn resolve_max_password_length<F>(get: &F, override_len: Option<usize>) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    const FIELD: &str = "maximum password length";
    let env_len = parse_usize_env(
        FIELD,
        MAX_PASSWORD_LENGTH_ENV,
        get(MAX_PASSWORD_LENGTH_ENV).as_deref(),
    );
    let config_len = user_config_length_limit(|limits| limits.max_password_length);
    resolve_usize_from_tiers(
        FIELD,
        override_len,
        env_len,
        config_len,
        mmcp_auth::MAX_PASSWORD_LENGTH,
        MIN_VALID_LENGTH_LIMIT,
    )
}

/// Resolve the effective maximum account handle length. Same cascade
/// as [`resolve_min_password_length`], over [`MAX_HANDLE_LENGTH_ENV`],
/// `[limits] max_handle_length`, and [`mmcp_auth::MAX_HANDLE_LENGTH`].
pub(crate) fn resolve_max_handle_length<F>(get: &F, override_len: Option<usize>) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    const FIELD: &str = "maximum handle length";
    let env_len = parse_usize_env(
        FIELD,
        MAX_HANDLE_LENGTH_ENV,
        get(MAX_HANDLE_LENGTH_ENV).as_deref(),
    );
    let config_len = user_config_length_limit(|limits| limits.max_handle_length);
    resolve_usize_from_tiers(
        FIELD,
        override_len,
        env_len,
        config_len,
        mmcp_auth::MAX_HANDLE_LENGTH,
        // Below this floor, `mmcp_auth::provision_oauth_handle`'s
        // numeric-suffix collision retry has no room left for
        // `SUFFIX_RESERVE_BYTES`: the base handle truncation and the
        // suffix truncation collapse onto each other. A tier this
        // small is rejected and falls through, exactly like a tier of
        // `0` already is.
        mmcp_auth::MIN_VIABLE_MAX_HANDLE_LENGTH,
    )
}

/// Parse a raw `usize`-tier environment variable value, if any, into
/// a tier value. Logs and falls through (returns `None`) when the
/// variable is present but not a valid number, rather than silently
/// discarding it or defaulting it to zero.
fn parse_usize_env(field: &str, var_name: &str, raw: Option<&str>) -> Option<usize> {
    let raw = raw?;
    match raw.parse::<usize>() {
        Ok(n) => Some(n),
        Err(err) => {
            tracing::warn!(
                env_value = %raw,
                error = %err,
                "{var_name} is not a valid number; ignoring and falling through to the next {field} tier"
            );
            None
        }
    }
}

/// Precedence resolution given each tier's already-fetched value:
/// `override_len` beats `env_len` beats `config_len` beats
/// `default_len`. Any tier value strictly below `min_valid` counts as
/// absent (falls through), since a bound below that floor is never a
/// legitimate intent for the tunable being resolved: most callers
/// pass [`MIN_VALID_LENGTH_LIMIT`] (rejecting only `0`), while
/// `max_handle_length` passes the stricter
/// [`mmcp_auth::MIN_VIABLE_MAX_HANDLE_LENGTH`] floor, below which its
/// numeric-suffix collision retry loses room for the suffix it
/// appends. Whichever tier is the one actually rejected is logged,
/// naming `field`, the rejected value, and the floor, before falling
/// through to the next tier.
fn resolve_usize_from_tiers(
    field: &str,
    override_len: Option<usize>,
    env_len: Option<usize>,
    config_len: Option<usize>,
    default_len: usize,
    min_valid: usize,
) -> usize {
    if let Some(n) = override_len {
        if n >= min_valid {
            return n;
        }
        tracing::warn!(
            "{field} override tier rejected: {n} is below the minimum valid {field} of {min_valid}; falling through to the next {field} tier"
        );
    }
    if let Some(n) = env_len {
        if n >= min_valid {
            return n;
        }
        tracing::warn!(
            "{field} env tier rejected: {n} is below the minimum valid {field} of {min_valid}; falling through to the next {field} tier"
        );
    }
    if let Some(n) = config_len {
        if n >= min_valid {
            return n;
        }
        tracing::warn!(
            "{field} config tier rejected: {n} is below the minimum valid {field} of {min_valid}; falling through to the compiled-in default"
        );
    }
    default_len
}

/// Read a `usize` length-limit field out of `~/.mmcp/config.toml`
/// `[limits]`, if present. `select` picks which field (
/// `min_password_length` / `max_password_length` / `max_handle_length`)
/// this call resolves, so every cascade shares one read-and-log path
/// instead of duplicating it. Mirrors the read-only,
/// missing-file-or-section-means-`None` style
/// `mmcp_store::memory::user_config_max_auto_slug_length` already
/// uses for the same config file; never errors, since a broken or
/// absent user config must never fail server startup. A discovery or
/// parse failure is logged before falling through, rather than
/// discarded with no signal.
fn user_config_length_limit(
    select: impl Fn(&mmcp_core::config::LimitsConfig) -> Option<usize>,
) -> Option<usize> {
    let home = match mmcp_store::MmcpHome::discover() {
        Ok(home) => home,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "failed to discover MmcpHome while resolving a length-limit config tier; falling through to the next tier"
            );
            return None;
        }
    };
    let cfg = match home.load_user_config() {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "user config failed to parse while resolving a length-limit config tier; falling through to the next tier"
            );
            return None;
        }
    };
    cfg.limits.as_ref().and_then(select)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_support::WarnCounter;

    #[test]
    fn resolve_usize_from_tiers_prefers_explicit_override() {
        assert_eq!(
            resolve_usize_from_tiers("x", Some(10), Some(20), Some(30), 40, 1),
            10
        );
    }

    #[test]
    fn resolve_usize_from_tiers_falls_back_env_then_config_then_default() {
        assert_eq!(
            resolve_usize_from_tiers("x", None, Some(20), Some(30), 40, 1),
            20
        );
        assert_eq!(
            resolve_usize_from_tiers("x", None, None, Some(30), 40, 1),
            30
        );
        assert_eq!(resolve_usize_from_tiers("x", None, None, None, 40, 1), 40);
    }

    #[test]
    fn resolve_usize_from_tiers_treats_zero_as_absent_at_every_tier() {
        assert_eq!(
            resolve_usize_from_tiers("x", Some(0), Some(20), Some(30), 40, 1),
            20
        );
        assert_eq!(
            resolve_usize_from_tiers("x", Some(0), Some(0), Some(30), 40, 1),
            30
        );
        assert_eq!(
            resolve_usize_from_tiers("x", Some(0), Some(0), Some(0), 40, 1),
            40
        );
    }

    /// A `min_valid` floor above 1 must reject every tier value at or below it, not just `0`.
    #[test]
    fn resolve_usize_from_tiers_rejects_any_value_below_an_arbitrary_floor() {
        // Floor of 4: 1, 2, and 3 are all below it and must be
        // rejected at every tier, exactly like `--max-handle-length
        // 1` / `--max-handle-length 2` reaching the real
        // `max_handle_length` cascade would be.
        assert_eq!(
            resolve_usize_from_tiers("x", Some(2), Some(3), Some(30), 40, 4),
            30,
            "an override below the floor must fall through even though it is nonzero"
        );
        assert_eq!(
            resolve_usize_from_tiers("x", Some(1), Some(2), Some(3), 40, 4),
            40,
            "every tier below the floor must fall through to the compiled-in default"
        );
        assert_eq!(
            resolve_usize_from_tiers("x", Some(4), None, None, 40, 4),
            4,
            "a value exactly at the floor must be accepted"
        );
    }

    #[test]
    fn resolve_usize_from_tiers_warns_only_for_the_tier_actually_reached() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        // The config tier is Some(0) in both calls below, but the
        // first call never reaches it (override wins), so it must
        // never warn; the second call falls through to it, so it
        // must warn exactly once.
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(
                resolve_usize_from_tiers("x", Some(10), None, Some(0), 40, 1),
                10
            );
        });
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the zero config tier was never consulted, so it must not warn"
        );

        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(
                resolve_usize_from_tiers("x", None, None, Some(0), 40, 1),
                40
            );
        });
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "falling through the zero config tier must log exactly one warning"
        );
    }

    #[test]
    fn parse_usize_env_warns_on_malformed_value() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        let result = tracing::subscriber::with_default(subscriber, || {
            parse_usize_env("x", "MMCP_X", Some("not-a-number"))
        });

        assert_eq!(
            result, None,
            "a malformed env var must be treated as absent, not fabricated into a number"
        );
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a malformed env var must log exactly one warning instead of being silently discarded"
        );
    }

    #[test]
    fn parse_usize_env_accepts_a_valid_number() {
        assert_eq!(parse_usize_env("x", "MMCP_X", Some("12")), Some(12));
    }

    #[test]
    fn parse_usize_env_treats_absent_as_none() {
        assert_eq!(parse_usize_env("x", "MMCP_X", None), None);
    }
}
