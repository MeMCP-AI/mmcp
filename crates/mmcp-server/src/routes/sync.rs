//! Control-plane `/sync/*` endpoints.
//!
//! These are the HTTP endpoints the `mmcp-sync` client talks to
//! when the user runs `mmcp sync`, `mmcp pull`, or `mmcp push`.
//! They register version bumps, advertise the caller's effective
//! group list, and expose the tip commit of each group's `main`
//! branch so a client can decide whether a fetch is needed.
//!
//! Every handler here requires a valid per-user bearer token, verified
//! by [`AuthenticatedUser`].
//!
//! The request and response shapes live in `mmcp_sync::client`,
//! so this module and the sync engine cannot drift.

use std::str::FromStr;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use futures_util::stream;
use jiff::Timestamp;
use mmcp_core::memory::BumpIntent;
use mmcp_db::entities::memory::MemoryKind;
use mmcp_db::entities::memory_version;
use mmcp_db::repository::{group_repo, memory_repo};
use mmcp_git::{GitBackend, RepoHandle};
use mmcp_proto::ProtoError;
use mmcp_sync::{ManifestResponse, PushRequest, PushResponse, RefEntry, RefsResponse, RemoteGroup};
use sea_orm::EntityTrait;
use serde_json::json;
use uuid::Uuid;

use crate::routes::bearer_auth::AuthenticatedUser;
use crate::routes::defaults::MAX_CONCURRENT_MANIFEST_LOOKUPS;
use crate::routes::response::{self, FromInternalError, into_generic_response};
use crate::state::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/sync/manifest", get(get_manifest))
        .route("/sync/refs/{group_id}", get(get_refs))
        .route("/sync/push", post(post_push))
}

/// Error response for `/sync/*` routes, mirroring the `mcp.rs`
/// structure: an `IntoResponse` that serializes a `ProtoError`
/// value with the appropriate HTTP status code.
struct SyncErrorResponse {
    status: StatusCode,
    error: ProtoError,
}

impl IntoResponse for SyncErrorResponse {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.error }))).into_response()
    }
}

impl FromInternalError for SyncErrorResponse {
    fn from_internal_error() -> Self {
        SyncErrorResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: ProtoError::Internal(response::GENERIC_INTERNAL_ERROR_MESSAGE.to_string()),
        }
    }
}

fn not_found(kind: &str) -> SyncErrorResponse {
    SyncErrorResponse {
        status: StatusCode::NOT_FOUND,
        error: ProtoError::NotFound(kind.to_string()),
    }
}

fn invalid_request(msg: impl Into<String>) -> SyncErrorResponse {
    SyncErrorResponse {
        status: StatusCode::BAD_REQUEST,
        error: ProtoError::InvalidRequest(msg.into()),
    }
}

/// Return the caller's effective group manifest.
///
/// Today this is every group the server knows about, once the caller
/// is authenticated. A later phase filters this list by the
/// authenticated caller's effective role using
/// `mmcp_core::acl::resolve_effective_role`. The request/response
/// shape stays the same, so the upgrade is a behavior-only
/// change.
async fn get_manifest(
    State(state): State<ServerState>,
    _caller: AuthenticatedUser,
) -> Result<Json<ManifestResponse>, SyncErrorResponse> {
    let conn = state.database.connection();
    let rows = mmcp_db::entities::group::Entity::find()
        .all(conn)
        .await
        .map_err(into_generic_response)?;

    // Bounded concurrency instead of one row's read_manifest +
    // walk_history after another: each row's git reads are
    // independent. `buffer_unordered` completes rows out of
    // submission order, so each result carries its original index and
    // the collected vector is sorted back into `rows` order below.
    let mut results: Vec<(usize, RemoteGroup)> = stream::iter(rows.into_iter().enumerate())
        .map(|(index, row)| {
            let state = state.clone();
            async move {
                let group = manifest_row_to_remote_group(&state, row).await;
                (index, group)
            }
        })
        .buffer_unordered(MAX_CONCURRENT_MANIFEST_LOOKUPS)
        .collect()
        .await;
    results.sort_by_key(|(index, _)| *index);
    let groups = results.into_iter().map(|(_, group)| group).collect();

    Ok(Json(ManifestResponse { groups }))
}

