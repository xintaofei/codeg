use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ModelProvider::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ModelProvider::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ModelProvider::Name).string().not_null())
                    .col(ColumnDef::new(ModelProvider::ApiUrl).text().not_null())
                    .col(ColumnDef::new(ModelProvider::ApiKey).text().not_null())
                    .col(
                        ColumnDef::new(ModelProvider::AgentTypesJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ModelProvider::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ModelProvider::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ModelProvider::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ModelProvider {
    Table,
    Id,
    Name,
    ApiUrl,
    ApiKey,
    AgentTypesJson,
    CreatedAt,
    UpdatedAt,
}
