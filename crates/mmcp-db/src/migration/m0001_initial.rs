//! Initial schema migration.
//!
//! Creates every mmcp table in dependency order. Re-running the
//! `down` direction drops them in reverse.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Users::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Users::Id).uuid().not_null().primary_key())
                    .col(
                        ColumnDef::new(Users::Handle)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Users::DisplayName).string().null())
                    .col(ColumnDef::new(Users::PasswordHash).string().null())
                    .col(ColumnDef::new(Users::Email).string().null())
                    .col(ColumnDef::new(Users::CreatedAt).big_integer().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Orgs::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Orgs::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Orgs::Slug).string().not_null().unique_key())
                    .col(ColumnDef::new(Orgs::DisplayName).string().null())
                    .col(ColumnDef::new(Orgs::CreatedAt).big_integer().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OrgMembers::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OrgMembers::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OrgMembers::OrgId).uuid().not_null())
                    .col(ColumnDef::new(OrgMembers::UserId).uuid().not_null())
                    .col(ColumnDef::new(OrgMembers::Role).small_integer().not_null())
                    .col(
                        ColumnDef::new(OrgMembers::GrantedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_org_members_org")
                            .from(OrgMembers::Table, OrgMembers::OrgId)
                            .to(Orgs::Table, Orgs::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_org_members_user")
                            .from(OrgMembers::Table, OrgMembers::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Groups::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Groups::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Groups::Slug).string().not_null())
                    .col(ColumnDef::new(Groups::OwnerKind).small_integer().not_null())
                    .col(ColumnDef::new(Groups::OwnerId).uuid().not_null())
                    .col(ColumnDef::new(Groups::DisplayName).string().null())
                    .col(ColumnDef::new(Groups::CreatedAt).big_integer().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(GroupMemberships::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(GroupMemberships::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(GroupMemberships::GroupId).uuid().not_null())
                    .col(
                        ColumnDef::new(GroupMemberships::PrincipalKind)
                            .small_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GroupMemberships::PrincipalId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GroupMemberships::Role)
                            .small_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GroupMemberships::GrantedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_memberships_group")
                            .from(GroupMemberships::Table, GroupMemberships::GroupId)
                            .to(Groups::Table, Groups::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Memories::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Memories::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Memories::GroupId).uuid().not_null())
                    .col(ColumnDef::new(Memories::Slug).string().not_null())
                    .col(ColumnDef::new(Memories::Kind).small_integer().not_null())
                    .col(
                        ColumnDef::new(Memories::Mandatory)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(Memories::LatestVersion).string().null())
                    .col(ColumnDef::new(Memories::CreatedAt).big_integer().not_null())
                    .col(ColumnDef::new(Memories::UpdatedAt).big_integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_memories_group")
                            .from(Memories::Table, Memories::GroupId)
                            .to(Groups::Table, Groups::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(MemoryVersions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MemoryVersions::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(MemoryVersions::MemoryId).uuid().not_null())
                    .col(ColumnDef::new(MemoryVersions::Version).string().not_null())
                    .col(ColumnDef::new(MemoryVersions::Commit).string().not_null())
                    .col(ColumnDef::new(MemoryVersions::AuthorId).uuid().not_null())
                    .col(
                        ColumnDef::new(MemoryVersions::PublishedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(MemoryVersions::Summary).string().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_versions_memory")
                            .from(MemoryVersions::Table, MemoryVersions::MemoryId)
                            .to(Memories::Table, Memories::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_versions_author")
                            .from(MemoryVersions::Table, MemoryVersions::AuthorId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Sessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Sessions::SessionId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Sessions::UserId).uuid().null())
                    .col(ColumnDef::new(Sessions::ProjectUuid).uuid().null())
                    .col(
                        ColumnDef::new(Sessions::TurnCounter)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Sessions::TranscriptPath).string().null())
                    .col(
                        ColumnDef::new(Sessions::TranscriptSignature)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Sessions::PostCompaction)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(Sessions::StartedAt).big_integer().not_null())
                    .col(
                        ColumnDef::new(Sessions::LastSeenAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_sessions_user")
                            .from(Sessions::Table, Sessions::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(MemoryReads::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MemoryReads::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(MemoryReads::SessionId).string().not_null())
                    .col(ColumnDef::new(MemoryReads::MemoryId).uuid().not_null())
                    .col(ColumnDef::new(MemoryReads::Turn).integer().not_null())
                    .col(ColumnDef::new(MemoryReads::Version).string().null())
                    .col(
                        ColumnDef::new(MemoryReads::Verified)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(MemoryReads::ReadAt).big_integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_reads_session")
                            .from(MemoryReads::Table, MemoryReads::SessionId)
                            .to(Sessions::Table, Sessions::SessionId)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_reads_memory")
                            .from(MemoryReads::Table, MemoryReads::MemoryId)
                            .to(Memories::Table, Memories::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop in reverse dependency order.
        manager
            .drop_table(Table::drop().table(MemoryReads::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Sessions::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(MemoryVersions::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Memories::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(GroupMemberships::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Groups::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OrgMembers::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Orgs::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Users::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
    Handle,
    DisplayName,
    PasswordHash,
    Email,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Orgs {
    Table,
    Id,
    Slug,
    DisplayName,
    CreatedAt,
}

#[derive(DeriveIden)]
enum OrgMembers {
    Table,
    Id,
    OrgId,
    UserId,
    Role,
    GrantedAt,
}

#[derive(DeriveIden)]
enum Groups {
    Table,
    Id,
    Slug,
    OwnerKind,
    OwnerId,
    DisplayName,
    CreatedAt,
}

#[derive(DeriveIden)]
enum GroupMemberships {
    Table,
    Id,
    GroupId,
    PrincipalKind,
    PrincipalId,
    Role,
    GrantedAt,
}

#[derive(DeriveIden)]
enum Memories {
    Table,
    Id,
    GroupId,
    Slug,
    Kind,
    Mandatory,
    LatestVersion,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum MemoryVersions {
    Table,
    Id,
    MemoryId,
    Version,
    Commit,
    AuthorId,
    PublishedAt,
    Summary,
}

#[derive(DeriveIden)]
enum Sessions {
    Table,
    SessionId,
    UserId,
    ProjectUuid,
    TurnCounter,
    TranscriptPath,
    TranscriptSignature,
    PostCompaction,
    StartedAt,
    LastSeenAt,
}

#[derive(DeriveIden)]
enum MemoryReads {
    Table,
    Id,
    SessionId,
    MemoryId,
    Turn,
    Version,
    Verified,
    ReadAt,
}
