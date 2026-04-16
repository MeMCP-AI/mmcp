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
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mmcp_db::repository::group_repo;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::state::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/git/:group_id/info/refs", get(info_refs))
        .route("/git/:group_id/git-upload-pack", post(upload_pack))
        .route("/git/:group_id/git-receive-pack", post(receive_pack))
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
    body: Bytes,
) -> Result<Response, GitHttpError> {
    let uuid = parse_group_path(&group_id)?;
    let repo_path = ensure_group(&state, uuid).await?;
    run_pack_command("upload-pack", &repo_path, body.to_vec(), "application/x-git-upload-pack-result").await
}

async fn receive_pack(
    State(state): State<ServerState>,
    Path(group_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, GitHttpError> {
    let uuid = parse_group_path(&group_id)?;
    enforce_write(&headers, uuid)?;
    let repo_path = ensure_group(&state, uuid).await?;
    run_pack_command("receive-pack", &repo_path, body.to_vec(), "application/x-git-receive-pack-result").await
}

/// Run `git <subcommand> --stateless-rpc <repo>` with the request
/// body piped into stdin, and stream the stdout back as the HTTP
/// response body.
async fn run_pack_command(
    subcommand: &str,
    repo_path: &std::path::Path,
    body: Vec<u8>,
    content_type: &'static str,
) -> Result<Response, GitHttpError> {
    let mut child = tokio::process::Command::new("git")
        .arg(subcommand)
        .arg("--stateless-rpc")
        .arg(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(GitHttpError::internal)?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&body).await.map_err(GitHttpError::internal)?;
        stdin.shutdown().await.map_err(GitHttpError::internal)?;
    }

    let output = child.wait_with_output().await.map_err(GitHttpError::internal)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(GitHttpError::Internal(format!(
            "git {subcommand} failed: {stderr}"
        )));
    }

    Ok((
        StatusCode::OK,
        [
            ("Content-Type", content_type),
            ("Cache-Control", "no-cache"),
        ],
        output.stdout,
    )
        .into_response())
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
