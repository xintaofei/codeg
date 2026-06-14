use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_space")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    /// Bound root folder (must be a git repo). Plain column — cross-subsystem
    /// reference to `folder.id`, no FK.
    pub folder_id: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    /// Space default `IssueConfig` (JSON). NULL = engine default. Issues with
    /// `config_inherits` resolve their config from this at read time.
    pub default_config: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
