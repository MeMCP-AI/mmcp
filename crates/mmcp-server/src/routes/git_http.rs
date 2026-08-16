//! Native git smart HTTP responder.
//!
//! Implements the git smart HTTP v0/v1/v2 protocol subset clients need
//! to `clone`, `fetch`, and `push` against the server's bare repositories.
//! All pack-protocol work runs in-process via
//! [`gix::Repository::serve_upload_pack_info_refs`],
//! [`gix::Repository::serve_pack_upload_v2_dispatch_auto`], and
//! [`gix::Repository::serve_pack_receive`]. No `git` binary is ever
//! spawned on the server host.
//!
//! The route surface:
//!
//! - `GET  /git/:group_id.git/info/refs?service=git-upload-pack`
//! - `GET  /git/:group_id.git/info/refs?service=git-receive-pack`
//! - `POST /git/:group_id.git/git-upload-pack`
//! - `POST /git/:group_id.git/git-receive-pack`
//!
//! ACL enforcement runs before any serve call. Reads (`git-upload-pack`,
//! both the `info/refs` advertisement and the pack transfer itself)
//! require a valid per-user bearer credential
//! ([`crate::routes::bearer_auth::AuthenticatedUser`], the same
//! credential the `/sync/*` control plane requires). Writes
//! (`git-receive-pack`) reject unless a shared-secret token matches
//! `MMCP_PUSH_TOKEN`. There is no per-group ACL yet on either path:
//! any authenticated user may read any group, and the single global
//! push token authorizes writes to every group.

use std::path::{Path as StdPath, PathBuf};
use std::str::FromStr;
use std::sync::atomic::AtomicBool;

use axum::{
    Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::TryStreamExt;
use mmcp_db::repository::group_repo;
use serde::Deserialize;
use tokio_util::io::{ReaderStream, StreamReader, SyncIoBridge};
use uuid::Uuid;

use crate::routes::bearer_auth::{AuthenticatedUser, BearerAuthRejection, verify_bearer};
use crate::routes::response::{self, FromInternalError, into_generic_response};
use crate::state::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/git/{group_id}/info/refs", get(info_refs))
        .route("/git/{group_id}/git-upload-pack", post(upload_pack))
        .route("/git/{group_id}/git-receive-pack", post(receive_pack))
}

#[derive(Deserialize)]
struct InfoRefsQuery {
    service: String,
}

/// Parse a path segment of the form `<uuid>.git` into a UUID.
fn parse_group_path(raw: &str) -> Result<Uuid, GitHttpError> {
    let stripped = raw
        .strip_suffix(".git")
        .ok_or(GitHttpError::NotFound("group path must end with .git"))?;
    Uuid::from_str(stripped).map_err(|_| GitHttpError::NotFound("group id is not a UUID"))
}

/// Ensure the requested group exists on the server and return the
/// filesystem path to its bare repository.
async fn ensure_group(state: &ServerState, group_id: Uuid) -> Result<PathBuf, GitHttpError> {
    let conn = state.database.connection();
    let row = group_repo::find_by_id(conn, group_id)
        .await
        .map_err(into_generic_response)?
        .ok_or(GitHttpError::NotFound("group not found"))?;
    let _ = row;
    Ok(state.group_repo_path(group_id))
}

/// Protocol version the client negotiated via the `Git-Protocol` HTTP
/// header. `version=2` selects the v2 stateful command dispatch; anything
/// else (missing header, `version=1`, `version=0`) falls back to v0/v1.
///
/// The header format is a semicolon-delimited list of `key=value` pairs.
/// Matching ignores case on `version` and trims whitespace, mirroring
/// upstream git's own parser.
fn negotiated_protocol_version(headers: &HeaderMap) -> u8 {
    let Some(raw) = headers.get("git-protocol").and_then(|v| v.to_str().ok()) else {
        return 1;
    };
    for entry in raw.split([';', ':']) {
        let entry = entry.trim();
        let Some((key, value)) = entry.split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("version") && value.trim() == "2" {
            return 2;
        }
    }
    1
}

