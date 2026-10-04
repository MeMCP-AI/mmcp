//! Per-client rate limit of the unauthenticated auth routes.

mod auth_rate_limit_middleware;
mod auth_rate_limit_refusal;
mod auth_rate_limit_sweep_report;
mod auth_rate_limit_sweeper;
mod auth_rate_limiter;
mod client_address_resolver;
mod client_key;
mod defaults;

pub use auth_rate_limit_middleware::enforce_auth_rate_limit;
pub use auth_rate_limit_refusal::AuthRateLimitRefusal;
pub use auth_rate_limit_sweep_report::AuthRateLimitSweepReport;
pub use auth_rate_limit_sweeper::spawn_auth_rate_limit_sweeper;
pub use auth_rate_limiter::AuthRateLimiter;
pub use client_address_resolver::ClientAddressResolver;
pub use client_key::ClientKey;
