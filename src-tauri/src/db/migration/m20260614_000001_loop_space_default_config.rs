use sea_orm_migration::prelude::*;

/// Space-level default issue config + per-issue inherit flag. `loop_space`
/// carries an optional JSON `IssueConfig` (NULL = engine default); each
/// `loop_issue` gains a boolean marking whether it inherits that default
/// (resolved at read time) or uses its own stored config.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(LoopSpace::Table)
                    .add_column(ColumnDef::new(LoopSpace::DefaultConfig).text().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(LoopIssue::Table)
                    .add_column(
                        ColumnDef::new(LoopIssue::ConfigInherits)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(LoopIssue::Table)
                    .drop_column(LoopIssue::ConfigInherits)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(LoopSpace::Table)
                    .drop_column(LoopSpace::DefaultConfig)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum LoopSpace {
    Table,
    DefaultConfig,
}

#[derive(DeriveIden)]
enum LoopIssue {
    Table,
    ConfigInherits,
}
