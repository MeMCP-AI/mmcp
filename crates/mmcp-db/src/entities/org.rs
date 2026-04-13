//! `orgs` table: organisations grouping multiple users together.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Row in the `orgs` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "orgs")]
pub struct Model {
    /// UUIDv7 primary key.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    /// Unique lowercase URL slug.
    #[sea_orm(unique, indexed)]
    pub slug: String,

    /// Optional display name.
    pub display_name: Option<String>,

    /// Creation time, milliseconds since Unix epoch.
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::org_member::Entity")]
    Members,
}

impl Related<super::org_member::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Members.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
