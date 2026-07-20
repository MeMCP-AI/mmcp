//! HTTP client wrapping the control-plane endpoints served by
//! `mmcp-server` under `/sync/`.
//!
//! This client talks the sync control protocol: listing the groups
//! the caller has access to, fetching their current ref heads,
//! and registering version bumps when local edits ship. The
//! corresponding server-side handlers land in the `mmcp-server`
//! phase of the implementation plan.

use std::sync::Arc;
use std::time::Duration;

use mmcp_core::memory::BumpIntent;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::SyncError;

/// One entry in the `/sync/manifest` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteGroup {
    /// Group identifier.
    pub group_id: Uuid,

    /// Human-readable slug as it lives in the group's manifest.
    pub slug: String,

    /// Commit hash at the tip of `main` on the server side.
    pub head_commit: String,
}

/// Response body for `GET /sync/manifest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestResponse {
    pub groups: Vec<RemoteGroup>,
}

/// Response body for `GET /sync/refs/:group_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefsResponse {
    pub group_id: Uuid,
    pub refs: Vec<RefEntry>,
}

/// A single advertised ref on the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefEntry {
    pub name: String,
    pub commit: String,
}

/// Request body for `POST /sync/push`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRequest {
    pub group_id: Uuid,
    pub memory_id: Uuid,
    pub commit: String,
    pub bump: BumpIntent,
    #[serde(default)]
    pub message: Option<String>,
}

/// Response body for `POST /sync/push`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResponse {
    pub group_id: Uuid,
    pub memory_id: Uuid,
    pub assigned_version: String,
    pub tag: String,
}

/// Body the server sends back on a 409 conflict so the client can
/// surface a useful resolution hint without parsing free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictBody {
    pub memory_id: Uuid,
    pub local_commit: String,
    pub remote_commit: String,
}

/// HTTP client wrapping the `/sync/*` endpoints.
#[derive(Debug, Clone)]
pub struct SyncClient {
    inner: Arc<SyncClientInner>,
}

#[derive(Debug)]
struct SyncClientInner {
    http: Client,
    base_url: String,
    bearer_token: Option<String>,
}

impl SyncClient {
    /// Build a new client pointed at the given server base URL. A
    /// trailing slash on `base_url` is stripped so path joins are
    /// well-formed.
    pub fn new(base_url: impl Into<String>) -> Result<Self, SyncError> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(SyncError::transport)?;
        let base_url = base_url.into();
        let base_url = base_url.trim_end_matches('/').to_string();
        Ok(Self {
            inner: Arc::new(SyncClientInner {
                http,
                base_url,
                bearer_token: None,
            }),
        })
    }

    /// Attach a bearer token to every request. Rebuilds the inner
    /// arc so existing clones keep their old authorization state.
    #[must_use]
    pub fn with_bearer(self, token: impl Into<String>) -> Self {
        let inner = SyncClientInner {
            http: self.inner.http.clone(),
            base_url: self.inner.base_url.clone(),
            bearer_token: Some(token.into()),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.inner.base_url, path)
    }

    /// Build the git smart HTTP URL for a group's bare repo on the
    /// configured server. Used by the sync engine to feed the git
    /// content-plane subprocess a remote to push to or fetch from.
    #[must_use]
    pub fn git_url_for(&self, group_id: Uuid) -> String {
        format!("{}/git/{}.git", self.inner.base_url, group_id)
    }

    /// Derive git content-plane credentials from the control-plane
    /// bearer token, if any. Returning the same auth for both planes
    /// means a user who configures `--token` once gets authenticated
    /// pushes to mmcp-server's smart-HTTP endpoint for free, without
    /// a second credential source to keep in sync.
    #[must_use]
    pub fn git_credentials(&self) -> mmcp_git::Credentials {
        match self.inner.bearer_token.as_deref() {
            Some(token) if !token.is_empty() => mmcp_git::Credentials::bearer(token),
            _ => mmcp_git::Credentials::None,
        }
    }

    fn request_builder(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut builder = self.inner.http.request(method, self.url(path));
        if let Some(token) = self.inner.bearer_token.as_ref() {
            builder = builder.bearer_auth(token);
        }
        builder
    }

    /// Fetch the caller's effective group manifest.
    pub async fn get_manifest(&self) -> Result<ManifestResponse, SyncError> {
        let response = self
            .request_builder(reqwest::Method::GET, "/sync/manifest")
            .send()
            .await
            .map_err(SyncError::transport)?;
        parse_ok_body(response).await
    }

    /// Fetch the advertised refs for a specific group.
    pub async fn get_refs(&self, group_id: Uuid) -> Result<RefsResponse, SyncError> {
        let path = format!("/sync/refs/{group_id}");
        let response = self
            .request_builder(reqwest::Method::GET, &path)
            .send()
            .await
            .map_err(SyncError::transport)?;
        parse_ok_body(response).await
    }

    /// Register a version bump for a pending local edit.
    ///
    /// A 409 response is translated into a structured
    /// [`SyncError::Conflict`] so callers can branch on it without
    /// parsing free text.
    pub async fn push_version(&self, req: &PushRequest) -> Result<PushResponse, SyncError> {
        let response = self
            .request_builder(reqwest::Method::POST, "/sync/push")
            .json(req)
            .send()
            .await
            .map_err(SyncError::transport)?;

        let status = response.status();
        if status.is_success() {
            return response
                .json::<PushResponse>()
                .await
                .map_err(SyncError::transport);
        }

        if status == StatusCode::CONFLICT {
            let conflict: ConflictBody = response.json().await.map_err(SyncError::transport)?;
            return Err(SyncError::Conflict {
                memory: conflict.memory_id,
                local_commit: conflict.local_commit,
                remote_commit: conflict.remote_commit,
            });
        }

        Err(remote_error(status, response).await)
    }
}

