use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryBackendKind {
    Off,
    LocalSqlite,
    ExternalMcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    Auto,
    OnRequest,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Project,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRel {
    CausedBy,
    FixedBy,
    RelatesTo,
    PartOf,
    Supersedes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalMcpMapping {
    pub server_id: String,
    pub write_tool: String,
    pub search_tool: String,
    pub link_tool: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySettings {
    pub backend: MemoryBackendKind,
    pub scope: MemoryScope,
    pub external: Option<ExternalMcpMapping>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryKind {
    pub id: i32,
    pub key: String,
    pub name: String,
    pub instruction: String,
    pub mode: MemoryMode,
    pub builtin: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryKindDraft {
    pub name: String,
    pub instruction: String,
    pub mode: MemoryMode,
}
