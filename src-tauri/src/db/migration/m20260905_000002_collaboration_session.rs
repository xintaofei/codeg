use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Collaboration sessions and turns for the continuous-delegation MVP
/// (v2 design §4.2 / §4.3).
///
/// `collaboration_session` — the exclusive write relationship over ONE child
/// conversation, created when the first rework turn for a frozen completed
/// source is accepted. `source_task_id` and `child_conversation_id` are
/// UNIQUE: one session per source, one session per child, forever.
///
/// `collaboration_turn` — one rework round. States: accepted / preparing /
/// dispatching / running / cancel_requested (ACTIVE) and completed / failed /
/// canceled / interrupted / outcome_unknown (TERMINAL). The partial unique
/// index makes "at most one active turn per session" a database guarantee,
/// not an in-process promise.
const SESSION: &str = "collaboration_session";
const TURN: &str = "collaboration_turn";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new(SESSION))
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("source_task_id"))
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("parent_conversation_id"))
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("child_conversation_id"))
                            .integer()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Alias::new("state")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("resume_binding_json"))
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("created_at"))
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("closed_at"))
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Alias::new(TURN))
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("session_id")).text().not_null())
                    .col(ColumnDef::new(Alias::new("ordinal")).integer().not_null())
                    .col(ColumnDef::new(Alias::new("request_id")).text().not_null())
                    .col(ColumnDef::new(Alias::new("message")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("initiator_kind"))
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("initiator_parent_conversation_id"))
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("initiator_tool_use_id"))
                            .text()
                            .null(),
                    )
                    .col(ColumnDef::new(Alias::new("state")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("execution_id"))
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("connection_id")).text().null())
                    .col(ColumnDef::new(Alias::new("result_text")).text().null())
                    .col(
                        ColumnDef::new(Alias::new("text_truncated"))
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(Alias::new("error_code")).text().null())
                    .col(ColumnDef::new(Alias::new("error_message")).text().null())
                    .col(
                        ColumnDef::new(Alias::new("created_at"))
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("started_at"))
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("finished_at"))
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(Alias::new("version")).integer().not_null())
                    .to_owned(),
            )
            .await?;

        // Idempotency + ordering contracts.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_collab_turn_session_request")
                    .table(Alias::new(TURN))
                    .col(Alias::new("session_id"))
                    .col(Alias::new("request_id"))
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_collab_turn_session_ordinal")
                    .table(Alias::new(TURN))
                    .col(Alias::new("session_id"))
                    .col(Alias::new("ordinal"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Database-guaranteed single active turn per session. The coordinator's
        // CAS transitions are the first line of defense; this index is the
        // backstop that makes concurrent admission impossible, not just
        // unlikely.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_collaboration_turn_one_active \
                 ON collaboration_turn(session_id) \
                 WHERE state IN ('accepted','preparing','dispatching','running','cancel_requested')",
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_collab_turn_session_ordinal_order")
                    .table(Alias::new(TURN))
                    .col(Alias::new("session_id"))
                    .col(Alias::new("ordinal"))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for name in [
            "idx_collab_turn_session_ordinal_order",
            "idx_collaboration_turn_one_active",
            "idx_collab_turn_session_ordinal",
            "idx_collab_turn_session_request",
        ] {
            manager
                .drop_index(
                    Index::drop()
                        .if_exists()
                        .name(name)
                        .table(Alias::new(TURN))
                        .to_owned(),
                )
                .await?;
        }
        manager
            .drop_table(Table::drop().table(Alias::new(TURN)).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Alias::new(SESSION)).to_owned())
            .await
    }
}
