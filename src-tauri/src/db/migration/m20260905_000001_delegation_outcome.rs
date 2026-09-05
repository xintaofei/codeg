use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Immutable store of COMPLETED delegation results (`delegation_outcome`).
///
/// PR1 scope (v2 design §4.1): one row per broker `task_id` (= the child row's
/// `delegation_call_id`), written ONCE when the task's current execution wins
/// the terminal race with a SUCCESSFUL outcome. Canceled / failed terminals are
/// deliberately NEVER written here — a canceled task keeps the upstream
/// `resume_delegation` path, which re-runs the SAME task id until it either
/// completes (then the success result is frozen) or stays interrupted. Only the
/// first successful result for a task is stored; later writes for the same task
/// can never overwrite it (the service layer compares and reports a conflict —
/// the DB enforces "insert only" via the primary key).
///
/// `resume_binding_json` carries the non-sensitive facts needed to strictly
/// re-attach the same external agent session later (external session id,
/// canonical cwd, agent type, execution-config fingerprint). It must NEVER
/// contain tokens, API keys, or environment variables.
///
/// No foreign keys: the frozen result outlives the child conversation row
/// (history stays readable after the child is deleted) and follows the parent
/// conversation's data lifecycle.
const TABLE: &str = "delegation_outcome";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new(TABLE))
                    .col(
                        ColumnDef::new(Alias::new("task_id"))
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("parent_conversation_id"))
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("parent_tool_use_id")).text().null())
                    .col(
                        ColumnDef::new(Alias::new("child_conversation_id"))
                            .integer()
                            .null(),
                    )
                    .col(ColumnDef::new(Alias::new("agent_type")).text().not_null())
                    // Only success results are frozen; the check makes the
                    // "no canceled/failed history" contract explicit in the
                    // schema itself.
                    .col(
                        ColumnDef::new(Alias::new("status"))
                            .text()
                            .not_null()
                            .check(Expr::col(Alias::new("status")).eq("completed")),
                    )
                    .col(ColumnDef::new(Alias::new("text")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("duration_ms"))
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("text_truncated")).boolean().not_null())
                    .col(
                        ColumnDef::new(Alias::new("completed_at"))
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("schema_version"))
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(ColumnDef::new(Alias::new("resume_binding_json")).text().null())
                    .to_owned(),
            )
            .await?;

        // Parent-scoped listing support (which tasks does this parent own?) —
        // the per-task lookup hits the primary key.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_delegation_outcome_parent")
                    .table(Alias::new(TABLE))
                    .col(Alias::new("parent_conversation_id"))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .if_exists()
                    .name("idx_delegation_outcome_parent")
                    .table(Alias::new(TABLE))
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Alias::new(TABLE)).to_owned())
            .await
    }
}
