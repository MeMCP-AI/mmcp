//! Passkey credential repository.

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait};
use uuid::Uuid;

use crate::entities::passkey_credential::{ActiveModel, Column, Entity, Model};
use crate::entities::user;
use crate::error::DbError;
use crate::repository::credential_epoch::{
    CredentialChange, CredentialWrite, bump_credential_epoch,
};

/// Insert a new passkey credential and increment its owner's credential epoch.
///
/// Both writes commit together, and the returned owner carries the epoch the commit produced.
/// A missing owner is [`DbError::CredentialOwnerMissing`] and leaves no passkey row.
pub async fn create(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
    user_id: Uuid,
    name: String,
    credential_json: String,
    created_at: i64,
) -> Result<CredentialWrite<Model>, DbError> {
    let txn = conn.begin().await?;
    let owner = bump_credential_epoch(&txn, user_id, CredentialChange::PasskeyAdded).await?;
    let active = ActiveModel {
        id: Set(id),
        user_id: Set(user_id),
        name: Set(name),
        credential_json: Set(credential_json),
        created_at: Set(created_at),
        last_used_at: Set(None),
    };
    let credential = active.insert(&txn).await?;
    txn.commit().await?;
    Ok(CredentialWrite { credential, owner })
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
    let row = find_by_id(conn, id).await?.ok_or(DbError::NotFound)?;
    let mut active: ActiveModel = row.into();
    active.credential_json = Set(credential_json);
    active.last_used_at = Set(Some(now));
    Ok(active.update(conn).await?)
}

/// Delete a credential by id and increment its owner's credential epoch.
///
/// Returns the owner as the commit left it, or `None` when no row was deleted, in which case no epoch changes.
pub async fn delete(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
) -> Result<Option<user::Model>, DbError> {
    let txn = conn.begin().await?;
    let Some(row) = Entity::find_by_id(id).one(&txn).await? else {
        return Ok(None);
    };
    // The increment comes first so concurrent changes of one user serialize on the user row.
    // A delete that removes no row rolls the increment back by dropping the transaction.
    let owner = bump_credential_epoch(&txn, row.user_id, CredentialChange::PasskeyRemoved).await?;
    let deleted = Entity::delete_many()
        .filter(Column::Id.eq(id))
        .exec(&txn)
        .await?;
    if deleted.rows_affected != 1 {
        return Ok(None);
    }
    txn.commit().await?;
    Ok(Some(owner))
}
