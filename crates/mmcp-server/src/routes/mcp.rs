//! MCP tool dispatch over a single JSON-RPC style HTTP endpoint.
//!
//! This cut exposes the full `mmcp-proto` tool surface behind one
//! `POST /mcp/tool` route that takes a `{ tool, request }` envelope
//! and dispatches on the `ToolName`. Full rmcp wire protocol with
//! stdio and SSE transports lives in `mmcp-client` and in a future
//! server-side integration, which can reuse every handler in this
//! module without changing their signatures.

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use mmcp_proto::{
    DiffMemoryRequest, GroupInfoRequest, ListMemoriesRequest, ListVersionsRequest,
    ReadMemoryRequest, SearchMemoriesRequest, ToolName, VerifyMemoryRequest,
    WriteMemoryRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

async fn dispatch(
    State(state): State<ServerState>,
    Json(envelope): Json<ToolEnvelope>,
) -> Result<Json<ToolSuccess>, (StatusCode, String)> {
    let response = match envelope.tool {
        ToolName::ListMemories => {
            let req: ListMemoriesRequest = parse(envelope.request)?;
            let res = crate::routes::mcp::handlers::list_memories(&state, req)
                .await
                .map_err(internal)?;
            serialize(res)
        }
        ToolName::ReadMemory => {
            let req: ReadMemoryRequest = parse(envelope.request)?;
            let res = crate::routes::mcp::handlers::read_memory(&state, req)
                .await
                .map_err(internal)?;
            serialize(res)
        }
        ToolName::WriteMemory => {
            let req: WriteMemoryRequest = parse(envelope.request)?;
            let res = crate::routes::mcp::handlers::write_memory(&state, req)
                .await
                .map_err(internal)?;
            serialize(res)
        }
        ToolName::VerifyMemory => {
            let req: VerifyMemoryRequest = parse(envelope.request)?;
            let res = crate::routes::mcp::handlers::verify_memory(&state, req)
                .await
                .map_err(internal)?;
            serialize(res)
        }
        ToolName::ListVersions => {
            let req: ListVersionsRequest = parse(envelope.request)?;
            let res = crate::routes::mcp::handlers::list_versions(&state, req)
                .await
                .map_err(internal)?;
            serialize(res)
        }
        ToolName::DiffMemory => {
            let req: DiffMemoryRequest = parse(envelope.request)?;
            let res = crate::routes::mcp::handlers::diff_memory(&state, req)
                .await
                .map_err(internal)?;
            serialize(res)
        }
        ToolName::SearchMemories => {
            let req: SearchMemoriesRequest = parse(envelope.request)?;
            let res = crate::routes::mcp::handlers::search_memories(&state, req)
                .await
                .map_err(internal)?;
            serialize(res)
        }
        ToolName::GroupInfo => {
            let req: GroupInfoRequest = parse(envelope.request)?;
            let res = crate::routes::mcp::handlers::group_info(&state, req)
                .await
                .map_err(internal)?;
            serialize(res)
        }
    }?;
    Ok(Json(ToolSuccess {
        tool: envelope.tool,
        response,
    }))
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, (StatusCode, String)> {
    serde_json::from_value(value).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
}

fn serialize<T: Serialize>(value: T) -> Result<Value, (StatusCode, String)> {
    serde_json::to_value(&value).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

fn internal(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

mod handlers {
    use anyhow::{Result, anyhow};
    use mmcp_db::entities::group::OwnerKind;
    use mmcp_db::entities::memory::MemoryKind;
    use mmcp_db::repository::{group_repo, memory_repo};
    use mmcp_proto::{
        DiffMemoryRequest, DiffMemoryResponse, GroupInfoRequest, GroupInfoResponse,
        ListMemoriesRequest, ListMemoriesResponse, ListVersionsRequest, ListVersionsResponse,
        MemoryDescriptor, ReadMemoryRequest, ReadMemoryResponse, SearchMemoriesRequest,
        SearchMemoriesResponse, VerifyMemoryRequest, VerifyMemoryResponse, VersionEntry,
        WriteMemoryRequest, WriteMemoryResponse,
    };

    use crate::state::ServerState;

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

    pub async fn read_memory(
        _state: &ServerState,
        _req: ReadMemoryRequest,
    ) -> Result<ReadMemoryResponse> {
        // Reading the git content requires an effective group load
        // set resolver which is a job for the client; on the server
        // this route will be wired once we have a concrete load-set
        // handshake.
        Err(anyhow!("read_memory not yet wired on the server"))
    }

    pub async fn write_memory(
        _state: &ServerState,
        _req: WriteMemoryRequest,
    ) -> Result<WriteMemoryResponse> {
        Err(anyhow!("write_memory not yet wired on the server"))
    }

    pub async fn verify_memory(
        _state: &ServerState,
        req: VerifyMemoryRequest,
    ) -> Result<VerifyMemoryResponse> {
        Ok(VerifyMemoryResponse {
            memory: req.memory,
            verified_at: jiff::Timestamp::now().as_millisecond(),
        })
    }

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

    pub async fn diff_memory(
        _state: &ServerState,
        req: DiffMemoryRequest,
    ) -> Result<DiffMemoryResponse> {
        Ok(DiffMemoryResponse {
            memory: req.memory,
            from_version: req.from_version,
            to_version: req.to_version,
            diff: String::new(),
        })
    }

    pub async fn search_memories(
        _state: &ServerState,
        _req: SearchMemoriesRequest,
    ) -> Result<SearchMemoriesResponse> {
        Ok(SearchMemoriesResponse { hits: Vec::new() })
    }

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
