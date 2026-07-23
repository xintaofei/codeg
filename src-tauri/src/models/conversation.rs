use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::agent::AgentType;
use super::message::{MessageTurn, TurnUsage};
use crate::db::entities::conversation::ConversationKind;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegation_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbConversationSummary {
    pub id: i32,
    pub folder_id: i32,
    pub title: Option<String>,
    /// Mirror of `conversation.title_locked`: the user renamed this row by hand,
    /// so the auto-title backfill must leave it alone.
    pub title_locked: bool,
    pub agent_type: AgentType,
    pub status: String,
    /// Mirrors `conversation.kind` — drives sidebar visibility/grouping
    /// (serialized as "regular" | "chat" | "loop" | "delegate").
    pub kind: ConversationKind,
    pub model: Option<String>,
    pub git_branch: Option<String>,
    pub external_id: Option<String>,
    pub message_count: u32,
    /// Number of direct, non-deleted delegation children. Drives the sidebar
    /// chevron: a row with `child_count > 0` is expandable into its sub-session
    /// subtree. Not stored on the row — computed by a single GROUP BY aggregate
    /// (`fill_child_counts`) over the returned set, so `child_count > 0` iff
    /// `list_children` would return rows. Always serialized (the frontend reads
    /// `child_count` to decide chevron visibility).
    pub child_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Mirror of `conversation.pinned_at`: when set, the sidebar shows this row in
    /// its "Pinned" section (sorted by this timestamp descending) instead of its
    /// folder group. Serialized as `null` when absent so the frontend's
    /// `pinned_at: string | null` always sees the field.
    pub pinned_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegation_call_id: Option<String>,
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
    /// Byte length of the source transcript this parse consumed (exact — the
    /// parser measures the buffer it read, never a racy stat). Present only
    /// for parsers reading a single session file (Claude today; `None`
    /// elsewhere). The frontend retires background-overlay turns
    /// (`AcpEvent::BackgroundActivity`) whose `watermark <=` this value once
    /// a detail (re)fetch catches up — the race-free hand-off between the
    /// live overlay and persisted turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_watermark: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbConversationDetail {
    pub summary: DbConversationSummary,
    pub turns: Vec<MessageTurn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_stats: Option<SessionStats>,
    /// See [`ConversationDetail::transcript_watermark`] — threaded through from
    /// the parser detail on the DB-backed fetch path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_watermark: Option<u64>,
    /// Id of the persisted user turn the live-correlation pass identified as the
    /// in-flight prompt (only present while a turn is running on this
    /// conversation's connection; `None` otherwise). The frontend uses it to
    /// locate — and, while the live reply is in hand, hide — the partial
    /// assistant turn some agents (OpenCode, Gemini) persist after the prompt
    /// mid-stream, which would otherwise double-render against the live reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_flight_user_turn_id: Option<String>,
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
    /// Already-imported conversations whose title was refreshed from the
    /// agent's session file (e.g. an AI-generated title that did not yet exist
    /// at first import). Manual renames are never touched.
    pub updated: u32,
    pub skipped: u32,
}

/// Reconciliation state of one locally-discovered session against the codeg DB,
/// keyed by `(external_id, agent_type)` — the same identity `import_one` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanSessionStatus {
    /// Not in the DB — importable.
    New,
    /// At least one live (non-deleted) row exists — already imported.
    Imported,
    /// Only soft-deleted rows exist — never resurrected by import.
    Deleted,
}

/// One locally-discovered agent session in the import-picker scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanSession {
    /// The agent's own session id (`ConversationSummary::id`).
    pub external_id: String,
    pub agent_type: AgentType,
    pub title: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub message_count: u32,
    pub model: Option<String>,
    pub git_branch: Option<String>,
    pub status: ScanSessionStatus,
}

/// One folder group in the import-picker scan: all sessions sharing a
/// normalize-matched cwd, plus how that path reconciles against the `folder`
/// table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanFolder {
    /// Display/import path. When a folder row normalize-matches the sessions'
    /// cwd this is the ROW's stored string (the unique key `add_folder` upserts
    /// on), so importing can never mint a near-duplicate row from a trailing-
    /// slash or separator variant.
    pub path: String,
    pub name: String,
    /// A live (non-deleted) folder row exists for this path. `false` with a
    /// `folder_id` means the row is soft-deleted and import will reopen it.
    pub exists_in_codeg: bool,
    pub folder_id: Option<i32>,
    pub agent_types: Vec<AgentType>,
    pub sessions: Vec<ScanSession>,
}

/// Result of scanning every local agent's sessions for the import picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub folders: Vec<ScanFolder>,
    /// Sessions whose transcript carries no cwd — not importable (an import
    /// target folder cannot be derived), surfaced only as a count.
    pub no_folder_count: u32,
    pub total_sessions: u32,
    pub importable_count: u32,
}

/// Selection key sent back by the import picker: identifies one scanned
/// session by the same `(agent_type, external_id)` identity the DB dedups on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedSessionKey {
    pub agent_type: AgentType,
    pub external_id: String,
}

/// Per-folder tally of one batch import.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportFolderOutcome {
    pub path: String,
    pub folder_id: i32,
    /// No live folder row existed before this import (freshly created, or a
    /// soft-deleted row was reopened).
    pub created: bool,
    pub imported: u32,
    pub updated: u32,
    pub skipped: u32,
}

/// Aggregate result of `import_selected_sessions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSelectedResult {
    pub imported: u32,
    pub updated: u32,
    pub skipped: u32,
    /// Selection keys that no longer resolved to a scanned session (deleted on
    /// disk between scan and import, or bogus input).
    pub not_found: u32,
    /// Sessions that failed because their folder group errored.
    pub failed: u32,
    pub created_folders: u32,
    pub folders: Vec<ImportFolderOutcome>,
    /// Human-readable per-folder failure messages, capped by the command.
    pub errors: Vec<String>,
}
