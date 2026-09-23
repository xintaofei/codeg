use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "pipeline_attempt")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub run_id: i32,
    pub step_id: String,
    pub iteration: i32,
    pub status: String,
    pub connection_id: Option<String>,
    pub conversation_id: Option<i32>,
    pub model_requested: Option<String>,
    pub model_actual: Option<String>,
    pub verdict: Option<String>,
    pub verdict_source: Option<String>,
    pub notes: Option<String>,
    pub summary: Option<String>,
    pub started_at: DateTimeUtc,
    pub ended_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::pipeline_run::Entity",
        from = "Column::RunId",
        to = "super::pipeline_run::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Run,
}

impl Related<super::pipeline_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Run.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
