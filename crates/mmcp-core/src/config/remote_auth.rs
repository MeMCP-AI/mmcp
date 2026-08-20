//! [`RemoteAuth`]: auth selector for a [`crate::config::Remote::DirectGit`] remote.

use serde::{Deserialize, Serialize};

/// Auth selector for a [`crate::config::Remote::DirectGit`] remote.
///
/// Own file rather than sharing [`crate::config::remote`]: unlike
/// [`crate::config::Remote`]'s own `default_include_in_push_all`
/// helper, this is a full independent selector type, not a
/// tightly-coupled implementation detail of one field.
///
/// Deliberately not a free-text env-var-name field: that shape would
/// let a git-tracked config redirect an arbitrary already-set env var
/// to an arbitrary URL, exfiltrating whatever that variable holds.
/// The actual env var name is always derived from the remote's own
/// `name` ([`crate::config::SyncConfig::token_env_name`]), never
/// user-supplied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteAuth {
    /// No credential; rely on ambient git/SSH environment (agent,
    /// known_hosts, netrc, whatever the user's own git is already
    /// configured with). Correct default for a plain `ssh://` remote.
    #[default]
    None,
    /// Same runtime effect as `None` today (maps to
    /// `mmcp_git::Credentials::None`); kept as a distinct explicit
    /// variant so a config can document intent (this remote
    /// deliberately relies on the user's SSH agent) even though
    /// there is nothing extra to configure.
    SshAgent,
    /// Bearer credential read from the derived
    /// `MMCP_SYNC_TOKEN_<NAME>` env var
    /// ([`crate::config::SyncConfig::token_env_name`]), mapped to
    /// `mmcp_git::Credentials::BearerHttp`.
    Bearer,
}
