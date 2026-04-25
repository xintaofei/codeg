use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "squad_run")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub folder_id: i32,
    pub origin_conversation_id: Option<i32>,
    pub mode: String,
    pub status: String,
    pub goal_summary: String,
    pub base_branch: Option<String>,
    pub isolation_mode: String,
    pub started_with_dirty_base: bool,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub started_at: Option<DateTimeUtc>,
    pub completed_at: Option<DateTimeUtc>,
    pub cancelled_at: Option<DateTimeUtc>,
    pub error_message: Option<String>,
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
