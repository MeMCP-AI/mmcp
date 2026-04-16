//! `oauth_accounts` table: external OAuth provider links.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Row in the `oauth_accounts` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "oauth_accounts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    pub user_id: Uuid,

    /// Provider slug: `"github"`, `"google"`, etc.
    pub provider: String,

    /// The user ID on the provider's side.
    pub provider_user_id: String,

    /// Email address reported by the provider, if any.
    pub email: Option<String>,

    /// Most recent access token. Nullable because not all flows
    /// retain it after the initial exchange.
    #[sea_orm(column_type = "Text", nullable)]
    pub access_token: Option<String>,

    /// Refresh token for providers that issue one.
    #[sea_orm(column_type = "Text", nullable)]
    pub refresh_token: Option<String>,

    pub created_at: i64,
    pub updated_at: i64,
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
