//! Integration tests for the `/mcp/tool` HTTP dispatch route.
//!
//! `routes::mcp` was at 3% line coverage in the baseline — the
//! three server-backed tools (`list_memories`, `list_versions`,
//! `group_info`), the not-implemented gate for client-side tools
//! (`read_memory` / `write_memory` / `verify_memory` / `diff_memory`
//! / `search_memories`), and the validation error path for
//! malformed request envelopes were all unverified.

use std::net::SocketAddr;

use mmcp_db::entities::group::OwnerKind;
use mmcp_db::entities::memory::MemoryKind as DbMemoryKind;
use mmcp_db::entities::memory_version;
use mmcp_db::repository::{group_repo, memory_repo, user_repo};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

/// Bring up the full server on an ephemeral port, backed by in-
/// memory sqlite and a tempdir repo root.
async fn start_server() -> (SocketAddr, mmcp_server::state::ServerState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = mmcp_server::config::ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: "sqlite::memory:".to_string(),
        repo_root: tmp.path().to_path_buf(),
        token_key: [0u8; 32],
        oauth_providers: vec![],
        origin: "http://localhost:8787".to_string(),
    };
    let state = mmcp_server::state::ServerState::initialize(&cfg)
        .await
        .expect("state init");
    let app = mmcp_server::app::build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });
    (addr, state, tmp)
}

/// Insert a group row plus a memory row for it. Returns `(group_id,
/// memory_id)`. Does **not** touch disk beyond the tempdir-backed
/// server state.
async fn seed_group_with_memory(
    state: &mmcp_server::state::ServerState,
    slug: &str,
    memory_slug: &str,
) -> (Uuid, Uuid) {
    let group = Uuid::now_v7();
    let owner = Uuid::now_v7();
    group_repo::create(
        state.database.connection(),
        group_repo::NewGroup {
            id: group,
            slug: slug.into(),
            owner_kind: OwnerKind::User,
            owner_id: owner,
            display_name: Some("Team".into()),
            created_at: 1_700_000_000,
        },
    )
    .await
    .expect("insert group");
    let memory = Uuid::now_v7();
    memory_repo::create(
        state.database.connection(),
        memory_repo::NewMemory {
            id: memory,
            group_id: group,
            slug: memory_slug.into(),
            kind: DbMemoryKind::Rule,
            mandatory: true,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        },
    )
    .await
    .expect("insert memory");
    (group, memory)
}

fn envelope(tool: &str, request: serde_json::Value) -> serde_json::Value {
    json!({ "tool": tool, "request": request })
}

async fn post_tool(addr: SocketAddr, body: serde_json::Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/mcp/tool"))
        .json(&body)
        .send()
        .await
        .expect("POST /mcp/tool")
}

#[tokio::test]
async fn mcp_tool_list_memories_with_no_group_returns_empty_list() {
    let (addr, _state, _tmp) = start_server().await;
    let resp = post_tool(addr, envelope("list_memories", json!({}))).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    let memories = body
        .pointer("/response/memories")
        .and_then(|v| v.as_array())
        .expect("memories array");
    assert!(memories.is_empty());
}

