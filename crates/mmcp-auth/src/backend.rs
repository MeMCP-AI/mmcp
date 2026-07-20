//! `axum-login` backend adapter.
//!
//! Implements [`AuthnBackend`] over `mmcp-db` so the `axum-login`
//! session middleware can authenticate and retrieve users. Three
//! credential types are supported:
//!
//! - **Password**: traditional handle + password login.
//! - **OAuth**: provider callback carrying the provider slug and
//!   the external user ID resolved from the token exchange.
//! - **Passkey**: WebAuthn assertion result carrying the credential
//!   ID and the authenticator data.
//!
//! The backend holds a `DatabaseConnection` (cheaply cloneable in
//! SeaORM) and delegates credential checking to the primitives in
//! [`crate::password`] and [`crate::token`].

use std::fmt;

use axum_login::{AuthUser, AuthnBackend, UserId};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::error::AuthError;
use crate::password;
use mmcp_db::repository::{oauth_repo, passkey_repo, user_repo};

// ── AuthUser impl ───────────────────────────────────────────────

/// Wrapper around the database user model that carries the session
/// auth hash (password hash bytes). `axum-login` requires `Debug +
/// Clone + Send + Sync` on the user type and a stable auth hash
/// the session layer can verify on each request.
#[derive(Debug, Clone)]
pub struct MmcpUser {
    pub id: Uuid,
    pub handle: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    /// Stored as bytes so `session_auth_hash` can return a slice.
    auth_hash: Vec<u8>,
}

impl MmcpUser {
    pub fn from_db(model: mmcp_db::entities::user::Model) -> Self {
        let auth_hash = model
            .password_hash
            .as_deref()
            .unwrap_or("no-password")
            .as_bytes()
            .to_vec();
        Self {
            id: model.id,
            handle: model.handle,
            display_name: model.display_name,
            email: model.email,
            auth_hash,
        }
    }
}

impl AuthUser for MmcpUser {
    type Id = Uuid;

    fn id(&self) -> Self::Id {
        self.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        &self.auth_hash
    }
}

// ── Credentials ─────────────────────────────────────────────────

/// Credential types the backend can authenticate.
#[derive(Debug, Clone)]
pub enum Credentials {
    /// Handle + plaintext password.
    Password { handle: String, password: String },

    /// OAuth callback: the token exchange already happened and the
    /// handler resolved the provider-side user ID. The backend
    /// either finds the linked account or creates a new user.
    OAuth {
        provider: String,
        provider_user_id: String,
        email: Option<String>,
        access_token: Option<String>,
        refresh_token: Option<String>,
    },

    /// Passkey assertion: the WebAuthn ceremony completed and the
    /// handler verified the signature. The credential row ID
    /// identifies which user to load.
    Passkey { credential_row_id: Uuid },
}

// ── Backend ─────────────────────────────────────────────────────

/// The auth backend wired into the `axum-login` middleware.
#[derive(Clone)]
pub struct MmcpAuthBackend {
    conn: DatabaseConnection,
}

impl fmt::Debug for MmcpAuthBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MmcpAuthBackend").finish()
    }
}

impl MmcpAuthBackend {
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }
}

impl AuthnBackend for MmcpAuthBackend {
    type User = MmcpUser;
    type Credentials = Credentials;
    type Error = AuthError;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        match creds {
            Credentials::Password {
                handle,
                password: pw,
            } => {
                let user = user_repo::find_by_handle(&self.conn, &handle)
                    .await
                    .map_err(|e| AuthError::Claims(e.to_string()))?;
                let Some(user) = user else {
                    return Ok(None);
                };
                let Some(ref hash) = user.password_hash else {
                    return Ok(None);
                };
                match password::verify_password(&pw, hash) {
                    Ok(()) => Ok(Some(MmcpUser::from_db(user))),
                    Err(AuthError::InvalidCredentials) => Ok(None),
                    Err(other) => Err(other),
                }
            }

            Credentials::OAuth {
                provider,
                provider_user_id,
                email,
                access_token,
                refresh_token,
            } => {
                // Find existing link.
                let existing =
                    oauth_repo::find_by_provider(&self.conn, &provider, &provider_user_id)
                        .await
                        .map_err(|e| AuthError::Claims(e.to_string()))?;

                if let Some(link) = existing {
                    // Update tokens.
                    let now = jiff::Timestamp::now().as_millisecond();
                    let _ = oauth_repo::update_tokens(
                        &self.conn,
                        link.id,
                        access_token,
                        refresh_token,
                        now,
                    )
                    .await
                    .map_err(|e| AuthError::Claims(e.to_string()))?;
                    let user = user_repo::find_by_id(&self.conn, link.user_id)
                        .await
                        .map_err(|e| AuthError::Claims(e.to_string()))?;
                    return Ok(user.map(MmcpUser::from_db));
                }

                // First-time OAuth: auto-create user + link.
                let now = jiff::Timestamp::now().as_millisecond();
                let user_id = Uuid::now_v7();
                let handle = format!("{provider}_{provider_user_id}");
                let user = user_repo::create(
                    &self.conn,
                    user_repo::NewUser {
                        id: user_id,
                        handle,
                        display_name: None,
                        password_hash: None,
                        email: email.clone(),
                        created_at: now,
                    },
                )
                .await
                .map_err(|e| AuthError::Claims(e.to_string()))?;
                let _ = oauth_repo::create(
                    &self.conn,
                    oauth_repo::NewOauthAccount {
                        id: Uuid::now_v7(),
                        user_id,
                        provider,
                        provider_user_id,
                        email,
                        access_token,
                        refresh_token,
                        created_at: now,
                    },
                )
                .await
                .map_err(|e| AuthError::Claims(e.to_string()))?;
                Ok(Some(MmcpUser::from_db(user)))
            }

            Credentials::Passkey { credential_row_id } => {
                let cred = passkey_repo::find_by_id(&self.conn, credential_row_id)
                    .await
                    .map_err(|e| AuthError::Claims(e.to_string()))?;
                let Some(cred) = cred else {
                    return Ok(None);
                };
                let user = user_repo::find_by_id(&self.conn, cred.user_id)
                    .await
                    .map_err(|e| AuthError::Claims(e.to_string()))?;
                Ok(user.map(MmcpUser::from_db))
            }
        }
    }

    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<Self::User>, Self::Error> {
        let user = user_repo::find_by_id(&self.conn, *user_id)
            .await
            .map_err(|e| AuthError::Claims(e.to_string()))?;
        Ok(user.map(MmcpUser::from_db))
    }
}

/// Type alias used across the server for the auth session extractor.
pub type AuthSession = axum_login::AuthSession<MmcpAuthBackend>;
