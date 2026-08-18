use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // `detail=none` rejects column-filtered MATCH queries (`words :`, `bigrams :`),
        // which the short-query path relies on. Recreate the table with
        // `detail=column` so those queries work. Schema upgrade + repopulation are
        // handled by the indexer once it observes the older schema_version below.
        conn.execute_unprepared("DROP TABLE IF EXISTS message_search_short")
            .await?;
        conn.execute_unprepared(
            "CREATE VIRTUAL TABLE message_search_short USING fts5(\
             words, bigrams, content='', contentless_delete=1, detail=column, \
             tokenize='unicode61 remove_diacritics 2')",
        )
        .await?;

        // Columns reserved for the calibration / p95 watchdog / independent short
        // threshold design were never read or written. Drop them instead of
        // shipping dead schema.
        manager
            .alter_table(
                Table::alter()
                    .table(SearchIndexState::Table)
                    .drop_column(SearchIndexState::ScanMsPerMb)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SearchIndexState::Table)
                    .drop_column(SearchIndexState::LastCalibrationAt)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SearchIndexState::Table)
                    .drop_column(SearchIndexState::ShortThresholdMb)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("DROP TABLE IF EXISTS message_search_short")
            .await?;
        conn.execute_unprepared(
            "CREATE VIRTUAL TABLE message_search_short USING fts5(\
             words, bigrams, content='', contentless_delete=1, detail=none, \
             tokenize='unicode61 remove_diacritics 2')",
        )
        .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SearchIndexState::Table)
                    .add_column(
                        ColumnDef::new(SearchIndexState::ScanMsPerMb).double().null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SearchIndexState::Table)
                    .add_column(
                        ColumnDef::new(SearchIndexState::LastCalibrationAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SearchIndexState::Table)
                    .add_column(
                        ColumnDef::new(SearchIndexState::ShortThresholdMb)
                            .double()
                            .not_null()
                            .default(40.0),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum SearchIndexState {
    Table,
    ScanMsPerMb,
    LastCalibrationAt,
    ShortThresholdMb,
}
