//! `memory_reads` table: per-session memory read tracking.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Row in the `memory_reads` table.
///
/// Records that a session read (and optionally verified) a specific
/// memory at a specific turn. The mandatory-memory enforcement layer
/// queries this table to decide whether to let a tool call through
/// or to return a gate error.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "memory_reads")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    #[sea_orm(indexed)]
    pub session_id: String,

    #[sea_orm(indexed)]
    pub memory_id: Uuid,

    /// Turn counter at the time of the read.
    pub turn: i32,

    /// Version string the session saw. Used to notice when the
    /// memory changes between reads inside the same session.
    pub version: Option<String>,

    /// True if the read was also explicitly verified via the
    /// `verify_memory` tool.
    pub verified: bool,

    pub read_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::session::Entity",
        from = "Column::SessionId",
        to = "super::session::Column::SessionId",
        on_delete = "Cascade"
    )]
    Session,

    #[sea_orm(
        belongs_to = "super::memory::Entity",
        from = "Column::MemoryId",
        to = "super::memory::Column::Id",
        on_delete = "Cascade"
    )]
    Memory,
}

impl Related<super::session::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Session.def()
    }
}

impl Related<super::memory::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Memory.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
