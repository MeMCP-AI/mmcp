//! User table repository.

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entities::user::{ActiveModel, Column, Entity, Model};
use crate::error::DbError;

/// Parameters for creating a new user row.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub id: Uuid,
    pub handle: String,
    pub display_name: Option<String>,
    pub password_hash: Option<String>,
    pub email: Option<String>,
    pub created_at: i64,
}

/// Insert a fresh user row. Returns the inserted model.
pub async fn create(conn: &sea_orm::DatabaseConnection, new: NewUser) -> Result<Model, DbError> {
    let active = ActiveModel {
        id: Set(new.id),
        handle: Set(new.handle),
        display_name: Set(new.display_name),
        password_hash: Set(new.password_hash),
        email: Set(new.email),
        created_at: Set(new.created_at),
    };
    Ok(active.insert(conn).await?)
}

/// Look up a user by primary key.
pub async fn find_by_id(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find_by_id(id).one(conn).await?)
}

/// Look up a user by their unique login handle.
pub async fn find_by_handle(
    conn: &sea_orm::DatabaseConnection,
    handle: &str,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find()
        .filter(Column::Handle.eq(handle))
        .one(conn)
        .await?)
}

/// Convenience: fetch by id or return `DbError::NotFound`.
pub async fn require(conn: &sea_orm::DatabaseConnection, id: Uuid) -> Result<Model, DbError> {
    find_by_id(conn, id).await?.ok_or(DbError::NotFound)
}

/// Update the display name and password hash fields on a user row.
pub async fn update_profile(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
    display_name: Option<String>,
    password_hash: Option<String>,
) -> Result<Model, DbError> {
    let mut active: ActiveModel = require(conn, id).await?.into();
    active.display_name = Set(display_name);
    if let Some(hash) = password_hash {
        active.password_hash = Set(Some(hash));
    }
    Ok(active.update(conn).await?)
}
