use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_criterion")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Owning artifact (design or task). Reviews judge these criteria.
    pub artifact_id: i32,
    /// Auto-assigned label like `AC-1`.
    pub label: String,
    pub text: String,
    pub sort: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
