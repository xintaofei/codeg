use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(CanvasNode::Table)
                    .add_column(ColumnDef::new(CanvasNode::PipelineId).integer().null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_canvas_node_pipeline_id")
                    .table(CanvasNode::Table)
                    .col(CanvasNode::PipelineId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_canvas_node_pipeline_id")
                    .table(CanvasNode::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(CanvasNode::Table)
                    .drop_column(CanvasNode::PipelineId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum CanvasNode {
    Table,
    PipelineId,
}