#[tokio::test]
async fn mcp_tool_list_memories_returns_group_contents() {
    let (addr, state, _tmp) = start_server().await;
    let (group, memory) = seed_group_with_memory(&state, "team-rust", "rules").await;
    let resp = post_tool(
        addr,
        envelope("list_memories", json!({ "group": group.to_string() })),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(
        body.get("tool").and_then(|v| v.as_str()),
        Some("list_memories")
    );
    let memories = body
        .pointer("/response/memories")
        .and_then(|v| v.as_array())
        .expect("memories array");
    assert_eq!(memories.len(), 1);
    let descriptor = &memories[0];
    assert_eq!(
        descriptor.get("id").and_then(|v| v.as_str()),
        Some(memory.to_string().as_str())
    );
    assert_eq!(descriptor.get("slug").and_then(|v| v.as_str()), Some("rules"));
    assert_eq!(descriptor.get("kind").and_then(|v| v.as_str()), Some("rule"));
    assert_eq!(descriptor.get("mandatory").and_then(|v| v.as_bool()), Some(true));
}

#[tokio::test]
async fn mcp_tool_list_memories_honors_only_mandatory_filter() {
    let (addr, state, _tmp) = start_server().await;
    let (group, _mandatory) = seed_group_with_memory(&state, "team-rust", "rules").await;

    // Seed a second, non-mandatory memory in the same group.
    let non_mandatory = Uuid::now_v7();
    memory_repo::create(
        state.database.connection(),
        memory_repo::NewMemory {
            id: non_mandatory,
            group_id: group,
            slug: "notes".into(),
            kind: DbMemoryKind::Reference,
            mandatory: false,
            created_at: 1,
            updated_at: 1,
        },
    )
    .await
    .unwrap();

    let resp = post_tool(
        addr,
        envelope(
            "list_memories",
            json!({ "group": group.to_string(), "only_mandatory": true }),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let memories = body
        .pointer("/response/memories")
        .and_then(|v| v.as_array())
        .expect("memories array");
    assert_eq!(memories.len(), 1);
    assert_eq!(
        memories[0].get("slug").and_then(|v| v.as_str()),
        Some("rules")
    );
}

#[tokio::test]
async fn mcp_tool_list_versions_returns_recorded_versions() {
    let (addr, state, _tmp) = start_server().await;
    let (_group, memory) = seed_group_with_memory(&state, "team-rust", "rules").await;

    // author_id foreign-keys onto users — seed a user row whose id
    // we can use as the version author.
    let author = Uuid::now_v7();
    user_repo::create(
        state.database.connection(),
        user_repo::NewUser {
            id: author,
            handle: "alice".into(),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();

    memory_repo::record_version(
        state.database.connection(),
        memory_version::Model {
            id: Uuid::now_v7(),
            memory_id: memory,
            version: "0.1.0".into(),
            commit: "abc123".into(),
            author_id: author,
            published_at: 10,
            summary: Some("first publish".into()),
        },
    )
    .await
    .unwrap();

    let resp = post_tool(
        addr,
        envelope("list_versions", json!({ "memory": memory.to_string() })),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let versions = body
        .pointer("/response/versions")
        .and_then(|v| v.as_array())
        .expect("versions array");
    assert_eq!(versions.len(), 1);
    assert_eq!(
        versions[0].get("version").and_then(|v| v.as_str()),
        Some("0.1.0")
    );
    assert_eq!(
        versions[0].get("commit").and_then(|v| v.as_str()),
        Some("abc123")
    );
    assert_eq!(
        versions[0].get("summary").and_then(|v| v.as_str()),
        Some("first publish")
    );
}

#[tokio::test]
async fn mcp_tool_group_info_returns_metadata_for_known_group() {
    let (addr, state, _tmp) = start_server().await;
    let (group, _memory) = seed_group_with_memory(&state, "team-rust", "rules").await;
    let resp = post_tool(
        addr,
        envelope("group_info", json!({ "group": group.to_string() })),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let response = body.get("response").expect("response field");
    assert_eq!(
        response.get("id").and_then(|v| v.as_str()),
        Some(group.to_string().as_str())
    );
    assert_eq!(response.get("slug").and_then(|v| v.as_str()), Some("team-rust"));
    assert_eq!(
        response.get("display_name").and_then(|v| v.as_str()),
        Some("Team")
    );
    assert_eq!(response.get("memory_count").and_then(|v| v.as_u64()), Some(1));
    assert!(
        response
            .get("owner")
            .and_then(|v| v.as_str())
            .map(|s| s.starts_with("user:"))
            .unwrap_or(false)
    );
    assert_eq!(
        response.get("effective_role").and_then(|v| v.as_str()),
        Some("read")
    );
}

#[tokio::test]
async fn mcp_tool_group_info_returns_500_for_unknown_group() {
    let (addr, _state, _tmp) = start_server().await;
    let resp = post_tool(
        addr,
        envelope("group_info", json!({ "group": Uuid::now_v7().to_string() })),
    )
    .await;
    assert_eq!(resp.status(), 500);
    let body: serde_json::Value = resp.json().await.unwrap();
    let kind = body.pointer("/error/kind").and_then(|v| v.as_str());
    assert_eq!(kind, Some("internal"));
}

#[tokio::test]
async fn mcp_tool_client_side_tools_return_501_not_implemented() {
    let (addr, _state, _tmp) = start_server().await;
    // Every tool whose control plane lives on the client must
    // surface a structured 501 rather than a silent 200.
    for tool in [
        "read_memory",
        "write_memory",
        "verify_memory",
        "diff_memory",
        "search_memories",
    ] {
        let resp = post_tool(addr, envelope(tool, json!({}))).await;
        assert_eq!(resp.status(), 501, "{tool} should be 501 NotImplemented");
        let body: serde_json::Value = resp.json().await.unwrap();
        let kind = body.pointer("/error/kind").and_then(|v| v.as_str());
        assert_eq!(kind, Some("not_implemented"), "{tool} kind tag");
    }
}

#[tokio::test]
async fn mcp_tool_invalid_request_payload_returns_400_bad_request() {
    let (addr, _state, _tmp) = start_server().await;
    // `list_memories` expects a request object; feeding a string
    // where the struct goes trips the `parse_request` validator,
    // which must produce a `400 invalid_request` error.
    let resp = post_tool(addr, envelope("list_memories", json!("not an object"))).await;
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    let kind = body.pointer("/error/kind").and_then(|v| v.as_str());
    assert_eq!(kind, Some("invalid_request"));
}

#[tokio::test]
async fn mcp_tool_rejects_envelope_without_tool_name() {
    let (addr, _state, _tmp) = start_server().await;
    // A malformed envelope fails at the axum `Json` extraction
    // layer and surfaces a 4xx before reaching `dispatch`.
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/mcp/tool"))
        .json(&json!({ "not_tool": "list_memories" }))
        .send()
        .await
        .expect("send");
    assert!(
        resp.status().is_client_error(),
        "malformed envelope should be client error, got {}",
        resp.status()
    );
}
