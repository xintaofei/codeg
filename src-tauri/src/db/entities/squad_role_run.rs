use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "squad_role_run")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub squad_run_id: i32,
    pub role_kind: String,
    pub role_profile_snapshot_json: String,
    pub connection_id: Option<String>,
    pub session_id: Option<String>,
    pub conversation_id: Option<i32>,
    pub workspace_path: Option<String>,
    pub branch_name: Option<String>,
    pub status: String,
    pub last_event_at: Option<DateTimeUtc>,
    pub budget_state_json: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
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
