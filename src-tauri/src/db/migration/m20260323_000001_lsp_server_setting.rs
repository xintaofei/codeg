use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(LspServerSetting::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LspServerSetting::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(LspServerSetting::ServerId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(LspServerSetting::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(LspServerSetting::SortOrder)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(LspServerSetting::InstalledVersion)
                            .string()
                            .null(),
                    )
                    .col(ColumnDef::new(LspServerSetting::ConfigJson).text().null())
                    .col(
                        ColumnDef::new(LspServerSetting::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(LspServerSetting::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_lsp_server_setting_sort_order")
                    .table(LspServerSetting::Table)
                    .col(LspServerSetting::SortOrder)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(LspServerSetting::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum LspServerSetting {
    Table,
    Id,
    ServerId,
    Enabled,
    SortOrder,
    InstalledVersion,
    ConfigJson,
    CreatedAt,
    UpdatedAt,
}
