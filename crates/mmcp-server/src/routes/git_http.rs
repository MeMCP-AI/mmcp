//! Native git smart HTTP responder.
//!
//! Implements the subset of the git smart HTTP v1/v2 protocol
//! clients need to `clone`, `fetch`, and `push` against the
//! server's bare repositories. The heavy lifting is delegated
//! to the user's installed `git http-backend` binary, which is
//! the CGI-style handler Git itself ships for hosting bare
//! repositories over HTTP. Running it as a subprocess is the
//! same approach Gitea, Gitolite, and cgit use.
//!
//! The route surface:
//!
//! - `GET  /git/:group_id.git/info/refs?service=git-upload-pack`
//! - `GET  /git/:group_id.git/info/refs?service=git-receive-pack`
//! - `POST /git/:group_id.git/git-upload-pack`
//! - `POST /git/:group_id.git/git-receive-pack`
//!
//! ACL enforcement runs before any subprocess is spawned. The
//! unauthenticated baseline in this phase allows reads for any
//! existing group and rejects writes. When OAuth and passkeys
//! land, the `AuthUser` extractor runs first and feeds the ACL
//! resolver with a real user id.

use std::path::PathBuf;
use std::process::Stdio;
use std::str::FromStr;

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
use tokio_util::io::{ReaderStream, StreamReader};
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
async fn ensure_group(
    state: &ServerState,
    group_id: Uuid,
) -> Result<PathBuf, GitHttpError> {
    let conn = state.database.connection();
    let row = group_repo::find_by_id(conn, group_id)
        .await
        .map_err(GitHttpError::internal)?
        .ok_or(GitHttpError::NotFound("group not found"))?;
    let _ = row;
    Ok(state.group_repo_path(group_id))
}

async fn info_refs(
    State(state): State<ServerState>,
    Path(group_id): Path<String>,
    Query(query): Query<InfoRefsQuery>,
    headers: HeaderMap,
) -> Result<Response, GitHttpError> {
    let uuid = parse_group_path(&group_id)?;
    let repo_path = ensure_group(&state, uuid).await?;
    // For now allow reads unconditionally; writes still require
    // authentication once Phase 7 lands.
    if query.service == "git-receive-pack" {
        enforce_write(&headers, uuid)?;
    }

    let output_type = format!("application/x-{}-advertisement", query.service);
    let mut body = service_announcement(&query.service);

    let mut cmd = tokio::process::Command::new("git");
    cmd.arg(query.service.trim_start_matches("git-"))
        .arg("--stateless-rpc")
        .arg("--advertise-refs")
        .arg(&repo_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_git_protocol_env(&mut cmd, &headers);
    let output = cmd.output().await.map_err(GitHttpError::internal)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(GitHttpError::Internal(format!(
            "git {service} failed: {stderr}",
            service = query.service
        )));
    }
    body.extend_from_slice(&output.stdout);

    Ok((
        StatusCode::OK,
        [
            ("Content-Type", output_type.as_str()),
            ("Cache-Control", "no-cache"),
        ],
        body,
    )
        .into_response())
}

