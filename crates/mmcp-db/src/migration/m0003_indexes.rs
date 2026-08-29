//! Add indexes for hot-path filter columns.
//!
//! SQLite does not auto-index a foreign-key child column, and until
//! now the schema defined exactly one index in total
//! (`idx_oauth_provider_user`, m0002). Every filter below is a real
//! query already issued by a repository function, each forcing a
//! full table scan without its index.
//!
//! `memory_repo::search_by_slug`'s `lower(slug) LIKE '%needle%'`
//! predicate is deliberately left unindexed: a leading `%` wildcard
//! defeats a B-tree range scan regardless of whether the index is a
//! plain column index or a `lower(slug)` expression index (verified
//! empirically via `EXPLAIN QUERY PLAN`, which still reports a full
//! `SCAN` with such an index present), and no other query in this
//! codebase filters on `lower(slug)` with an equality or
//! no-leading-wildcard shape that an expression index could serve.
//! Indexing arbitrary substring search requires a different
//! structure entirely (SQLite FTS5, e.g.), which is out of scope
//! here.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx_memories_group_id")
                    .table(Memories::Table)
                    .col(Memories::GroupId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_memory_versions_memory_id")
                    .table(MemoryVersions::Table)
                    .col(MemoryVersions::MemoryId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_groups_owner")
                    .table(Groups::Table)
                    .col(Groups::OwnerKind)
                    .col(Groups::OwnerId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_group_memberships_group_id")
                    .table(GroupMemberships::Table)
                    .col(GroupMemberships::GroupId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_org_members_org_id")
                    .table(OrgMembers::Table)
                    .col(OrgMembers::OrgId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_memory_reads_session_id")
                    .table(MemoryReads::Table)
                    .col(MemoryReads::SessionId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_memory_reads_memory_id")
                    .table(MemoryReads::Table)
                    .col(MemoryReads::MemoryId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_passkey_credentials_user_id")
                    .table(PasskeyCredentials::Table)
                    .col(PasskeyCredentials::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_oauth_accounts_user_id")
                    .table(OauthAccounts::Table)
                    .col(OauthAccounts::UserId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_oauth_accounts_user_id")
                    .table(OauthAccounts::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_passkey_credentials_user_id")
                    .table(PasskeyCredentials::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_memory_reads_memory_id")
                    .table(MemoryReads::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_memory_reads_session_id")
                    .table(MemoryReads::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_org_members_org_id")
                    .table(OrgMembers::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_group_memberships_group_id")
                    .table(GroupMemberships::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_groups_owner")
                    .table(Groups::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_memory_versions_memory_id")
                    .table(MemoryVersions::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_memories_group_id")
                    .table(Memories::Table)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

// Re-use table/column idens from earlier migrations.
#[derive(DeriveIden)]
enum Memories {
    Table,
    GroupId,
}

#[derive(DeriveIden)]
enum MemoryVersions {
    Table,
    MemoryId,
}

#[derive(DeriveIden)]
enum Groups {
    Table,
    OwnerKind,
    OwnerId,
}

#[derive(DeriveIden)]
enum GroupMemberships {
    Table,
    GroupId,
}

#[derive(DeriveIden)]
enum OrgMembers {
    Table,
    OrgId,
}

#[derive(DeriveIden)]
enum MemoryReads {
    Table,
    SessionId,
    MemoryId,
}

#[derive(DeriveIden)]
enum PasskeyCredentials {
    Table,
    UserId,
}

#[derive(DeriveIden)]
enum OauthAccounts {
    Table,
    UserId,
}
