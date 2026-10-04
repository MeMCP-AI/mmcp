//! HTTP route groups.

pub mod auth;
pub mod bearer_auth;
mod defaults;
pub mod git_http;
pub mod health;
pub mod mcp;
mod oauth_flow_cookie;
mod passkey_ceremony;
mod registration_limits;
pub mod response;
pub mod sync;
