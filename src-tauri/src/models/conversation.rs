use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::agent::AgentType;
use super::message::{MessageTurn, TurnUsage};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: String,
    pub agent_type: AgentType,
    pub folder_path: Option<String>,
    pub folder_name: Option<String>,
    pub title: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub message_count: u32,
    pub model: Option<String>,
    pub git_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbConversationSummary {
    pub id: i32,
    pub folder_id: i32,
    pub folder_name: Option<String>,
    pub folder_path: Option<String>,
    pub title: Option<String>,
    pub agent_type: AgentType,
    pub status: String,
    pub model: Option<String>,
    pub git_branch: Option<String>,
    pub external_id: Option<String>,
    pub message_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub total_usage: Option<TurnUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    pub total_duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_used_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_usage_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationDetail {
    pub summary: ConversationSummary,
    pub turns: Vec<MessageTurn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_stats: Option<SessionStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbConversationDetail {
    pub summary: DbConversationSummary,
    pub turns: Vec<MessageTurn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_stats: Option<SessionStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderInfo {
    pub path: String,
    pub name: String,
    pub agent_types: Vec<AgentType>,
    pub conversation_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStats {
    pub total_conversations: u32,
    pub total_messages: u32,
    pub by_agent: Vec<AgentConversationCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConversationCount {
    pub agent_type: AgentType,
    pub conversation_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidebarData {
    pub folders: Vec<FolderInfo>,
    pub stats: AgentStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported: u32,
    pub skipped: u32,
}
