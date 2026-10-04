//! User table repository.

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::entities::oauth_account;
use crate::entities::user::{ActiveModel, Column, Entity, Model};
use crate::error::DbError;
use crate::repository::credential_epoch::{
    CredentialChange, INITIAL_CREDENTIAL_EPOCH, bump_credential_epoch,
    log_committed_credential_change,
};
use crate::repository::oauth_repo::{self, NewOauthAccount};

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
    insert(conn, new).await
}

/// Insert the user row at the initial credential epoch on `conn`.
async fn insert<C: ConnectionTrait>(conn: &C, new: NewUser) -> Result<Model, DbError> {
    let active = ActiveModel {
        id: Set(new.id),
        handle: Set(new.handle),
        display_name: Set(new.display_name),
        password_hash: Set(new.password_hash),
        email: Set(new.email),
        created_at: Set(new.created_at),
        credential_epoch: Set(INITIAL_CREDENTIAL_EPOCH),
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
///
/// A supplied password hash is a credential change: the epoch increments in the same transaction,
/// and the returned model carries the epoch the commit produced.
/// A display name alone leaves the epoch unchanged.
pub async fn update_profile(
    conn: &sea_orm::DatabaseConnection,
    id: Uuid,
    display_name: Option<String>,
    password_hash: Option<String>,
) -> Result<Model, DbError> {
    let Some(hash) = password_hash else {
        let mut active: ActiveModel = require(conn, id).await?.into();
        active.display_name = Set(display_name);
        return Ok(active.update(conn).await?);
    };
    let txn = conn.begin().await?;
    let bumped = bump_credential_epoch(&txn, id, None).await?;
    let mut active: ActiveModel = bumped.into();
    active.display_name = Set(display_name);
    active.password_hash = Set(Some(hash));
    let updated = active.update(&txn).await?;
    txn.commit().await?;
    log_committed_credential_change(&updated, CredentialChange::PasswordSet);
    Ok(updated)
}

/// Create a user and its first OAuth link in one transaction, both at the initial credential epoch.
///
/// The link always belongs to the created user, whatever `link.user_id` holds.
/// No increment happens: the account does not exist before this call, so no other session can hold it.
/// A failing link insert rolls the user back, so a first-time login never leaves a link-less account.
pub async fn create_with_oauth_link(
    conn: &sea_orm::DatabaseConnection,
    new: NewUser,
    mut link: NewOauthAccount,
) -> Result<(Model, oauth_account::Model), DbError> {
    link.user_id = new.id;
    let txn = conn.begin().await?;
    let user = insert(&txn, new).await?;
    let link = oauth_repo::insert(&txn, link).await?;
    txn.commit().await?;
    Ok((user, link))
}