async fn upload_pack(
    State(state): State<ServerState>,
    Path(group_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GitHttpError> {
    let uuid = parse_group_path(&group_id)?;
    let repo_path = ensure_group(&state, uuid).await?;
    stream_pack_command_guarded(
        "upload-pack",
        &repo_path,
        body,
        "application/x-git-upload-pack-result",
        None,
        &headers,
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
    // Serialize writes per-group so two concurrent pushes cannot
    // race `git receive-pack` and leave refs in an inconsistent state.
    // Reads (`upload-pack`) stay unserialized — they only observe the
    // on-disk tree and tolerate concurrent writers safely.
    let write_lock = state.repo_write_lock(uuid);
    let guard = write_lock.lock_owned().await;
    stream_pack_command_guarded(
        "receive-pack",
        &repo_path,
        body,
        "application/x-git-receive-pack-result",
        Some(guard),
        &headers,
    )
    .await
}

/// Run `git <subcommand> --stateless-rpc <repo>` with the request
/// body streamed into stdin and the subprocess's stdout streamed
/// back as the HTTP response body.
///
/// Memory footprint stays bounded regardless of pack size: bytes
/// flow client → axum → child stdin in one direction and child
/// stdout → axum → client in the other, with a small ring buffer
/// at each hop.
///
/// An optional `write_guard` is held alongside the child process so
/// the lock stays acquired for the whole streaming lifetime (used by
/// `receive-pack` to serialize concurrent writers per group).
///
/// The request `headers` are inspected for `Git-Protocol` so the
/// subprocess negotiates the same protocol version the client asked
/// for (v0/v1/v2). Without this forwarding, v2-capable clients
/// silently downgrade to v0.
async fn stream_pack_command_guarded(
    subcommand: &'static str,
    repo_path: &std::path::Path,
    body: Body,
    content_type: &'static str,
    write_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    headers: &HeaderMap,
) -> Result<Response, GitHttpError> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg(subcommand)
        .arg("--stateless-rpc")
        .arg(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_git_protocol_env(&mut cmd, headers);
    let mut child = cmd.spawn().map_err(GitHttpError::internal)?;

    let mut stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    // Pump request body into git's stdin. Stream end triggers EOF
    // on stdin via drop, which is how git knows the push/fetch
    // request is complete.
    tokio::spawn(async move {
        let body_stream = body.into_data_stream().map_err(std::io::Error::other);
        let mut body_reader = StreamReader::new(body_stream);
        let _ = tokio::io::copy(&mut body_reader, &mut stdin).await;
        // stdin drops here → EOF to git.
    });

    // Capture stderr into a log so subprocess failures are visible
    // when the streamed stdout response gets truncated.
    tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut stderr = stderr;
        if tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf)
            .await
            .is_ok()
            && !buf.is_empty()
        {
            let text = String::from_utf8_lossy(&buf).into_owned();
            tracing::warn!(target: "mmcp_server::git_http", subcommand, stderr = %text, "git subprocess produced stderr");
        }
    });

    // Reap the child once stdout closes so it doesn't linger as a
    // zombie. The write-lock guard is moved into the waiting task
    // so the lock releases only after the subprocess exits — not
    // when the response starts streaming.
    tokio::spawn(async move {
        let _ = child.wait().await;
        drop(write_guard);
    });

    let stdout_stream = ReaderStream::new(stdout);
    let response_body = Body::from_stream(stdout_stream);

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

/// Forward the client's `Git-Protocol` header (if any) to the
/// subprocess via the `GIT_PROTOCOL` environment variable. Git's
/// own smart-HTTP CGI does the same thing. Without this, clients
/// that negotiate protocol v2 silently get v0 replies, losing
/// ref filtering and partial-clone optimizations on large repos.
fn apply_git_protocol_env(cmd: &mut tokio::process::Command, headers: &HeaderMap) {
    if let Some(value) = headers.get("git-protocol")
        && let Ok(raw) = value.to_str()
        && !raw.is_empty()
    {
        cmd.env("GIT_PROTOCOL", raw);
    }
}

/// pkt-line formatted service advertisement prefix, required by
/// the smart HTTP v1 protocol before the actual `git --advertise-refs`
/// output starts.
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
/// Until Phase 7 wires OAuth and passkeys, the server requires a
/// shared-secret bearer token supplied via `MMCP_PUSH_TOKEN` and
/// reads it from the `Authorization` header. Missing or wrong
/// token returns 401/403. The real role-based enforcement will
/// replace this once there's a real authenticated user.
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
            GitHttpError::Internal(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response()
            }
            GitHttpError::Unauthorized => {
                (StatusCode::UNAUTHORIZED, "missing or invalid token".to_string()).into_response()
            }
            GitHttpError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.to_string()).into_response(),
        }
    }
}
