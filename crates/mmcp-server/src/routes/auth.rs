//! Authentication endpoints.
//!
//! Three authentication strategies are supported:
//!
//! - **Password**: `POST /auth/register` (gated by
//!   [`crate::config::ServerConfig::allow_self_registration`],
//!   closed by default) + `POST /auth/login`
//! - **OAuth**: `GET /auth/oauth/:provider/authorize` redirects to
//!   the provider, `GET /auth/oauth/:provider/callback` exchanges
//!   the code for a token and logs the user in.
//! - **Passkey**: `POST /auth/passkey/register/start` +
//!   `POST /auth/passkey/register/finish` for enrollment (the
//!   caller must already hold an authenticated session; a passkey
//!   is always enrolled onto the caller's own account), and
//!   `POST /auth/passkey/login/start` +
//!   `POST /auth/passkey/login/finish` for authentication.
//!
//! All flows issue an `axum-login` session cookie on success.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use jiff::Timestamp;
use mmcp_auth::{AuthSession, Credentials, hash_password, validate_password_policy};
use mmcp_db::repository::{passkey_repo, user_repo};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;
use webauthn_rs::prelude::*;

use crate::routes::defaults::{
    AUTH_REQUEST_BODY_LIMIT_BYTES, MAX_DISPLAY_NAME_LENGTH, MAX_EMAIL_LENGTH,
    OAUTH_STATE_HEX_LENGTH, OAUTH_STATE_SESSION_KEY_PREFIX, OAUTH_STATE_TOKEN_BYTES,
    PASSKEY_CEREMONY_TTL,
};
use crate::routes::registration_limits::RegistrationLimits;
use crate::routes::response::{self, FromInternalError, into_generic_response};
use crate::state::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new()
        // Password
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        // OAuth
        .route("/auth/oauth/{provider}/authorize", get(oauth_authorize))
        .route("/auth/oauth/{provider}/callback", get(oauth_callback))
        // Passkey
        .route("/auth/passkey/register/start", post(passkey_register_start))
        .route(
            "/auth/passkey/register/finish",
            post(passkey_register_finish),
        )
        .route("/auth/passkey/login/start", post(passkey_login_start))
        .route("/auth/passkey/login/finish", post(passkey_login_finish))
        .layer(DefaultBodyLimit::max(AUTH_REQUEST_BODY_LIMIT_BYTES))
}

// ── Password ────────────────────────────────────────────────────

/// Compiled-in default tier of the handle length bound.
/// See [`mmcp_auth::MAX_HANDLE_LENGTH`]'s doc comment for the cascade that overrides it.
/// `pub` so the unit tests below can assert the default tier's own numeric value.
pub use mmcp_auth::MAX_HANDLE_LENGTH;

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

/// Check `value`'s byte length against `max`, returning a
/// [`AuthHttpError::FieldTooLong`] naming the offending field, its
/// actual length, and the max when it is exceeded.
fn validate_max_length(field: &'static str, value: &str, max: usize) -> Result<(), AuthHttpError> {
    let actual = value.len();
    if actual > max {
        return Err(AuthHttpError::FieldTooLong { field, actual, max });
    }
    Ok(())
}

/// Reject `value` if it is empty or trims to an empty string,
/// returning [`AuthHttpError::FieldBlank`] naming the offending
/// field.
fn validate_non_blank(field: &'static str, value: &str) -> Result<(), AuthHttpError> {
    if value.trim().is_empty() {
        return Err(AuthHttpError::FieldBlank { field });
    }
    Ok(())
}

/// Validate a [`RegisterRequest`]'s fields at the HTTP boundary, so
/// a rejected request never reaches the password hasher or the
/// database: an empty/whitespace-only handle, email, or display name
/// is rejected, the handle is checked against the server's resolved
/// [`max_handle_length`](crate::config::ServerConfig::max_handle_length)
/// bound, every other field carries a fixed maximum length, and the
/// password is checked against the server's resolved
/// [`min_password_length`](crate::config::ServerConfig::min_password_length) /
/// [`max_password_length`](crate::config::ServerConfig::max_password_length)
/// bounds.
fn validate_register_request(
    req: &RegisterRequest,
    limits: RegistrationLimits,
) -> Result<(), AuthHttpError> {
    validate_non_blank("handle", &req.handle)?;
    validate_max_length("handle", &req.handle, limits.max_handle_length)?;
    if let Some(email) = &req.email {
        validate_non_blank("email", email)?;
        validate_max_length("email", email, MAX_EMAIL_LENGTH)?;
    }
    if let Some(display_name) = &req.display_name {
        validate_non_blank("display_name", display_name)?;
        validate_max_length("display_name", display_name, MAX_DISPLAY_NAME_LENGTH)?;
    }
    validate_password_policy(
        &req.password,
        limits.min_password_length,
        limits.max_password_length,
    )
    .map_err(AuthHttpError::PasswordPolicy)?;
    Ok(())
}

