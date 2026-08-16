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

#[cfg(test)]
use crate::defaults::MAX_HANDLE_LENGTH;
use crate::defaults::{MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS, SUFFIX_RESERVE_BYTES};
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
    /// Effective handle length bound, captured at construction.
    /// `AuthnBackend::authenticate`'s signature is fixed by the trait.
    /// This field is the only route by which the bound reaches [`provision_oauth_handle`].
    max_handle_length: usize,
}

impl fmt::Debug for MmcpAuthBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MmcpAuthBackend").finish()
    }
}

impl MmcpAuthBackend {
    /// Build the backend, capturing the caller's already-resolved
    /// effective `max_handle_length` for use by the OAuth
    /// JIT-provisioning path.
    pub fn new(conn: DatabaseConnection, max_handle_length: usize) -> Self {
        Self {
            conn,
            max_handle_length,
        }
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
                let handle = provision_oauth_handle(
                    &self.conn,
                    &provider,
                    &provider_user_id,
                    self.max_handle_length,
                )
                .await?;
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
/// first (bounded to `max_handle_length` bytes, the caller's already
/// config-resolved effective bound; see [`MAX_HANDLE_LENGTH`]'s doc
/// comment for the cascade it comes from), then falls back to
/// numeric-suffixed candidates when it collides with an existing
/// user. The collision path exists because `/auth/register` places
/// no namespace restriction on `handle`: an attacker who
/// pre-registers the literal string a real OAuth user would be
/// assigned could otherwise permanently deny that user their first
/// OAuth login.
///
/// Suffixed candidates truncate from a base already shortened by
/// [`SUFFIX_RESERVE_BYTES`], not from the full-length `base`
/// returned when unsuffixed: see that constant's doc comment for why
/// truncating the suffix onto an already-capped base would otherwise
/// collapse every candidate back onto `base` itself.
async fn provision_oauth_handle(
    conn: &DatabaseConnection,
    provider: &str,
    provider_user_id: &str,
    max_handle_length: usize,
) -> Result<String, AuthError> {
    let base =
        truncate_to_byte_length(&format!("{provider}_{provider_user_id}"), max_handle_length);
    if user_repo::find_by_handle(conn, &base)
        .await
        .map_err(|source| AuthError::UserLookup {
            handle: base.clone(),
            source,
        })?
        .is_none()
    {
        return Ok(base);
    }
    // `saturating_sub` is a defense-in-depth floor: the config
    // cascade in `mmcp_server::config::resolve_max_handle_length`
    // already rejects any effective bound below
    // `MIN_VIABLE_MAX_HANDLE_LENGTH` before it ever reaches this
    // function, but this arithmetic stays underflow-safe on its own
    // for any other caller, present or future, that resolves
    // `max_handle_length` some other way.
    let suffix_base = truncate_to_byte_length(
        &format!("{provider}_{provider_user_id}"),
        max_handle_length.saturating_sub(SUFFIX_RESERVE_BYTES),
    );
    for suffix in 2..=MAX_OAUTH_HANDLE_COLLISION_ATTEMPTS {
        let candidate =
            truncate_to_byte_length(&format!("{suffix_base}-{suffix}"), max_handle_length);
        if user_repo::find_by_handle(conn, &candidate)
            .await
            .map_err(|source| AuthError::UserLookup {
                handle: candidate.clone(),
                source,
            })?
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
        let handle = provision_oauth_handle(&conn, "github", "1001", MAX_HANDLE_LENGTH)
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

        let handle = provision_oauth_handle(&conn, "github", "1001", MAX_HANDLE_LENGTH)
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

        let handle = provision_oauth_handle(&conn, "github", "1001", MAX_HANDLE_LENGTH)
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

        let err = provision_oauth_handle(&conn, "github", "1001", MAX_HANDLE_LENGTH)
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

    /// Regression guard for the base-at-cap collapse: a
    /// `provider_user_id` long enough that `base` is already
    /// truncated to exactly `MAX_HANDLE_LENGTH` bytes. Before
    /// reserving suffix room, `format!("{base}-{suffix}")` truncated
    /// straight back down to `base` on every retry, so every
    /// candidate collided with the known-taken base handle and the
    /// real OAuth user could never log in. Asserts both properties
    /// [`SUFFIX_RESERVE_BYTES`] exists to guarantee: a suffixed
    /// candidate is distinct from the base, and successive candidates
    /// are distinct from each other rather than repeating the same
    /// collapsed string.
    #[tokio::test]
    async fn provision_oauth_handle_keeps_suffix_candidates_distinct_when_base_hits_the_cap() {
        let conn = test_db().await;
        let provider_user_id = "1".repeat(100);
        let base =
            truncate_to_byte_length(&format!("github_{provider_user_id}"), MAX_HANDLE_LENGTH);
        assert_eq!(
            base.len(),
            MAX_HANDLE_LENGTH,
            "test setup must actually push the base to the cap"
        );
        create_user(&conn, &base).await;

        let first = provision_oauth_handle(&conn, "github", &provider_user_id, MAX_HANDLE_LENGTH)
            .await
            .expect("provision handle");
        assert_ne!(
            first, base,
            "a suffixed candidate must never collapse back onto the taken base handle"
        );

        // Squat the first candidate too, so the next retry must be
        // distinct from BOTH the base and the first candidate, not a
        // re-truncated repeat of either.
        create_user(&conn, &first).await;
        let second = provision_oauth_handle(&conn, "github", &provider_user_id, MAX_HANDLE_LENGTH)
            .await
            .expect("provision handle");
        assert_ne!(second, base);
        assert_ne!(
            second, first,
            "successive suffix candidates must differ from each other"
        );
    }

    /// `provision_oauth_handle` stays underflow-safe for a caller that bypasses the config cascade.
    /// `max_handle_length = 2` is below `SUFFIX_RESERVE_BYTES` (3), so the base handle collides
    /// deliberately to force the collision-retry path (the only path
    /// that ever computes `max_handle_length - SUFFIX_RESERVE_BYTES`)
    /// to actually run.
    #[tokio::test]
    async fn provision_oauth_handle_never_underflows_on_a_too_small_max_handle_length() {
        let conn = test_db().await;
        // "g_1" truncated to 2 bytes is "g_"; pre-registering it
        // forces the retry path that subtracts SUFFIX_RESERVE_BYTES.
        create_user(&conn, "g_").await;

        let result = provision_oauth_handle(&conn, "g", "1", 2).await;

        assert!(
            matches!(
                result,
                Ok(_) | Err(AuthError::HandleAllocationExhausted { .. })
            ),
            "a too-small max_handle_length must produce a well-defined outcome (a candidate \
             handle or a clean exhaustion error), never panic or wrap: {result:?}"
        );
    }
}
