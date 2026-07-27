use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(CustomAgent::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CustomAgent::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    // The ACP registry id (e.g. "goose"). Also the identity in
                    // `AgentType::Custom`, the transcript directory name, and
                    // the binary-cache key — hence UNIQUE.
                    .col(
                        ColumnDef::new(CustomAgent::RegistryId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(CustomAgent::Name).string().not_null())
                    .col(
                        ColumnDef::new(CustomAgent::Description)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(CustomAgent::Version).string().not_null())
                    // "npx" | "uvx" | "binary" — which channel of `spec_json`
                    // to launch through when an agent publishes several.
                    .col(
                        ColumnDef::new(CustomAgent::DistributionKind)
                            .string()
                            .not_null(),
                    )
                    // The ACP registry's `distribution` object, verbatim, so a
                    // registry schema addition needs no migration here.
                    .col(ColumnDef::new(CustomAgent::SpecJson).text().not_null())
                    .col(ColumnDef::new(CustomAgent::IconUrl).string().null())
                    .col(
                        ColumnDef::new(CustomAgent::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CustomAgent::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(CustomAgent::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum CustomAgent {
    Table,
    Id,
    RegistryId,
    Name,
    Description,
    Version,
    DistributionKind,
    SpecJson,
    IconUrl,
    CreatedAt,
    UpdatedAt,
}
