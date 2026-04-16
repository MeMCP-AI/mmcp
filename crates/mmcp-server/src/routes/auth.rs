//! Authentication endpoints.
//!
//! Three authentication strategies are supported:
//!
//! - **Password**: `POST /auth/register` + `POST /auth/login`
//! - **OAuth**: `GET /auth/oauth/:provider/authorize` redirects to
//!   the provider, `GET /auth/oauth/:provider/callback` exchanges
//!   the code for a token and logs the user in.
//! - **Passkey**: `POST /auth/passkey/register/start` +
//!   `POST /auth/passkey/register/finish` for enrollment, and
//!   `POST /auth/passkey/login/start` +
//!   `POST /auth/passkey/login/finish` for authentication.
//!
//! All flows issue an `axum-login` session cookie on success.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use jiff::Timestamp;
use mmcp_auth::{AuthSession, Credentials, hash_password};
use mmcp_db::repository::{passkey_repo, user_repo};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;
use webauthn_rs::prelude::*;

use crate::state::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new()
        // Password
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        // OAuth
        .route("/auth/oauth/:provider/authorize", get(oauth_authorize))
        .route("/auth/oauth/:provider/callback", get(oauth_callback))
        // Passkey
        .route("/auth/passkey/register/start", post(passkey_register_start))
        .route(
            "/auth/passkey/register/finish",
            post(passkey_register_finish),
        )
        .route("/auth/passkey/login/start", post(passkey_login_start))
        .route("/auth/passkey/login/finish", post(passkey_login_finish))
}

// ── Password ────────────────────────────────────────────────────

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
) -> Result<(StatusCode, Json<RegisterResponse>), AuthHttpError> {
    let hash = hash_password(&req.password).map_err(AuthHttpError::internal)?;
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
    .map_err(|_| AuthHttpError::Conflict("handle already taken"))?;
    Ok((StatusCode::CREATED, Json(RegisterResponse { user_id })))
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
    mut auth_session: AuthSession,
    State(state): State<ServerState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AuthHttpError> {
    let user = auth_session
        .authenticate(Credentials::Password {
            handle: req.handle.clone(),
            password: req.password,
        })
        .await
        .map_err(AuthHttpError::internal)?
        .ok_or(AuthHttpError::Unauthorized("invalid credentials"))?;

    auth_session.login(&user).await.map_err(AuthHttpError::internal)?;

    let now = Timestamp::now().as_second();
    let claims =
        mmcp_auth::SessionClaims::new_with_lifetime(user.id, Uuid::now_v7(), now, 3600);
    let token = state
        .token_issuer
        .issue(&claims)
        .map_err(AuthHttpError::internal)?;
    Ok(Json(LoginResponse {
        token,
        user_id: user.id,
        expires_at: claims.exp,
    }))
}

// ── OAuth ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OAuthCallbackQuery {
    code: String,
    #[allow(dead_code)]
    state: Option<String>,
}

async fn oauth_authorize(
    State(state): State<ServerState>,
    Path(provider): Path<String>,
) -> Result<Redirect, AuthHttpError> {
    let cfg = state
        .oauth_providers
        .get(&provider)
        .ok_or(AuthHttpError::NotFound("unknown OAuth provider"))?;

    let callback_url = format!("{}/auth/oauth/{}/callback", state.origin, provider);
    let authorize_url = format!(
        "{}?client_id={}&redirect_uri={}&scope=user:email",
        cfg.auth_url,
        cfg.client_id,
        urlencoding::encode(&callback_url),
    );
    Ok(Redirect::temporary(&authorize_url))
}

/// GitHub user info response (partial).
#[derive(Deserialize)]
struct GitHubUser {
    id: u64,
    login: String,
    email: Option<String>,
}

async fn oauth_callback(
    mut auth_session: AuthSession,
    State(state): State<ServerState>,
    Path(provider): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Result<Response, AuthHttpError> {
    let cfg = state
        .oauth_providers
        .get(&provider)
        .ok_or(AuthHttpError::NotFound("unknown OAuth provider"))?;

    let callback_url = format!("{}/auth/oauth/{}/callback", state.origin, provider);

    // Exchange the authorization code for an access token.
    let http = reqwest::Client::new();
    let token_resp = http
        .post(&cfg.token_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("code", query.code.as_str()),
            ("redirect_uri", callback_url.as_str()),
        ])
        .send()
        .await
        .map_err(AuthHttpError::internal)?;

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        #[allow(dead_code)]
        token_type: Option<String>,
    }
    let tokens: TokenResponse = token_resp
        .json()
        .await
        .map_err(AuthHttpError::internal)?;

    // Fetch user info from the provider.
    let userinfo_resp = http
        .get(&cfg.userinfo_url)
        .header("Authorization", format!("Bearer {}", tokens.access_token))
        .header("User-Agent", "mmcp-server")
        .send()
        .await
        .map_err(AuthHttpError::internal)?;
    let gh_user: GitHubUser = userinfo_resp
        .json()
        .await
        .map_err(AuthHttpError::internal)?;

    let user = auth_session
        .authenticate(Credentials::OAuth {
            provider,
            provider_user_id: gh_user.id.to_string(),
            email: gh_user.email.or(Some(format!("{}@github", gh_user.login))),
            access_token: Some(tokens.access_token),
            refresh_token: None,
        })
        .await
        .map_err(AuthHttpError::internal)?
        .ok_or(AuthHttpError::Unauthorized("oauth authentication failed"))?;

    auth_session.login(&user).await.map_err(AuthHttpError::internal)?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "user_id": user.id,
            "handle": user.handle,
            "message": "OAuth login successful"
        })),
    )
        .into_response())
}

