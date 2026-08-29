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

/// List memories in `group_id`, optionally narrowed by mandatory
/// flag and/or kind before the row ever leaves SQLite.
///
/// Both filters are pushed into the `WHERE` clause rather than
/// applied by the caller after `.all()` materializes every row:
/// `group_id` alone can return an unbounded number of rows for a
/// large group, and filtering in Rust still pays that full fetch
/// cost. An empty `kinds` disables the kind filter entirely (matches
/// every kind), mirroring the "no filter" meaning callers already
/// give an empty kind list.
pub async fn list_in_group(
    conn: &sea_orm::DatabaseConnection,
    group_id: Uuid,
    only_mandatory: Option<bool>,
    kinds: &[MemoryKind],
) -> Result<Vec<Model>, DbError> {
    let mut query = Entity::find().filter(Column::GroupId.eq(group_id));
    if let Some(mandatory) = only_mandatory {
        query = query.filter(Column::Mandatory.eq(mandatory));
    }
    if !kinds.is_empty() {
        query = query.filter(Column::Kind.is_in(kinds.iter().copied()));
    }
    Ok(query.all(conn).await?)
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

/// List every published version row for `memory_id`.
///
/// `limit` is opt-in: `None` preserves the historical unbounded
/// behavior every existing caller relies on, matching the per-caller
/// approach `walk_history`'s own `limit` parameter took (limiting is
/// something only a caller that has decided it wants pagination
/// requests, never a silently-changed default).
pub async fn list_versions(
    conn: &sea_orm::DatabaseConnection,
    memory_id: Uuid,
    limit: Option<u64>,
) -> Result<Vec<memory_version::Model>, DbError> {
    use sea_orm::QuerySelect;
    let mut query =
        memory_version::Entity::find().filter(memory_version::Column::MemoryId.eq(memory_id));
    if let Some(limit) = limit {
        query = query.limit(limit);
    }
    Ok(query.all(conn).await?)
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
