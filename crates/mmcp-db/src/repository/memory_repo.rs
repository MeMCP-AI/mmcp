//! Memory table repository.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter, Set,
};
use uuid::Uuid;

use crate::entities::memory::{ActiveModel, Column, Entity, MemoryKind, Model};
use crate::entities::memory_version;
use crate::error::DbError;

#[derive(Debug, Clone)]
pub struct NewMemory {
    pub id: Uuid,
    pub group_id: Uuid,
    pub slug: String,
    pub kind: MemoryKind,
    pub mandatory: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

pub async fn create(conn: &sea_orm::DatabaseConnection, new: NewMemory) -> Result<Model, DbError> {
    let active = ActiveModel {
        id: Set(new.id),
        group_id: Set(new.group_id),
        slug: Set(new.slug),
        kind: Set(new.kind),
        mandatory: Set(new.mandatory),
        latest_version: Set(None),
        created_at: Set(new.created_at),
        updated_at: Set(new.updated_at),
    };
    Ok(active.insert(conn).await?)
}

pub async fn find_by_id(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find_by_id(id).one(conn).await?)
}

pub async fn find_by_group_and_slug(
    conn: &sea_orm::DatabaseConnection,
    group_id: Uuid,
    slug: &str,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find()
        .filter(Column::GroupId.eq(group_id))
        .filter(Column::Slug.eq(slug))
        .one(conn)
        .await?)
}

pub async fn list_in_group(
    conn: &sea_orm::DatabaseConnection,
    group_id: Uuid,
) -> Result<Vec<Model>, DbError> {
    Ok(Entity::find()
        .filter(Column::GroupId.eq(group_id))
        .all(conn)
        .await?)
}

/// Count of memories in `group_id`, via `SELECT COUNT(*)` rather than
/// materializing every row: callers that only need the count (e.g.
/// `group_info`'s `memory_count`) should never pay for loading and
/// dropping every `Model` in the group just to call `.len()`.
pub async fn count_in_group(
    conn: &sea_orm::DatabaseConnection,
    group_id: Uuid,
) -> Result<u64, DbError> {
    Ok(Entity::find()
        .filter(Column::GroupId.eq(group_id))
        .count(conn)
        .await?)
}

/// Update the latest published version string and touch `updated_at`.
pub async fn set_latest_version(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
    version: String,
    updated_at: i64,
) -> Result<Model, DbError> {
    let existing = find_by_id(conn, id).await?.ok_or(DbError::NotFound)?;
    let mut active: ActiveModel = existing.into();
    active.latest_version = Set(Some(version));
    active.updated_at = Set(updated_at);
    Ok(active.update(conn).await?)
}

/// Insert a new published version row.
pub async fn record_version(
    conn: &sea_orm::DatabaseConnection,
    row: memory_version::Model,
) -> Result<memory_version::Model, DbError> {
    let active = memory_version::ActiveModel {
        id: Set(row.id),
        memory_id: Set(row.memory_id),
        version: Set(row.version),
        commit: Set(row.commit),
        author_id: Set(row.author_id),
        published_at: Set(row.published_at),
        summary: Set(row.summary),
    };
    Ok(active.insert(conn).await?)
}

pub async fn list_versions(
    conn: &sea_orm::DatabaseConnection,
    memory_id: Uuid,
) -> Result<Vec<memory_version::Model>, DbError> {
    Ok(memory_version::Entity::find()
        .filter(memory_version::Column::MemoryId.eq(memory_id))
        .all(conn)
        .await?)
}

/// Case-insensitive substring search over memory slugs.
///
/// Returns at most `limit` rows. Matching is performed in SQL via
/// `LIKE` with both sides lowered so SQLite and Postgres behave the
/// same without depending on per-backend collation settings.
pub async fn search_by_slug(
    conn: &sea_orm::DatabaseConnection,
    needle: &str,
    limit: u64,
) -> Result<Vec<Model>, DbError> {
    use sea_orm::{QueryOrder, QuerySelect};
    let pattern = format!("%{}%", needle.to_lowercase());
    Ok(Entity::find()
        .filter(
            sea_orm::sea_query::Expr::expr(sea_orm::sea_query::Func::lower(
                sea_orm::sea_query::Expr::col(Column::Slug),
            ))
            .like(pattern),
        )
        .order_by_asc(Column::Slug)
        .limit(limit)
        .all(conn)
        .await?)
}
