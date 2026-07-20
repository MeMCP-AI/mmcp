//! Add passkey credentials and OAuth account link tables.
//!
//! Supports the Phase 7 authentication flows: WebAuthn/FIDO2
//! passkeys and OAuth2 authorization code grants (GitHub, Google,
//! etc.). Both tables reference `users(id)` with cascade deletes
//! so deleting a user cleans up all associated credentials.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── passkey_credentials ─────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(PasskeyCredentials::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PasskeyCredentials::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PasskeyCredentials::UserId).uuid().not_null())
                    .col(ColumnDef::new(PasskeyCredentials::Name).string().not_null())
                    .col(
                        ColumnDef::new(PasskeyCredentials::CredentialJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PasskeyCredentials::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PasskeyCredentials::LastUsedAt)
                            .big_integer()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_passkey_user")
                            .from(PasskeyCredentials::Table, PasskeyCredentials::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // ── oauth_accounts ──────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(OauthAccounts::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthAccounts::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OauthAccounts::UserId).uuid().not_null())
                    .col(ColumnDef::new(OauthAccounts::Provider).string().not_null())
                    .col(
                        ColumnDef::new(OauthAccounts::ProviderUserId)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthAccounts::Email).string().null())
                    .col(ColumnDef::new(OauthAccounts::AccessToken).text().null())
                    .col(ColumnDef::new(OauthAccounts::RefreshToken).text().null())
                    .col(
                        ColumnDef::new(OauthAccounts::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthAccounts::UpdatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oauth_user")
                            .from(OauthAccounts::Table, OauthAccounts::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Unique constraint: one link per provider per user.
        manager
            .create_index(
                Index::create()
                    .name("idx_oauth_provider_user")
                    .table(OauthAccounts::Table)
                    .col(OauthAccounts::Provider)
                    .col(OauthAccounts::ProviderUserId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OauthAccounts::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PasskeyCredentials::Table).to_owned())
            .await?;
        Ok(())
    }
}

// Re-use Users iden from the initial migration.
#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum PasskeyCredentials {
    Table,
    Id,
    UserId,
    Name,
    /// JSON-serialized `webauthn_rs::prelude::Passkey`.
    CredentialJson,
    CreatedAt,
    LastUsedAt,
}

#[derive(DeriveIden)]
enum OauthAccounts {
    Table,
    Id,
    UserId,
    /// Provider slug: "github", "google", etc.
    Provider,
    /// The user ID on the provider's side.
    ProviderUserId,
    Email,
    AccessToken,
    RefreshToken,
    CreatedAt,
    UpdatedAt,
}
