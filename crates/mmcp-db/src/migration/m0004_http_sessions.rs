//! Add the `http_sessions` table backing the server's HTTP session store.
//!
//! Rows are keyed by the SHA-256 digest of the session id, never the id itself.
//! A read of the table or of a backup therefore yields no usable session cookie.
//! The table is distinct from `sessions`, which tracks AI-client sessions.

use sea_orm_migration::prelude::*;

/// Serves the batched delete of expired rows.
const EXPIRES_AT_INDEX_NAME: &str = "idx_http_sessions_expires_at";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(HttpSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(HttpSessions::SessionIdSha256)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(HttpSessions::Data).text().not_null())
                    .col(
                        ColumnDef::new(HttpSessions::ExpiresAt)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name(EXPIRES_AT_INDEX_NAME)
                    .table(HttpSessions::Table)
                    .col(HttpSessions::ExpiresAt)
                    .if_not_exists()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name(EXPIRES_AT_INDEX_NAME)
                    .table(HttpSessions::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(HttpSessions::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum HttpSessions {
    Table,
    /// Lowercase hex SHA-256 of the session id.
    SessionIdSha256,
    /// Session data map as JSON text.
    Data,
    /// Expiry instant, epoch milliseconds.
    ExpiresAt,
}
