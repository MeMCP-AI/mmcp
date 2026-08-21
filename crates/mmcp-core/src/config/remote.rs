//! [`Remote`]: one configured sync remote.

use serde::{Deserialize, Serialize};

use crate::config::RemoteAuth;

/// One configured sync remote.
///
/// Internally tagged (`kind = "..."`) rather than untagged: an
/// untagged enum collapses every variant's parse failure into one
/// generic "data did not match any variant" error, which loses the
/// per-field precision `deny_unknown_fields` otherwise gives each
/// variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Remote {
    /// A remote mmcp server, reached over the control-plane and
    /// content-plane HTTP APIs the client already speaks.
    MmcpServer {
        /// Unique name for this remote within its effective set.
        name: String,
        /// Base URL of the mmcp server.
        url: String,
        /// Whether this remote is the default push target. At most
        /// one remote in a given `remotes` list may set this; see
        /// [`crate::config::SyncConfig::validate`].
        #[serde(default)]
        default: bool,
        /// Whether `mmcp push --all-remotes` includes this remote.
        #[serde(default = "default_include_in_push_all")]
        include_in_push_all: bool,
    },
    /// A bare git remote reached directly, with no mmcp control
    /// plane: no manifest, no ACL, no version-bump registration
    /// through it.
    DirectGit {
        /// Unique name for this remote within its effective set.
        name: String,
        /// Git remote URL (any scheme the local git supports).
        url: String,
        /// How to authenticate against this remote.
        #[serde(default)]
        auth: RemoteAuth,
        /// Which group this remote is scoped to (UUID or slug). A
        /// `direct-git` remote's `url` names ONE concrete git
        /// repository, unlike an `mmcp-server` remote's templated
        /// URL shared across every group, so pushing several
        /// unrelated groups' histories to the same repo's `main` is
        /// incoherent. At project level, `None` resolves to the
        /// project's own group at resolution time
        /// (`mmcp_store::sync::remotes::resolve_direct_git_group`,
        /// not this struct); at user level, `None` is a loud resolution
        /// error there, never silently defaulted, since a group-less
        /// user-level entry would push whichever project is active
        /// into the same shared repo.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        /// Whether this remote is the default push target. At most
        /// one remote in a given `remotes` list may set this; see
        /// [`crate::config::SyncConfig::validate`].
        #[serde(default)]
        default: bool,
        /// Whether `mmcp push --all-remotes` includes this remote.
        #[serde(default = "default_include_in_push_all")]
        include_in_push_all: bool,
    },
}

/// Default for [`Remote::MmcpServer::include_in_push_all`] and
/// [`Remote::DirectGit::include_in_push_all`]: a remote participates
/// in a bulk push unless it explicitly opts out.
fn default_include_in_push_all() -> bool {
    true
}

impl Remote {
    /// This remote's `name` field, whichever variant it is.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::MmcpServer { name, .. } | Self::DirectGit { name, .. } => name,
        }
    }

    /// This remote's `default` field, whichever variant it is.
    #[must_use]
    pub fn is_default(&self) -> bool {
        match self {
            Self::MmcpServer { default, .. } | Self::DirectGit { default, .. } => *default,
        }
    }

    /// This remote's `include_in_push_all` field, whichever variant
    /// it is.
    #[must_use]
    pub fn include_in_push_all(&self) -> bool {
        match self {
            Self::MmcpServer {
                include_in_push_all,
                ..
            }
            | Self::DirectGit {
                include_in_push_all,
                ..
            } => *include_in_push_all,
        }
    }

    /// This remote's kind tag, matching the `kind` value its TOML
    /// form serializes under.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::MmcpServer { .. } => "mmcp-server",
            Self::DirectGit { .. } => "direct-git",
        }
    }

    /// This remote's declared `group` (UUID or slug), if any.
    /// Always `None` for [`Remote::MmcpServer`], which is not scoped
    /// to a single group: its templated URL serves every group in
    /// the effective set. Only [`Remote::DirectGit`] carries this
    /// field; see its own doc comment for the resolution rule.
    #[must_use]
    pub fn group(&self) -> Option<&str> {
        match self {
            Self::MmcpServer { .. } => None,
            Self::DirectGit { group, .. } => group.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn mmcp_server_group_accessor_is_always_none() {
        let remote = Remote::MmcpServer {
            name: "primary".to_string(),
            url: "https://mmcp.example.com".to_string(),
            default: true,
            include_in_push_all: true,
        };
        assert_eq!(remote.group(), None);
    }
}
