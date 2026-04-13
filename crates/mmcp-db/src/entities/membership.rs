//! `group_memberships` table: principal-to-group access grants.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Access role granted on a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum GroupRole {
    #[sea_orm(num_value = 0)]
    Read = 0,

    #[sea_orm(num_value = 1)]
    Write = 1,

    #[sea_orm(num_value = 2)]
    Admin = 2,
}

/// Discriminator for the principal of a membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum PrincipalKind {
    #[sea_orm(num_value = 0)]
    User = 0,

    #[sea_orm(num_value = 1)]
    Org = 1,

    #[sea_orm(num_value = 2)]
    Group = 2,
}

/// Row in the `group_memberships` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "group_memberships")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    #[sea_orm(indexed)]
    pub group_id: Uuid,

    pub principal_kind: PrincipalKind,

    #[sea_orm(indexed)]
    pub principal_id: Uuid,

    pub role: GroupRole,

    pub granted_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::group::Entity",
        from = "Column::GroupId",
        to = "super::group::Column::Id",
        on_delete = "Cascade"
    )]
    Group,
}

impl Related<super::group::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Group.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
