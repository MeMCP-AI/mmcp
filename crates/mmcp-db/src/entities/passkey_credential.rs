//! `passkey_credentials` table: WebAuthn/FIDO2 public key credentials.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Row in the `passkey_credentials` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "passkey_credentials")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    pub user_id: Uuid,

    /// Human-readable label chosen by the user during registration
    /// (e.g. "MacBook Touch ID", "YubiKey 5").
    pub name: String,

    /// JSON-serialized `webauthn_rs::prelude::Passkey`. Stored as
    /// opaque text so the DB layer does not depend on `webauthn-rs`.
    #[sea_orm(column_type = "Text")]
    pub credential_json: String,

    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_delete = "Cascade"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
