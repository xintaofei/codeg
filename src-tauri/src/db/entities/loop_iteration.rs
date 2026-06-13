use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// What an iteration's agent run does. (Note: `verify` is NOT a stage — it is a
/// deterministic engine step run between implement and review.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    #[sea_orm(string_value = "triage")]
    Triage,
    #[sea_orm(string_value = "refine")]
    Refine,
    #[sea_orm(string_value = "design")]
    Design,
    #[sea_orm(string_value = "plan")]
    Plan,
    #[sea_orm(string_value = "implement")]
    Implement,
    #[sea_orm(string_value = "review")]
    Review,
    #[sea_orm(string_value = "finalize")]
    Finalize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum IterationStatus {
    #[sea_orm(string_value = "queued")]
    Queued,
    #[sea_orm(string_value = "running")]
    Running,
    #[sea_orm(string_value = "succeeded")]
    Succeeded,
    #[sea_orm(string_value = "failed")]
    Failed,
    #[sea_orm(string_value = "interrupted")]
    Interrupted,
    #[sea_orm(string_value = "cancelled")]
    Cancelled,
}

/// Who launched the iteration. Engine-driven by default; `human` covers extra
/// turns a person injects while observing. (Distinct from `ActorKind`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum LaunchedBy {
    #[sea_orm(string_value = "engine")]
    Engine,
    #[sea_orm(string_value = "human")]
    Human,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_iteration")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub space_id: i32,
    pub issue_id: i32,
    pub stage: Stage,
    /// Node being advanced/reviewed (plain column, no FK — cycle break).
    pub target_artifact_id: Option<i32>,
    /// Review slot `[0, reviewer_count)`; NULL for non-review stages.
    pub slot_no: Option<i32>,
    /// Backing loop conversation (`conversation.id`, plain column). NULL between
    /// lease acquisition and conversation creation.
    pub conversation_id: Option<i32>,
    /// Unique secret injected into codeg-mcp; the host reverse-looks-up this
    /// iteration's context from it (never trusts agent-supplied ids).
    pub capability_token: String,
    pub status: IterationStatus,
    pub launched_by: LaunchedBy,
    pub attempt: i32,
    pub tokens_used: i64,
    /// JSON-encoded briefing manifest (audit).
    pub context_manifest: Option<String>,
    pub created_at: DateTimeUtc,
    pub started_at: Option<DateTimeUtc>,
    pub ended_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
