//! MCP tool dispatch over a single JSON-RPC style HTTP endpoint.
//!
//! This cut exposes the `mmcp-proto` tool surface that the server
//! can actually answer today behind one `POST /mcp/tool` route
//! that takes a `{ tool, request }` envelope and dispatches on
//! the `ToolName`. Tools whose control plane lives in the client
//! (read, write, verify, diff, search) return a structured
//! `ProtoError::NotImplemented` so clients see a real capability
//! gap rather than a fake success.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use mmcp_proto::{
    GroupInfoRequest, ListMemoriesRequest, ListVersionsRequest, ProtoError, ToolName,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::routes::response::{self, FromInternalError, into_generic_response};
use crate::state::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new().route("/mcp/tool", post(dispatch))
}

#[derive(Deserialize)]
struct ToolEnvelope {
    tool: ToolName,
    request: Value,
}

#[derive(Serialize)]
struct ToolSuccess {
    tool: ToolName,
    response: Value,
}

/// Dispatch a `{ tool, request }` envelope.
///
/// Successful tool calls return HTTP 200 with the tool name and
/// serialized response payload. Protocol-level errors (validation
/// failures, unimplemented tools, internal failures) surface as
/// `ProtoError` values serialized into HTTP responses with the
/// conventional status code: 400 for request validation, 501 for
/// unimplemented, 500 for internal failures.
async fn dispatch(
    State(state): State<ServerState>,
    Json(envelope): Json<ToolEnvelope>,
) -> Result<Json<ToolSuccess>, ToolErrorResponse> {
    let tool = envelope.tool;
    let response = match tool {
        ToolName::ListMemories => {
            let req: ListMemoriesRequest = parse_request(envelope.request)?;
            let res = handlers::list_memories(&state, req)
                .await
                .map_err(into_generic_response)?;
            serialize_response(&res)?
        }
        ToolName::ListVersions => {
            let req: ListVersionsRequest = parse_request(envelope.request)?;
            let res = handlers::list_versions(&state, req)
                .await
                .map_err(into_generic_response)?;
            serialize_response(&res)?
        }
        ToolName::GroupInfo => {
            let req: GroupInfoRequest = parse_request(envelope.request)?;
            let res = handlers::group_info(&state, req)
                .await
                .map_err(into_generic_response)?;
            serialize_response(&res)?
        }
        ToolName::ReadMemory
        | ToolName::WriteMemory
        | ToolName::VerifyMemory
        | ToolName::DiffMemory
        | ToolName::SearchMemories => {
            return Err(ToolErrorResponse {
                status: StatusCode::NOT_IMPLEMENTED,
                error: ProtoError::NotImplemented(format!(
                    "{name} is served by the client against local git state, not by the server",
                    name = tool.as_str()
                )),
            });
        }
    };
    Ok(Json(ToolSuccess { tool, response }))
}

/// Envelope for an error response from `dispatch`.
///
/// Serializes as `{ "tool": ..., "error": { "kind": ..., "message": ... } }`
/// so clients get a structured payload regardless of HTTP status.
pub(crate) struct ToolErrorResponse {
    status: StatusCode,
    error: ProtoError,
}

impl IntoResponse for ToolErrorResponse {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.error });
        (self.status, Json(body)).into_response()
    }
}

fn parse_request<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ToolErrorResponse> {
    serde_json::from_value(value).map_err(|e| ToolErrorResponse {
        status: StatusCode::BAD_REQUEST,
        error: ProtoError::InvalidRequest(e.to_string()),
    })
}

fn serialize_response<T: Serialize>(value: &T) -> Result<Value, ToolErrorResponse> {
    serde_json::to_value(value).map_err(|e| ToolErrorResponse {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        error: ProtoError::Internal(format!("response serialization failed: {e}")),
    })
}

