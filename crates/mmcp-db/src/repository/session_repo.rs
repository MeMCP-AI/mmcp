//! Session and memory-read tracking.

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entities::memory_read;
use crate::entities::session::{ActiveModel, Column, Entity, Model};
use crate::error::DbError;

#[derive(Debug, Clone)]
pub struct NewSession {
    pub session_id: String,
    pub user_id: Option<Uuid>,
    pub project_uuid: Option<Uuid>,
    pub transcript_path: Option<String>,
    pub started_at: i64,
}

pub async fn upsert(conn: &sea_orm::DatabaseConnection, new: NewSession) -> Result<Model, DbError> {
    if let Some(existing) = find(conn, &new.session_id).await? {
        let mut active: ActiveModel = existing.into();
        active.user_id = Set(new.user_id);
        active.project_uuid = Set(new.project_uuid);
        active.transcript_path = Set(new.transcript_path);
        active.last_seen_at = Set(new.started_at);
        Ok(active.update(conn).await?)
    } else {
        let active = ActiveModel {
            session_id: Set(new.session_id),
            user_id: Set(new.user_id),
            project_uuid: Set(new.project_uuid),
            turn_counter: Set(0),
            transcript_path: Set(new.transcript_path),
            transcript_signature: Set(None),
            post_compaction: Set(false),
            started_at: Set(new.started_at),
            last_seen_at: Set(new.started_at),
        };
        Ok(active.insert(conn).await?)
    }
}

pub async fn find(
    conn: &sea_orm::DatabaseConnection,
    session_id: &str,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find_by_id(session_id.to_string()).one(conn).await?)
}

/// Atomically bump the session's turn counter and return the new value.
pub async fn bump_turn(
    conn: &sea_orm::DatabaseConnection,
    session_id: &str,
    now: i64,
) -> Result<i32, DbError> {
    let existing = find(conn, session_id).await?.ok_or(DbError::NotFound)?;
    let next = existing.turn_counter + 1;
    let mut active: ActiveModel = existing.into();
    active.turn_counter = Set(next);
    active.last_seen_at = Set(now);
    active.update(conn).await?;
    Ok(next)
}

/// Record that a compaction was detected and clear the read flags the
/// caller cares about.
pub async fn mark_post_compaction(
    conn: &sea_orm::DatabaseConnection,
    session_id: &str,
    signature: Option<String>,
    now: i64,
) -> Result<Model, DbError> {
    let existing = find(conn, session_id).await?.ok_or(DbError::NotFound)?;
    let mut active: ActiveModel = existing.into();
    active.post_compaction = Set(true);
    active.transcript_signature = Set(signature);
    active.last_seen_at = Set(now);
    Ok(active.update(conn).await?)
}

/// Clear the post-compaction flag once the caller has re-read their
/// mandatory memories.
pub async fn clear_post_compaction(
    conn: &sea_orm::DatabaseConnection,
    session_id: &str,
) -> Result<Model, DbError> {
    let existing = find(conn, session_id).await?.ok_or(DbError::NotFound)?;
    let mut active: ActiveModel = existing.into();
    active.post_compaction = Set(false);
    Ok(active.update(conn).await?)
}

/// Record that a session read a specific memory during a specific turn.
pub async fn record_read(
    conn: &sea_orm::DatabaseConnection,
    row: memory_read::Model,
) -> Result<memory_read::Model, DbError> {
    let active = memory_read::ActiveModel {
        id: Set(row.id),
        session_id: Set(row.session_id),
        memory_id: Set(row.memory_id),
        turn: Set(row.turn),
        version: Set(row.version),
        verified: Set(row.verified),
        read_at: Set(row.read_at),
    };
    Ok(active.insert(conn).await?)
}

/// List reads the session has recorded for a given memory.
pub async fn reads_for_memory(
    conn: &sea_orm::DatabaseConnection,
    session_id: &str,
    memory_id: Uuid,
) -> Result<Vec<memory_read::Model>, DbError> {
    Ok(memory_read::Entity::find()
        .filter(memory_read::Column::SessionId.eq(session_id.to_string()))
        .filter(memory_read::Column::MemoryId.eq(memory_id))
        .all(conn)
        .await?)
}

/// True if the session has at least one recorded read for the memory.
pub async fn has_read(
    conn: &sea_orm::DatabaseConnection,
    session_id: &str,
    memory_id: Uuid,
) -> Result<bool, DbError> {
    Ok(!reads_for_memory(conn, session_id, memory_id)
        .await?
        .is_empty())
}

// The `Column` import is required for filter predicates above.
#[allow(dead_code)]
fn _use_column(_: Column) {}
