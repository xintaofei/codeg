use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// DAG edge kind. Canonical direction: `from` = the dependent node
/// (derived/review/result), `to` = the referenced node (its source/parent/
/// subject). So `derives_from`: child→parent; `skips_to`: reached-node→
/// skipped-over ancestor; `reviews`: review→task; `results_from`: result→task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    #[sea_orm(string_value = "derives_from")]
    DerivesFrom,
    #[sea_orm(string_value = "skips_to")]
    SkipsTo,
    #[sea_orm(string_value = "reviews")]
    Reviews,
    #[sea_orm(string_value = "results_from")]
    ResultsFrom,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_link")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub space_id: i32,
    pub from_artifact_id: i32,
    pub to_artifact_id: i32,
    pub kind: LinkKind,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
