use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // work_task: one folder-bound delegation unit driven through the
        // todo → queued → running ⇄ awaiting_input → review → merging → done
        // pipeline (side paths: failed / canceled). `done` ⟺ merged.
        manager
            .create_table(
                Table::create()
                    .table(WorkTask::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WorkTask::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    // Soft reference to the project folder (never a worktree
                    // folder). No hard FK: folders soft-delete and every query
                    // joins on the live folder anyway.
                    .col(ColumnDef::new(WorkTask::FolderId).integer().not_null())
                    .col(ColumnDef::new(WorkTask::Title).text().not_null())
                    // JSON blob: { prompt_blocks, display_text, agent_type?,
                    // mode_id?, config_values, label_snapshot? } — per-task
                    // overrides; empty fields inherit the folder settings.
                    .col(ColumnDef::new(WorkTask::Config).text().not_null())
                    .col(
                        ColumnDef::new(WorkTask::Status)
                            .string()
                            .not_null()
                            .default("todo"),
                    )
                    // agent_error | setup_error | verdict_blocked | interrupted
                    .col(ColumnDef::new(WorkTask::FailureReason).string().null())
                    .col(ColumnDef::new(WorkTask::LastError).text().null())
                    // Execution generation: bumped on every start/retry/return.
                    // Events are matched on (connection_id, run_seq); stale or
                    // post-cancel events are dropped.
                    .col(
                        ColumnDef::new(WorkTask::RunSeq)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(WorkTask::SortOrder)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(WorkTask::WorktreeFolderId).integer().null())
                    .col(ColumnDef::new(WorkTask::ConversationId).integer().null())
                    // Live ACP connection of the current generation (not durable
                    // across restart).
                    .col(ColumnDef::new(WorkTask::ConnectionId).string().null())
                    .col(ColumnDef::new(WorkTask::BaseBranch).string().null())
                    .col(ColumnDef::new(WorkTask::BaseSha).string().null())
                    .col(ColumnDef::new(WorkTask::WorkBranch).string().null())
                    // Persisted merge intent { pre_merge_head, synced_base_sha?,
                    // message, strategy } written in the same transaction as the
                    // review→merging CAS; crash recovery reads git truth against it.
                    .col(ColumnDef::new(WorkTask::MergeState).text().null())
                    // NULL = nothing pending; 'failed' = worktree cleanup failed,
                    // retryable from the card. Never rolls `done` back.
                    .col(ColumnDef::new(WorkTask::CleanupState).string().null())
                    // success | needs_review | blocked (P1, MCP self-report).
                    .col(ColumnDef::new(WorkTask::Verdict).string().null())
                    .col(ColumnDef::new(WorkTask::ResultSummary).text().null())
                    .col(ColumnDef::new(WorkTask::FilesChanged).integer().null())
                    .col(ColumnDef::new(WorkTask::Additions).integer().null())
                    .col(ColumnDef::new(WorkTask::Deletions).integer().null())
                    .col(ColumnDef::new(WorkTask::MergeCommit).string().null())
                    .col(
                        ColumnDef::new(WorkTask::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkTask::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkTask::StartedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    // settled = entered review/failed; finished = done/canceled.
                    .col(
                        ColumnDef::new(WorkTask::SettledAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(WorkTask::FinishedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(WorkTask::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_work_task_folder")
                    .table(WorkTask::Table)
                    .col(WorkTask::FolderId)
                    .to_owned(),
            )
            .await?;

        // Reconcile / board sweeps by status.
        manager
            .create_index(
                Index::create()
                    .name("idx_work_task_status")
                    .table(WorkTask::Table)
                    .col(WorkTask::Status)
                    .to_owned(),
            )
            .await?;

        // work_task_event: append-only progress timeline ("how it advanced").
        // Written in the SAME transaction as the state transition it records.
        manager
            .create_table(
                Table::create()
                    .table(WorkTaskEvent::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WorkTaskEvent::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WorkTaskEvent::TaskId).integer().not_null())
                    // created / status_changed / config_effective / agent_progress
                    // / agent_verdict / merge_attempt / merge_conflict /
                    // cleanup_failed / resume_fallback / user_action / diff_stat
                    .col(ColumnDef::new(WorkTaskEvent::Kind).string().not_null())
                    // user | engine | agent
                    .col(ColumnDef::new(WorkTaskEvent::Actor).string().not_null())
                    .col(ColumnDef::new(WorkTaskEvent::Payload).text().null())
                    .col(
                        ColumnDef::new(WorkTaskEvent::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkTaskEvent::Table, WorkTaskEvent::TaskId)
                            .to(WorkTask::Table, WorkTask::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_work_task_event_task_created")
                    .table(WorkTaskEvent::Table)
                    .col(WorkTaskEvent::TaskId)
                    .col(WorkTaskEvent::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // work_task_settings: one row per folder — default agent/config,
        // auto_process, max_concurrent, merge defaults. Dies with the folder
        // (every read joins the live folder).
        manager
            .create_table(
                Table::create()
                    .table(WorkTaskSettings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WorkTaskSettings::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(WorkTaskSettings::FolderId)
                            .integer()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(WorkTaskSettings::Config).text().not_null())
                    .col(
                        ColumnDef::new(WorkTaskSettings::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkTaskSettings::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // conversation.origin_cwd: the native transcript's first working
        // directory when that differs from the current folder — written by an
        // explicit conversation move or a deleted-worktree re-parent. The
        // Gemini/Cline/OpenClaw stale-external-id fallback matches on
        // `origin_cwd ?? folder.path`; moving back to the origin clears it.
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .add_column(ColumnDef::new(Conversation::OriginCwd).text().null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_column(Conversation::OriginCwd)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(WorkTaskSettings::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(WorkTaskEvent::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(WorkTask::Table).if_exists().to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum WorkTask {
    Table,
    Id,
    FolderId,
    Title,
    Config,
    Status,
    FailureReason,
    LastError,
    RunSeq,
    SortOrder,
    WorktreeFolderId,
    ConversationId,
    ConnectionId,
    BaseBranch,
    BaseSha,
    WorkBranch,
    MergeState,
    CleanupState,
    Verdict,
    ResultSummary,
    FilesChanged,
    Additions,
    Deletions,
    MergeCommit,
    CreatedAt,
    UpdatedAt,
    StartedAt,
    SettledAt,
    FinishedAt,
    DeletedAt,
}

#[derive(DeriveIden)]
enum WorkTaskEvent {
    Table,
    Id,
    TaskId,
    Kind,
    Actor,
    Payload,
    CreatedAt,
}

#[derive(DeriveIden)]
enum WorkTaskSettings {
    Table,
    Id,
    FolderId,
    Config,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    OriginCwd,
}
