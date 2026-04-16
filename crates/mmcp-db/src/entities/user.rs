//! `users` table: mmcp end user accounts.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Row in the `users` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users")]
pub struct Model {
    /// UUIDv7 primary key.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    /// Unique lowercase login handle.
    #[sea_orm(unique, indexed)]
    pub handle: String,

    /// Optional display name shown in the WebUI.
    pub display_name: Option<String>,

    /// Argon2 password hash. Nullable so accounts can be
    /// passkey-only or OAuth-only.
    pub password_hash: Option<String>,

    /// Primary email address used for notifications and recovery.
    /// Nullable for local-only offline accounts.
    pub email: Option<String>,

    /// Account creation time in milliseconds since Unix epoch.
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::org_member::Entity")]
    OrgMemberships,

    #[sea_orm(has_many = "super::memory_version::Entity")]
    AuthoredVersions,

    #[sea_orm(has_many = "super::session::Entity")]
    Sessions,

    #[sea_orm(has_many = "super::passkey_credential::Entity")]
    PasskeyCredentials,

    #[sea_orm(has_many = "super::oauth_account::Entity")]
    OauthAccounts,
}

impl Related<super::org_member::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OrgMemberships.def()
    }
}

impl Related<super::memory_version::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AuthoredVersions.def()
    }
}

impl Related<super::session::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Sessions.def()
    }
}

impl Related<super::passkey_credential::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PasskeyCredentials.def()
    }
}

impl Related<super::oauth_account::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OauthAccounts.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
