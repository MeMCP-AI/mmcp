//! OAuth account link repository.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::entities::oauth_account::{ActiveModel, Column, Entity, Model};
use crate::error::DbError;
use crate::repository::credential_epoch::{
    CredentialChange, CredentialWrite, bump_credential_epoch, log_committed_credential_change,
};

/// Parameters for linking a new OAuth account.
#[derive(Debug, Clone)]
pub struct NewOauthAccount {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub provider_user_id: String,
    pub email: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub created_at: i64,
}

/// Insert a new OAuth account link and increment its owner's credential epoch.
///
/// Both writes commit together, and the returned owner carries the epoch the commit produced.
/// A missing owner is [`DbError::CredentialOwnerMissing`] and leaves no link row.
pub async fn create(
    conn: &sea_orm::DatabaseConnection,
    new: NewOauthAccount,
) -> Result<CredentialWrite<Model>, DbError> {
    let txn = conn.begin().await?;
    let owner = bump_credential_epoch(&txn, new.user_id, None).await?;
    let credential = insert(&txn, new).await?;
    txn.commit().await?;
    log_committed_credential_change(&owner, CredentialChange::OauthLinkAdded);
    Ok(CredentialWrite { credential, owner })
}

/// Insert the link row on `conn` without touching any epoch.
///
/// Only the first-time login path uses it: its user row is created in the same transaction at the initial epoch.
pub(crate) async fn insert<C: ConnectionTrait>(
    conn: &C,
    new: NewOauthAccount,
) -> Result<Model, DbError> {
    let active = ActiveModel {
        id: Set(new.id),
        user_id: Set(new.user_id),
        provider: Set(new.provider),
        provider_user_id: Set(new.provider_user_id),
        email: Set(new.email),
        access_token: Set(new.access_token),
        refresh_token: Set(new.refresh_token),
        created_at: Set(new.created_at),
        updated_at: Set(new.created_at),
    };
    Ok(active.insert(conn).await?)
}

/// Find a linked account by provider + provider user id. Returns
/// `None` if no link exists yet (first-time OAuth login for this
/// external account).
pub async fn find_by_provider(
    conn: &sea_orm::DatabaseConnection,
    provider: &str,
    provider_user_id: &str,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find()
        .filter(Column::Provider.eq(provider))
        .filter(Column::ProviderUserId.eq(provider_user_id))
        .one(conn)
        .await?)
}

/// All OAuth links for a given user.
pub async fn find_by_user(
    conn: &sea_orm::DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<Model>, DbError> {
    Ok(Entity::find()
        .filter(Column::UserId.eq(user_id))
        .all(conn)
        .await?)
}

/// Update tokens after a token refresh or re-authorization.
pub async fn update_tokens(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
    access_token: Option<String>,
    refresh_token: Option<String>,
    now: i64,
) -> Result<Model, DbError> {
    let row = Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or(DbError::NotFound)?;
    let mut active: ActiveModel = row.into();
    active.access_token = Set(access_token);
    active.refresh_token = Set(refresh_token);
    active.updated_at = Set(now);
    Ok(active.update(conn).await?)
}
