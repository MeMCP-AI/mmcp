//! Authentication endpoints.
//!
//! Minimal surface for this cut: register a user with a password
//! and log in to receive a session token. OAuth and passkey flows
//! are left for a follow-up because they require more wiring than
//! a single commit should bundle.

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use jiff::Timestamp;
use mmcp_auth::{SessionClaims, hash_password, verify_password};
use mmcp_db::repository::user_repo;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
}

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub handle: String,
    pub password: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Serialize)]
pub struct RegisterResponse {
    pub user_id: Uuid,
}

async fn register(
    State(state): State<ServerState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, (StatusCode, String)> {
    let hash = hash_password(&req.password)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let user_id = Uuid::now_v7();
    user_repo::create(
        state.database.connection(),
        user_repo::NewUser {
            id: user_id,
            handle: req.handle,
            display_name: req.display_name,
            password_hash: Some(hash),
            email: req.email,
            created_at: Timestamp::now().as_millisecond(),
        },
    )
    .await
    .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
    Ok(Json(RegisterResponse { user_id }))
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub handle: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user_id: Uuid,
    pub expires_at: i64,
}

async fn login(
    State(state): State<ServerState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, String)> {
    let user = user_repo::find_by_handle(state.database.connection(), &req.handle)
        .await
        .map_err(|e: mmcp_db::DbError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "unknown handle".to_string()))?;

    let Some(hash) = user.password_hash.as_deref() else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "no password set for this account".into(),
        ));
    };
    verify_password(&req.password, hash)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid credentials".to_string()))?;

    let now = Timestamp::now().as_second();
    let claims = SessionClaims::new_with_lifetime(user.id, Uuid::now_v7(), now, 3600);
    let token = state
        .token_issuer
        .issue(&claims)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(LoginResponse {
        token,
        user_id: user.id,
        expires_at: claims.exp,
    }))
}
