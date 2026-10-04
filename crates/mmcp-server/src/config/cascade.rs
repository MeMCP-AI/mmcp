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

use std::num::NonZeroU32;

use super::auth_rate_limit::{AuthRateLimitConfig, default_network_quota, non_zero_or_one};
use super::auth_rate_limit_overrides::AuthRateLimitOverrides;
use super::defaults::{
    DEFAULT_AUTH_RATE_LIMIT_BURST, DEFAULT_AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH,
    DEFAULT_AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH, DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE,
    IPV6_ADDRESS_BITS, MAX_HANDLE_LENGTH_ENV, MAX_PASSWORD_LENGTH_ENV, MIN_PASSWORD_LENGTH_ENV,
    MIN_VALID_LENGTH_LIMIT,
};
use super::env_value::non_blank;
use super::ipv6_network_tier_config::Ipv6NetworkTierConfig;

/// Resolve the effective minimum password length.
/// Highest-precedence source wins:
/// 1. `override_len`, the `--min-password-length` CLI flag.
/// 2. [`MIN_PASSWORD_LENGTH_ENV`] environment variable.
/// 3. `limits`'s `min_password_length`, from `~/.mmcp/config.toml`
///    `[limits]` ([`load_user_limits`]).
/// 4. [`mmcp_auth::MIN_PASSWORD_LENGTH`], the compiled-in fallback.
pub(crate) fn resolve_min_password_length<F>(
    get: &F,
    override_len: Option<usize>,
    limits: Option<&mmcp_core::config::LimitsConfig>,
) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    const FIELD: &str = "minimum password length";
    let env_len = parse_usize_env(
        FIELD,
        MIN_PASSWORD_LENGTH_ENV,
        get(MIN_PASSWORD_LENGTH_ENV).as_deref(),
    );
    let config_len = limits.and_then(|l| l.min_password_length);
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
/// `limits`'s `max_password_length`, and [`mmcp_auth::MAX_PASSWORD_LENGTH`].
pub(crate) fn resolve_max_password_length<F>(
    get: &F,
    override_len: Option<usize>,
    limits: Option<&mmcp_core::config::LimitsConfig>,
) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    const FIELD: &str = "maximum password length";
    let env_len = parse_usize_env(
        FIELD,
        MAX_PASSWORD_LENGTH_ENV,
        get(MAX_PASSWORD_LENGTH_ENV).as_deref(),
    );
    let config_len = limits.and_then(|l| l.max_password_length);
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
/// `limits`'s `max_handle_length`, and [`mmcp_auth::MAX_HANDLE_LENGTH`].
pub(crate) fn resolve_max_handle_length<F>(
    get: &F,
    override_len: Option<usize>,
    limits: Option<&mmcp_core::config::LimitsConfig>,
) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    const FIELD: &str = "maximum handle length";
    let env_len = parse_usize_env(
        FIELD,
        MAX_HANDLE_LENGTH_ENV,
        get(MAX_HANDLE_LENGTH_ENV).as_deref(),
    );
    let config_len = limits.and_then(|l| l.max_handle_length);
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

/// Resolve the effective requests per minute of the auth rate limit.
/// Highest-precedence source wins:
/// 1. `override_raw`, the `--auth-rate-limit-per-minute` flag or, when absent, `MMCP_AUTH_RATE_LIMIT_PER_MINUTE`, as the argument parser resolved them.
/// 2. `limits`'s `auth_rate_limit_per_minute`, from `~/.mmcp/config.toml` `[limits]`.
/// 3. The compiled-in default.
pub(crate) fn resolve_auth_rate_limit_per_minute(
    override_raw: Option<&str>,
    limits: Option<&mmcp_core::config::LimitsConfig>,
) -> NonZeroU32 {
    resolve_rate_limit_value(
        "auth rate limit per minute",
        "--auth-rate-limit-per-minute or MMCP_AUTH_RATE_LIMIT_PER_MINUTE",
        override_raw,
        limits.and_then(|l| l.auth_rate_limit_per_minute),
        non_zero_or_one(DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE),
    )
}

