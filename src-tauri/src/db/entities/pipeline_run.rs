use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "pipeline_run")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub pipeline_id: Option<i32>,
    pub folder_id: i32,
    pub worktree_folder_id: Option<i32>,
    pub parent_conversation_id: Option<i32>,
    #[sea_orm(column_type = "Text")]
    pub graph: String,
    pub status: String,
    pub isolation: String,
    pub display_text: Option<String>,
    pub current_step_id: Option<String>,
    pub current_iteration: i32,
    pub error: Option<String>,
    pub started_at: DateTimeUtc,
    pub ended_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::pipeline::Entity",
        from = "Column::PipelineId",
        to = "super::pipeline::Column::Id",
        on_update = "NoAction",
        on_delete = "SetNull"
    )]
    Pipeline,
    #[sea_orm(has_many = "super::pipeline_attempt::Entity")]
    Attempts,
}

impl Related<super::pipeline::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Pipeline.def()
    }
}

impl Related<super::pipeline_attempt::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Attempts.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
