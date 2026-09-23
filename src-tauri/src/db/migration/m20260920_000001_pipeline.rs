use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Pipeline::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Pipeline::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Pipeline::Name).string().not_null())
                    .col(ColumnDef::new(Pipeline::PresetKey).string().null())
                    .col(ColumnDef::new(Pipeline::FolderId).integer().null())
                    .col(ColumnDef::new(Pipeline::Graph).text().not_null())
                    .col(ColumnDef::new(Pipeline::Isolation).string().not_null())
                    .col(
                        ColumnDef::new(Pipeline::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Pipeline::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Pipeline::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_pipeline_folder_id")
                    .table(Pipeline::Table)
                    .col(Pipeline::FolderId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uq_pipeline_preset_key")
                    .table(Pipeline::Table)
                    .col(Pipeline::PresetKey)
                    .unique()
                    .and_where(Expr::col(Pipeline::PresetKey).is_not_null())
                    .and_where(Expr::col(Pipeline::DeletedAt).is_null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(PipelineRun::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PipelineRun::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PipelineRun::PipelineId).integer().null())
                    .col(ColumnDef::new(PipelineRun::FolderId).integer().not_null())
                    .col(
                        ColumnDef::new(PipelineRun::WorktreeFolderId)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PipelineRun::ParentConversationId)
                            .integer()
                            .null(),
                    )
                    .col(ColumnDef::new(PipelineRun::Graph).text().not_null())
                    .col(ColumnDef::new(PipelineRun::Status).string().not_null())
                    .col(
                        ColumnDef::new(PipelineRun::Isolation)
                            .string()
                            .not_null()
                            .default("worktree_per_run"),
                    )
                    .col(ColumnDef::new(PipelineRun::DisplayText).text().null())
                    .col(ColumnDef::new(PipelineRun::CurrentStepId).string().null())
                    .col(
                        ColumnDef::new(PipelineRun::CurrentIteration)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(PipelineRun::Error).text().null())
                    .col(
                        ColumnDef::new(PipelineRun::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PipelineRun::EndedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_pipeline_run_pipeline")
                            .from(PipelineRun::Table, PipelineRun::PipelineId)
                            .to(Pipeline::Table, Pipeline::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_pipeline_run_folder_status")
                    .table(PipelineRun::Table)
                    .col(PipelineRun::FolderId)
                    .col(PipelineRun::Status)
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(PipelineAttempt::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PipelineAttempt::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PipelineAttempt::RunId).integer().not_null())
                    .col(ColumnDef::new(PipelineAttempt::StepId).string().not_null())
                    .col(
                        ColumnDef::new(PipelineAttempt::Iteration)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PipelineAttempt::Status).string().not_null())
                    .col(
                        ColumnDef::new(PipelineAttempt::ConnectionId)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PipelineAttempt::ConversationId)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PipelineAttempt::ModelRequested)
                            .string()
                            .null(),
                    )
                    .col(ColumnDef::new(PipelineAttempt::ModelActual).string().null())
                    .col(ColumnDef::new(PipelineAttempt::Verdict).string().null())
                    .col(
                        ColumnDef::new(PipelineAttempt::VerdictSource)
                            .string()
                            .null(),
                    )
                    .col(ColumnDef::new(PipelineAttempt::Notes).text().null())
                    .col(ColumnDef::new(PipelineAttempt::Summary).text().null())
                    .col(
                        ColumnDef::new(PipelineAttempt::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PipelineAttempt::EndedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_pipeline_attempt_run")
                            .from(PipelineAttempt::Table, PipelineAttempt::RunId)
                            .to(PipelineRun::Table, PipelineRun::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uq_pipeline_attempt_run_step_iteration")
                    .table(PipelineAttempt::Table)
                    .col(PipelineAttempt::RunId)
                    .col(PipelineAttempt::StepId)
                    .col(PipelineAttempt::Iteration)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PipelineAttempt::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PipelineRun::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Pipeline::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Pipeline {
    Table,
    Id,
    Name,
    PresetKey,
    FolderId,
    Graph,
    Isolation,
    CreatedAt,
    UpdatedAt,
    DeletedAt,
}

#[derive(DeriveIden)]
enum PipelineRun {
    Table,
    Id,
    PipelineId,
    FolderId,
    WorktreeFolderId,
    ParentConversationId,
    Graph,
    Status,
    Isolation,
    DisplayText,
    CurrentStepId,
    CurrentIteration,
    Error,
    StartedAt,
    EndedAt,
}

#[derive(DeriveIden)]
enum PipelineAttempt {
    Table,
    Id,
    RunId,
    StepId,
    Iteration,
    Status,
    ConnectionId,
    ConversationId,
    ModelRequested,
    ModelActual,
    Verdict,
    VerdictSource,
    Notes,
    Summary,
    StartedAt,
    EndedAt,
}

#[cfg(test)]
mod tests {
    use sea_orm::Database;
    use sea_orm_migration::MigratorTrait;

    use crate::db::migration::Migrator;

    /// Runs every migration up, then rolls back all migrations to zero on
    /// in-memory SQLite. This ensures the full up/down cycle (including the
    /// three new pipeline/memory migrations) is idempotent without depending on
    /// fixes to upstream migrations.
    #[tokio::test]
    async fn pipeline_migrations_up_then_down_on_in_memory_sqlite() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        Migrator::up(&conn, None).await.expect("up");
        Migrator::down(&conn, None).await.expect("down");
    }
}
