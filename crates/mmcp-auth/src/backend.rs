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

/// Maximum accepted length of an OAuth-provisioned handle, in bytes.
/// Mirrors the password-registration path's handle bound
/// (`mmcp_server::routes::auth::MAX_HANDLE_LENGTH`), kept as its own
/// constant because the two paths live in different crates and
/// cannot share one definition directly; a JIT-created account must
/// never exceed what the register endpoint would ever accept.
const MAX_OAUTH_HANDLE_LENGTH: usize = 64;

/// Maximum number of numeric-suffix retries when the preferred
/// OAuth handle is already taken by an unrelated account, before
/// giving up with [`AuthError::HandleAllocationExhausted`].
const MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS: u32 = 20;

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
                let handle =
                    provision_oauth_handle(&self.conn, &provider, &provider_user_id).await?;
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

/// Resolve a free handle for a first-time OAuth login.
///
/// Tries the preferred `{provider}_{provider_user_id}` identifier
/// first (bounded to [`MAX_OAUTH_HANDLE_LENGTH`] bytes), then falls
/// back to numeric-suffixed candidates when it collides with an
/// existing user. The collision path exists because
/// `/auth/register` places no namespace restriction on `handle`: an
/// attacker who pre-registers the literal string a real OAuth user
/// would be assigned could otherwise permanently deny that user
/// their first OAuth login.
async fn provision_oauth_handle(
    conn: &DatabaseConnection,
    provider: &str,
    provider_user_id: &str,
) -> Result<String, AuthError> {
    let base = truncate_to_byte_length(
        &format!("{provider}_{provider_user_id}"),
        MAX_OAUTH_HANDLE_LENGTH,
    );
    if user_repo::find_by_handle(conn, &base)
        .await
        .map_err(|e| AuthError::Claims(e.to_string()))?
        .is_none()
    {
        return Ok(base);
    }
    for suffix in 2..=MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS {
        let candidate =
            truncate_to_byte_length(&format!("{base}-{suffix}"), MAX_OAUTH_HANDLE_LENGTH);
        if user_repo::find_by_handle(conn, &candidate)
            .await
            .map_err(|e| AuthError::Claims(e.to_string()))?
            .is_none()
        {
            return Ok(candidate);
        }
    }
    Err(AuthError::HandleAllocationExhausted {
        provider: provider.to_string(),
        attempts: MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS,
    })
}

/// Truncate `value` to at most `max_len` bytes, backing off to the
/// nearest earlier UTF-8 char boundary so a handle derived from
/// provider-controlled input can never split a multibyte character
/// or exceed the register endpoint's own handle length bound.
fn truncate_to_byte_length(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }
    let mut end = max_len;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// Type alias used across the server for the auth session extractor.
pub type AuthSession = axum_login::AuthSession<MmcpAuthBackend>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_to_byte_length_keeps_short_values_unchanged() {
        assert_eq!(truncate_to_byte_length("github_123", 64), "github_123");
    }

    #[test]
    fn truncate_to_byte_length_caps_long_values_without_splitting_chars() {
        let long = "a".repeat(100);
        let truncated = truncate_to_byte_length(&long, 64);
        assert_eq!(truncated.len(), 64);
    }

    #[test]
    fn truncate_to_byte_length_backs_off_to_a_char_boundary() {
        // Each 'e' with acute accent is 2 bytes in UTF-8; a hard cut
        // at byte 5 would land mid-character.
        let value = "é".repeat(10);
        let truncated = truncate_to_byte_length(&value, 5);
        assert!(truncated.len() <= 5);
        assert!(String::from_utf8(truncated.into_bytes()).is_ok());
    }

    /// In-memory, migrated database connection for the
    /// `provision_oauth_handle` collision tests below: real
    /// `user_repo::find_by_handle` lookups against real rows, not a
    /// mock.
    async fn test_db() -> DatabaseConnection {
        let db = mmcp_db::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        db.migrate().await.expect("run migrations");
        db.into_connection()
    }

    async fn create_user(conn: &DatabaseConnection, handle: &str) {
        user_repo::create(
            conn,
            user_repo::NewUser {
                id: Uuid::now_v7(),
                handle: handle.to_string(),
                display_name: None,
                password_hash: None,
                email: None,
                created_at: 0,
            },
        )
        .await
        .expect("create seed user");
    }

    #[tokio::test]
    async fn provision_oauth_handle_returns_the_preferred_handle_when_free() {
        let conn = test_db().await;
        let handle = provision_oauth_handle(&conn, "github", "1001")
            .await
            .expect("provision handle");
        assert_eq!(handle, "github_1001");
    }

    #[tokio::test]
    async fn provision_oauth_handle_retries_with_a_numeric_suffix_on_collision() {
        let conn = test_db().await;
        // Pre-register the exact handle a real OAuth login would be
        // assigned, exactly as an attacker could do today via
        // `/auth/register` (no namespace restriction on `handle`).
        create_user(&conn, "github_1001").await;

        let handle = provision_oauth_handle(&conn, "github", "1001")
            .await
            .expect("provision handle");
        assert_eq!(
            handle, "github_1001-2",
            "a squatted base handle must fall through to a numeric-suffix candidate, \
             not deny the real OAuth user their first login"
        );
    }

    #[tokio::test]
    async fn provision_oauth_handle_skips_every_taken_suffix() {
        let conn = test_db().await;
        create_user(&conn, "github_1001").await;
        create_user(&conn, "github_1001-2").await;

        let handle = provision_oauth_handle(&conn, "github", "1001")
            .await
            .expect("provision handle");
        assert_eq!(handle, "github_1001-3");
    }

    #[tokio::test]
    async fn provision_oauth_handle_exhausts_after_every_attempt_collides() {
        let conn = test_db().await;
        create_user(&conn, "github_1001").await;
        for suffix in 2..=MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS {
            create_user(&conn, &format!("github_1001-{suffix}")).await;
        }

        let err = provision_oauth_handle(&conn, "github", "1001")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AuthError::HandleAllocationExhausted {
                attempts: MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS,
                ..
            }
        ));
    }
}
