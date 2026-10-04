//! Constants of the auth rate limit that no configuration layer overrides.

use std::time::Duration;

/// Leading bits of an IPv6 address that identify one client.
/// One host or one subnet of a LAN holds a /64, so a client rotating addresses inside it keeps one budget (RFC 4291 section 2.5.4).
pub(super) const IPV6_CLIENT_PREFIX_LENGTH: u32 = 64;

/// Period of the sweep that drops the clients whose budget is full again.
/// A client that stays silent for a full replenishment is back at a fresh state, so the period only bounds how long its entry stays in memory.
pub(crate) const AUTH_RATE_LIMIT_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Smallest `Retry-After` the limiter ever sends, in seconds.
/// A refusal whose wait rounds to zero would invite an immediate retry, so the answer is never below one.
pub(super) const MIN_RETRY_AFTER_SECONDS: u64 = 1;
