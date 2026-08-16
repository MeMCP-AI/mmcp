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

use mmcp_core::conventions::PUSH_TOKEN_HEADER;
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
    push_credential: Option<String>,
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
                push_credential: None,
            }),
        })
    }

    /// Attach the control-plane bearer token, sent as the
    /// `Authorization` header on every `/sync/*` request. Rebuilds
    /// the inner arc so existing clones keep their old
    /// authorization state.
    ///
    /// This is a per-user PASETO session token, verified server
    /// side against `mmcp_auth::TokenVerifier`. It does not feed
    /// [`SyncClient::git_credentials`]: the content plane compares
    /// against a separate shared secret, see
    /// [`SyncClient::with_push_credential`].
    #[must_use]
    pub fn with_bearer(self, token: impl Into<String>) -> Self {
        let inner = SyncClientInner {
            http: self.inner.http.clone(),
            base_url: self.inner.base_url.clone(),
            bearer_token: Some(token.into()),
            push_credential: self.inner.push_credential.clone(),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Attach the content-plane push credential, returned by
    /// [`SyncClient::git_credentials`] for the git smart-HTTP
    /// subprocess. Rebuilds the inner arc so existing clones keep
    /// their old credential state.
    ///
    /// This is a shared secret compared by exact string equality
    /// against the server's own `MMCP_PUSH_TOKEN`. It does not feed
    /// the `/sync/*` `Authorization` header, see
    /// [`SyncClient::with_bearer`].
    #[must_use]
    pub fn with_push_credential(self, token: impl Into<String>) -> Self {
        let inner = SyncClientInner {
            http: self.inner.http.clone(),
            base_url: self.inner.base_url.clone(),
            bearer_token: self.inner.bearer_token.clone(),
            push_credential: Some(token.into()),
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

    /// Git content-plane credentials, built from the push
    /// credential set via [`SyncClient::with_push_credential`], if
    /// any. The control-plane bearer token from
    /// [`SyncClient::with_bearer`] is a distinct credential type
    /// (a per-user PASETO session token) and never feeds this
    /// method: the server's git smart-HTTP endpoint compares
    /// against its own shared `MMCP_PUSH_TOKEN` secret, not against
    /// a PASETO token.
    #[must_use]
    pub fn git_credentials(&self) -> mmcp_git::Credentials {
        match self.inner.push_credential.as_deref() {
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
    /// When a push credential is configured via
    /// [`SyncClient::with_push_credential`], it is also attached as
    /// the [`PUSH_TOKEN_HEADER`] header: the server's `POST
    /// /sync/push` handler requires this header in addition to the
    /// control-plane bearer token.
    /// The credential is omitted, rather than sent empty, when unset or empty,
    /// mirroring [`SyncClient::git_credentials`]'s own
    /// empty-string guard. `get_manifest` and `get_refs` are reads
    /// and never attach this header: only this write path is gated
    /// on the server side.
    ///
    /// A 409 response is translated into a structured
    /// [`SyncError::Conflict`] so callers can branch on it without
    /// parsing free text.
    pub async fn push_version(&self, req: &PushRequest) -> Result<PushResponse, SyncError> {
        let mut builder = self.request_builder(reqwest::Method::POST, "/sync/push");
        if let Some(token) = self.inner.push_credential.as_deref()
            && !token.is_empty()
        {
            builder = builder.header(PUSH_TOKEN_HEADER, token);
        }

        let response = builder
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// `git_url_for` must produce `{base_url}/git/{uuid}.git`
    /// verbatim: mutations that short-circuit the format (e.g.
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

    /// `git_credentials` returns `None` when no push credential was
    /// configured. Locks in the negative branch of the match so a
    /// mutation replacing the body with `Default::default()` would
    /// only agree on this case; the positive-branch tests below
    /// then disagree and catch it.
    #[test]
    fn git_credentials_without_push_credential_returns_none() {
        let client = SyncClient::new("http://localhost:0").expect("build client");
        assert_eq!(client.git_credentials(), mmcp_git::Credentials::None);
    }

    #[test]
    fn git_credentials_with_nonempty_push_credential_returns_bearer() {
        let client = SyncClient::new("http://localhost:0")
            .expect("build client")
            .with_push_credential("push-secret-token");
        assert_eq!(
            client.git_credentials(),
            mmcp_git::Credentials::bearer("push-secret-token")
        );
    }

    /// A control-plane bearer token configured via `with_bearer`
    /// must never leak into `git_credentials`: the two planes carry
    /// mutually incompatible credential types, and the server
    /// rejects a PASETO token presented as the git push secret.
    #[test]
    fn with_bearer_alone_never_leaks_into_git_credentials() {
        let client = SyncClient::new("http://localhost:0")
            .expect("build client")
            .with_bearer("paseto-control-plane-token");
        assert_eq!(client.git_credentials(), mmcp_git::Credentials::None);
    }

    /// The two credential slots are independent: setting both to
    /// different values keeps each in its own plane, and setting
    /// one never overwrites the other on the rebuilt inner arc.
    #[test]
    fn with_bearer_and_with_push_credential_compose_independently() {
        let client = SyncClient::new("http://localhost:0")
            .expect("build client")
            .with_bearer("paseto-control-plane-token")
            .with_push_credential("shared-push-secret");
        assert_eq!(
            client.git_credentials(),
            mmcp_git::Credentials::bearer("shared-push-secret"),
            "git_credentials must carry the push credential, never the bearer token"
        );
    }

    /// The `Some(token) if !token.is_empty()` guard specifically
    /// demands a non-empty token. Mutation testing flagged the
    /// `!token.is_empty()` predicate as escaping: a caller that
    /// stored an empty push credential must fall back to `None`,
    /// not produce a `Credentials::bearer("")` call that the server
    /// would then silently reject.
    #[test]
    fn git_credentials_with_empty_push_credential_falls_back_to_none() {
        let client = SyncClient::new("http://localhost:0")
            .expect("build client")
            .with_push_credential("");
        assert_eq!(client.git_credentials(), mmcp_git::Credentials::None);
    }

    /// Minimal but realistic [`PushRequest`]/[`PushResponse`] pair
    /// for the wiremock tests below: field values are arbitrary,
    /// only their round-trip through the mocked `/sync/push`
    /// endpoint matters.
    fn sample_push_request() -> PushRequest {
        PushRequest {
            group_id: Uuid::parse_str("00000000-0000-0000-0000-000000000010").unwrap(),
            memory_id: Uuid::parse_str("00000000-0000-0000-0000-000000000011").unwrap(),
            commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            bump: BumpIntent::Minor,
            message: None,
        }
    }

    fn sample_push_response(req: &PushRequest) -> PushResponse {
        PushResponse {
            group_id: req.group_id,
            memory_id: req.memory_id,
            assigned_version: "0.2.0".to_string(),
            tag: "v0.2.0".to_string(),
        }
    }

    /// Matches a request carrying no `x-mmcp-push-token` header at
    /// all. Mirrors `mmcp-store`'s `NoAuthorizationHeader` pattern
    /// (`crates/mmcp-store/tests/build_engine_bearer.rs`): wiremock
    /// ships matchers for an exact or present header, but none for
    /// absence.
    struct NoPushTokenHeader;

    impl wiremock::Match for NoPushTokenHeader {
        fn matches(&self, request: &wiremock::Request) -> bool {
            !request.headers.contains_key(PUSH_TOKEN_HEADER)
        }
    }

    /// `push_version` must attach the configured push credential as
    /// the literal `x-mmcp-push-token` header (asserted as a raw
    /// string, not via [`PUSH_TOKEN_HEADER`], so a drift between the
    /// constant's value and the server's own literal would still
    /// fail this test rather than agreeing with itself).
    #[tokio::test]
    async fn push_version_sends_push_token_header_when_configured() {
        let server = wiremock::MockServer::start().await;
        let req = sample_push_request();
        let resp = sample_push_response(&req);
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/sync/push"))
            .and(wiremock::matchers::header(
                "x-mmcp-push-token",
                "push-secret-token",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(&resp))
            .expect(1)
            .mount(&server)
            .await;

        let client = SyncClient::new(server.uri())
            .expect("build client")
            .with_push_credential("push-secret-token");

        let result = client
            .push_version(&req)
            .await
            .expect("push_version must succeed");
        assert_eq!(result.assigned_version, resp.assigned_version);
    }

    /// No push credential configured must mean no header at all:
    /// the mock only accepts a request with the header absent, so a
    /// regression that always attaches it (even empty) fails this
    /// test.
    #[tokio::test]
    async fn push_version_omits_push_token_header_when_not_configured() {
        let server = wiremock::MockServer::start().await;
        let req = sample_push_request();
        let resp = sample_push_response(&req);
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/sync/push"))
            .and(NoPushTokenHeader)
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(&resp))
            .expect(1)
            .mount(&server)
            .await;

        let client = SyncClient::new(server.uri()).expect("build client");

        client
            .push_version(&req)
            .await
            .expect("push_version must succeed");
    }

    /// An empty push credential must behave like no credential at
    /// all: the `!token.is_empty()` guard in `push_version` mirrors
    /// `git_credentials_with_empty_push_credential_falls_back_to_none`
    /// above, and mutation testing flagged this exact predicate as
    /// escaping there.
    #[tokio::test]
    async fn push_version_omits_push_token_header_when_credential_is_empty() {
        let server = wiremock::MockServer::start().await;
        let req = sample_push_request();
        let resp = sample_push_response(&req);
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/sync/push"))
            .and(NoPushTokenHeader)
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(&resp))
            .expect(1)
            .mount(&server)
            .await;

        let client = SyncClient::new(server.uri())
            .expect("build client")
            .with_push_credential("");

        client
            .push_version(&req)
            .await
            .expect("push_version must succeed");
    }
}
