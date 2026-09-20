use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MemoryKind::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MemoryKind::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(MemoryKind::Key)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(MemoryKind::Name).string().not_null())
                    .col(ColumnDef::new(MemoryKind::Instruction).text().not_null())
                    .col(ColumnDef::new(MemoryKind::Mode).string().not_null())
                    .col(ColumnDef::new(MemoryKind::Builtin).boolean().not_null())
                    .col(ColumnDef::new(MemoryKind::Enabled).boolean().not_null())
                    .col(
                        ColumnDef::new(MemoryKind::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MemoryKind::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        let backend = manager.get_database_backend();
        let statements = [
            ("decision", "Decisions", "Record an architectural or product decision with its reason and alternatives rejected.", "auto"),
            ("fixed_bug", "Fixed bugs", "Record a bug that was found and fixed: symptom, root cause, fix.", "auto"),
            ("task_summary", "Task summaries", "Record what a task changed and how it was verified.", "on_request"),
            ("preference", "Facts and preferences", "Record a stable fact about the user, project or tooling that future runs should respect.", "on_request"),
        ];
        for (key, name, instruction, mode) in statements {
            manager
                .get_connection()
                .execute(Statement::from_sql_and_values(
                    backend,
                    r#"INSERT OR IGNORE INTO memory_kind (key, name, instruction, mode, builtin, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"#,
                    [key.into(), name.into(), instruction.into(), mode.into(), true.into(), true.into()],
                ))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MemoryKind::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum MemoryKind {
    Table,
    Id,
    Key,
    Name,
    Instruction,
    Mode,
    Builtin,
    Enabled,
    CreatedAt,
    UpdatedAt,
}