async fn info_refs(
    State(state): State<ServerState>,
    Path(group_id): Path<String>,
    Query(query): Query<InfoRefsQuery>,
    headers: HeaderMap,
) -> Result<Response, GitHttpError> {
    let uuid = parse_group_path(&group_id)?;
    // Validate the service and run its auth check BEFORE the
    // `ensure_group` database lookup below: an unauthenticated or
    // wrongly-authenticated caller must be rejected identically
    // whether the requested group exists or not. `ensure_group`'s own
    // 404 is reachable only past a passing credential for a known
    // service; otherwise its distinct status/body from an auth
    // rejection, OR from the unknown-service rejection below, becomes
    // an existence oracle for every group UUID. The `match` (rather
    // than an `if`/`else if` that silently falls through) is what
    // closes that third path: an unrecognized `service` value is
    // rejected here, uniformly, before any group lookup runs, instead
    // of reaching `ensure_group` unauthenticated.
    match query.service.as_str() {
        "git-receive-pack" => enforce_write(&state, &headers, uuid)?,
        "git-upload-pack" => {
            // Read (clone/fetch) advertisement. Requires the same
            // per-user bearer credential as the `/sync/*` control
            // plane (`AuthenticatedUser`), not the shared-secret push
            // token: an unauthenticated caller must not be able to
            // enumerate a group's refs, the first step toward cloning
            // its full bare repo content.
            verify_bearer(&headers, &state).map_err(GitHttpError::BearerAuth)?;
        }
        _ => return Err(GitHttpError::NotFound("unknown git service")),
    }
    let repo_path = ensure_group(&state, uuid).await?;
    let protocol_version = negotiated_protocol_version(&headers);
    let service = query.service.clone();
    let content_type = format!("application/x-{service}-advertisement");

    let body =
        tokio::task::spawn_blocking(move || advertise_refs(&repo_path, &service, protocol_version))
            .await
            .map_err(into_generic_response)??;

    Ok((
        StatusCode::OK,
        [
            ("Content-Type", content_type.as_str()),
            ("Cache-Control", "no-cache"),
        ],
        body,
    )
        .into_response())
}

/// Produce the complete `info/refs` response body for the requested
/// service and protocol version, in-process via gix.
fn advertise_refs(
    repo_path: &StdPath,
    service: &str,
    protocol_version: u8,
) -> Result<Vec<u8>, GitHttpError> {
    let repo = gix::open(repo_path).map_err(into_generic_response)?;
    let mut out = service_announcement(service);
    match (service, protocol_version) {
        ("git-upload-pack", 2) => {
            let opts = gix::protocol::upload_pack::OptionsV2::default();
            repo.serve_upload_pack_info_refs_v2(&mut out, &opts)
                .map_err(into_generic_response)?;
        }
        ("git-upload-pack", _) => {
            let opts = gix::protocol::upload_pack::Options::default();
            repo.serve_upload_pack_info_refs(&mut out, &opts)
                .map_err(into_generic_response)?;
        }
        ("git-receive-pack", _) => {
            let opts = gix::protocol::receive_pack::advertisement::Options::default();
            repo.serve_receive_pack_info_refs(&mut out, &opts)
                .map_err(into_generic_response)?;
        }
        _ => return Err(GitHttpError::NotFound("unknown git service")),
    }
    Ok(out)
}

