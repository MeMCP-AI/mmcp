//! [`RemoteAuth`]: auth selector for a [`crate::config::Remote::DirectGit`] remote.

use serde::{Deserialize, Serialize};

/// Auth selector for a [`crate::config::Remote::DirectGit`] remote.
///
/// Own file rather than sharing the sibling module that defines
/// [`crate::config::Remote`]: unlike that struct's own
/// `default_include_in_push_all` helper, this is a full independent
/// selector type, not a tightly-coupled implementation detail of one
/// field.
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
    /// known_hosts, netrc, an ambient `GIT_SSH_COMMAND`, whatever the
    /// user's own git is already configured with) to decide which
    /// identity authenticates. Correct default for a plain `ssh://`
    /// remote. The transport layer (`mmcp_git::Credentials::None`)
    /// still forces the git subprocess headless: an unanswerable
    /// credential prompt fails fast instead of hanging, and an
    /// ambient `GIT_SSH_COMMAND`, or a `core.sshCommand` git-config
    /// value when no ambient value exists, is respected and extended
    /// with a batch-mode flag rather than replaced, so only a fully
    /// unconfigured caller falls back to the transport layer's own
    /// bare default. One narrower, disclosed gap: a non-OpenSSH
    /// ambient or `core.sshCommand` value may not accept the appended
    /// flag as intended.
    #[default]
    None,
    /// Same runtime effect as `None` today (maps to
    /// `mmcp_git::Credentials::None`, headless by construction); kept
    /// as a distinct explicit variant so a config can document intent
    /// (this remote deliberately relies on the user's SSH agent) even
    /// though there is nothing extra to configure.
    SshAgent,
    /// Bearer credential read from the derived
    /// `MMCP_SYNC_TOKEN_<NAME>` env var
    /// ([`crate::config::SyncConfig::token_env_name`]), mapped to
    /// `mmcp_git::Credentials::BearerHttp`.
    Bearer,
}