/// Resolve the effective burst of the auth rate limit.
/// Same cascade as [`resolve_auth_rate_limit_per_minute`], over `--auth-rate-limit-burst` or `MMCP_AUTH_RATE_LIMIT_BURST`, `limits`'s `auth_rate_limit_burst`, and the compiled-in default.
pub(crate) fn resolve_auth_rate_limit_burst(
    override_raw: Option<&str>,
    limits: Option<&mmcp_core::config::LimitsConfig>,
) -> NonZeroU32 {
    resolve_rate_limit_value(
        "auth rate limit burst",
        "--auth-rate-limit-burst or MMCP_AUTH_RATE_LIMIT_BURST",
        override_raw,
        limits.and_then(|l| l.auth_rate_limit_burst),
        non_zero_or_one(DEFAULT_AUTH_RATE_LIMIT_BURST),
    )
}

/// One auth rate limit tunable through the override, config file and default tiers.
/// The override tier already holds the flag or the environment variable, which the argument parser reads, so no second environment read happens here.
/// A tier that is zero, not a number, or above what the limiter counts in falls through to the next tier.
fn resolve_rate_limit_value(
    field: &str,
    source: &str,
    override_raw: Option<&str>,
    config_value: Option<usize>,
    default: NonZeroU32,
) -> NonZeroU32 {
    let override_value = parse_usize_env(field, source, override_raw)
        .and_then(|value| within_limiter_range(field, "override", value));
    let config_value = config_value.and_then(|value| within_limiter_range(field, "config", value));
    let default_value = usize::try_from(default.get()).unwrap_or(usize::MAX);
    let resolved = resolve_usize_from_tiers(
        field,
        override_value,
        None,
        config_value,
        default_value,
        MIN_VALID_LENGTH_LIMIT,
    );
    u32::try_from(resolved)
        .ok()
        .and_then(NonZeroU32::new)
        .unwrap_or(default)
}

/// `value` when the limiter can count in it, `None` with a warning when it is above [`u32::MAX`].
fn within_limiter_range(field: &str, tier: &str, value: usize) -> Option<usize> {
    let max = usize::try_from(u32::MAX).unwrap_or(usize::MAX);
    if value <= max {
        return Some(value);
    }
    tracing::warn!(
        "{field} {tier} tier rejected: {value} is above the maximum valid {field} of {max}; falling through to the next {field} tier"
    );
    None
}

/// Resolve every setting of the auth rate limit: the client budget, its IPv6 prefix and the optional IPv6 network budget.
pub(crate) fn resolve_auth_rate_limit(
    overrides: &AuthRateLimitOverrides,
    limits: Option<&mmcp_core::config::LimitsConfig>,
) -> AuthRateLimitConfig {
    let per_minute = resolve_auth_rate_limit_per_minute(overrides.per_minute.as_deref(), limits);
    let burst = resolve_auth_rate_limit_burst(overrides.burst.as_deref(), limits);
    let ipv6_client_prefix_length =
        resolve_ipv6_client_prefix_length(overrides.ipv6_client_prefix_length.as_deref(), limits);
    let ipv6_network_tier = resolve_ipv6_network_tier(
        overrides,
        limits,
        ClientTier {
            per_minute,
            burst,
            ipv6_prefix_length: ipv6_client_prefix_length,
        },
    );
    AuthRateLimitConfig {
        per_minute,
        burst,
        ipv6_client_prefix_length,
        ipv6_network_tier,
    }
}

/// The resolved client budget the IPv6 network budget is checked against.
#[derive(Clone, Copy)]
struct ClientTier {
    per_minute: NonZeroU32,
    burst: NonZeroU32,
    ipv6_prefix_length: u8,
}

/// Resolve the leading bits of an IPv6 address that identify one client.
/// Same cascade as [`resolve_auth_rate_limit_per_minute`].
/// A value outside 1 to 128 falls through to the next tier with a warning.
fn resolve_ipv6_client_prefix_length(
    override_raw: Option<&str>,
    limits: Option<&mmcp_core::config::LimitsConfig>,
) -> u8 {
    const FIELD: &str = "auth rate limit IPv6 client prefix length";
    let override_value = parse_usize_env(
        FIELD,
        "--auth-rate-limit-ipv6-client-prefix-length or MMCP_AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH",
        override_raw,
    )
    .and_then(|value| within_prefix_range(FIELD, "override", value));
    let config_value = limits
        .and_then(|l| l.auth_rate_limit_ipv6_client_prefix_length)
        .and_then(|value| within_prefix_range(FIELD, "config", value));
    let default = DEFAULT_AUTH_RATE_LIMIT_IPV6_CLIENT_PREFIX_LENGTH;
    let resolved = resolve_usize_from_tiers(
        FIELD,
        override_value,
        None,
        config_value,
        usize::from(default),
        MIN_VALID_LENGTH_LIMIT,
    );
    u8::try_from(resolved).unwrap_or(default)
}

