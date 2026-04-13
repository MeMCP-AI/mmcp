//! Org table repository.

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entities::org::{ActiveModel, Column, Entity, Model};
use crate::entities::org_member;
use crate::error::DbError;

/// Parameters for creating a new org.
#[derive(Debug, Clone)]
pub struct NewOrg {
    pub id: Uuid,
    pub slug: String,
    pub display_name: Option<String>,
    pub created_at: i64,
}

pub async fn create(
    conn: &sea_orm::DatabaseConnection,
    new: NewOrg,
) -> Result<Model, DbError> {
    let active = ActiveModel {
        id: Set(new.id),
        slug: Set(new.slug),
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

pub async fn find_by_slug(
    conn: &sea_orm::DatabaseConnection,
    slug: &str,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find()
        .filter(Column::Slug.eq(slug))
        .one(conn)
        .await?)
}

/// List every user that belongs to the given org with their role.
pub async fn list_members(
    conn: &sea_orm::DatabaseConnection,
    org_id: Uuid,
) -> Result<Vec<org_member::Model>, DbError> {
    Ok(org_member::Entity::find()
        .filter(org_member::Column::OrgId.eq(org_id))
        .all(conn)
        .await?)
}

/// Add a user to an org with a specific role.
pub async fn add_member(
    conn: &sea_orm::DatabaseConnection,
    membership: org_member::Model,
) -> Result<org_member::Model, DbError> {
    let active = org_member::ActiveModel {
        id: Set(membership.id),
        org_id: Set(membership.org_id),
        user_id: Set(membership.user_id),
        role: Set(membership.role),
        granted_at: Set(membership.granted_at),
    };
    Ok(active.insert(conn).await?)
}
