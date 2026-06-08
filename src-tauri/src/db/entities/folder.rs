use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "folder")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    #[sea_orm(unique)]
    pub path: String,
    pub git_branch: Option<String>,
    pub default_agent_type: Option<String>,
    pub last_opened_at: DateTimeUtc,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub deleted_at: Option<DateTimeUtc>,
    pub is_open: bool,
    pub sort_order: i32,
    pub color: String,
    /// Root folder this one was created under (for worktree folders). NULL for
    /// top-level folders. Flattened: a worktree of a worktree still points at the
    /// original root, never an intermediate worktree.
    pub parent_id: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::conversation::Entity")]
    Conversations,

    #[sea_orm(has_many = "super::opened_tab::Entity")]
    OpenedTabs,

    #[sea_orm(has_many = "super::folder_command::Entity")]
    FolderCommands,
}

impl Related<super::conversation::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversations.def()
    }
}

impl Related<super::opened_tab::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OpenedTabs.def()
    }
}

impl Related<super::folder_command::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::FolderCommands.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