/// Resolve one group row's tip commit and build its [`RemoteGroup`]
/// advertisement. A missing manifest or history read is not an
/// error at this layer: the group is still advertised, just at the
/// conventional zero commit.
async fn manifest_row_to_remote_group(
    state: &ServerState,
    row: mmcp_db::entities::group::Model,
) -> RemoteGroup {
    let handle = RepoHandle::new(
        row.id,
        state.group_repo_path(row.id).to_string_lossy().into_owned(),
    );
    let head_commit = match state.git.read_manifest(&handle).await {
        Ok(_manifest) => {
            // We have a manifest, so there's a main branch. Resolve
            // its commit through `list_tree` on an empty prefix: gix
            // returns the root-tree listing, which implies we can
            // walk history. But for the manifest endpoint callers
            // only need the tip commit; grab it via walk_history.
            match state.git.walk_history(&handle, ".mmcp.toml").await {
                Ok(mut history) => history
                    .drain(..)
                    .next()
                    .map(|c| c.id)
                    .unwrap_or_else(|| mmcp_core::conventions::ZERO_COMMIT.to_string()),
                Err(err) => {
                    tracing::warn!(
                        group_id = %row.id,
                        group_slug = %row.slug,
                        error = %err,
                        "walk_history failed while resolving the manifest tip commit; \
                         advertising the group at the zero commit"
                    );
                    mmcp_core::conventions::ZERO_COMMIT.to_string()
                }
            }
        }
        Err(err) => {
            tracing::warn!(
                group_id = %row.id,
                group_slug = %row.slug,
                error = %err,
                "read_manifest failed while building the sync manifest; advertising the \
                 group at the zero commit"
            );
            mmcp_core::conventions::ZERO_COMMIT.to_string()
        }
    };
    RemoteGroup {
        group_id: row.id,
        slug: row.slug,
        head_commit,
    }
}

/// Return the advertised refs for a single group.
async fn get_refs(
    State(state): State<ServerState>,
    _caller: AuthenticatedUser,
    Path(group_id): Path<String>,
) -> Result<Json<RefsResponse>, SyncErrorResponse> {
    let group_uuid =
        Uuid::from_str(&group_id).map_err(|_| invalid_request("group_id is not a UUID"))?;
    let conn = state.database.connection();
    let _group = group_repo::find_by_id(conn, group_uuid)
        .await
        .map_err(into_generic_response)?
        .ok_or_else(|| not_found("group"))?;

    let handle = RepoHandle::new(
        group_uuid,
        state
            .group_repo_path(group_uuid)
            .to_string_lossy()
            .into_owned(),
    );
    let main_tip = state
        .git
        .walk_history(&handle, ".mmcp.toml")
        .await
        .map_err(into_generic_response)?
        .into_iter()
        .next()
        .map(|c| c.id);

    let mut refs = Vec::new();
    if let Some(tip) = main_tip {
        refs.push(RefEntry {
            name: mmcp_core::conventions::MAIN_BRANCH_REF.to_string(),
            commit: tip,
        });
    }

    Ok(Json(RefsResponse {
        group_id: group_uuid,
        refs,
    }))
}

