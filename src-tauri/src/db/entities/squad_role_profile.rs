use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "squad_role_profile")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub folder_id: i32,
    pub role_kind: String,
    pub enabled: bool,
    pub agent_type: String,
    pub registry_id: String,
    pub model_provider_id: Option<i32>,
    pub model_id: Option<String>,
    pub env_json: Option<String>,
    pub system_prompt: String,
    pub workspace_policy: String,
    pub default_run_mode: String,
    pub mode_id: Option<String>,
    pub config_options_json: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
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