// ── Passkey ─────────────────────────────────────────────────────

/// In-flight passkey registration state. In production this would
/// live in the session store or a short-lived cache; for now we
/// use a global mutex keyed by user id.
type PasskeyRegState = Arc<Mutex<std::collections::HashMap<Uuid, PasskeyRegistration>>>;
type PasskeyAuthState = Arc<Mutex<std::collections::HashMap<Uuid, PasskeyAuthentication>>>;

/// Lazily initialized global registration state. Entries expire
/// after a few minutes in practice because the ceremony must
/// complete quickly; we do not garbage-collect here.
fn reg_state() -> &'static PasskeyRegState {
    static STATE: std::sync::OnceLock<PasskeyRegState> = std::sync::OnceLock::new();
    STATE.get_or_init(|| Arc::new(Mutex::new(std::collections::HashMap::new())))
}

fn auth_state() -> &'static PasskeyAuthState {
    static STATE: std::sync::OnceLock<PasskeyAuthState> = std::sync::OnceLock::new();
    STATE.get_or_init(|| Arc::new(Mutex::new(std::collections::HashMap::new())))
}

#[derive(Deserialize)]
struct PasskeyRegStartRequest {
    user_id: Uuid,
}

/// Start the passkey registration ceremony.
///
/// The caller must already be authenticated (knows their user_id).
/// Returns the `CreationChallengeResponse` the browser passes to
/// `navigator.credentials.create()`.
async fn passkey_register_start(
    State(state): State<ServerState>,
    Json(req): Json<PasskeyRegStartRequest>,
) -> Result<Json<CreationChallengeResponse>, AuthHttpError> {
    let conn = state.database.connection();
    let user = user_repo::find_by_id(conn, req.user_id)
        .await
        .map_err(AuthHttpError::internal)?
        .ok_or(AuthHttpError::NotFound("user not found"))?;

    // Load existing credentials so the server can exclude them.
    let existing_creds = passkey_repo::find_by_user(conn, user.id)
        .await
        .map_err(AuthHttpError::internal)?;
    let existing: Vec<Passkey> = existing_creds
        .iter()
        .filter_map(|c| serde_json::from_str(&c.credential_json).ok())
        .collect();

    let exclude_creds: Vec<CredentialID> = existing
        .iter()
        .map(|pk| pk.cred_id().clone())
        .collect();
    let (ccr, reg_state_value) = state
        .webauthn
        .start_passkey_registration(
            user.id,
            &user.handle,
            &user.handle,
            if exclude_creds.is_empty() {
                None
            } else {
                Some(exclude_creds)
            },
        )
        .map_err(AuthHttpError::internal)?;

    // Stash the registration state so `finish` can complete it.
    reg_state().lock().await.insert(user.id, reg_state_value);

    Ok(Json(ccr))
}

#[derive(Deserialize)]
struct PasskeyRegFinishRequest {
    user_id: Uuid,
    credential_name: String,
    response: RegisterPublicKeyCredential,
}

async fn passkey_register_finish(
    State(state): State<ServerState>,
    Json(req): Json<PasskeyRegFinishRequest>,
) -> Result<Json<serde_json::Value>, AuthHttpError> {
    let pending = reg_state()
        .lock()
        .await
        .remove(&req.user_id)
        .ok_or(AuthHttpError::BadRequest(
            "no pending registration for this user",
        ))?;

    let passkey = state
        .webauthn
        .finish_passkey_registration(&req.response, &pending)
        .map_err(AuthHttpError::internal)?;

    let cred_json =
        serde_json::to_string(&passkey).map_err(AuthHttpError::internal)?;
    let now = Timestamp::now().as_millisecond();
    passkey_repo::create(
        state.database.connection(),
        Uuid::now_v7(),
        req.user_id,
        req.credential_name,
        cred_json,
        now,
    )
    .await
    .map_err(AuthHttpError::internal)?;

    Ok(Json(serde_json::json!({ "status": "registered" })))
}

