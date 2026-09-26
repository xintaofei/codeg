use sea_orm_migration::prelude::*;

/// Bookkeeping for the message search indexer: which conversations
/// `message_fts` covers, and as of which `conversation.updated_at`. The indexer
/// re-indexes a conversation only when its `updated_at` no longer matches, so
/// a pass over an unchanged workspace parses nothing. Created with raw SQL to
/// sit next to the FTS5 table it describes (no entity: only the indexer reads
/// it).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS message_fts_state (
                    conversation_id INTEGER PRIMARY KEY NOT NULL,
                    indexed_updated_at TEXT NOT NULL,
                    indexed_at TEXT NOT NULL
                )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS message_fts_state")
            .await?;
        Ok(())
    }
}