async fn upload_pack(
    State(state): State<ServerState>,
    Path(group_id): Path<String>,
    // Every request on this route is a clone/fetch of full group
    // content: gated behind the same per-user bearer credential as
    // the `/sync/*` control plane and `info_refs`'s `git-upload-pack`
    // branch, unlike `git-receive-pack` which authorizes writes via
    // the separate shared-secret push token.
    _caller: AuthenticatedUser,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GitHttpError> {
    let uuid = parse_group_path(&group_id)?;
    let repo_path = ensure_group(&state, uuid).await?;
    let protocol_version = negotiated_protocol_version(&headers);
    stream_serve(
        ServeKind::UploadPack { protocol_version },
        repo_path,
        body,
        "application/x-git-upload-pack-result",
        None,
    )
    .await
}

async fn receive_pack(
    State(state): State<ServerState>,
    Path(group_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GitHttpError> {
    let uuid = parse_group_path(&group_id)?;
    enforce_write(&state, &headers, uuid)?;
    let repo_path = ensure_group(&state, uuid).await?;
    // Serialize writes per-group so two concurrent pushes can't race the
    // receive-pack state machine and leave refs in an inconsistent
    // state. Reads (upload-pack) stay unserialized; they only observe
    // the on-disk tree.
    let write_lock = state.repo_write_lock(uuid);
    let guard = write_lock.lock_owned().await;
    stream_serve(
        ServeKind::ReceivePack,
        repo_path,
        body,
        "application/x-git-receive-pack-result",
        Some(guard),
    )
    .await
}

#[derive(Clone, Copy)]
enum ServeKind {
    UploadPack { protocol_version: u8 },
    ReceivePack,
}

impl ServeKind {
    fn label(self) -> &'static str {
        match self {
            ServeKind::UploadPack { .. } => "upload-pack",
            ServeKind::ReceivePack => "receive-pack",
        }
    }
}

/// Drive a gix serve endpoint against an async HTTP request/response
/// pair. Request body bytes flow client → async duplex → blocking
/// reader → serve; serve → blocking writer → async duplex → response
/// body. The gix serve APIs are blocking, so the actual work runs on
/// `spawn_blocking` with [`SyncIoBridge`] wrappers on each pipe half.
///
/// An optional `write_guard` is held alongside the blocking task so a
/// per-group write lock stays acquired for the whole streaming lifetime
/// (receive-pack only).
async fn stream_serve(
    kind: ServeKind,
    repo_path: PathBuf,
    body: Body,
    content_type: &'static str,
    write_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
) -> Result<Response, GitHttpError> {
    // Bounded duplex pipes. 64 KiB is enough for pkt-line framing
    // without blocking the producer; pack bytes stream through in
    // side-band frames up to 64 KiB each regardless of buffer size.
    let (mut req_writer_async, req_reader_async) = tokio::io::duplex(64 * 1024);
    let (resp_writer_async, resp_reader_async) = tokio::io::duplex(64 * 1024);

    // Pump the HTTP request body into the async half of the request
    // pipe. When `body` completes, `req_writer_async` drops and the
    // blocking reader on the other side sees EOF, which is what the
    // serve state machines rely on to know the client's request is
    // done.
    tokio::spawn(async move {
        let stream = body.into_data_stream().map_err(std::io::Error::other);
        let mut reader = StreamReader::new(stream);
        let _ = tokio::io::copy(&mut reader, &mut req_writer_async).await;
    });

    // Run the serve state machine on a blocking task. We can only call
    // `SyncIoBridge::new` from a context that has a tokio runtime
    // handle; spawn_blocking satisfies that since the blocking task
    // is owned by the runtime.
    let service_label = kind.label();
    tokio::task::spawn_blocking(move || {
        let reader = SyncIoBridge::new(req_reader_async);
        let writer = SyncIoBridge::new(resp_writer_async);
        let interrupt = AtomicBool::new(false);
        if let Err(err) = run_serve(kind, &repo_path, reader, writer, &interrupt) {
            tracing::warn!(
                target: "mmcp_server::git_http",
                service = service_label,
                kind = err.kind_label(),
                error = %err,
                "gix serve failed",
            );
        }
        // Release the per-group write lock (if any) only after the
        // blocking task finishes so a concurrent push waits for the
        // whole request to complete, not just for the response to
        // start streaming.
        drop(write_guard);
    });

    let response_body = Body::from_stream(ReaderStream::new(resp_reader_async));
    Ok((
        StatusCode::OK,
        [
            ("Content-Type", content_type),
            ("Cache-Control", "no-cache"),
        ],
        response_body,
    )
        .into_response())
}

/// Failure modes from [`run_serve`]'s four gix dispatch paths.
///
/// Each variant wraps the concrete gix error for its path so a caller
/// can match on the failing stage instead of parsing the rendered
/// message.
#[derive(Debug, thiserror::Error)]
enum ServeError {
    #[error("failed to open the repository")]
    OpenRepo(#[source] Box<gix::open::Error>),
    #[error("upload-pack v1 dispatch failed")]
    UploadPackV1(#[source] Box<gix::repository::serve::ServePackUploadV1Error>),
    #[error("upload-pack v2 dispatch failed")]
    UploadPackV2(#[source] Box<gix::repository::serve::ServePackUploadError>),
    #[error("receive-pack dispatch failed")]
    ReceivePack(#[source] Box<gix::repository::serve::ServePackReceiveError>),
}

impl ServeError {
    /// Short, stable identifier for structured logging.
    fn kind_label(&self) -> &'static str {
        match self {
            ServeError::OpenRepo(_) => "open_repo",
            ServeError::UploadPackV1(_) => "upload_pack_v1",
            ServeError::UploadPackV2(_) => "upload_pack_v2",
            ServeError::ReceivePack(_) => "receive_pack",
        }
    }
}

/// Dispatch to the appropriate gix serve entry point. Blocking; runs
/// on the `spawn_blocking` thread.
fn run_serve<R, W>(
    kind: ServeKind,
    repo_path: &StdPath,
    reader: R,
    mut writer: W,
    interrupt: &AtomicBool,
) -> Result<(), ServeError>
where
    R: std::io::Read,
    W: std::io::Write,
{
    let repo = gix::open(repo_path).map_err(|e| ServeError::OpenRepo(Box::new(e)))?;
    match kind {
        ServeKind::UploadPack {
            protocol_version: 2,
        } => {
            let _outcome = repo
                .serve_pack_upload_v2_dispatch_auto(reader, &mut writer, interrupt)
                .map_err(|e| ServeError::UploadPackV2(Box::new(e)))?;
        }
        ServeKind::UploadPack { .. } => {
            let _outcome = repo
                .serve_pack_upload_v1_auto(reader, &mut writer, interrupt)
                .map_err(|e| ServeError::UploadPackV1(Box::new(e)))?;
        }
        ServeKind::ReceivePack => {
            let mut progress = gix::progress::Discard;
            let _outcome = repo
                .serve_pack_receive(reader, &mut writer, &mut progress, interrupt)
                .map_err(|e| ServeError::ReceivePack(Box::new(e)))?;
        }
    }
    Ok(())
}

/// pkt-line formatted service advertisement prefix, required by the
/// smart HTTP v1/v2 protocol before the serve-side content.
fn service_announcement(service: &str) -> Vec<u8> {
    let body = format!("# service={service}\n");
    let mut out = Vec::new();
    let len = body.len() + 4;
    out.extend_from_slice(format!("{len:04x}").as_bytes());
    out.extend_from_slice(body.as_bytes());
    out.extend_from_slice(b"0000");
    out
}

/// Enforce write access for `receive-pack` requests.
///
/// A single global bearer token, resolved through the typed config
/// layer (`ServerConfig::push_token`, env `MMCP_PUSH_TOKEN`),
/// authorizes pushes to EVERY group hosted by this server; there is
/// no per-group token concept yet. `group_id` plays no role in the
/// authorization decision itself; it is accepted only so a rejected
/// push's audit log line names the group being targeted. Missing or
/// wrong token returns 403/401. Real per-group role-based
/// enforcement replaces this once there's a real authenticated user
/// and a per-group token store.
///
/// Thin wrapper over [`enforce_push_token`]: extracts the `Bearer `
/// credential from the request's own `Authorization` header, the
/// only credential slot `git-receive-pack` requests carry.
fn enforce_write(
    state: &ServerState,
    headers: &HeaderMap,
    group_id: Uuid,
) -> Result<(), GitHttpError> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let presented = auth.strip_prefix("Bearer ");
    enforce_push_token(state, presented, group_id)
}

/// Core shared-secret push-token comparison, shared by
/// [`enforce_write`] (git smart-HTTP `receive-pack`) and
/// `crate::routes::sync::post_push` (mmcp issue #190: `post_push`
/// previously accepted a plain per-user bearer token for a write
/// capability equivalent to `receive-pack`, a materially weaker
/// credential than this shared secret). One comparison, two callers,
/// so the two write paths can never drift apart on what counts as a
/// valid push credential.
///
/// `presented` is the caller's candidate token, already stripped of
/// any transport-specific framing (e.g. a `Bearer ` prefix) by the
/// caller; `None` covers both a missing credential and one that
/// doesn't match the expected framing, so it never partially matches
/// a configured token by accident.
pub(crate) fn enforce_push_token(
    state: &ServerState,
    presented: Option<&str>,
    group_id: Uuid,
) -> Result<(), GitHttpError> {
    let Some(expected) = state.push_token.as_deref() else {
        return Err(GitHttpError::Forbidden(
            "push disabled: set MMCP_PUSH_TOKEN",
        ));
    };
    if presented != Some(expected) {
        tracing::warn!(group = %group_id, "push rejected: invalid or missing push token");
        return Err(GitHttpError::Unauthorized);
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) enum GitHttpError {
    NotFound(&'static str),
    Internal(String),
    Unauthorized,
    Forbidden(&'static str),
    /// A per-user bearer credential (`AuthenticatedUser`, verified via
    /// [`verify_bearer`]) was missing or failed verification.
    /// Delegates its response to [`BearerAuthRejection::into_response`]
    /// so the read (upload-pack) path renders byte-identical status,
    /// headers, and log lines to the `/sync/*` control plane instead
    /// of a hand-rolled duplicate.
    BearerAuth(BearerAuthRejection),
}

impl FromInternalError for GitHttpError {
    fn from_internal_error() -> Self {
        GitHttpError::Internal(response::GENERIC_INTERNAL_ERROR_MESSAGE.to_string())
    }
}

impl IntoResponse for GitHttpError {
    fn into_response(self) -> Response {
        match self {
            GitHttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.to_string()).into_response(),
            GitHttpError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
            GitHttpError::Unauthorized => {
                // Emit `WWW-Authenticate` so stock git's HTTP auth flow
                // retries with credentials from the user's credential
                // helper instead of surfacing a bare 401. The scheme
                // must match what the server actually accepts: the
                // only acceptance path is `strip_prefix("Bearer ")`
                // above, so the challenge advertises Bearer, not
                // Basic; a Basic challenge would make every
                // credential-helper retry fail by construction.
                (
                    StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", r#"Bearer realm="mmcp""#)],
                    "missing or invalid token".to_string(),
                )
                    .into_response()
            }
            GitHttpError::Forbidden(msg) => {
                (StatusCode::FORBIDDEN, msg.to_string()).into_response()
            }
            GitHttpError::BearerAuth(rejection) => rejection.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use axum::http::HeaderValue;

    use super::*;

    /// Bootstrap a real `ServerState` with the caller's choice of
    /// push token, so `enforce_write` is exercised through the typed
    /// config layer end to end, not a hand-rolled stand-in.
    async fn state_with_push_token(push_token: Option<&str>) -> (ServerState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = crate::config::test_support::minimal_server_config(tmp.path().to_path_buf());
        let cfg = crate::config::ServerConfig {
            push_token: push_token.map(str::to_string),
            ..cfg
        };
        let state = ServerState::initialize(&cfg).await.expect("state init");
        (state, tmp)
    }

    fn bearer_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).expect("valid header value"),
        );
        headers
    }

    #[tokio::test]
    async fn enforce_write_rejects_with_forbidden_when_no_push_token_is_configured() {
        let (state, _tmp) = state_with_push_token(None).await;
        let err = enforce_write(&state, &HeaderMap::new(), Uuid::now_v7()).unwrap_err();
        assert!(matches!(err, GitHttpError::Forbidden(_)));
    }

    #[tokio::test]
    async fn enforce_write_rejects_with_unauthorized_on_wrong_token() {
        let (state, _tmp) = state_with_push_token(Some("s3cr3t")).await;
        let err = enforce_write(&state, &bearer_headers("wrong"), Uuid::now_v7()).unwrap_err();
        assert!(matches!(err, GitHttpError::Unauthorized));
    }

    #[tokio::test]
    async fn enforce_write_rejects_with_unauthorized_when_header_is_missing() {
        let (state, _tmp) = state_with_push_token(Some("s3cr3t")).await;
        let err = enforce_write(&state, &HeaderMap::new(), Uuid::now_v7()).unwrap_err();
        assert!(matches!(err, GitHttpError::Unauthorized));
    }

    #[tokio::test]
    async fn enforce_write_accepts_the_matching_bearer_token() {
        let (state, _tmp) = state_with_push_token(Some("s3cr3t")).await;
        assert!(enforce_write(&state, &bearer_headers("s3cr3t"), Uuid::now_v7()).is_ok());
    }

    #[tokio::test]
    async fn enforce_write_authorizes_every_group_id_under_the_single_global_token() {
        // Documents the current model explicitly: `group_id` plays no
        // role in the authorization decision, so two different
        // groups both pass under the same global token.
        let (state, _tmp) = state_with_push_token(Some("s3cr3t")).await;
        assert!(enforce_write(&state, &bearer_headers("s3cr3t"), Uuid::now_v7()).is_ok());
        assert!(enforce_write(&state, &bearer_headers("s3cr3t"), Uuid::now_v7()).is_ok());
    }

    #[test]
    fn run_serve_reports_a_typed_open_repo_error_with_its_structured_kind() {
        // A directory that is not a git repository makes `gix::open`
        // fail, proving the dispatch path actually reaches
        // `ServeError::OpenRepo` end to end (error -> variant ->
        // structured label), not just that the enum compiles.
        let tmp = tempfile::tempdir().expect("tempdir");
        let interrupt = AtomicBool::new(false);
        let err = run_serve(
            ServeKind::ReceivePack,
            tmp.path(),
            std::io::empty(),
            Vec::<u8>::new(),
            &interrupt,
        )
        .unwrap_err();
        assert!(matches!(err, ServeError::OpenRepo(_)));
        assert_eq!(err.kind_label(), "open_repo");
    }

    #[test]
    fn unauthorized_response_advertises_a_bearer_challenge_matching_what_is_accepted() {
        let response = GitHttpError::Unauthorized.into_response();
        let challenge = response
            .headers()
            .get("WWW-Authenticate")
            .expect("WWW-Authenticate header present")
            .to_str()
            .expect("header is valid utf-8");
        assert!(
            challenge.starts_with("Bearer"),
            "challenge scheme must match the Bearer-only acceptance path in enforce_write, \
             got: {challenge}"
        );
    }
}
