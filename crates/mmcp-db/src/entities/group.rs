//! `groups` table: the unit of storage and permissioning.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Discriminator for the owner of a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum OwnerKind {
    #[sea_orm(num_value = 0)]
    User = 0,

    #[sea_orm(num_value = 1)]
    Org = 1,
}

/// Row in the `groups` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "groups")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    /// Slug unique within the owner's namespace.
    #[sea_orm(indexed)]
    pub slug: String,

    pub owner_kind: OwnerKind,

    /// UUID of the owning user or org depending on `owner_kind`.
    #[sea_orm(indexed)]
    pub owner_id: Uuid,

    pub display_name: Option<String>,

    /// Creation time, ms since Unix epoch.
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::memory::Entity")]
    Memories,

    #[sea_orm(has_many = "super::membership::Entity")]
    Memberships,
}

impl Related<super::memory::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Memories.def()
    }
}

impl Related<super::membership::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Memberships.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
