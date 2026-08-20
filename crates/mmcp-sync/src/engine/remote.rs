//! [`BoundRemote`]: one sync remote fully wired for the engine to
//! push/fetch/pull against, plus the [`RemoteTransport`] type it
//! composes. [`crate::engine::push_scope::PushScope`] (which
//! selector `push` uses to choose among several bound remotes) lives
//! in its own sibling file: it is a selector, not a description of
//! one bound remote.
//!
//! `mmcp-core::config::Remote` is the parsed, unresolved TOML shape
//! (a `kind` tag plus per-kind fields, still string-addressed). A
//! `BoundRemote` is what `mmcp-store::sync::build_engine` produces
//! from a resolved config entry: a ready `SyncClient` for an
//! `mmcp-server` remote, or a plain URL/credentials/group triple for
//! a `direct-git` remote, either way ready for
//! [`crate::SyncEngine::push`] / `fetch` / `pull` to use without any
//! further config lookup.

use uuid::Uuid;

use crate::client::SyncClient;

/// Transport-specific connection details for one [`BoundRemote`].
#[derive(Debug, Clone)]
pub enum RemoteTransport {
    /// Reached over the control-plane and content-plane HTTP APIs an
    /// `mmcp-server` speaks. Carries a fully-configured
    /// [`SyncClient`] (base URL plus whichever bearer / push
    /// credential the remote resolved to).
    MmcpServer(SyncClient),
    /// A bare git remote reached directly, with no mmcp control
    /// plane: no manifest, no ACL, no version-bump registration.
    /// Scoped to exactly one group, unlike an `mmcp-server` remote's
    /// URL template shared across every group.
    DirectGit {
        /// Git remote URL (any scheme the local git supports).
        url: String,
        /// Credentials for the native backend's `git` subprocess.
        creds: mmcp_git::Credentials,
        /// The single group this remote pushes/fetches.
        group_id: Uuid,
    },
}

impl RemoteTransport {
    /// This transport's effective git content-plane credentials.
    #[must_use]
    pub fn git_credentials(&self) -> mmcp_git::Credentials {
        match self {
            Self::MmcpServer(client) => client.git_credentials(),
            Self::DirectGit { creds, .. } => creds.clone(),
        }
    }
}

/// One sync remote, fully resolved and ready for the engine to use.
///
/// Built once by `mmcp-store::sync::build_engine` per entry in the
/// caller's effective remote set, then handed to
/// [`crate::SyncEngine::new`] as part of the engine's fixed remote
/// list for its lifetime.
#[derive(Debug, Clone)]
pub struct BoundRemote {
    /// Unique name within the engine's remote list. Feeds
    /// [`BoundRemote::tracking_ref`] and [`PushScope::Named`]
    /// matching.
    pub name: String,
    /// Whether this is the engine's default push target and the
    /// only remote [`crate::SyncEngine::pull`] fast-forwards local
    /// `main` from. At most one `BoundRemote` in a given engine's
    /// list should carry `default: true`; the caller (the
    /// mmcp-store resolver) guarantees this before construction.
    pub default: bool,
    /// Whether `PushScope::All` includes this remote.
    pub include_in_push_all: bool,
    /// Transport-specific connection details.
    pub transport: RemoteTransport,
}

impl BoundRemote {
    /// Local remote-tracking ref this remote's `fetch` writes into
    /// and this remote's `pull` (when it is the default) fast-
    /// forwards from: `refs/remotes/<name>/main`.
    ///
    /// Per-remote rather than the shared `MAIN_REMOTE_TRACKING_REF`
    /// constant (`refs/remotes/origin/main`), which only ever
    /// mirrored a single remote's `origin/<branch>` layout; that
    /// stops holding once an engine carries more than one remote.
    #[must_use]
    pub fn tracking_ref(&self) -> String {
        format!("refs/remotes/{}/main", self.name)
    }

    /// This remote's effective git content-plane credentials.
    #[must_use]
    pub fn git_credentials(&self) -> mmcp_git::Credentials {
        self.transport.git_credentials()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn mmcp_server_remote(name: &str, default: bool) -> BoundRemote {
        BoundRemote {
            name: name.to_string(),
            default,
            include_in_push_all: true,
            transport: RemoteTransport::MmcpServer(
                SyncClient::new("https://mmcp.example.com").expect("build client"),
            ),
        }
    }

    #[test]
    fn tracking_ref_is_scoped_to_the_remote_name() {
        let remote = mmcp_server_remote("prod-eu", false);
        assert_eq!(remote.tracking_ref(), "refs/remotes/prod-eu/main");
    }

    #[test]
    fn direct_git_transport_credentials_are_returned_verbatim() {
        let remote = BoundRemote {
            name: "mirror".to_string(),
            default: false,
            include_in_push_all: false,
            transport: RemoteTransport::DirectGit {
                url: "ssh://git@example.com/mirror.git".to_string(),
                creds: mmcp_git::Credentials::bearer("secret"),
                group_id: Uuid::now_v7(),
            },
        };
        assert_eq!(
            remote.git_credentials(),
            mmcp_git::Credentials::bearer("secret")
        );
    }

    #[test]
    fn mmcp_server_transport_credentials_delegate_to_the_client() {
        let remote = mmcp_server_remote("primary", true);
        assert_eq!(remote.git_credentials(), mmcp_git::Credentials::None);
    }
}
