use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(CustomAgent::Table)
                    // User declaration that the agent reads the shared
                    // `.agents/skills` store; gates every skills surface.
                    // Defaults off — codeg cannot detect an arbitrary ACP
                    // agent's skill directory, so the user opts in.
                    .add_column(
                        ColumnDef::new(CustomAgent::SkillsSharedStore)
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
                    .table(CustomAgent::Table)
                    .drop_column(CustomAgent::SkillsSharedStore)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum CustomAgent {
    Table,
    SkillsSharedStore,
}
