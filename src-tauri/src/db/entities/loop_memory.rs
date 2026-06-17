use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

use super::loop_artifact_revision::ActorKind;

/// Memory category. `constitution` carries the space-level charter; the rest are
/// learnings injected per stage (see the briefing matrix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    #[sea_orm(string_value = "constitution")]
    Constitution,
    #[sea_orm(string_value = "constraint")]
    Constraint,
    #[sea_orm(string_value = "decision")]
    Decision,
    #[sea_orm(string_value = "preference")]
    Preference,
    #[sea_orm(string_value = "pitfall")]
    Pitfall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    #[sea_orm(string_value = "active")]
    Active,
    #[sea_orm(string_value = "archived")]
    Archived,
    #[sea_orm(string_value = "superseded")]
    Superseded,
}

/// How much a memory is trusted: `human`-authored, `distilled` by the reflect
/// stage from confirmed work, or `proposed` (agent-recorded, unvetted). Shown in
/// the briefing index for the agent's judgment — never used to rank or filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    #[sea_orm(string_value = "human")]
    Human,
    #[sea_orm(string_value = "distilled")]
    Distilled,
    #[sea_orm(string_value = "proposed")]
    Proposed,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_memory")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub space_id: i32,
    pub kind: MemoryKind,
    pub source: ActorKind,
    pub title: String,
    pub summary: Option<String>,
    pub content: String,
    pub trust_tier: TrustTier,
    pub status: MemoryStatus,
    pub superseded_by: Option<i32>,
    pub source_issue_id: Option<i32>,
    pub source_artifact_id: Option<i32>,
    pub produced_by_iteration_id: Option<i32>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
