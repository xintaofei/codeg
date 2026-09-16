use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConversationComposerDraft::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationComposerDraft::ConversationId)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationComposerDraft::Text)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationComposerDraft::Revision)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationComposerDraft::Origin)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationComposerDraft::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ConversationComposerDraft::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationComposerDraft {
    Table,
    ConversationId,
    Text,
    Revision,
    Origin,
    UpdatedAt,
}