async fn parse_ok_body<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, SyncError> {
    let status = response.status();
    if !status.is_success() {
        return Err(remote_error(status, response).await);
    }
    response.json::<T>().await.map_err(SyncError::transport)
}

async fn remote_error(status: StatusCode, response: reqwest::Response) -> SyncError {
    let status_code = status.as_u16();
    let body = response.text().await.unwrap_or_default();
    let message = if body.is_empty() {
        status.canonical_reason().unwrap_or("unknown").to_string()
    } else {
        body
    };
    SyncError::Remote {
        status: status_code,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `git_url_for` must produce `{base_url}/git/{uuid}.git`
    /// verbatim — mutations that short-circuit the format (e.g.
    /// returning an empty or constant string) would otherwise
    /// route pushes at the wrong URL silently.
    #[test]
    fn git_url_for_formats_base_url_plus_group_uuid_dot_git() {
        let client = SyncClient::new("https://mmcp.example.com").expect("build client");
        let group = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let url = client.git_url_for(group);
        assert_eq!(
            url,
            "https://mmcp.example.com/git/00000000-0000-0000-0000-000000000001.git"
        );
    }

    #[test]
    fn git_url_for_uses_base_url_without_trailing_slash() {
        // `new` strips a single trailing slash; a slash-terminated
        // input must still produce the no-double-slash form.
        let client = SyncClient::new("https://mmcp.example.com/").expect("build client");
        let group = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let url = client.git_url_for(group);
        assert!(!url.contains("com//git/"), "no double slash: {url}");
        assert!(url.ends_with(".git"));
    }

    /// `git_credentials` returns `None` when no bearer token was
    /// configured. Locks in the negative branch of the match so a
    /// mutation replacing the body with `Default::default()` would
    /// only agree on this case; the positive-branch tests below
    /// then disagree and catch it.
    #[test]
    fn git_credentials_without_bearer_returns_none() {
        let client = SyncClient::new("http://localhost:0").expect("build client");
        assert_eq!(client.git_credentials(), mmcp_git::Credentials::None);
    }

    #[test]
    fn git_credentials_with_nonempty_bearer_returns_bearer() {
        let client = SyncClient::new("http://localhost:0")
            .expect("build client")
            .with_bearer("ghp_test_token");
        assert_eq!(
            client.git_credentials(),
            mmcp_git::Credentials::bearer("ghp_test_token")
        );
    }

    /// The `Some(token) if !token.is_empty()` guard specifically
    /// demands a non-empty token. Mutation testing flagged the
    /// `!token.is_empty()` predicate as escaping — a caller that
    /// stored an empty bearer string must fall back to `None`, not
    /// produce a `Credentials::bearer("")` call that the server
    /// would then silently reject.
    #[test]
    fn git_credentials_with_empty_bearer_string_falls_back_to_none() {
        let client = SyncClient::new("http://localhost:0")
            .expect("build client")
            .with_bearer("");
        assert_eq!(client.git_credentials(), mmcp_git::Credentials::None);
    }
}