impl FromInternalError for ToolErrorResponse {
    fn from_internal_error() -> Self {
        ToolErrorResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: ProtoError::Internal(response::GENERIC_INTERNAL_ERROR_MESSAGE.to_string()),
        }
    }
}

mod handlers {
    use anyhow::{Result, anyhow};
    use mmcp_db::entities::group::OwnerKind;
    use mmcp_db::entities::memory::MemoryKind;
    use mmcp_db::repository::{group_repo, memory_repo};
    use mmcp_proto::{
        GroupInfoRequest, GroupInfoResponse, ListMemoriesRequest, ListMemoriesResponse,
        ListVersionsRequest, ListVersionsResponse, MemoryDescriptor, VersionEntry,
    };

    use crate::state::ServerState;

    /// Return a metadata listing of every memory in the given group,
    /// filtered by optional kind and mandatory toggles.
    pub async fn list_memories(
        state: &ServerState,
        req: ListMemoriesRequest,
    ) -> Result<ListMemoriesResponse> {
        let conn = state.database.connection();
        let memories = match req.group {
            Some(group) => memory_repo::list_in_group(conn, group).await?,
            None => Vec::new(),
        };
        let filtered: Vec<_> = memories
            .into_iter()
            .filter(|m| {
                req.only_mandatory.is_none_or(|flag| flag == m.mandatory)
                    && (req.kinds.is_empty() || req.kinds.contains(&kind_to_string(m.kind)))
            })
            .map(to_descriptor)
            .collect();
        Ok(ListMemoriesResponse { memories: filtered })
    }

    /// Return every published version row recorded for the given
    /// memory, oldest first. The caller is responsible for sorting
    /// into presentation order.
    pub async fn list_versions(
        state: &ServerState,
        req: ListVersionsRequest,
    ) -> Result<ListVersionsResponse> {
        let conn = state.database.connection();
        let rows = memory_repo::list_versions(conn, req.memory).await?;
        let versions = rows
            .into_iter()
            .map(|r| VersionEntry {
                version: r.version,
                commit: r.commit,
                author: r.author_id.to_string(),
                published_at: r.published_at,
                summary: r.summary,
            })
            .collect();
        Ok(ListVersionsResponse {
            memory: req.memory,
            versions,
        })
    }

    /// Return metadata about a group as held in the server's
    /// control plane: slug, owner, display name, and the memory
    /// count computed from the index table. `effective_role` is
    /// populated once the ACL handshake lands in a later phase.
    pub async fn group_info(
        state: &ServerState,
        req: GroupInfoRequest,
    ) -> Result<GroupInfoResponse> {
        let conn = state.database.connection();
        let group = group_repo::find_by_id(conn, req.group)
            .await?
            .ok_or_else(|| anyhow!("group not found"))?;
        let memories = memory_repo::list_in_group(conn, req.group).await?;
        let owner = match group.owner_kind {
            OwnerKind::User => format!("user:{}", group.owner_id),
            OwnerKind::Org => format!("org:{}", group.owner_id),
        };
        Ok(GroupInfoResponse {
            id: group.id,
            slug: group.slug,
            owner,
            display_name: group.display_name,
            memory_count: memories.len() as u32,
            effective_role: "read".to_string(),
        })
    }

    fn kind_to_string(kind: MemoryKind) -> String {
        match kind {
            MemoryKind::Rule => "rule",
            MemoryKind::Snapshot => "snapshot",
            MemoryKind::Log => "log",
            MemoryKind::Reference => "reference",
            MemoryKind::Scratch => "scratch",
        }
        .to_string()
    }

    fn to_descriptor(m: mmcp_db::entities::memory::Model) -> MemoryDescriptor {
        MemoryDescriptor {
            id: m.id,
            group: m.group_id,
            slug: m.slug.clone(),
            name: m.slug.clone(),
            description: String::new(),
            kind: kind_to_string(m.kind),
            mandatory: m.mandatory,
            latest_version: m.latest_version,
        }
    }
}
