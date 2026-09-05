use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConversationEditHidden::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationEditHidden::ConversationId)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationEditHidden::HiddenTsJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationEditHidden::UpdatedAt)
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
                    .table(ConversationEditHidden::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationEditHidden {
    Table,
    ConversationId,
    HiddenTsJson,
    UpdatedAt,
}
