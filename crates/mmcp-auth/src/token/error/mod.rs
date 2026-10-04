//! Error types of the token module.

mod oauth_flow_open_error;
mod oauth_flow_seal_error;

pub use oauth_flow_open_error::OauthFlowOpenError;
pub use oauth_flow_seal_error::OauthFlowSealError;
