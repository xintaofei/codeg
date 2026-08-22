use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum PkRoundStatus {
    #[sea_orm(string_value = "ready")]
    Ready,
    #[sea_orm(string_value = "running")]
    Running,
    #[sea_orm(string_value = "finished")]
    Finished,
    #[sea_orm(string_value = "canceled")]
    Canceled,
    #[sea_orm(string_value = "interrupted")]
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "pk_round")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub folder_id: i32,
    #[sea_orm(column_type = "Text")]
    pub task: String,
    #[sea_orm(column_type = "Text")]
    pub config: String,
    pub status: PkRoundStatus,
    pub failure_reason: Option<String>,
    /// JSON: serialized judge verdict (scores, summary, raw text). Null if
    /// no judge was configured or the judge hasn't run yet.
    #[sea_orm(column_type = "Text", nullable)]
    pub judge_result: Option<String>,
    /// idle | running | done | error | skipped
    #[sea_orm(column_type = "Text", nullable)]
    pub judge_status: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub finished_at: Option<DateTimeUtc>,
    pub deleted_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::folder::Entity",
        from = "Column::FolderId",
        to = "super::folder::Column::Id"
    )]
    Folder,
}

impl Related<super::folder::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Folder.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
