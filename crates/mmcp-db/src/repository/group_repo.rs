//! Group table repository.

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entities::group::{ActiveModel, Column, Entity, Model, OwnerKind};
use crate::entities::membership;
use crate::error::DbError;

#[derive(Debug, Clone)]
pub struct NewGroup {
    pub id: Uuid,
    pub slug: String,
    pub owner_kind: OwnerKind,
    pub owner_id: Uuid,
    pub display_name: Option<String>,
    pub created_at: i64,
}

pub async fn create(conn: &sea_orm::DatabaseConnection, new: NewGroup) -> Result<Model, DbError> {
    let active = ActiveModel {
        id: Set(new.id),
        slug: Set(new.slug),
        owner_kind: Set(new.owner_kind),
        owner_id: Set(new.owner_id),
        display_name: Set(new.display_name),
        created_at: Set(new.created_at),
    };
    Ok(active.insert(conn).await?)
}

pub async fn find_by_id(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find_by_id(id).one(conn).await?)
}

/// List every group owned by the given user or org.
pub async fn list_by_owner(
    conn: &sea_orm::DatabaseConnection,
    owner_kind: OwnerKind,
    owner_id: Uuid,
) -> Result<Vec<Model>, DbError> {
    Ok(Entity::find()
        .filter(Column::OwnerKind.eq(owner_kind))
        .filter(Column::OwnerId.eq(owner_id))
        .all(conn)
        .await?)
}

/// List all membership rows attached to a group.
pub async fn list_memberships(
    conn: &sea_orm::DatabaseConnection,
    group_id: Uuid,
) -> Result<Vec<membership::Model>, DbError> {
    Ok(membership::Entity::find()
        .filter(membership::Column::GroupId.eq(group_id))
        .all(conn)
        .await?)
}

pub async fn add_membership(
    conn: &sea_orm::DatabaseConnection,
    row: membership::Model,
) -> Result<membership::Model, DbError> {
    let active = membership::ActiveModel {
        id: Set(row.id),
        group_id: Set(row.group_id),
        principal_kind: Set(row.principal_kind),
        principal_id: Set(row.principal_id),
        role: Set(row.role),
        granted_at: Set(row.granted_at),
    };
    Ok(active.insert(conn).await?)
}
