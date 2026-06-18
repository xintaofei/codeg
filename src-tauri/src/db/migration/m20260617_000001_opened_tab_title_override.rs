use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(OpenedTab::Table)
                    .add_column(ColumnDef::new(OpenedTab::TitleOverride).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(OpenedTab::Table)
                    .drop_column(OpenedTab::TitleOverride)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum OpenedTab {
    Table,
    TitleOverride,
}
