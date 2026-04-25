use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "squad_task")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub squad_run_id: i32,
    pub assigned_role_kind: String,
    pub title: String,
    pub description: String,
    pub input_summary: Option<String>,
    pub status: String,
    pub depends_on_json: Option<String>,
    pub priority: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub completed_at: Option<DateTimeUtc>,
    pub error_message: Option<String>,
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
