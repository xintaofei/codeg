use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MessageSearchDocument::Table)
                    .add_column(
                        ColumnDef::new(MessageSearchDocument::BlockOffsets)
                            .text()
                            .not_null()
                            .default("[]"),
                    )
                    .to_owned(),
            )
            .await?;

        // The new location metadata changes the normalized document format.
        // Bump the singleton search schema version so the background indexer
        // rebuilds old documents and fills the new column with real offsets.
        manager
            .get_connection()
            .execute_unprepared("DELETE FROM message_search_trigram")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DELETE FROM message_search_short")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DELETE FROM message_search_document")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("UPDATE search_index_state SET schema_version = 2 WHERE id = 1")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MessageSearchDocument::Table)
                    .drop_column(MessageSearchDocument::BlockOffsets)
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared("UPDATE search_index_state SET schema_version = 1 WHERE id = 1")
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum MessageSearchDocument {
    Table,
    BlockOffsets,
}
