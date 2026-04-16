//! OAuth account link repository.

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entities::oauth_account::{ActiveModel, Column, Entity, Model};
use crate::error::DbError;

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

/// Insert a new OAuth account link.
pub async fn create(
    conn: &sea_orm::DatabaseConnection,
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
