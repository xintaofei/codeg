use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::agent::AgentType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadRoleKind {
    Conductor,
    Frontend,
    Backend,
    Worker,
}

impl SquadRoleKind {
    pub const fn all() -> [Self; 4] {
        [Self::Conductor, Self::Frontend, Self::Backend, Self::Worker]
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conductor => "conductor",
            Self::Frontend => "frontend",
            Self::Backend => "backend",
            Self::Worker => "worker",
        }
    }

    pub const fn default_workspace_policy(self) -> SquadWorkspacePolicy {
        match self {
            Self::Conductor => SquadWorkspacePolicy::ReadOnly,
            Self::Frontend | Self::Backend | Self::Worker => SquadWorkspacePolicy::WriteIsolated,
        }
    }

    pub const fn default_prompt(self) -> &'static str {
        match self {
            Self::Conductor => "Coordinate the squad, break the user goal into safe tasks, and summarize decisions and blockers.",
            Self::Frontend => "Act as the frontend role. Focus on UI, React, TypeScript, state management, accessibility, and i18n.",
            Self::Backend => "Act as the backend role. Focus on Rust, database schema, API boundaries, runtime safety, and correctness.",
            Self::Worker => "Act as the implementation worker. Execute scoped coding tasks carefully and report exact changed files and validation results.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadRunMode {
    Manual,
    ConductorDispatch,
    AllHandsReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadRunStatus {
    Pending,
    Preparing,
    Running,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadRoleRunStatus {
    Pending,
    Preparing,
    Connecting,
    Connected,
    Prompting,
    Stopped,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadTaskStatus {
    Pending,
    Assigned,
    Running,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadArtifactType {
    Summary,
    Diff,
    FilePatch,
    Review,
    Terminal,
    Plan,
    Warning,
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadWorkspacePolicy {
    ReadOnly,
    WriteIsolated,
    WriteShared,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadRoleProfileInfo {
    pub id: i32,
    pub folder_id: i32,
    pub role_kind: SquadRoleKind,
    pub enabled: bool,
    pub agent_type: AgentType,
    pub registry_id: String,
    pub model_provider_id: Option<i32>,
    pub model_id: Option<String>,
    pub env_json: Option<String>,
    pub system_prompt: String,
    pub workspace_policy: SquadWorkspacePolicy,
    pub default_run_mode: SquadRunMode,
    pub mode_id: Option<String>,
    pub config_options_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadRoleProfilePatch {
    pub enabled: Option<bool>,
    pub agent_type: Option<AgentType>,
    pub registry_id: Option<String>,
    pub model_provider_id: Option<i32>,
    pub model_id: Option<String>,
    pub env_json: Option<String>,
    pub system_prompt: Option<String>,
    pub workspace_policy: Option<SquadWorkspacePolicy>,
    pub default_run_mode: Option<SquadRunMode>,
    pub mode_id: Option<String>,
    pub config_options_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadRunInfo {
    pub id: i32,
    pub folder_id: i32,
    pub origin_conversation_id: Option<i32>,
    pub mode: SquadRunMode,
    pub status: SquadRunStatus,
    pub goal_summary: String,
    pub base_branch: Option<String>,
    pub isolation_mode: String,
    pub started_with_dirty_base: bool,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadRoleRunInfo {
    pub id: i32,
    pub squad_run_id: i32,
    pub role_kind: SquadRoleKind,
    pub role_profile_snapshot_json: String,
    pub connection_id: Option<String>,
    pub session_id: Option<String>,
    pub conversation_id: Option<i32>,
    pub workspace_path: Option<String>,
    pub branch_name: Option<String>,
    pub status: SquadRoleRunStatus,
    pub last_event_at: Option<String>,
    pub budget_state_json: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadTaskInfo {
    pub id: i32,
    pub squad_run_id: i32,
    pub assigned_role_kind: SquadRoleKind,
    pub title: String,
    pub description: String,
    pub input_summary: Option<String>,
    pub status: SquadTaskStatus,
    pub depends_on_json: Option<String>,
    pub priority: i32,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadArtifactInfo {
    pub id: i32,
    pub squad_run_id: i32,
    pub squad_role_run_id: Option<i32>,
    pub task_id: Option<i32>,
    pub role_kind: Option<SquadRoleKind>,
    pub artifact_type: SquadArtifactType,
    pub title: String,
    pub content_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadRunSnapshot {
    pub run: SquadRunInfo,
    pub roles: Vec<SquadRoleRunInfo>,
    pub tasks: Vec<SquadTaskInfo>,
    pub artifacts: Vec<SquadArtifactInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub squad_run_id: i32,
    pub seq: i64,
    pub at: DateTime<Utc>,
    pub role_kind: Option<SquadRoleKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}
