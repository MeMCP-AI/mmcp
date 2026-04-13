//! `memory_versions` table: published version history.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Row in the `memory_versions` table.
///
/// Each row corresponds to exactly one commit published to the
/// canonical branch. `version` is the semver string assigned by the
/// server at push time from the editor's bump intent.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "memory_versions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    #[sea_orm(indexed)]
    pub memory_id: Uuid,

    /// Semver version string (e.g. "1.2.3").
    pub version: String,

    /// Commit hash identifying the tree state, as a hex string.
    pub commit: String,

    #[sea_orm(indexed)]
    pub author_id: Uuid,

    pub published_at: i64,

    pub summary: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::memory::Entity",
        from = "Column::MemoryId",
        to = "super::memory::Column::Id",
        on_delete = "Cascade"
    )]
    Memory,

    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::AuthorId",
        to = "super::user::Column::Id",
        on_delete = "Restrict"
    )]
    Author,
}

impl Related<super::memory::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Memory.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Author.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
