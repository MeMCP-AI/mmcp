//! `memories` table: server-side index of memory entries.
//!
//! The actual memory file content lives in the group's git
//! repository. This table holds only the metadata the control plane
//! needs for listing, permission checks, and version tracking.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Behavioral classification mirrored from `mmcp_core::memory::MemoryKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum MemoryKind {
    #[sea_orm(num_value = 0)]
    Rule = 0,

    #[sea_orm(num_value = 1)]
    Snapshot = 1,

    #[sea_orm(num_value = 2)]
    Log = 2,

    #[sea_orm(num_value = 3)]
    Reference = 3,

    #[sea_orm(num_value = 4)]
    Scratch = 4,
}

/// Row in the `memories` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "memories")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    #[sea_orm(indexed)]
    pub group_id: Uuid,

    /// File slug inside the group's repo (without extension).
    pub slug: String,

    pub kind: MemoryKind,

    pub mandatory: bool,

    /// Latest published version as a semver string, or NULL for
    /// memories that have not been published yet.
    pub latest_version: Option<String>,

    /// Creation and last-update timestamps, ms since epoch.
    pub created_at: i64,
    pub updated_at: i64,
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

    #[sea_orm(has_many = "super::memory_version::Entity")]
    Versions,

    #[sea_orm(has_many = "super::memory_read::Entity")]
    Reads,
}

impl Related<super::group::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Group.def()
    }
}

impl Related<super::memory_version::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Versions.def()
    }
}

impl Related<super::memory_read::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Reads.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
