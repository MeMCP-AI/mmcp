//! `http_sessions` table: server-side HTTP session records.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Row in the `http_sessions` table.
///
/// The key is the SHA-256 digest of the session id, never the id.
/// The id is a bearer secret the database must not hold.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "http_sessions")]
pub struct Model {
    /// Lowercase hex SHA-256 digest of the session id.
    #[sea_orm(primary_key, auto_increment = false)]
    pub session_id_sha256: String,

    /// Session data map serialized as JSON text, opaque to the DB layer.
    #[sea_orm(column_type = "Text")]
    pub data: String,

    /// Expiry instant in epoch milliseconds.
    pub expires_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