async fn register(
    State(state): State<ServerState>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<RegisterResponse>), AuthHttpError> {
    if !state.allow_self_registration {
        return Err(AuthHttpError::RegistrationDisabled);
    }
    validate_register_request(
        &req,
        RegistrationLimits {
            min_password_length: state.min_password_length,
            max_password_length: state.max_password_length,
            max_handle_length: state.max_handle_length,
        },
    )?;
    let hash = hash_password(&req.password).map_err(into_generic_response)?;
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

/// Bound [`LoginRequest::password`] before it ever reaches
/// [`mmcp_auth::verify_password`]'s Argon2 step. Login shares the
/// same unauthenticated-router, hashing-amplification threat model
/// `/auth/register`'s password-length check already guards against
/// (see [`AUTH_REQUEST_BODY_LIMIT_BYTES`]); only the maximum bound
/// applies here (there is no minimum to enforce against an unknown
/// existing account's password).
fn validate_login_request(
    req: &LoginRequest,
    max_password_length: usize,
) -> Result<(), AuthHttpError> {
    validate_max_length("password", &req.password, max_password_length)
}

async fn login(
    mut auth_session: AuthSession,
    State(state): State<ServerState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AuthHttpError> {
    validate_login_request(&req, state.max_password_length)?;
    let user = auth_session
        .authenticate(Credentials::Password {
            handle: req.handle.clone(),
            password: req.password,
        })
        .await
        .map_err(into_generic_response)?
        .ok_or(AuthHttpError::Unauthorized("invalid credentials"))?;

    auth_session
        .login(&user)
        .await
        .map_err(into_generic_response)?;

    let now = Timestamp::now().as_second();
    let claims = mmcp_auth::SessionClaims::new_with_lifetime(user.id, Uuid::now_v7(), now, 3600);
    let token = state
        .token_issuer
        .issue(&claims)
        .map_err(into_generic_response)?;
    Ok(Json(LoginResponse {
        token,
        user_id: user.id,
        expires_at: claims.exp,
    }))
}

// ── OAuth ───────────────────────────────────────────────────────

/// Builds the per-provider session key from [`OAUTH_STATE_SESSION_KEY_PREFIX`].
/// Concurrent flows against different providers get distinct keys,
/// so they cannot clobber each other's pending state.
fn oauth_state_session_key(provider: &str) -> String {
    format!("{OAUTH_STATE_SESSION_KEY_PREFIX}{provider}")
}

/// Mints a fresh OAuth CSRF `state` token from the OS CSPRNG, hex-encoded.
/// Hex encoding needs no extra escaping under [`oauth_authorize`]'s percent-encoding of the whole value.
fn generate_oauth_state() -> Result<String, AuthHttpError> {
    let mut bytes = [0u8; OAUTH_STATE_TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(into_generic_response)?;
    Ok(hex::encode(bytes))
}

#[derive(Deserialize)]
struct OAuthCallbackQuery {
    code: String,
    /// CSRF token minted by [`oauth_authorize`] and echoed back by the provider.
    /// [`oauth_callback`] compares it against the value stored in the caller's session before any token exchange.
    state: Option<String>,
}

async fn oauth_authorize(
    auth_session: AuthSession,
    State(state): State<ServerState>,
    Path(provider): Path<String>,
) -> Result<Redirect, AuthHttpError> {
    let cfg = state
        .oauth_providers
        .get(&provider)
        .ok_or(AuthHttpError::NotFound("unknown OAuth provider"))?;

    let csrf_state = generate_oauth_state()?;
    auth_session
        .session
        .insert(&oauth_state_session_key(&provider), &csrf_state)
        .await
        .map_err(into_generic_response)?;

    let callback_url = format!("{}/auth/oauth/{}/callback", state.origin, provider);
    let authorize_url = format!(
        "{}?client_id={}&redirect_uri={}&scope=user:email&state={}",
        cfg.auth_url,
        cfg.client_id,
        urlencoding::encode(&callback_url),
        urlencoding::encode(&csrf_state),
    );
    Ok(Redirect::temporary(&authorize_url))
}

/// Why an OAuth callback's `state` failed validation against the
/// stored CSRF token.
///
/// Every variant maps to the same uniform external
/// [`AuthHttpError::InvalidOAuthState`] 400 response; [`oauth_callback`]
/// turns each into its own log line before returning that response.
#[derive(Debug, Error, PartialEq, Eq)]
enum OAuthStateRejection {
    /// The callback query carried no `state` parameter at all.
    #[error("no state parameter present in the callback query")]
    MissingFromQuery,
    /// The query carried a `state`, but the session had none stored
    /// (expired, already consumed, or the session cookie was
    /// withheld).
    #[error("no stored csrf state found in the session")]
    NoStoredState,
    /// The received `state` is not exactly [`OAUTH_STATE_HEX_LENGTH`]
    /// characters, rejected before the equality comparison below.
    #[error("state length {received_len} does not match the expected {expected_len}")]
    LengthMismatch {
        received_len: usize,
        expected_len: usize,
    },
    /// The received `state` has the right length but does not equal
    /// the value stored at authorize time.
    #[error("state does not match the value issued at authorize time")]
    ValueMismatch,
}

/// Validate an OAuth callback's `state` against the session's stored
/// value.
///
/// Pure and side-effect-free, unlike [`oauth_callback`] itself, so
/// each rejection cause is independently unit testable without a
/// running server.
fn validate_oauth_state(
    received: Option<&str>,
    expected: Option<&str>,
) -> Result<(), OAuthStateRejection> {
    let received = received.ok_or(OAuthStateRejection::MissingFromQuery)?;
    let expected = expected.ok_or(OAuthStateRejection::NoStoredState)?;
    if received.len() != OAUTH_STATE_HEX_LENGTH {
        return Err(OAuthStateRejection::LengthMismatch {
            received_len: received.len(),
            expected_len: OAUTH_STATE_HEX_LENGTH,
        });
    }
    if received != expected {
        return Err(OAuthStateRejection::ValueMismatch);
    }
    Ok(())
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

    // Consume the stored state before comparing.
    // A token is usable for at most one callback, regardless of whether the comparison below passes.
    let expected_state: Option<String> = auth_session
        .session
        .remove(&oauth_state_session_key(&provider))
        .await
        .map_err(into_generic_response)?;

    // Every cause collapses to the same uniform 400 body
    // (`AuthHttpError::InvalidOAuthState`), per `global-coding-rules-errors`'s
    // security-mandated-uniform-response exception, but each still
    // gets its own log line so a deployment failure like "the session
    // cookie never round-trips" is diagnosable from logs alone
    // instead of surfacing only as a blanket rejection.
    if let Err(rejection) = validate_oauth_state(query.state.as_deref(), expected_state.as_deref())
    {
        match &rejection {
            // The caller-controlled query is simply missing the
            // parameter; not evidence of a server-side problem.
            OAuthStateRejection::MissingFromQuery => {
                tracing::debug!(provider = %provider, error = %rejection, "oauth callback rejected");
            }
            OAuthStateRejection::NoStoredState
            | OAuthStateRejection::LengthMismatch { .. }
            | OAuthStateRejection::ValueMismatch => {
                tracing::warn!(provider = %provider, error = %rejection, "oauth callback rejected");
            }
        }
        return Err(AuthHttpError::InvalidOAuthState);
    }

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
        .map_err(into_generic_response)?;

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        #[allow(dead_code)]
        token_type: Option<String>,
    }
    let tokens: TokenResponse = token_resp.json().await.map_err(into_generic_response)?;

    // Fetch user info from the provider.
    let userinfo_resp = http
        .get(&cfg.userinfo_url)
        .header("Authorization", format!("Bearer {}", tokens.access_token))
        .header("User-Agent", "mmcp-server")
        .send()
        .await
        .map_err(into_generic_response)?;
    let gh_user: GitHubUser = userinfo_resp.json().await.map_err(into_generic_response)?;

    let user = auth_session
        .authenticate(Credentials::OAuth {
            provider,
            provider_user_id: gh_user.id.to_string(),
            email: gh_user.email.or(Some(format!("{}@github", gh_user.login))),
            access_token: Some(tokens.access_token),
            refresh_token: None,
        })
        .await
        .map_err(into_generic_response)?
        .ok_or(AuthHttpError::Unauthorized("oauth authentication failed"))?;

    auth_session
        .login(&user)
        .await
        .map_err(into_generic_response)?;

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

/// In-flight passkey registration state, keyed by user id and
/// timestamped so [`purge_stale_ceremonies`] can evict entries past
/// [`PASSKEY_CEREMONY_TTL`]. In production this would live in the
/// session store or a short-lived cache; for now we use a global
/// mutex.
type PasskeyRegState =
    Arc<Mutex<std::collections::HashMap<Uuid, (std::time::Instant, PasskeyRegistration)>>>;
type PasskeyAuthState =
    Arc<Mutex<std::collections::HashMap<Uuid, (std::time::Instant, PasskeyAuthentication)>>>;

/// Lazily initialized global registration state.
fn reg_state() -> &'static PasskeyRegState {
    static STATE: std::sync::OnceLock<PasskeyRegState> = std::sync::OnceLock::new();
    STATE.get_or_init(|| Arc::new(Mutex::new(std::collections::HashMap::new())))
}

fn auth_state() -> &'static PasskeyAuthState {
    static STATE: std::sync::OnceLock<PasskeyAuthState> = std::sync::OnceLock::new();
    STATE.get_or_init(|| Arc::new(Mutex::new(std::collections::HashMap::new())))
}

/// Remove entries older than [`PASSKEY_CEREMONY_TTL`] from `map`.
/// Called immediately before every insert into
/// [`reg_state`]/[`auth_state`], so an abandoned ceremony never lives
/// past its natural completion window instead of accumulating for
/// the process lifetime.
fn purge_stale_ceremonies<T>(map: &mut std::collections::HashMap<Uuid, (std::time::Instant, T)>) {
    map.retain(|_, (inserted_at, _)| inserted_at.elapsed() < PASSKEY_CEREMONY_TTL);
}

/// Remove and return the ceremony state for `user_id`, but only when
/// it has not aged past [`PASSKEY_CEREMONY_TTL`]. An entry that is
/// present but stale (not yet reached by [`purge_stale_ceremonies`]'s
/// next insert-time sweep) is treated the same as absent: the
/// ceremony window has already closed, so `finish` must not complete
/// it.
fn take_ceremony<T>(
    map: &mut std::collections::HashMap<Uuid, (std::time::Instant, T)>,
    user_id: Uuid,
) -> Option<T> {
    let (inserted_at, value) = map.remove(&user_id)?;
    (inserted_at.elapsed() < PASSKEY_CEREMONY_TTL).then_some(value)
}

/// Start the passkey registration ceremony.
///
/// The identity being enrolled is the CALLER's own authenticated
/// session user, never a value from the request body: a passkey
/// registration ceremony must not be startable for an arbitrary
/// target account by an unauthenticated caller.
/// Returns the `CreationChallengeResponse` the browser passes to
/// `navigator.credentials.create()`.
async fn passkey_register_start(
    auth_session: AuthSession,
    State(state): State<ServerState>,
) -> Result<Json<CreationChallengeResponse>, AuthHttpError> {
    let session_user = auth_session
        .user
        .ok_or(AuthHttpError::Unauthorized("authentication required"))?;
    let conn = state.database.connection();

    // Load existing credentials so the server can exclude them.
    let existing_creds = passkey_repo::find_by_user(conn, session_user.id)
        .await
        .map_err(into_generic_response)?;
    let existing: Vec<Passkey> = existing_creds
        .iter()
        .filter_map(|c| serde_json::from_str(&c.credential_json).ok())
        .collect();

    let exclude_creds: Vec<CredentialID> = existing.iter().map(|pk| pk.cred_id().clone()).collect();
    let (ccr, reg_state_value) = state
        .webauthn
        .start_passkey_registration(
            session_user.id,
            &session_user.handle,
            &session_user.handle,
            if exclude_creds.is_empty() {
                None
            } else {
                Some(exclude_creds)
            },
        )
        .map_err(into_generic_response)?;

    // Stash the registration state so `finish` can complete it.
    {
        let mut pending = reg_state().lock().await;
        purge_stale_ceremonies(&mut pending);
        pending.insert(
            session_user.id,
            (std::time::Instant::now(), reg_state_value),
        );
    }

    Ok(Json(ccr))
}

#[derive(Deserialize)]
struct PasskeyRegFinishRequest {
    credential_name: String,
    response: RegisterPublicKeyCredential,
}

/// Finish the passkey registration ceremony and attach the new
/// credential to the CALLER's own authenticated session user; the
/// identity is never taken from the request body (see
/// [`passkey_register_start`]).
async fn passkey_register_finish(
    auth_session: AuthSession,
    State(state): State<ServerState>,
    Json(req): Json<PasskeyRegFinishRequest>,
) -> Result<Json<serde_json::Value>, AuthHttpError> {
    let session_user = auth_session
        .user
        .ok_or(AuthHttpError::Unauthorized("authentication required"))?;

    let pending = take_ceremony(&mut *reg_state().lock().await, session_user.id).ok_or(
        AuthHttpError::BadRequest("no pending registration for this user"),
    )?;

    let passkey = state
        .webauthn
        .finish_passkey_registration(&req.response, &pending)
        .map_err(into_generic_response)?;

    let cred_json = serde_json::to_string(&passkey).map_err(into_generic_response)?;
    let now = Timestamp::now().as_millisecond();
    passkey_repo::create(
        state.database.connection(),
        Uuid::now_v7(),
        session_user.id,
        req.credential_name,
        cred_json,
        now,
    )
    .await
    .map_err(into_generic_response)?;

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
        .map_err(into_generic_response)?
        .ok_or(AuthHttpError::NotFound("user not found"))?;

    let creds = passkey_repo::find_by_user(conn, user.id)
        .await
        .map_err(into_generic_response)?;
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
        .map_err(into_generic_response)?;

    {
        let mut pending = auth_state().lock().await;
        purge_stale_ceremonies(&mut pending);
        pending.insert(user.id, (std::time::Instant::now(), auth_state_value));
    }

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
        .map_err(into_generic_response)?
        .ok_or(AuthHttpError::NotFound("user not found"))?;

    let pending = take_ceremony(&mut *auth_state().lock().await, user.id).ok_or(
        AuthHttpError::BadRequest("no pending authentication for this user"),
    )?;

    let auth_result = state
        .webauthn
        .finish_passkey_authentication(&req.response, &pending)
        .map_err(into_generic_response)?;

    // Update the credential counter in the DB to prevent replay.
    let creds = passkey_repo::find_by_user(conn, user.id)
        .await
        .map_err(into_generic_response)?;
    for cred_row in &creds {
        if let Ok(mut pk) = serde_json::from_str::<Passkey>(&cred_row.credential_json)
            && pk.update_credential(&auth_result) == Some(true)
        {
            let updated_json = serde_json::to_string(&pk).map_err(into_generic_response)?;
            let now = Timestamp::now().as_millisecond();
            let _ = passkey_repo::update_after_auth(conn, cred_row.id, updated_json, now).await;
        }
    }

    // Look up which credential row was used so we can resolve the
    // user via the auth backend.
    let matched_cred = creds
        .first()
        .ok_or(AuthHttpError::Unauthorized("no matching credential found"))?;

    let authed_user = auth_session
        .authenticate(Credentials::Passkey {
            credential_row_id: matched_cred.id,
        })
        .await
        .map_err(into_generic_response)?
        .ok_or(AuthHttpError::Unauthorized("passkey authentication failed"))?;

    auth_session
        .login(&authed_user)
        .await
        .map_err(into_generic_response)?;

    Ok(Json(serde_json::json!({
        "user_id": authed_user.id,
        "handle": authed_user.handle,
        "message": "Passkey login successful"
    })))
}

// ── Error response ──────────────────────────────────────────────

/// Wire-facing error for every `/auth/*` handler. Each distinct
/// validation cause is its own variant with structured fields
/// (never a formatted string carrying the actual/max values as
/// text), and [`AuthHttpError::PasswordPolicy`] source-chains the
/// underlying [`mmcp_auth::AuthError`] instead of flattening it to a
/// string one frame earlier, so a caller matching on the password
/// error's own typed variants (e.g. distinguishing
/// [`mmcp_auth::AuthError::PasswordBlank`] from
/// [`mmcp_auth::AuthError::PasswordTooShort`]) can still do so.
#[derive(Debug, Error)]
enum AuthHttpError {
    #[error("{0}")]
    Unauthorized(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    BadRequest(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    /// `POST /auth/register` was called while
    /// [`crate::state::ServerStateInner::allow_self_registration`] is
    /// `false` (the closed-by-default state; see
    /// [`crate::config::ServerConfig::allow_self_registration`]).
    /// Rejected before any validation, hashing, or database work.
    #[error(
        "self-registration is disabled on this server; set MMCP_ALLOW_SELF_REGISTRATION=true \
         to enable it"
    )]
    RegistrationDisabled,
    /// A request field exceeded its maximum accepted length.
    #[error("field '{field}' is too long: {actual} bytes exceeds the {max}-byte maximum")]
    FieldTooLong {
        field: &'static str,
        actual: usize,
        max: usize,
    },
    /// A request field required to be non-blank was empty or
    /// whitespace-only.
    #[error("field '{field}' must not be empty or whitespace-only")]
    FieldBlank { field: &'static str },
    /// The OAuth callback's `state` parameter was missing or mismatched against [`oauth_authorize`]'s stored value.
    /// Rejected before any token-exchange call.
    /// Closes the login CSRF / authorization-code-injection path,
    /// where a forged callback binds an attacker's own code into a victim's session.
    #[error(
        "oauth state parameter is missing or does not match the value issued at authorize time"
    )]
    InvalidOAuthState,
    /// The submitted password failed the password policy (blank, too
    /// short, or too long). Built with an explicit
    /// `.map_err(AuthHttpError::PasswordPolicy)` at the one call
    /// site, never `#[from]`: an automatic `From<AuthError>` impl on
    /// this enum would leave `?`'s implicit conversion ambiguous
    /// against the many other `.map_err(into_generic_response)`
    /// call sites in this module that also resolve to
    /// `AuthHttpError` through [`FromInternalError`].
    #[error("{0}")]
    PasswordPolicy(#[source] mmcp_auth::AuthError),
    #[error("{0}")]
    Internal(String),
}

impl FromInternalError for AuthHttpError {
    fn from_internal_error() -> Self {
        AuthHttpError::Internal(response::GENERIC_INTERNAL_ERROR_MESSAGE.to_string())
    }
}

impl IntoResponse for AuthHttpError {
    fn into_response(self) -> Response {
        let status = match self {
            AuthHttpError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AuthHttpError::NotFound(_) => StatusCode::NOT_FOUND,
            AuthHttpError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AuthHttpError::Conflict(_) => StatusCode::CONFLICT,
            AuthHttpError::RegistrationDisabled => StatusCode::FORBIDDEN,
            AuthHttpError::FieldTooLong { .. }
            | AuthHttpError::FieldBlank { .. }
            | AuthHttpError::InvalidOAuthState
            | AuthHttpError::PasswordPolicy(_) => StatusCode::BAD_REQUEST,
            AuthHttpError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = self.to_string();
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    // ── validate_oauth_state: each rejection cause is its own path ──

    #[test]
    fn validate_oauth_state_rejects_a_missing_query_parameter() {
        assert_eq!(
            validate_oauth_state(None, Some("expected")),
            Err(OAuthStateRejection::MissingFromQuery)
        );
    }

    #[test]
    fn validate_oauth_state_rejects_when_nothing_was_stored() {
        let received = "a".repeat(OAUTH_STATE_HEX_LENGTH);
        assert_eq!(
            validate_oauth_state(Some(&received), None),
            Err(OAuthStateRejection::NoStoredState)
        );
    }

    #[test]
    fn validate_oauth_state_rejects_a_length_mismatch_before_any_equality_comparison() {
        let received = "a".repeat(OAUTH_STATE_HEX_LENGTH - 1);
        // `expected` deliberately differs too, so a value-equality
        // check would ALSO reject this input: asserting the exact
        // `LengthMismatch` variant (not just "some error") proves the
        // length guard is what actually fires, not equality doing
        // double duty.
        let expected = "b".repeat(OAUTH_STATE_HEX_LENGTH);
        assert_eq!(
            validate_oauth_state(Some(&received), Some(&expected)),
            Err(OAuthStateRejection::LengthMismatch {
                received_len: OAUTH_STATE_HEX_LENGTH - 1,
                expected_len: OAUTH_STATE_HEX_LENGTH,
            })
        );
    }

    #[test]
    fn validate_oauth_state_rejects_a_same_length_value_mismatch() {
        let received = "a".repeat(OAUTH_STATE_HEX_LENGTH);
        let expected = "b".repeat(OAUTH_STATE_HEX_LENGTH);
        assert_eq!(
            validate_oauth_state(Some(&received), Some(&expected)),
            Err(OAuthStateRejection::ValueMismatch)
        );
    }

    #[test]
    fn validate_oauth_state_accepts_a_matching_value() {
        let value = "a".repeat(OAUTH_STATE_HEX_LENGTH);
        assert_eq!(validate_oauth_state(Some(&value), Some(&value)), Ok(()));
    }

    fn valid_request() -> RegisterRequest {
        RegisterRequest {
            handle: "alice".to_string(),
            password: "correcthorsebatterystaple".to_string(),
            email: Some("alice@example.com".to_string()),
            display_name: Some("Alice".to_string()),
        }
    }

    /// Run [`validate_register_request`] with the compiled-in
    /// default password and handle-length bounds, matching what
    /// `ServerState` resolves to when no override/env/config tier is
    /// set for any of them.
    fn validate(req: &RegisterRequest) -> Result<(), AuthHttpError> {
        validate_register_request(
            req,
            RegistrationLimits {
                min_password_length: mmcp_auth::MIN_PASSWORD_LENGTH,
                max_password_length: mmcp_auth::MAX_PASSWORD_LENGTH,
                max_handle_length: MAX_HANDLE_LENGTH,
            },
        )
    }

    #[test]
    fn accepts_a_valid_request() {
        assert!(validate(&valid_request()).is_ok());
    }

    #[test]
    fn rejects_empty_password() {
        let req = RegisterRequest {
            password: String::new(),
            ..valid_request()
        };
        let err = validate(&req).unwrap_err();
        assert!(matches!(
            err,
            AuthHttpError::PasswordPolicy(mmcp_auth::AuthError::PasswordBlank)
        ));
    }

    #[test]
    fn accepts_whitespace_only_password_at_min_length() {
        // 8 spaces meets MIN_PASSWORD_LENGTH (8): whitespace content
        // gets no special treatment, so this passes like any other
        // password of the same length.
        let req = RegisterRequest {
            password: " ".repeat(mmcp_auth::MIN_PASSWORD_LENGTH),
            ..valid_request()
        };
        assert!(validate(&req).is_ok());
    }

    #[test]
    fn rejects_over_length_password() {
        let req = RegisterRequest {
            password: "a".repeat(mmcp_auth::MAX_PASSWORD_LENGTH + 1),
            ..valid_request()
        };
        assert!(matches!(
            validate(&req),
            Err(AuthHttpError::PasswordPolicy(
                mmcp_auth::AuthError::PasswordTooLong { .. }
            ))
        ));
    }

    #[test]
    fn rejects_empty_handle() {
        let req = RegisterRequest {
            handle: "   ".to_string(),
            ..valid_request()
        };
        assert!(matches!(
            validate(&req),
            Err(AuthHttpError::FieldBlank { field: "handle" })
        ));
    }

    #[test]
    fn rejects_over_length_handle() {
        let req = RegisterRequest {
            handle: "h".repeat(MAX_HANDLE_LENGTH + 1),
            ..valid_request()
        };
        assert!(matches!(
            validate(&req),
            Err(AuthHttpError::FieldTooLong {
                field: "handle",
                ..
            })
        ));
    }

    #[test]
    fn rejects_empty_email() {
        let req = RegisterRequest {
            email: Some("   ".to_string()),
            ..valid_request()
        };
        assert!(matches!(
            validate(&req),
            Err(AuthHttpError::FieldBlank { field: "email" })
        ));
    }

    #[test]
    fn rejects_over_length_email() {
        let req = RegisterRequest {
            email: Some("a".repeat(MAX_EMAIL_LENGTH + 1)),
            ..valid_request()
        };
        assert!(matches!(
            validate(&req),
            Err(AuthHttpError::FieldTooLong { field: "email", .. })
        ));
    }

    #[test]
    fn rejects_empty_display_name() {
        let req = RegisterRequest {
            display_name: Some("   ".to_string()),
            ..valid_request()
        };
        assert!(matches!(
            validate(&req),
            Err(AuthHttpError::FieldBlank {
                field: "display_name"
            })
        ));
    }

    #[test]
    fn rejects_over_length_display_name() {
        let req = RegisterRequest {
            display_name: Some("a".repeat(MAX_DISPLAY_NAME_LENGTH + 1)),
            ..valid_request()
        };
        assert!(matches!(
            validate(&req),
            Err(AuthHttpError::FieldTooLong {
                field: "display_name",
                ..
            })
        ));
    }

    #[test]
    fn a_narrower_caller_supplied_min_password_length_is_actually_enforced() {
        // Proves `validate_register_request` reads its own
        // parameters (as `ServerState` resolves them), not the
        // module-level `mmcp_auth` constants directly: a password
        // that satisfies the compiled-in default minimum must still
        // be rejected once the caller narrows the floor above it.
        let req = RegisterRequest {
            password: "shortpw1".to_string(),
            ..valid_request()
        };
        assert!(
            validate_register_request(
                &req,
                RegistrationLimits {
                    min_password_length: 4,
                    max_password_length: mmcp_auth::MAX_PASSWORD_LENGTH,
                    max_handle_length: MAX_HANDLE_LENGTH,
                }
            )
            .is_ok()
        );
        assert!(matches!(
            validate_register_request(
                &req,
                RegistrationLimits {
                    min_password_length: 20,
                    max_password_length: mmcp_auth::MAX_PASSWORD_LENGTH,
                    max_handle_length: MAX_HANDLE_LENGTH,
                }
            ),
            Err(AuthHttpError::PasswordPolicy(
                mmcp_auth::AuthError::PasswordTooShort { min: 20, .. }
            ))
        ));
    }

    #[test]
    fn a_narrower_caller_supplied_max_handle_length_is_actually_enforced() {
        // Mirrors `a_narrower_caller_supplied_min_password_length_is_actually_enforced`:
        // proves `validate_register_request` reads its own
        // `max_handle_length` parameter (as `ServerState` resolves
        // it through the config cascade), not the module-level
        // `MAX_HANDLE_LENGTH` constant directly. A handle that
        // satisfies the compiled-in default must still be rejected
        // once the caller narrows the cap below its own length.
        let req = RegisterRequest {
            handle: "h".repeat(10),
            ..valid_request()
        };
        assert!(
            validate_register_request(
                &req,
                RegistrationLimits {
                    min_password_length: mmcp_auth::MIN_PASSWORD_LENGTH,
                    max_password_length: mmcp_auth::MAX_PASSWORD_LENGTH,
                    max_handle_length: MAX_HANDLE_LENGTH,
                }
            )
            .is_ok()
        );
        assert!(matches!(
            validate_register_request(
                &req,
                RegistrationLimits {
                    min_password_length: mmcp_auth::MIN_PASSWORD_LENGTH,
                    max_password_length: mmcp_auth::MAX_PASSWORD_LENGTH,
                    max_handle_length: 8,
                }
            ),
            Err(AuthHttpError::FieldTooLong {
                field: "handle",
                actual: 10,
                max: 8,
            })
        ));
    }

    #[test]
    fn login_request_within_the_bound_is_accepted() {
        let req = LoginRequest {
            handle: "alice".to_string(),
            password: "any-password".to_string(),
        };
        assert!(validate_login_request(&req, mmcp_auth::MAX_PASSWORD_LENGTH).is_ok());
    }

    #[test]
    fn login_request_over_the_bound_is_rejected() {
        let req = LoginRequest {
            handle: "alice".to_string(),
            password: "a".repeat(mmcp_auth::MAX_PASSWORD_LENGTH + 1),
        };
        assert!(matches!(
            validate_login_request(&req, mmcp_auth::MAX_PASSWORD_LENGTH),
            Err(AuthHttpError::FieldTooLong {
                field: "password",
                ..
            })
        ));
    }

    /// Build a ceremony-state map with one entry aged past
    /// [`PASSKEY_CEREMONY_TTL`] and one fresh entry, keyed by the ids
    /// returned as `(stale_id, fresh_id)`.
    fn map_with_a_stale_and_a_fresh_entry() -> (
        std::collections::HashMap<Uuid, (std::time::Instant, ())>,
        Uuid,
        Uuid,
    ) {
        let stale_id = Uuid::now_v7();
        let fresh_id = Uuid::now_v7();
        let mut map = std::collections::HashMap::new();
        let stale_insert_time = std::time::Instant::now()
            .checked_sub(PASSKEY_CEREMONY_TTL + std::time::Duration::from_secs(1))
            .expect("test clock has more than TTL + 1s of headroom behind now");
        map.insert(stale_id, (stale_insert_time, ()));
        map.insert(fresh_id, (std::time::Instant::now(), ()));
        (map, stale_id, fresh_id)
    }

    #[test]
    fn purge_stale_ceremonies_evicts_only_entries_past_the_ttl() {
        let (mut map, stale_id, fresh_id) = map_with_a_stale_and_a_fresh_entry();
        purge_stale_ceremonies(&mut map);
        assert!(
            !map.contains_key(&stale_id),
            "an entry older than PASSKEY_CEREMONY_TTL must be purged"
        );
        assert!(
            map.contains_key(&fresh_id),
            "an entry within PASSKEY_CEREMONY_TTL must survive the purge"
        );
    }

    #[test]
    fn take_ceremony_refuses_a_stale_entry_but_returns_a_fresh_one() {
        let (mut map, stale_id, fresh_id) = map_with_a_stale_and_a_fresh_entry();
        assert!(
            take_ceremony(&mut map, stale_id).is_none(),
            "a stale entry must not be handed back to finish the ceremony"
        );
        assert!(
            take_ceremony(&mut map, fresh_id).is_some(),
            "a fresh entry must still be usable to finish the ceremony"
        );
    }
}
