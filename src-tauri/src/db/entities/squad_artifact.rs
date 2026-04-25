use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "squad_artifact")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub squad_run_id: i32,
    pub squad_role_run_id: Option<i32>,
    pub task_id: Option<i32>,
    pub role_kind: Option<String>,
    pub artifact_type: String,
    pub title: String,
    pub content_json: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::squad_run::Entity",
        from = "Column::SquadRunId",
        to = "super::squad_run::Column::Id"
    )]
    SquadRun,
}

impl Related<super::squad_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SquadRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
