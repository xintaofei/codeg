use sea_orm_migration::prelude::*;

/// Full-text index over conversation message content, behind the ⌘K
/// "Messages" search. A standalone FTS5 table (no external content table):
/// the indexer rewrites a conversation's rows wholesale — one DELETE plus one
/// INSERT per turn — whenever the conversation changed, which keeps writes
/// simple and lets the agent parsers evolve without schema churn.
/// `conversation_id`, `turn_idx` and `role` are UNINDEXED metadata (returned
/// with a hit, never matched); matching happens on `content` only.
///
/// The `trigram` tokenizer indexes every three-character sequence, so a query
/// matches any substring of three or more characters: part of a word, or a
/// word inside Chinese or Japanese text written without spaces. Matching is
/// case-insensitive and ignores diacritics.
///
/// Created with raw SQL — SeaORM's schema builder has no FTS5 virtual-table
/// concept.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
                    content,
                    conversation_id UNINDEXED,
                    turn_idx UNINDEXED,
                    role UNINDEXED,
                    tokenize = 'trigram remove_diacritics 1'
                )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS message_fts")
            .await?;
        Ok(())
    }
}
