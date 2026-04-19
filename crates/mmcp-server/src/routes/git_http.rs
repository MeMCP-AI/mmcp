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
//! ACL enforcement runs before any serve call. The unauthenticated
//! baseline in this phase allows reads for any existing group and
//! rejects writes unless a shared-secret token matches `MMCP_PUSH_TOKEN`.

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
        .map_err(GitHttpError::internal)?
        .ok_or(GitHttpError::NotFound("group not found"))?;
    let _ = row;
    Ok(state.group_repo_path(group_id))
}

/// Protocol version to drive serve with. v1 only for now: a stock git
/// clone over the fork's v2 path fails during the pack phase with
/// `bad band #119`, meaning the client is already in sideband mode when
/// a section header (starting with `w`, so `wanted-refs\n`) arrives.
/// v1 serve plus the fork's band-1 sideband wrapping is wire-correct
/// end-to-end; stock git downgrades transparently when the server's
/// `info/refs` response is v1-shaped.
///
/// TODO: re-enable v2 once the fork's auto-fetch response shape works
/// with a stock git 2.x clone.
fn negotiated_protocol_version(_headers: &HeaderMap) -> u8 {
    1
}

async fn info_refs(
    State(state): State<ServerState>,
    Path(group_id): Path<String>,
    Query(query): Query<InfoRefsQuery>,
    headers: HeaderMap,
) -> Result<Response, GitHttpError> {
    let uuid = parse_group_path(&group_id)?;
    let repo_path = ensure_group(&state, uuid).await?;
    if query.service == "git-receive-pack" {
        enforce_write(&headers, uuid)?;
    }
    let protocol_version = negotiated_protocol_version(&headers);
    let service = query.service.clone();
    let content_type = format!("application/x-{service}-advertisement");

    let body = tokio::task::spawn_blocking(move || advertise_refs(&repo_path, &service, protocol_version))
        .await
        .map_err(GitHttpError::internal)??;

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
    let repo = gix::open(repo_path).map_err(GitHttpError::internal)?;
    let mut out = service_announcement(service);
    match (service, protocol_version) {
        ("git-upload-pack", 2) => {
            let opts = gix::protocol::upload_pack::OptionsV2::default();
            repo.serve_upload_pack_info_refs_v2(&mut out, &opts)
                .map_err(GitHttpError::internal)?;
        }
        ("git-upload-pack", _) => {
            let opts = gix::protocol::upload_pack::Options::default();
            repo.serve_upload_pack_info_refs(&mut out, &opts)
                .map_err(GitHttpError::internal)?;
        }
        ("git-receive-pack", _) => {
            let opts = gix::protocol::receive_pack::advertisement::Options::default();
            repo.serve_receive_pack_info_refs(&mut out, &opts)
                .map_err(GitHttpError::internal)?;
        }
        _ => return Err(GitHttpError::NotFound("unknown git service")),
    }
    Ok(out)
}

async fn upload_pack(
    State(state): State<ServerState>,
    Path(group_id): Path<String>,
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
    enforce_write(&headers, uuid)?;
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

/// Dispatch to the appropriate gix serve entry point. Blocking; runs
/// on the `spawn_blocking` thread. Errors are returned as boxed `dyn`
/// because the upload-pack and receive-pack error enums are distinct
/// and the caller only logs.
fn run_serve<R, W>(
    kind: ServeKind,
    repo_path: &StdPath,
    reader: R,
    mut writer: W,
    interrupt: &AtomicBool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>>
where
    R: std::io::Read,
    W: std::io::Write,
{
    let repo = gix::open(repo_path)?;
    match kind {
        ServeKind::UploadPack { protocol_version: 2 } => {
            let _outcome = repo.serve_pack_upload_v2_dispatch_auto(reader, &mut writer, interrupt)?;
        }
        ServeKind::UploadPack { .. } => {
            let _outcome = repo.serve_pack_upload_v1_auto(reader, &mut writer, interrupt)?;
        }
        ServeKind::ReceivePack => {
            let mut progress = gix::progress::Discard;
            let _outcome = repo.serve_pack_receive(reader, &mut writer, &mut progress, interrupt)?;
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
/// Until the full auth stack lands the server requires a shared-secret
/// bearer token supplied via `MMCP_PUSH_TOKEN`. Missing or wrong token
/// returns 401/403. Real role-based enforcement replaces this once
/// there's a real authenticated user.
fn enforce_write(headers: &HeaderMap, _group_id: Uuid) -> Result<(), GitHttpError> {
    let expected = match std::env::var("MMCP_PUSH_TOKEN") {
        Ok(token) if !token.is_empty() => token,
        _ => return Err(GitHttpError::Forbidden("push disabled: set MMCP_PUSH_TOKEN")),
    };
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let presented = auth.strip_prefix("Bearer ").unwrap_or("");
    if presented != expected {
        return Err(GitHttpError::Unauthorized);
    }
    Ok(())
}

#[derive(Debug)]
enum GitHttpError {
    NotFound(&'static str),
    Internal(String),
    Unauthorized,
    Forbidden(&'static str),
}

impl GitHttpError {
    fn internal<E: std::fmt::Display>(err: E) -> Self {
        GitHttpError::Internal(err.to_string())
    }
}

impl IntoResponse for GitHttpError {
    fn into_response(self) -> Response {
        match self {
            GitHttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.to_string()).into_response(),
            GitHttpError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
            GitHttpError::Unauthorized => {
                // Emit `WWW-Authenticate` so stock git's HTTP auth flow
                // can respond with a Basic-auth challenge instead of
                // surfacing a bare 401. Without this header, git
                // clients treat the request as a hard failure rather
                // than retrying with credentials from the user's
                // credential helper.
                (
                    StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", r#"Basic realm="mmcp""#)],
                    "missing or invalid token".to_string(),
                )
                    .into_response()
            }
            GitHttpError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.to_string()).into_response(),
        }
    }
}
