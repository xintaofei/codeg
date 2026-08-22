use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationComposerDraft::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(ConversationComposerDraft::Attachments)
                            .text()
                            .not_null()
                            .default("[]"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationComposerDraft::Table)
                    .drop_column(ConversationComposerDraft::Attachments)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationComposerDraft {
    Table,
    Attachments,
}