/// `value` when it is a prefix length of an IPv6 address, `None` with a warning when it is above 128.
fn within_prefix_range(field: &str, tier: &str, value: usize) -> Option<usize> {
    if value <= usize::from(IPV6_ADDRESS_BITS) {
        return Some(value);
    }
    tracing::warn!(
        "{field} {tier} tier rejected: {value} is above the maximum valid {field} of {IPV6_ADDRESS_BITS}; falling through to the next {field} tier"
    );
    None
}

/// Resolve the IPv6 network budget, `None` when it is switched off or its settings contradict the client budget.
///
/// The prefix length goes through the override, config file and default tiers, and `0` at any tier switches the budget off.
/// A blank value counts as absent, so `0` is the explicit setting that removes the budget.
/// A prefix outside 1 to 128, or not shorter than the IPv6 client prefix, warns and removes the budget.
/// A quota below the client quota is kept with a warning, since the network budget then refuses before the client budget does.
fn resolve_ipv6_network_tier(
    overrides: &AuthRateLimitOverrides,
    limits: Option<&mmcp_core::config::LimitsConfig>,
    client: ClientTier,
) -> Option<Ipv6NetworkTierConfig> {
    const PREFIX_FIELD: &str = "auth rate limit IPv6 network prefix length";
    let prefix_length = parse_usize_env(
        PREFIX_FIELD,
        "--auth-rate-limit-ipv6-network-prefix-length or MMCP_AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH",
        overrides.ipv6_network_prefix_length.as_deref(),
    )
    .or_else(|| limits.and_then(|l| l.auth_rate_limit_ipv6_network_prefix_length))
    .unwrap_or_else(|| usize::from(DEFAULT_AUTH_RATE_LIMIT_IPV6_NETWORK_PREFIX_LENGTH));
    if prefix_length == 0 {
        return None;
    }
    let Ok(prefix_length) = u8::try_from(prefix_length) else {
        warn_unusable_network_prefix(prefix_length);
        return None;
    };
    if prefix_length > IPV6_ADDRESS_BITS {
        warn_unusable_network_prefix(usize::from(prefix_length));
        return None;
    }
    if prefix_length >= client.ipv6_prefix_length {
        tracing::warn!(
            network_prefix_length = prefix_length,
            client_prefix_length = client.ipv6_prefix_length,
            "the IPv6 network prefix length is not shorter than the IPv6 client prefix length; the IPv6 network budget is off"
        );
        return None;
    }
    let per_minute = resolve_rate_limit_value(
        "auth rate limit IPv6 network per minute",
        "--auth-rate-limit-ipv6-network-per-minute or MMCP_AUTH_RATE_LIMIT_IPV6_NETWORK_PER_MINUTE",
        overrides.ipv6_network_per_minute.as_deref(),
        limits.and_then(|l| l.auth_rate_limit_ipv6_network_per_minute),
        default_network_quota(client.per_minute),
    );
    let burst = resolve_rate_limit_value(
        "auth rate limit IPv6 network burst",
        "--auth-rate-limit-ipv6-network-burst or MMCP_AUTH_RATE_LIMIT_IPV6_NETWORK_BURST",
        overrides.ipv6_network_burst.as_deref(),
        limits.and_then(|l| l.auth_rate_limit_ipv6_network_burst),
        default_network_quota(client.burst),
    );
    if per_minute < client.per_minute || burst < client.burst {
        tracing::warn!(
            "the IPv6 network quota is below the client quota, so the network budget refuses before the client budget does"
        );
    }
    Some(Ipv6NetworkTierConfig {
        prefix_length,
        per_minute,
        burst,
    })
}

fn warn_unusable_network_prefix(prefix_length: usize) {
    tracing::warn!(
        prefix_length,
        "the IPv6 network prefix length is not between 1 and 128; the IPv6 network budget is off"
    );
}

