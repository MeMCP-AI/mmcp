//! The per-user credential epoch: a counter every credential change increments.
//!
//! The epoch feeds the session auth hash, so a credential change signs the user's other sessions out.
//! Every writer of a credential passes through [`bump_credential_epoch`] inside its own transaction.
//! After its commit, each writer reports the change through [`log_committed_credential_change`].

use sea_orm::sea_query::{Expr, ExprTrait};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::entities::user::{Column, Entity, Model};
use crate::error::DbError;

/// Epoch of a user no credential change has touched.
///
/// The `users.credential_epoch` migration default and every user creation share this value.
pub const INITIAL_CREDENTIAL_EPOCH: i64 = 0;

/// The credential changes that increment the epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialChange {
    /// A password hash was written.
    PasswordSet,
    /// A passkey was added.
    PasskeyAdded,
    /// A passkey was removed.
    PasskeyRemoved,
    /// An OAuth account was linked.
    OauthLinkAdded,
}

/// A credential row written together with its owner as the write committed it.
///
/// `owner` carries the epoch the write produced, so a login that follows the write in the same request signs in with it.
#[derive(Debug, Clone)]
pub struct CredentialWrite<T> {
    pub credential: T,
    pub owner: Model,
}

/// Increment the epoch of `user_id` and return the owner row as it stands after the increment.
///
/// `conn` is the caller's transaction: the increment commits or rolls back with the credential write it accompanies.
/// The increment runs before the credential write, so concurrent credential changes of one user serialize on the user row.
/// With `verified_epoch`, the increment applies only while the owner is still at that epoch.
/// A caller acting for a session passes the epoch the session was verified under,
/// so a credential change committed by another session in between refuses this write
/// and the acting session is never re-stamped past a change that should have signed it out.
/// A user that does not exist is [`DbError::CredentialOwnerMissing`],
/// and a moved epoch is [`DbError::CredentialEpochChanged`].
pub(crate) async fn bump_credential_epoch<C: ConnectionTrait>(
    conn: &C,
    user_id: Uuid,
    verified_epoch: Option<i64>,
) -> Result<Model, DbError> {
    let mut update = Entity::update_many()
        .col_expr(
            Column::CredentialEpoch,
            Expr::col(Column::CredentialEpoch).add(1),
        )
        .filter(Column::Id.eq(user_id));
    if let Some(epoch) = verified_epoch {
        update = update.filter(Column::CredentialEpoch.eq(epoch));
    }
    let mut updated = update.exec_with_returning(conn).await?;
    if let Some(owner) = updated.pop() {
        return Ok(owner);
    }
    match (Entity::find_by_id(user_id).one(conn).await?, verified_epoch) {
        (Some(_), Some(verified_epoch)) => Err(DbError::CredentialEpochChanged {
            user_id,
            verified_epoch,
        }),
        _ => Err(DbError::CredentialOwnerMissing { user_id }),
    }
}

/// Report a credential change that has committed, with the epoch it produced.
///
/// Called after the commit, so a rolled back write never reports a change that did not happen.
pub(crate) fn log_committed_credential_change(owner: &Model, change: CredentialChange) {
    tracing::info!(
        user_id = %owner.id,
        change = ?change,
        epoch = owner.credential_epoch,
        "credential changed, the user's other sessions are signed out"
    );
}
