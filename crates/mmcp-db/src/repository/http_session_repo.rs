//! HTTP session repository.
//!
//! Every function takes the SHA-256 digest key of a session id, never a raw id.
//! Instants are epoch milliseconds supplied by the caller, so tests drive time without a clock seam.

use sea_orm::sea_query::{Expr, OnConflict, Query};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, TryInsertResult};

use crate::entities::http_session::{ActiveModel, Column, Entity, Model};
use crate::error::DbError;

/// Outcome of [`insert_if_absent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpSessionInsertOutcome {
    /// The row was written.
    Inserted,
    /// A row with this key already exists and was left untouched.
    IdTaken,
}

/// Insert `row` unless its key already exists.
pub async fn insert_if_absent(
    conn: &DatabaseConnection,
    row: Model,
) -> Result<HttpSessionInsertOutcome, DbError> {
    let outcome = Entity::insert(ActiveModel::from(row))
        .on_conflict_do_nothing()
        .exec(conn)
        .await?;
    Ok(match outcome {
        TryInsertResult::Inserted(_) => HttpSessionInsertOutcome::Inserted,
        TryInsertResult::Conflicted | TryInsertResult::Empty => HttpSessionInsertOutcome::IdTaken,
    })
}

/// Insert `row`, or overwrite `data` and `expires_at` of the row holding the same key.
pub async fn upsert(conn: &DatabaseConnection, row: Model) -> Result<(), DbError> {
    Entity::insert(ActiveModel::from(row))
        .on_conflict(
            OnConflict::column(Column::SessionIdSha256)
                .update_columns([Column::Data, Column::ExpiresAt])
                .to_owned(),
        )
        .exec_without_returning(conn)
        .await?;
    Ok(())
}

/// Overwrite `data` of the row under `session_id_sha256` only while it still holds `expected_data`.
///
/// Returns whether the row changed.
/// A concurrent writer that changed the row first makes this return `false`, which lets the caller re-read and retry.
pub async fn replace_data_if_unchanged(
    conn: &DatabaseConnection,
    session_id_sha256: &str,
    expected_data: &str,
    new_data: String,
) -> Result<bool, DbError> {
    let updated = Entity::update_many()
        .col_expr(Column::Data, Expr::value(new_data))
        .filter(Column::SessionIdSha256.eq(session_id_sha256))
        .filter(Column::Data.eq(expected_data))
        .exec(conn)
        .await?;
    Ok(updated.rows_affected == 1)
}

/// The row under `session_id_sha256`, only when its expiry is strictly after `now`.
pub async fn find_unexpired(
    conn: &DatabaseConnection,
    session_id_sha256: &str,
    now: i64,
) -> Result<Option<Model>, DbError> {
    Ok(Entity::find_by_id(session_id_sha256.to_owned())
        .filter(Column::ExpiresAt.gt(now))
        .one(conn)
        .await?)
}

/// Delete the row under `session_id_sha256`; an absent row is not an error.
pub async fn delete(conn: &DatabaseConnection, session_id_sha256: &str) -> Result<(), DbError> {
    Entity::delete_by_id(session_id_sha256.to_owned())
        .exec(conn)
        .await?;
    Ok(())
}

/// Delete at most `batch_size` rows whose expiry is at or before `now`, returning the deleted count.
///
/// A key subquery carries the limit, because `DELETE ... LIMIT` is not portable across SQLite and Postgres.
/// The outer `DELETE` repeats the expiry predicate.
/// On Postgres a row extended after the subquery ran, and waited on by this statement, is re-evaluated against it.
/// That row survives.
pub async fn delete_expired_batch(
    conn: &DatabaseConnection,
    now: i64,
    batch_size: u64,
) -> Result<u64, DbError> {
    let expired_keys = Query::select()
        .column(Column::SessionIdSha256)
        .from(Entity)
        .and_where(Column::ExpiresAt.lte(now))
        .limit(batch_size)
        .to_owned();
    let deleted = Entity::delete_many()
        .filter(Column::SessionIdSha256.in_subquery(expired_keys))
        .filter(Column::ExpiresAt.lte(now))
        .exec(conn)
        .await?;
    Ok(deleted.rows_affected)
}
