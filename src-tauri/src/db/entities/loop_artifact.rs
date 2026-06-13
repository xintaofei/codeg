use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

use super::loop_artifact_revision::ActorKind;

/// DAG node kind = column in the per-issue lineage graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    #[sea_orm(string_value = "issue")]
    Issue,
    #[sea_orm(string_value = "requirement")]
    Requirement,
    #[sea_orm(string_value = "design")]
    Design,
    #[sea_orm(string_value = "task")]
    Task,
    #[sea_orm(string_value = "review")]
    Review,
    #[sea_orm(string_value = "result")]
    Result,
}

/// Engine-driven node status (humans never hand-edit these except via gates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatus {
    #[sea_orm(string_value = "pending")]
    Pending,
    #[sea_orm(string_value = "in_progress")]
    InProgress,
    #[sea_orm(string_value = "awaiting_approval")]
    AwaitingApproval,
    #[sea_orm(string_value = "done")]
    Done,
    #[sea_orm(string_value = "blocked")]
    Blocked,
    #[sea_orm(string_value = "superseded")]
    Superseded,
    #[sea_orm(string_value = "cancelled")]
    Cancelled,
}

/// Verdict carried only by `kind = review` artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    #[sea_orm(string_value = "pass")]
    Pass,
    #[sea_orm(string_value = "fail")]
    Fail,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_artifact")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub space_id: i32,
    pub issue_id: i32,
    pub kind: ArtifactKind,
    pub title: String,
    pub status: ArtifactStatus,
    pub origin: ActorKind,
    /// Iteration that produced this node (plain column, no FK — cycle break).
    pub produced_by_iteration_id: Option<i32>,
    /// Only set for `kind = review`.
    pub verdict: Option<ReviewVerdict>,
    /// Node-level rework counter (no-progress circuit breaker reads this).
    pub attempt: i32,
    pub last_failure_sig: Option<String>,
    pub sort: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
