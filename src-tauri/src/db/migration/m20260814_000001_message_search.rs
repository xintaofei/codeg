use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MessageSearchDocument::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MessageSearchDocument::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(MessageSearchDocument::ConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageSearchDocument::Text)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageSearchDocument::ContentHash)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageSearchDocument::SourceEndedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(MessageSearchDocument::SourceMessageCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(MessageSearchDocument::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_message_search_document_conversation")
                    .table(MessageSearchDocument::Table)
                    .col(MessageSearchDocument::ConversationId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SearchIndexState::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SearchIndexState::Id)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SearchIndexState::SchemaVersion)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(SearchIndexState::Mode)
                            .string()
                            .not_null()
                            .default("scan"),
                    )
                    .col(
                        ColumnDef::new(SearchIndexState::ThresholdMb)
                            .double()
                            .not_null()
                            .default(40.0),
                    )
                    .col(
                        ColumnDef::new(SearchIndexState::ShortFtsEnabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(SearchIndexState::ShortThresholdMb)
                            .double()
                            .not_null()
                            .default(40.0),
                    )
                    .col(ColumnDef::new(SearchIndexState::ScanMsPerMb).double().null())
                    .col(
                        ColumnDef::new(SearchIndexState::IndexedConversationCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(SearchIndexState::LastCalibrationAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(SearchIndexState::LastBackfillAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(SearchIndexState::UserEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(SearchIndexState::UserMode)
                            .string()
                            .not_null()
                            .default("auto"),
                    )
                    .to_owned(),
            )
            .await?;

        let conn = manager.get_connection();
        conn.execute_unprepared(
            "CREATE VIRTUAL TABLE IF NOT EXISTS message_search_trigram USING fts5(\
             text, content='', contentless_delete=1, detail=none, tokenize='trigram')",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE VIRTUAL TABLE IF NOT EXISTS message_search_short USING fts5(\
             words, bigrams, content='', contentless_delete=1, detail=none, \
             tokenize='unicode61 remove_diacritics 2')",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("DROP TABLE IF EXISTS message_search_trigram")
            .await?;
        conn.execute_unprepared("DROP TABLE IF EXISTS message_search_short")
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(MessageSearchDocument::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(SearchIndexState::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum MessageSearchDocument {
    Table,
    Id,
    ConversationId,
    Text,
    ContentHash,
    SourceEndedAt,
    SourceMessageCount,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum SearchIndexState {
    Table,
    Id,
    SchemaVersion,
    Mode,
    ThresholdMb,
    ShortFtsEnabled,
    ShortThresholdMb,
    ScanMsPerMb,
    IndexedConversationCount,
    LastCalibrationAt,
    LastBackfillAt,
    UserEnabled,
    UserMode,
}