/// Register a version bump for a local edit.
///
/// The request body is exactly the shape the sync engine sends
/// via `SyncClient::push_version`. The handler:
///
/// 1. Validates that the group and memory exist.
/// 2. Reads the memory's current `latest_version` from the
///    database and computes the next version via
///    `mmcp_sync::negotiate_next_version`.
/// 3. Writes a new `memory_versions` row, updates the memory's
///    `latest_version` pointer, and creates a lightweight tag
///    in the group's bare repository pointing at the reported
///    commit hash.
/// 4. Returns the assigned version string and the tag name.
async fn post_push(
    State(state): State<ServerState>,
    caller: AuthenticatedUser,
    Json(req): Json<PushRequest>,
) -> Result<Json<PushResponse>, SyncErrorResponse> {
    let conn = state.database.connection();
    let group = group_repo::find_by_id(conn, req.group_id)
        .await
        .map_err(into_generic_response)?
        .ok_or_else(|| not_found("group"))?;

    // Look up (or create) the memory row. For a first publish of
    // a brand-new memory, the server autocreates the row so the
    // client can push without a preceding "create memory" call.
    let memory = match memory_repo::find_by_id(conn, req.memory_id)
        .await
        .map_err(into_generic_response)?
    {
        Some(m) => m,
        None => {
            let now = Timestamp::now().as_millisecond();
            memory_repo::create(
                conn,
                memory_repo::NewMemory {
                    id: req.memory_id,
                    group_id: group.id,
                    slug: req.memory_id.to_string(),
                    kind: MemoryKind::Rule,
                    mandatory: false,
                    created_at: now,
                    updated_at: now,
                },
            )
            .await
            .map_err(into_generic_response)?
        }
    };

    let bump = parse_bump(&req.bump);
    let next_version = mmcp_sync::negotiate_next_version(memory.latest_version.as_deref(), bump)
        .map_err(into_generic_response)?;
    let version_str = next_version.to_string();

    let now = Timestamp::now().as_millisecond();
    let version_row = memory_version::Model {
        id: Uuid::now_v7(),
        memory_id: memory.id,
        version: version_str.clone(),
        commit: req.commit.clone(),
        author_id: caller.user_id,
        published_at: now,
        summary: req.message.clone(),
    };
    memory_repo::record_version(conn, version_row)
        .await
        .map_err(into_generic_response)?;
    memory_repo::set_latest_version(conn, memory.id, version_str.clone(), now)
        .await
        .map_err(into_generic_response)?;

    let tag_name = format!("v{version_str}");
    let handle = RepoHandle::new(
        group.id,
        state
            .group_repo_path(group.id)
            .to_string_lossy()
            .into_owned(),
    );
    if let Err(err) = state.git.tag(&handle, &tag_name, &req.commit).await {
        // Tag creation against a commit the server does not yet
        // hold is expected until the git content plane lands.
        // Log it and keep going so the version row is still
        // recorded and the client sees its assigned version.
        tracing::warn!(
            group = %group.id,
            memory = %memory.id,
            commit = %req.commit,
            error = %err,
            "sync push: tag creation deferred until content plane",
        );
    }

    Ok(Json(PushResponse {
        group_id: group.id,
        memory_id: memory.id,
        assigned_version: version_str,
        tag: tag_name,
    }))
}

fn parse_bump(value: &BumpIntent) -> BumpIntent {
    *value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_support::{WarnCounter, minimal_server_config};
    use mmcp_db::entities::group::OwnerKind;
    use tempfile::TempDir;

    /// Minimal `ServerState` for exercising
    /// `manifest_row_to_remote_group` directly, without the
    /// `tests/common` integration-test builder (a separate crate,
    /// unreachable from this in-crate unit test).
    async fn test_state() -> (ServerState, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let cfg = minimal_server_config(tmp.path().to_path_buf());
        let state = ServerState::initialize(&cfg).await.expect("state init");
        (state, tmp)
    }

    /// A group row with no backing bare repo must still be advertised
    /// at the conventional zero commit (unchanged behavior), but the
    /// `read_manifest` failure that led there must now log a warning
    /// naming the group instead of vanishing silently.
    #[tokio::test]
    async fn manifest_row_to_remote_group_warns_when_read_manifest_fails() {
        let (state, _tmp) = test_state().await;
        let group_id = Uuid::now_v7();
        let owner = Uuid::now_v7();
        group_repo::create(
            state.database.connection(),
            group_repo::NewGroup {
                id: group_id,
                slug: "no-repo".to_string(),
                owner_kind: OwnerKind::User,
                owner_id: owner,
                display_name: None,
                created_at: 0,
            },
        )
        .await
        .expect("insert group row");
        let row = group_repo::find_by_id(state.database.connection(), group_id)
            .await
            .expect("query group row")
            .expect("row was just inserted");

        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let subscriber = WarnCounter(count.clone());
        // `set_default` (rather than `with_default`, which only
        // accepts a synchronous closure) so the guard stays active
        // across the `.await` below; the default `#[tokio::test]`
        // current-thread runtime keeps this task on the one thread
        // the guard's thread-local applies to.
        let guard = tracing::subscriber::set_default(subscriber);
        let group = manifest_row_to_remote_group(&state, row).await;
        drop(guard);

        assert_eq!(
            group.head_commit,
            mmcp_core::conventions::ZERO_COMMIT,
            "a group with no backing repo must still advertise at the zero commit"
        );
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the read_manifest failure must log exactly one warning instead of vanishing silently"
        );
    }
}
