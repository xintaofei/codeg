use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptInputBlock {
    Text {
        text: String,
    },
    Image {
        data: String,
        mime_type: String,
        #[serde(default)]
        uri: Option<String>,
    },
    Resource {
        uri: String,
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        blob: Option<String>,
    },
    ResourceLink {
        uri: String,
        name: String,
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        description: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptCapabilitiesInfo {
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}

/// Events pushed from Rust backend to frontend via Tauri event system.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpEvent {
    /// Agent returned text content (streaming delta)
    ContentDelta { connection_id: String, text: String },
    /// Agent thinking/reasoning
    Thinking { connection_id: String, text: String },
    /// Raw SDK message forwarded from Claude ACP extension notification
    ClaudeSdkMessage {
        connection_id: String,
        session_id: String,
        message: serde_json::Value,
    },
    /// Agent initiated a tool call
    ToolCall {
        connection_id: String,
        tool_call_id: String,
        title: String,
        kind: String,
        status: String,
        content: Option<String>,
        raw_input: Option<String>,
        raw_output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        locations: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<serde_json::Value>,
    },
    /// Tool call status/content updated
    ToolCallUpdate {
        connection_id: String,
        tool_call_id: String,
        title: Option<String>,
        status: Option<String>,
        content: Option<String>,
        raw_input: Option<String>,
        raw_output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_output_append: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        locations: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<serde_json::Value>,
    },
    /// Agent requests permission
    PermissionRequest {
        connection_id: String,
        request_id: String,
        tool_call: serde_json::Value,
        options: Vec<PermissionOptionInfo>,
    },
    /// Turn completed
    TurnComplete {
        connection_id: String,
        session_id: String,
        stop_reason: String,
        agent_type: String,
    },
    /// Session established with agent-assigned session ID
    SessionStarted {
        connection_id: String,
        session_id: String,
    },
    /// Session modes are available for this connection
    SessionModes {
        connection_id: String,
        modes: SessionModeStateInfo,
    },
    /// Session configuration options are available/updated for this connection
    SessionConfigOptions {
        connection_id: String,
        config_options: Vec<SessionConfigOptionInfo>,
    },
    /// Initial selector payloads (modes/config options) have been emitted
    SelectorsReady { connection_id: String },
    /// Prompt capabilities for this connection
    PromptCapabilities {
        connection_id: String,
        prompt_capabilities: PromptCapabilitiesInfo,
    },
    /// Whether the agent supports session/fork
    ForkSupported {
        connection_id: String,
        supported: bool,
    },
    /// Current session mode changed
    ModeChanged {
        connection_id: String,
        mode_id: String,
    },
    /// Agent reported plan update for current turn
    PlanUpdate {
        connection_id: String,
        entries: Vec<PlanEntryInfo>,
    },
    /// Connection status changed
    StatusChanged {
        connection_id: String,
        status: ConnectionStatus,
    },
    /// Error occurred
    Error {
        connection_id: String,
        message: String,
        agent_type: String,
        /// Stable machine-readable identifier (e.g. "initialize_timeout").
        /// When present, the frontend renders a localized message keyed on
        /// this code; otherwise it falls back to `message`.
        code: Option<String>,
    },
    /// Available slash commands updated
    AvailableCommands {
        connection_id: String,
        commands: Vec<AvailableCommandInfo>,
    },
    /// Session usage/context window updated during conversation
    UsageUpdate {
        connection_id: String,
        used: u64,
        size: u64,
    },
    /// Agent has begun compacting/summarizing context history mid-turn.
    /// The frontend should keep the spinner running but display a hint
    /// such as "Compacting context…" and reset its idle watchdog.
    CompactionStarted {
        connection_id: String,
        session_id: String,
    },
    /// Compaction finished; agent will resume normal streaming.
    CompactionFinished {
        connection_id: String,
        session_id: String,
    },
    /// Non-fatal stream parse/decode warning. Surfaced to the UI so a
    /// silent loop swallow can't masquerade as a hang.
    StreamWarning {
        connection_id: String,
        message: String,
    },
    /// Turn was aborted because no events arrived for too long. The
    /// frontend treats this exactly like TurnComplete so the user can
    /// resume sending messages.
    TurnIdleTimeout {
        connection_id: String,
        session_id: String,
        agent_type: String,
        idle_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionOptionInfo {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionModeInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionModeStateInfo {
    pub current_mode_id: String,
    pub available_modes: Vec<SessionModeInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfigSelectOptionInfo {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfigSelectGroupInfo {
    pub group: String,
    pub name: String,
    pub options: Vec<SessionConfigSelectOptionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfigSelectInfo {
    pub current_value: String,
    pub options: Vec<SessionConfigSelectOptionInfo>,
    pub groups: Vec<SessionConfigSelectGroupInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionConfigKindInfo {
    Select(SessionConfigSelectInfo),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfigOptionInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub kind: SessionConfigKindInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanEntryInfo {
    pub content: String,
    pub priority: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Prompting,
    Disconnected,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub agent_type: crate::models::agent::AgentType,
    pub status: ConnectionStatus,
    pub owner_window_label: String,
    pub working_dir: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub session_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AcpAgentInfo {
    pub agent_type: crate::models::agent::AgentType,
    pub registry_id: String,
    pub registry_version: Option<String>,
    pub name: String,
    pub description: String,
    pub available: bool,
    pub distribution_type: String,
    pub enabled: bool,
    pub sort_order: i32,
    pub installed_version: Option<String>,
    pub env: BTreeMap<String, String>,
    pub config_json: Option<String>,
    pub config_file_path: Option<String>,
    pub opencode_auth_json: Option<String>,
    pub codex_auth_json: Option<String>,
    pub codex_config_toml: Option<String>,
    pub cline_secrets_json: Option<String>,
    pub model_provider_id: Option<i32>,
}

/// Lightweight status info for a single agent, used by connect() pre-check.
#[derive(Debug, Clone, Serialize)]
pub struct AcpAgentStatus {
    pub agent_type: crate::models::agent::AgentType,
    pub available: bool,
    pub enabled: bool,
    pub installed_version: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillScope {
    Global,
    Project,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillLayout {
    MarkdownFile,
    SkillDirectory,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSkillLocation {
    pub scope: AgentSkillScope,
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSkillItem {
    pub id: String,
    pub name: String,
    pub scope: AgentSkillScope,
    pub layout: AgentSkillLayout,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSkillsListResult {
    pub supported: bool,
    pub message: Option<String>,
    pub locations: Vec<AgentSkillLocation>,
    pub skills: Vec<AgentSkillItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSkillContent {
    pub skill: AgentSkillItem,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableCommandInfo {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkResultInfo {
    pub forked_session_id: String,
    pub original_session_id: String,
}
