use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // automation: one saved, schedulable, replayable composer launch.
        manager
            .create_table(
                Table::create()
                    .table(Automation::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Automation::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Automation::Name).string().not_null())
                    .col(
                        ColumnDef::new(Automation::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    // 'schedule' | 'manual' (string-backed DeriveActiveEnum)
                    .col(
                        ColumnDef::new(Automation::TriggerKind)
                            .string()
                            .not_null()
                            .default("schedule"),
                    )
                    // 5-field cron; required when trigger_kind = 'schedule'.
                    .col(ColumnDef::new(Automation::Cron).string().null())
                    // IANA tz name; cron is evaluated in this tz.
                    .col(
                        ColumnDef::new(Automation::Timezone)
                            .string()
                            .not_null()
                            .default("UTC"),
                    )
                    // Computed next fire, stored UTC — the scheduler's due key.
                    .col(
                        ColumnDef::new(Automation::NextRunAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(Automation::AgentType).string().not_null())
                    // Soft reference to folder.id (NULL = folderless). No hard FK:
                    // folders soft-delete and the service re-resolves at fire time.
                    .col(ColumnDef::new(Automation::RootFolderId).integer().null())
                    // 'worktree_per_run' | 'shared_in_root'
                    .col(
                        ColumnDef::new(Automation::Isolation)
                            .string()
                            .not_null()
                            .default("worktree_per_run"),
                    )
                    .col(ColumnDef::new(Automation::Branch).string().null())
                    .col(
                        ColumnDef::new(Automation::IsRemoteBranch)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    // JSON snapshot blob: { promptBlocks, displayText, modeId,
                    // configValues, labelSnapshot } — the captured composer state.
                    .col(ColumnDef::new(Automation::Config).text().not_null())
                    .col(
                        ColumnDef::new(Automation::LastRunAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(Automation::LastRunStatus).string().null())
                    .col(
                        ColumnDef::new(Automation::LastRunConversationId)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Automation::UnseenFailures)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Automation::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Automation::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Automation::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Scheduler poll: WHERE enabled AND next_run_at <= now.
        manager
            .create_index(
                Index::create()
                    .name("idx_automation_due")
                    .table(Automation::Table)
                    .col(Automation::Enabled)
                    .col(Automation::NextRunAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_automation_root_folder")
                    .table(Automation::Table)
                    .col(Automation::RootFolderId)
                    .to_owned(),
            )
            .await?;

        // automation_run: one launch+settle of an automation.
        manager
            .create_table(
                Table::create()
                    .table(AutomationRun::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AutomationRun::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AutomationRun::AutomationId)
                            .integer()
                            .not_null(),
                    )
                    // running | succeeded | failed | cancelled | skipped
                    .col(ColumnDef::new(AutomationRun::Status).string().not_null())
                    // 'schedule' | 'manual' — provenance of this run.
                    .col(ColumnDef::new(AutomationRun::Trigger).string().not_null())
                    // The UTC instant this run was scheduled for (audit of misses).
                    .col(
                        ColumnDef::new(AutomationRun::ScheduledFor)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AutomationRun::StartedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AutomationRun::EndedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    // Lazily-created produced conversation; SET NULL on its delete.
                    .col(
                        ColumnDef::new(AutomationRun::ConversationId)
                            .integer()
                            .null(),
                    )
                    // In-process ACP connection UUID (not durable across restart).
                    .col(ColumnDef::new(AutomationRun::ConnectionId).string().null())
                    // Worktree folder minted for this run (for GC / open).
                    .col(
                        ColumnDef::new(AutomationRun::WorktreeFolderId)
                            .integer()
                            .null(),
                    )
                    .col(ColumnDef::new(AutomationRun::StopReason).string().null())
                    .col(ColumnDef::new(AutomationRun::Error).text().null())
                    .col(ColumnDef::new(AutomationRun::Summary).text().null())
                    .col(
                        ColumnDef::new(AutomationRun::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AutomationRun::Table, AutomationRun::AutomationId)
                            .to(Automation::Table, Automation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AutomationRun::Table, AutomationRun::ConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        // History listing: per-automation, newest first.
        manager
            .create_index(
                Index::create()
                    .name("idx_automation_run_automation_created")
                    .table(AutomationRun::Table)
                    .col(AutomationRun::AutomationId)
                    .col(AutomationRun::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // Active-run sweeps / reconcile.
        manager
            .create_index(
                Index::create()
                    .name("idx_automation_run_status")
                    .table(AutomationRun::Table)
                    .col(AutomationRun::Status)
                    .to_owned(),
            )
            .await?;

        // At most one in-flight run per automation — a hard DB backstop against a
        // duplicate concurrent fire slipping past the in-process overlap guard
        // (e.g. two engine processes sharing one data dir). `running` is the only
        // non-terminal status, so a partial unique index on it covers the entire
        // active set while exempting the many terminal rows per automation.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_automation_run_one_active \
                 ON automation_run (automation_id) WHERE status = 'running'",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(AutomationRun::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Automation::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Automation {
    Table,
    Id,
    Name,
    Enabled,
    TriggerKind,
    Cron,
    Timezone,
    NextRunAt,
    AgentType,
    RootFolderId,
    Isolation,
    Branch,
    IsRemoteBranch,
    Config,
    LastRunAt,
    LastRunStatus,
    LastRunConversationId,
    UnseenFailures,
    CreatedAt,
    UpdatedAt,
    DeletedAt,
}

#[derive(DeriveIden)]
enum AutomationRun {
    Table,
    Id,
    AutomationId,
    Status,
    Trigger,
    ScheduledFor,
    StartedAt,
    EndedAt,
    ConversationId,
    ConnectionId,
    WorktreeFolderId,
    StopReason,
    Error,
    Summary,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Id,
}
