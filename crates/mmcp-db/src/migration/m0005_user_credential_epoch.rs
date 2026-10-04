//! Add the `credential_epoch` counter to `users`.
//!
//! Every credential change increments it, and it feeds the session auth hash.
//! Existing users start at the initial epoch.

use sea_orm_migration::prelude::*;

use crate::repository::credential_epoch::INITIAL_CREDENTIAL_EPOCH;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::CredentialEpoch)
                            .big_integer()
                            .not_null()
                            .default(INITIAL_CREDENTIAL_EPOCH),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::CredentialEpoch)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Users {
    Table,
    CredentialEpoch,
}
