//! Passkey credential repository.

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entities::passkey_credential::{ActiveModel, Column, Entity, Model};
use crate::error::DbError;

/// Insert a new passkey credential.
pub async fn create(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
    user_id: Uuid,
    name: String,
    credential_json: String,
    created_at: i64,
) -> Result<Model, DbError> {
    let active = ActiveModel {
        id: Set(id),
        user_id: Set(user_id),
        name: Set(name),
        credential_json: Set(credential_json),
        created_at: Set(created_at),
        last_used_at: Set(None),
    };
    Ok(active.insert(conn).await?)
}

/// All passkey credentials belonging to a user.
pub async fn find_by_user(
    conn: &sea_orm::DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<Model>, DbError> {
    Ok(Entity::find()
        .filter(Column::UserId.eq(user_id))
        .all(conn)
        .await?)
}

/// Find a single credential by its primary key.
pub async fn find_by_id(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find_by_id(id).one(conn).await?)
}

/// Update the `credential_json` and `last_used_at` after a
/// successful authentication (the counter inside the credential
/// advances on each use).
pub async fn update_after_auth(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
    credential_json: String,
    now: i64,
) -> Result<Model, DbError> {
    let row = find_by_id(conn, id)
        .await?
        .ok_or(DbError::NotFound)?;
    let mut active: ActiveModel = row.into();
    active.credential_json = Set(credential_json);
    active.last_used_at = Set(Some(now));
    Ok(active.update(conn).await?)
}

/// Delete a credential by id.
pub async fn delete(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
) -> Result<(), DbError> {
    Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}
