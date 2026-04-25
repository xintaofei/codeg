use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::agent::AgentType;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WorkspacePreset {
    pub default_agent_type: Option<AgentType>,
    pub model_provider_id: Option<i32>,
    pub approval_policy: Option<String>,
    pub skill_ids: Vec<String>,
    pub mcp_server_ids: Vec<String>,
    pub env_overrides: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderHistoryEntry {
    pub id: i32,
    pub path: String,
    pub name: String,
    pub last_opened_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderDetail {
    pub id: i32,
    pub name: String,
    pub path: String,
    pub git_branch: Option<String>,
    pub default_agent_type: Option<AgentType>,
    pub workspace_preset: Option<WorkspacePreset>,
    pub last_opened_at: DateTime<Utc>,
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenedTab {
    pub id: i32,
    pub folder_id: i32,
    pub conversation_id: Option<i32>,
    pub agent_type: AgentType,
    pub position: i32,
    pub is_active: bool,
    pub is_pinned: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderCommandInfo {
    pub id: i32,
    pub folder_id: i32,
    pub name: String,
    pub command: String,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