#[derive(Deserialize)]
struct PasskeyLoginStartRequest {
    handle: String,
}

async fn passkey_login_start(
    State(state): State<ServerState>,
    Json(req): Json<PasskeyLoginStartRequest>,
) -> Result<Json<RequestChallengeResponse>, AuthHttpError> {
    let conn = state.database.connection();
    let user = user_repo::find_by_handle(conn, &req.handle)
        .await
        .map_err(AuthHttpError::internal)?
        .ok_or(AuthHttpError::NotFound("user not found"))?;

    let creds = passkey_repo::find_by_user(conn, user.id)
        .await
        .map_err(AuthHttpError::internal)?;
    if creds.is_empty() {
        return Err(AuthHttpError::BadRequest("no passkeys registered"));
    }
    let passkeys: Vec<Passkey> = creds
        .iter()
        .filter_map(|c| serde_json::from_str(&c.credential_json).ok())
        .collect();

    let (rcr, auth_state_value) = state
        .webauthn
        .start_passkey_authentication(&passkeys)
        .map_err(AuthHttpError::internal)?;

    auth_state().lock().await.insert(user.id, auth_state_value);

    Ok(Json(rcr))
}

#[derive(Deserialize)]
struct PasskeyLoginFinishRequest {
    handle: String,
    response: PublicKeyCredential,
}

async fn passkey_login_finish(
    mut auth_session: AuthSession,
    State(state): State<ServerState>,
    Json(req): Json<PasskeyLoginFinishRequest>,
) -> Result<Json<serde_json::Value>, AuthHttpError> {
    let conn = state.database.connection();
    let user = user_repo::find_by_handle(conn, &req.handle)
        .await
        .map_err(AuthHttpError::internal)?
        .ok_or(AuthHttpError::NotFound("user not found"))?;

    let pending = auth_state()
        .lock()
        .await
        .remove(&user.id)
        .ok_or(AuthHttpError::BadRequest(
            "no pending authentication for this user",
        ))?;

    let auth_result = state
        .webauthn
        .finish_passkey_authentication(&req.response, &pending)
        .map_err(AuthHttpError::internal)?;

    // Update the credential counter in the DB to prevent replay.
    let creds = passkey_repo::find_by_user(conn, user.id)
        .await
        .map_err(AuthHttpError::internal)?;
    for cred_row in &creds {
        if let Ok(mut pk) = serde_json::from_str::<Passkey>(&cred_row.credential_json) {
            if pk.update_credential(&auth_result) == Some(true) {
                let updated_json =
                    serde_json::to_string(&pk).map_err(AuthHttpError::internal)?;
                let now = Timestamp::now().as_millisecond();
                let _ = passkey_repo::update_after_auth(conn, cred_row.id, updated_json, now)
                    .await;
            }
        }
    }

    // Look up which credential row was used so we can resolve the
    // user via the auth backend.
    let matched_cred = creds.first().ok_or(AuthHttpError::Unauthorized(
        "no matching credential found",
    ))?;

    let authed_user = auth_session
        .authenticate(Credentials::Passkey {
            credential_row_id: matched_cred.id,
        })
        .await
        .map_err(AuthHttpError::internal)?
        .ok_or(AuthHttpError::Unauthorized("passkey authentication failed"))?;

    auth_session
        .login(&authed_user)
        .await
        .map_err(AuthHttpError::internal)?;

    Ok(Json(serde_json::json!({
        "user_id": authed_user.id,
        "handle": authed_user.handle,
        "message": "Passkey login successful"
    })))
}

// ── Error response ──────────────────────────────────────────────

#[derive(Debug)]
enum AuthHttpError {
    Unauthorized(&'static str),
    NotFound(&'static str),
    BadRequest(&'static str),
    Conflict(&'static str),
    Internal(String),
}

impl AuthHttpError {
    fn internal<E: std::fmt::Display>(err: E) -> Self {
        AuthHttpError::Internal(err.to_string())
    }
}

impl IntoResponse for AuthHttpError {
    fn into_response(self) -> Response {
        match self {
            AuthHttpError::Unauthorized(msg) => {
                (StatusCode::UNAUTHORIZED, msg.to_string()).into_response()
            }
            AuthHttpError::NotFound(msg) => {
                (StatusCode::NOT_FOUND, msg.to_string()).into_response()
            }
            AuthHttpError::BadRequest(msg) => {
                (StatusCode::BAD_REQUEST, msg.to_string()).into_response()
            }
            AuthHttpError::Conflict(msg) => {
                (StatusCode::CONFLICT, msg.to_string()).into_response()
            }
            AuthHttpError::Internal(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response()
            }
        }
    }
}
