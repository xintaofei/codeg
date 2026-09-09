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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
    use sea_orm_migration::{MigratorTrait, SchemaManager};

    use crate::acp::delegation::spawner::DelegationLink;
    use crate::db::migration::Migrator;
    use crate::db::service::{conversation_service, delegation_task_service, folder_service};
    use crate::models::AgentType;

    fn sql(statement: &str) -> Statement {
        Statement::from_string(DbBackend::Sqlite, statement.to_owned())
    }

    async fn legacy_projection(
        conn: &sea_orm::DatabaseConnection,
        parent_id: i32,
        completed_task_id: &str,
        interrupted_task_id: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "parent": conversation_service::get_by_id(conn, parent_id)
                .await
                .expect("legacy parent"),
            "children": conversation_service::list_children(conn, parent_id)
                .await
                .expect("legacy children"),
            "completed_task_child": conversation_service::get_by_delegation_call_id(
                conn,
                completed_task_id,
            )
                .await
                .expect("legacy completed task child"),
            "interrupted_task_child": conversation_service::get_by_delegation_call_id(
                conn,
                interrupted_task_id,
            )
                .await
                .expect("legacy interrupted task child"),
        })
    }

    /// Reproduce an on-disk database at upstream/main's last schema before the
    /// durable ledger existed. The migration must leave all legacy delegation
    /// history readable exactly as before and make the new ledger usable.
    #[tokio::test]
    async fn upgrades_pre_ledger_database_without_rewriting_legacy_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pre-ledger.db");
        let url = format!("sqlite:{}?mode=rwc", path.to_string_lossy());
        let conn = Database::connect(url.clone())
            .await
            .expect("open pre-ledger db");
        conn.execute(sql("PRAGMA foreign_keys=ON;"))
            .await
            .expect("foreign keys");

        let migrations = <Migrator as MigratorTrait>::migrations();
        let ledger_idx = migrations
            .iter()
            .position(|migration| migration.name() == "m20260908_000001_delegation_task_ledger")
            .expect("ledger migration is registered");
        Migrator::up(&conn, Some(ledger_idx as u32))
            .await
            .expect("apply upstream pre-ledger schema");
        assert!(
            !SchemaManager::new(&conn)
                .has_table("delegation_task")
                .await
                .expect("inspect pre-ledger schema"),
            "the fixture must start before the ledger table exists"
        );

        let folder = folder_service::add_folder(&conn, "/workspace/legacy-project")
            .await
            .expect("legacy folder")
            .id;
        let parent = conversation_service::create(
            &conn,
            folder,
            AgentType::ClaudeCode,
            Some("Legacy parent".into()),
            Some("legacy-branch".into()),
        )
        .await
        .expect("legacy parent");
        let legacy_task_id = "legacy-delegation-call";
        let child = conversation_service::create_with_delegation(
            &conn,
            folder,
            AgentType::Codex,
            Some("Legacy completed child".into()),
            Some("legacy-branch".into()),
            Some(DelegationLink {
                parent_conversation_id: parent.id,
                parent_tool_use_id: "legacy-tool-use".into(),
                delegation_call_id: legacy_task_id.into(),
                admission: None,
            }),
        )
        .await
        .expect("legacy child");
        conversation_service::bind_external_id(&conn, child.id, "legacy-session", &[])
            .await
            .expect("legacy external session");
        conversation_service::update_status(
            &conn,
            child.id,
            crate::db::entities::conversation::ConversationStatus::Completed,
        )
        .await
        .expect("legacy completed status");
        conn.execute(sql(&format!(
            "UPDATE conversation SET message_count = 7 WHERE id = {}",
            child.id
        )))
        .await
        .expect("legacy message history");

        let interrupted_task_id = "legacy-interrupted-call";
        let interrupted_child = conversation_service::create_with_delegation(
            &conn,
            folder,
            AgentType::Codex,
            Some("Legacy interrupted child".into()),
            Some("legacy-branch".into()),
            Some(DelegationLink {
                parent_conversation_id: parent.id,
                parent_tool_use_id: "legacy-interrupted-tool-use".into(),
                delegation_call_id: interrupted_task_id.into(),
                admission: None,
            }),
        )
        .await
        .expect("legacy interrupted child");
        conversation_service::bind_external_id(
            &conn,
            interrupted_child.id,
            "legacy-interrupted-session",
            &[],
        )
        .await
        .expect("legacy interrupted external session");

        let before = legacy_projection(&conn, parent.id, legacy_task_id, interrupted_task_id).await;
        conn.close().await.expect("close pre-ledger db");

        // Production applies pending migrations on a fresh, single connection
        // at startup. Reopen here so no connection-local schema cache from the
        // fixture setup can make this easier than a real application upgrade.
        let conn = Database::connect(url).await.expect("reopen for upgrade");
        conn.execute(sql("PRAGMA foreign_keys=ON;"))
            .await
            .expect("foreign keys after reopen");
        Migrator::up(&conn, Some(1))
            .await
            .expect("apply ledger migration");
        assert!(
            SchemaManager::new(&conn)
                .has_table("delegation_task")
                .await
                .expect("inspect upgraded schema"),
            "the upgrade must create the ledger table"
        );
        assert_eq!(
            legacy_projection(&conn, parent.id, legacy_task_id, interrupted_task_id).await,
            before,
            "adding the ledger must not rewrite legacy conversation history"
        );
        for task_id in [legacy_task_id, interrupted_task_id] {
            assert!(
                delegation_task_service::lookup(&conn, parent.id, task_id)
                    .await
                    .expect("legacy ledger lookup")
                    .is_none(),
                "the migration must not invent ledger facts for pre-ledger children"
            );
        }

        let new_task_id = "post-upgrade-task";
        let new_child = conversation_service::create_with_delegation(
            &conn,
            folder,
            AgentType::Codex,
            Some("Post-upgrade child".into()),
            None,
            Some(DelegationLink {
                parent_conversation_id: parent.id,
                parent_tool_use_id: "post-upgrade-tool-use".into(),
                delegation_call_id: new_task_id.into(),
                admission: None,
            }),
        )
        .await
        .expect("post-upgrade child");
        let binding = delegation_task_service::ResumeBinding {
            agent_type: AgentType::Codex,
            external_session_id: "post-upgrade-session".into(),
            child_conversation_id: new_child.id,
            working_dir: "/workspace/legacy-project".into(),
            preferred_mode_id: Some("default".into()),
            preferred_config_values: BTreeMap::new(),
            config_fingerprint: "post-upgrade-fingerprint".into(),
        };
        let admitted = delegation_task_service::admit(
            &conn,
            delegation_task_service::AdmissionInput {
                task_id: new_task_id.into(),
                parent_conversation_id: parent.id,
                child_conversation_id: new_child.id,
                source_task_id: None,
                task: "Verify the upgraded ledger".into(),
                requested_working_dir: None,
                resume_binding: binding.clone(),
            },
        )
        .await
        .expect("admit post-upgrade task");
        assert!(matches!(
            admitted,
            delegation_task_service::AdmissionResult::New { .. }
        ));
        let entry = delegation_task_service::lookup(&conn, parent.id, new_task_id)
            .await
            .expect("post-upgrade lookup")
            .expect("post-upgrade ledger entry");
        assert_eq!(entry.child_conversation_id, new_child.id);
        assert_eq!(entry.resume_binding, binding);
        assert_eq!(
            entry.status,
            crate::acp::delegation::types::TaskStatus::Running
        );
    }
}
