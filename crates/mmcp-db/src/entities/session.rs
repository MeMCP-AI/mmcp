//! `sessions` table: AI session tracking.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Row in the `sessions` table.
///
/// Keyed by the `session_id` the AI client reports (e.g. the Claude
/// Code session UUID). Holds the per-session state that drives
/// staleness warnings and mandatory-memory enforcement.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sessions")]
pub struct Model {
    /// Stable session identifier from the AI client.
    #[sea_orm(primary_key, auto_increment = false)]
    pub session_id: String,

    /// User that owns this session. Nullable for local-only mode
    /// where there is no authenticated user.
    #[sea_orm(indexed)]
    pub user_id: Option<Uuid>,

    /// Project UUID the session is operating inside, if any.
    pub project_uuid: Option<Uuid>,

    /// Current turn counter, incremented by the UserPromptSubmit hook.
    pub turn_counter: i32,

    /// Path to the transcript file the AI client is writing to.
    pub transcript_path: Option<String>,

    /// Hash or length of the transcript at last check, used to
    /// detect compaction events.
    pub transcript_signature: Option<String>,

    /// True if a compaction was detected and the session has not yet
    /// re-verified mandatory memories.
    pub post_compaction: bool,

    pub started_at: i64,
    pub last_seen_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_delete = "SetNull"
    )]
    User,

    #[sea_orm(has_many = "super::memory_read::Entity")]
    Reads,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl Related<super::memory_read::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Reads.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
