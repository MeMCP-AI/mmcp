//! Server functions proxying browser requests to the live
//! `mmcp-server`.
//!
//! The Leptos SSR host runs these on the server side, calling
//! `mmcp-server` over HTTP with `reqwest`. The wasm client invokes
//! them via fetch against the Leptos origin, so the browser never
//! sees the real mmcp-server URL and we sidestep CORS entirely.
//!
//! Wire types here mirror the server's JSON shape. They intentionally
//! duplicate a subset of `mmcp-proto` / `mmcp-sync` to keep the wasm
//! bundle free of the full server crate graph (which pulls `gix`,
//! `sea-orm`, etc., none of which compile to wasm32). When a shared
//! wasm-safe DTO crate ships we can collapse this module.

use leptos::prelude::*;
use leptos::server_fn::ServerFnError;
use serde::{Deserialize, Serialize};

// ── Wire types ────────────────────────────────────────────────────

/// Successful response from `POST /auth/login`.
///
/// UUIDs stay as strings: the browser never does anything with them
/// other than pass them back to the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoginOk {
    pub token: String,
    pub user_id: String,
    pub expires_at: i64,
}

/// One entry in the `/sync/manifest` response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteGroup {
    pub group_id: String,
    pub slug: String,
    pub head_commit: String,
}

/// Summary of a single memory as returned by `list_memories`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDescriptor {
    pub id: String,
    pub group: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub mandatory: bool,
    pub latest_version: Option<String>,
}

// ── SSR-only helpers ─────────────────────────────────────────────

#[cfg(feature = "ssr")]
mod ssr {
    use leptos::server_fn::ServerFnError;

    /// Base URL of the running `mmcp-server`, read from
    /// `MMCP_SERVER_URL`. Falls back to the dev default used by
    /// `.mmcp.toml` so `cargo leptos watch` just works against a
    /// locally-running server.
    pub fn server_url() -> String {
        std::env::var("MMCP_SERVER_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".to_string())
    }

    /// Turn a `reqwest` error into a server-function error. Keeps
    /// the macro call sites terse.
    pub fn fn_err(e: impl std::fmt::Display) -> ServerFnError {
        ServerFnError::ServerError(e.to_string())
    }
}

// ── Server functions ─────────────────────────────────────────────

/// Authenticate with the upstream server and return the bearer
/// token the browser should attach to subsequent calls.
#[server]
pub async fn login(handle: String, password: String) -> Result<LoginOk, ServerFnError> {
    use ssr::*;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/auth/login", server_url()))
        .json(&serde_json::json!({
            "handle": handle,
            "password": password,
        }))
        .send()
        .await
        .map_err(fn_err)?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(ServerFnError::ServerError(format!(
            "login failed ({status}): {body}"
        )));
    }
    resp.json::<LoginOk>().await.map_err(fn_err)
}

/// List the groups visible to the caller.
#[server]
pub async fn list_manifest(token: Option<String>) -> Result<Vec<RemoteGroup>, ServerFnError> {
    use ssr::*;

    #[derive(Deserialize)]
    struct Envelope {
        groups: Vec<RemoteGroup>,
    }

    let mut req = reqwest::Client::new().get(format!("{}/sync/manifest", server_url()));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(fn_err)?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(ServerFnError::ServerError(format!(
            "manifest fetch failed ({status}): {body}"
        )));
    }
    let env: Envelope = resp.json().await.map_err(fn_err)?;
    Ok(env.groups)
}

/// List memories inside one group via the `/mcp/tool` dispatcher.
#[server]
pub async fn list_memories(
    token: Option<String>,
    group: String,
) -> Result<Vec<MemoryDescriptor>, ServerFnError> {
    use ssr::*;

    #[derive(Deserialize)]
    struct ResponseEnvelope {
        response: InnerResponse,
    }
    #[derive(Deserialize)]
    struct InnerResponse {
        memories: Vec<MemoryDescriptor>,
    }

    let body = serde_json::json!({
        "tool": "list_memories",
        "request": {
            "group": group,
            "kinds": [],
            "only_mandatory": null,
        }
    });
    let mut req = reqwest::Client::new()
        .post(format!("{}/mcp/tool", server_url()))
        .json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(fn_err)?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ServerFnError::ServerError(format!(
            "list_memories failed ({status}): {text}"
        )));
    }
    let env: ResponseEnvelope = resp.json().await.map_err(fn_err)?;
    Ok(env.response.memories)
}
