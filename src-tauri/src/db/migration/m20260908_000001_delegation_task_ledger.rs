use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DelegationTask::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DelegationTask::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(DelegationTask::TaskId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(DelegationTask::ParentConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DelegationTask::ChildConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(DelegationTask::SourceTaskId).string().null())
                    .col(ColumnDef::new(DelegationTask::Task).text().not_null())
                    .col(
                        ColumnDef::new(DelegationTask::RequestedWorkingDir)
                            .text()
                            .null(),
                    )
                    .col(ColumnDef::new(DelegationTask::Status).string().not_null())
                    .col(ColumnDef::new(DelegationTask::TerminalReport).text().null())
                    .col(
                        ColumnDef::new(DelegationTask::ResumeBinding)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DelegationTask::Released)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(DelegationTask::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DelegationTask::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_delegation_task_parent_conversation")
                            .from(DelegationTask::Table, DelegationTask::ParentConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_delegation_task_child_conversation")
                            .from(DelegationTask::Table, DelegationTask::ChildConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_delegation_task_parent")
                    .table(DelegationTask::Table)
                    .col(DelegationTask::ParentConversationId)
                    .col(DelegationTask::UpdatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_delegation_task_source_unique")
                    .table(DelegationTask::Table)
                    .col(DelegationTask::SourceTaskId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_delegation_task_child")
                    .table(DelegationTask::Table)
                    .col(DelegationTask::ChildConversationId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(DelegationTask::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum DelegationTask {
    Table,
    Id,
    TaskId,
    ParentConversationId,
    ChildConversationId,
    SourceTaskId,
    Task,
    RequestedWorkingDir,
    Status,
    TerminalReport,
    ResumeBinding,
    Released,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Id,
}
