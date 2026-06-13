use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Human-set urgency hint; influences (but does not strictly order) which issues
/// the engine surfaces first.
#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum IssuePriority {
    #[sea_orm(string_value = "high")]
    High,
    #[sea_orm(string_value = "medium")]
    Medium,
    #[sea_orm(string_value = "low")]
    Low,
}

/// Issue lifecycle. `pending` = created but not triggered (the explicit human
/// gate); `running` = driver active; `paused` = stopped dispatching (see
/// `pause_reason`); `blocked` = needs a human via the inbox; terminal `done` /
/// `cancelled`.
#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    #[sea_orm(string_value = "pending")]
    Pending,
    #[sea_orm(string_value = "running")]
    Running,
    #[sea_orm(string_value = "paused")]
    Paused,
    #[sea_orm(string_value = "blocked")]
    Blocked,
    #[sea_orm(string_value = "done")]
    Done,
    #[sea_orm(string_value = "cancelled")]
    Cancelled,
}

/// Distinguishes a manual pause from a budget circuit-breaker pause. Only
/// meaningful while `status = paused`.
#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    #[sea_orm(string_value = "manual")]
    Manual,
    #[sea_orm(string_value = "budget")]
    Budget,
}

/// Pipeline route decided by triage (or forced via config). `full` runs
/// refine→design→plan; `skip_design` skips design; `direct` skips both refine
/// and design (issue→plan). `undecided` until triage runs.
#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum IssueRoute {
    #[sea_orm(string_value = "undecided")]
    Undecided,
    #[sea_orm(string_value = "full")]
    Full,
    #[sea_orm(string_value = "skip_design")]
    SkipDesign,
    #[sea_orm(string_value = "direct")]
    Direct,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_issue")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub space_id: i32,
    pub seq_no: i32,
    pub title: String,
    pub description: String,
    pub priority: IssuePriority,
    pub status: IssueStatus,
    pub pause_reason: Option<PauseReason>,
    pub route: IssueRoute,
    /// JSON-encoded `models::loops::IssueConfig`.
    pub config: String,
    /// Engine-created worktree folder (`folder.id`, plain column).
    pub worktree_folder_id: Option<i32>,
    /// Merge baseline recorded at trigger time.
    pub base_branch: Option<String>,
    pub base_commit: Option<String>,
    /// Per-issue serial-task pipeline gate: the task whose implement/review
    /// cycle currently holds the issue worktree. NULL = a new task may start.
    pub active_task_artifact_id: Option<i32>,
    pub token_used: i64,
    /// NULL = unlimited (no artificial budget cap by default).
    pub token_budget: Option<i64>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub triggered_at: Option<DateTimeUtc>,
    pub ended_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
