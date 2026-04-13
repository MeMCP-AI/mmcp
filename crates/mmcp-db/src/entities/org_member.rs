//! `org_members` table: user-to-org membership.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Role a user holds inside an org.
///
/// Mirrors `mmcp_core::identity::Role` but is stored as a small
/// integer in the database for efficient indexing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum OrgRole {
    /// Read-only member. Can view all org-owned groups.
    #[sea_orm(num_value = 0)]
    Member = 0,

    /// Can create and edit groups inside the org.
    #[sea_orm(num_value = 1)]
    Maintainer = 1,

    /// Full admin including membership and group management.
    #[sea_orm(num_value = 2)]
    Owner = 2,
}

/// Row in the `org_members` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "org_members")]
pub struct Model {
    /// Surrogate UUID primary key. Lets us reference memberships
    /// in audit trails without carrying a composite key.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    #[sea_orm(indexed)]
    pub org_id: Uuid,

    #[sea_orm(indexed)]
    pub user_id: Uuid,

    pub role: OrgRole,

    /// When the membership was granted, ms since epoch.
    pub granted_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::org::Entity",
        from = "Column::OrgId",
        to = "super::org::Column::Id",
        on_delete = "Cascade"
    )]
    Org,

    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_delete = "Cascade"
    )]
    User,
}

impl Related<super::org::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Org.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
