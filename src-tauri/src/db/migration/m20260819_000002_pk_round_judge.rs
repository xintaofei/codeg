use sea_orm_migration::prelude::*;

/// Add `judge_result` and `judge_status` columns to `pk_round`.
///
/// The judge agent's structured verdict (scores, summary, raw text) was
/// previously live-only in the Zustand store — lost on refresh or restart.
/// Persisting it to the DB makes judge results survive across sessions,
/// which is essential for the export-report and share-screenshot flows.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PkRound::Table)
                    .add_column(ColumnDef::new(PkRound::JudgeResult).text().null())
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(PkRound::Table)
                    .add_column(
                        ColumnDef::new(PkRound::JudgeStatus)
                            .string()
                            .not_null()
                            .default("idle"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(PkRound::Table)
                    .drop_column(PkRound::JudgeStatus)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(PkRound::Table)
                    .drop_column(PkRound::JudgeResult)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum PkRound {
    Table,
    JudgeResult,
    JudgeStatus,
}
