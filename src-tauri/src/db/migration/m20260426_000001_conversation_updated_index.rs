use sea_orm_migration::prelude::*;

/// Adds a covering index on (deleted_at, updated_at DESC) to speed up
/// the very common `list_all` query, which orders by `updated_at DESC`
/// while filtering out soft-deleted rows.
///
/// The pre-existing `idx_conversation_deleted_created (deleted_at, created_at)`
/// does NOT cover the typical sort key (updated_at), forcing SQLite into
/// a full-table scan + filesort once the table grows.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_deleted_updated")
                    .table(Conversation::Table)
                    .col(Conversation::DeletedAt)
                    .col(Conversation::UpdatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_conversation_deleted_updated")
                    .table(Conversation::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    DeletedAt,
    UpdatedAt,
}