/// Parse a raw `usize`-tier environment variable value, if any, into a tier value.
/// A blank value counts as unset and returns `None` without logging.
/// A value that is present but not a valid number is logged and returns `None`, so the next tier applies.
/// It is never silently discarded or defaulted to zero.
fn parse_usize_env(field: &str, var_name: &str, raw: Option<&str>) -> Option<usize> {
    let raw = non_blank(raw)?;
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

/// Read `~/.mmcp/config.toml` `[limits]` once, shared by every
/// length-limit cascade above so `ServerConfig::from_source_with_overrides`
/// discovers `MmcpHome` and parses the user config file a single time
/// instead of once per tunable. Mirrors the read-only,
/// missing-file-or-section-means-`None` style
/// `mmcp_store::memory::user_config_max_auto_slug_length` already
/// uses for the same config file; never errors, since a broken or
/// absent user config must never fail server startup. A discovery or
/// parse failure is logged before falling through, rather than
/// discarded with no signal.
pub(crate) fn load_user_limits() -> Option<mmcp_core::config::LimitsConfig> {
    let home = match mmcp_store::MmcpHome::discover() {
        Ok(home) => home,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "failed to discover MmcpHome while resolving length-limit config tiers; falling through to the next tier"
            );
            return None;
        }
    };
    let cfg = match home.load_user_config() {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "user config failed to parse while resolving length-limit config tiers; falling through to the next tier"
            );
            return None;
        }
    };
    cfg.limits
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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

    /// A blank value is the unset form Compose interpolation produces, never a malformed number.
    #[test]
    fn parse_usize_env_treats_a_blank_value_as_absent_without_warning() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());

        let results = tracing::subscriber::with_default(subscriber, || {
            (
                parse_usize_env("x", "MMCP_X", Some("")),
                parse_usize_env("x", "MMCP_X", Some("  \n")),
            )
        });

        assert_eq!(results, (None, None));
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a blank value is not an invalid number and must not warn"
        );
    }

    #[test]
    fn parse_usize_env_parses_a_value_with_surrounding_whitespace() {
        assert_eq!(parse_usize_env("x", "MMCP_X", Some(" 12\n")), Some(12));
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

    fn limits_with_rate(
        per_minute: Option<usize>,
        burst: Option<usize>,
    ) -> mmcp_core::config::LimitsConfig {
        mmcp_core::config::LimitsConfig {
            auth_rate_limit_per_minute: per_minute,
            auth_rate_limit_burst: burst,
            ..mmcp_core::config::LimitsConfig::default()
        }
    }

    fn per_minute(override_raw: Option<&str>, limits: Option<usize>) -> u32 {
        resolve_auth_rate_limit_per_minute(override_raw, Some(&limits_with_rate(limits, None)))
            .get()
    }

    fn burst(override_raw: Option<&str>, limits: Option<usize>) -> u32 {
        resolve_auth_rate_limit_burst(override_raw, Some(&limits_with_rate(None, limits))).get()
    }

    #[test]
    fn auth_rate_limit_prefers_flag_then_environment_then_config_file_then_default() {
        // The argument parser hands the flag, else the environment value, over as one override.
        assert_eq!(per_minute(Some("40"), Some(30)), 40);
        assert_eq!(per_minute(None, Some(30)), 30);
        assert_eq!(per_minute(None, None), DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE);
        assert_eq!(burst(Some("7"), Some(5)), 7);
        assert_eq!(burst(None, Some(5)), 5);
        assert_eq!(burst(None, None), DEFAULT_AUTH_RATE_LIMIT_BURST);
        assert_eq!(
            resolve_auth_rate_limit_per_minute(None, None).get(),
            DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE,
            "an absent config file section leaves the default"
        );
    }

    #[test]
    fn auth_rate_limit_blank_override_is_absent_without_warning() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resolved = tracing::subscriber::with_default(WarnCounter(count.clone()), || {
            (per_minute(Some(""), Some(30)), burst(Some("  "), None))
        });
        assert_eq!(resolved, (30, DEFAULT_AUTH_RATE_LIMIT_BURST));
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn auth_rate_limit_zero_falls_through_with_a_warning() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resolved = tracing::subscriber::with_default(WarnCounter(count.clone()), || {
            per_minute(Some("0"), Some(30))
        });
        assert_eq!(resolved, 30);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);

        assert_eq!(
            per_minute(Some("0"), Some(0)),
            DEFAULT_AUTH_RATE_LIMIT_PER_MINUTE
        );
    }

    #[test]
    fn auth_rate_limit_not_a_number_falls_through_with_a_warning() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resolved = tracing::subscriber::with_default(WarnCounter(count.clone()), || {
            burst(Some("many"), Some(5))
        });
        assert_eq!(resolved, 5);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn auth_rate_limit_above_the_limiter_range_falls_through_with_a_warning() {
        let above_range = usize::try_from(u32::MAX).unwrap() + 1;
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resolved = tracing::subscriber::with_default(WarnCounter(count.clone()), || {
            (
                per_minute(Some(&above_range.to_string()), Some(30)),
                burst(None, Some(above_range)),
            )
        });
        assert_eq!(resolved, (30, DEFAULT_AUTH_RATE_LIMIT_BURST));
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn auth_rate_limit_accepts_the_largest_value_the_limiter_counts_in() {
        assert_eq!(per_minute(Some(&u32::MAX.to_string()), None), u32::MAX);
    }

    fn overrides(
        client_prefix: Option<&str>,
        network_prefix: Option<&str>,
        network_per_minute: Option<&str>,
        network_burst: Option<&str>,
    ) -> AuthRateLimitOverrides {
        let owned = |value: Option<&str>| value.map(str::to_owned);
        AuthRateLimitOverrides {
            ipv6_client_prefix_length: owned(client_prefix),
            ipv6_network_prefix_length: owned(network_prefix),
            ipv6_network_per_minute: owned(network_per_minute),
            ipv6_network_burst: owned(network_burst),
            ..AuthRateLimitOverrides::default()
        }
    }

    fn resolved(overrides: &AuthRateLimitOverrides) -> AuthRateLimitConfig {
        resolve_auth_rate_limit(overrides, None)
    }

    fn warnings_of<T>(run: impl FnOnce() -> T) -> (T, usize) {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let value = tracing::subscriber::with_default(WarnCounter(count.clone()), run);
        (value, count.load(std::sync::atomic::Ordering::SeqCst))
    }

    #[test]
    fn ipv6_settings_default_to_a_64_client_prefix_and_a_48_network_budget_at_ten_times_the_quota()
    {
        let config = resolved(&AuthRateLimitOverrides::default());
        assert_eq!(config.ipv6_client_prefix_length, 64);
        let network = config.ipv6_network_tier.unwrap();
        assert_eq!(network.prefix_length, 48);
        assert_eq!(network.per_minute.get(), 10 * config.per_minute.get());
        assert_eq!(network.burst.get(), 10 * config.burst.get());
    }

    #[test]
    fn the_default_network_quota_follows_a_configured_client_quota() {
        let config = resolved(&AuthRateLimitOverrides {
            per_minute: Some("6".to_string()),
            burst: Some("3".to_string()),
            ..AuthRateLimitOverrides::default()
        });
        let network = config.ipv6_network_tier.unwrap();
        assert_eq!((network.per_minute.get(), network.burst.get()), (60, 30));
    }

    #[test]
    fn ipv6_settings_prefer_flag_then_environment_then_config_file_then_default() {
        // The argument parser hands the flag, else the environment value, over as one override.
        let limits = mmcp_core::config::LimitsConfig {
            auth_rate_limit_ipv6_client_prefix_length: Some(60),
            auth_rate_limit_ipv6_network_prefix_length: Some(40),
            auth_rate_limit_ipv6_network_per_minute: Some(300),
            auth_rate_limit_ipv6_network_burst: Some(150),
            ..mmcp_core::config::LimitsConfig::default()
        };
        let from_file = resolve_auth_rate_limit(&AuthRateLimitOverrides::default(), Some(&limits));
        let network = from_file.ipv6_network_tier.unwrap();
        assert_eq!(from_file.ipv6_client_prefix_length, 60);
        assert_eq!(
            (
                network.prefix_length,
                network.per_minute.get(),
                network.burst.get()
            ),
            (40, 300, 150)
        );

        let from_override = resolve_auth_rate_limit(
            &overrides(Some("56"), Some("32"), Some("500"), Some("250")),
            Some(&limits),
        );
        let network = from_override.ipv6_network_tier.unwrap();
        assert_eq!(from_override.ipv6_client_prefix_length, 56);
        assert_eq!(
            (
                network.prefix_length,
                network.per_minute.get(),
                network.burst.get()
            ),
            (32, 500, 250)
        );
    }

    #[test]
    fn a_blank_ipv6_setting_is_absent_and_leaves_the_network_budget_on() {
        let (config, warnings) =
            warnings_of(|| resolved(&overrides(Some(""), Some("  "), Some(""), Some(""))));
        assert_eq!(config, AuthRateLimitConfig::default());
        assert_eq!(warnings, 0);
    }

    #[test]
    fn a_zero_network_prefix_switches_the_network_budget_off_at_every_layer() {
        let (config, warnings) = warnings_of(|| resolved(&overrides(None, Some("0"), None, None)));
        assert_eq!(config.ipv6_network_tier, None);
        assert_eq!(
            warnings, 0,
            "zero is the explicit setting, not a rejected value"
        );

        let limits = mmcp_core::config::LimitsConfig {
            auth_rate_limit_ipv6_network_prefix_length: Some(0),
            ..mmcp_core::config::LimitsConfig::default()
        };
        let from_file = resolve_auth_rate_limit(&AuthRateLimitOverrides::default(), Some(&limits));
        assert_eq!(from_file.ipv6_network_tier, None);

        let overridden =
            resolve_auth_rate_limit(&overrides(None, Some("40"), None, None), Some(&limits));
        assert_eq!(overridden.ipv6_network_tier.unwrap().prefix_length, 40);
    }

    #[test]
    fn removing_the_network_budget_leaves_the_client_budget_settings_unchanged() {
        let without = resolved(&overrides(None, Some("0"), None, None));
        let with = resolved(&AuthRateLimitOverrides::default());
        assert_eq!(
            (
                without.per_minute,
                without.burst,
                without.ipv6_client_prefix_length
            ),
            (with.per_minute, with.burst, with.ipv6_client_prefix_length)
        );
    }

    #[test]
    fn a_malformed_ipv6_value_warns_and_falls_through_the_cascade() {
        let (config, warnings) = warnings_of(|| {
            resolved(&overrides(
                Some("wide"),
                Some("big"),
                Some("lots"),
                Some("many"),
            ))
        });
        assert_eq!(config, AuthRateLimitConfig::default());
        assert_eq!(warnings, 4);
    }

    #[test]
    fn a_client_prefix_outside_1_to_128_warns_and_falls_through() {
        let (config, warnings) = warnings_of(|| {
            [
                resolved(&overrides(Some("0"), None, None, None)),
                resolved(&overrides(Some("129"), None, None, None)),
            ]
        });
        assert!(config.iter().all(|c| c.ipv6_client_prefix_length == 64));
        assert_eq!(warnings, 2);
    }

    #[test]
    fn a_network_prefix_above_128_warns_and_removes_the_network_budget() {
        let (config, warnings) =
            warnings_of(|| resolved(&overrides(None, Some("129"), None, None)));
        assert_eq!(config.ipv6_network_tier, None);
        assert_eq!(warnings, 1);
    }

    #[test]
    fn a_network_prefix_not_shorter_than_the_client_prefix_warns_and_removes_the_network_budget() {
        for network_prefix in ["64", "80"] {
            let (config, warnings) =
                warnings_of(|| resolved(&overrides(None, Some(network_prefix), None, None)));
            assert_eq!(config.ipv6_network_tier, None, "{network_prefix}");
            assert_eq!(warnings, 1, "{network_prefix}");
        }
        // A duplicate of a custom client prefix is rejected the same way.
        let (config, warnings) =
            warnings_of(|| resolved(&overrides(Some("56"), Some("56"), None, None)));
        assert_eq!(config.ipv6_network_tier, None);
        assert_eq!(warnings, 1);
    }

    #[test]
    fn a_network_quota_below_the_client_quota_is_kept_with_a_warning() {
        let (config, warnings) =
            warnings_of(|| resolved(&overrides(None, None, Some("5"), Some("2"))));
        let network = config.ipv6_network_tier.unwrap();
        assert_eq!((network.per_minute.get(), network.burst.get()), (5, 2));
        assert_eq!(warnings, 1);
    }

    #[test]
    fn a_network_quota_of_zero_falls_through_with_a_warning() {
        let (config, warnings) =
            warnings_of(|| resolved(&overrides(None, None, Some("0"), Some("0"))));
        assert_eq!(config, AuthRateLimitConfig::default());
        assert_eq!(warnings, 2);
    }
}
