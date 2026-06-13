use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Who authored a write — a human or an agent. Shared across
/// `loop_artifact.origin`, `loop_artifact_revision.actor_kind` and
/// `loop_memory.source`. (Distinct from `loop_iteration::LaunchedBy`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    #[sea_orm(string_value = "human")]
    Human,
    #[sea_orm(string_value = "agent")]
    Agent,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_artifact_revision")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub artifact_id: i32,
    pub seq: i32,
    pub content: String,
    pub actor_kind: ActorKind,
    /// Iteration that produced this revision (plain column, no FK — breaks the
    /// artifact↔iteration cycle).
    pub iteration_id: Option<i32>,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
