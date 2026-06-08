use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::agent::AgentType;

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
    pub last_opened_at: DateTime<Utc>,
    pub sort_order: i32,
    pub color: String,
    /// Root folder this one was created under (worktree folders only); NULL for
    /// top-level folders. Drives sidebar merge + worktree-branch detection.
    pub parent_id: Option<i32>,
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

/// Response for `list_opened_tabs`: the persisted tab set plus the current
/// workspace tab version. Clients seed their compare-and-set / echo logic from
/// `version`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedTabsSnapshot {
    pub items: Vec<OpenedTab>,
    pub version: i64,
}

/// Response for `save_opened_tabs`: whether the compare-and-set was applied, the
/// authoritative version after the call, and the canonical tab set. When
/// `accepted` is false the save was stale (another client won) and `tabs` is the
/// current truth to reconcile against.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveTabsOutcome {
    pub accepted: bool,
    pub version: i64,
    pub tabs: Vec<OpenedTab>,
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
