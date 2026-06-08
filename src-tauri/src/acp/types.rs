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

/// Image attached to a tool call on the ACP wire (e.g. codex-acp v0.14+
/// image generation). Re-export of `models::message::ImageData` — the same
/// payload is used by `ContentBlock::Image` / `ContentBlock::ImageGeneration`
/// and by `ToolCallState.images` for snapshot recovery.
pub type ToolCallImageInfo = crate::models::message::ImageData;

/// 所有 ACP 事件统一通过此 envelope 发出。
/// `seq` 用于前端去重锚点（Phase 0 占位 0，Phase 1 起严格递增）。
/// `connection_id` 上提到顶层，配合 `#[serde(flatten)]` 让 JSON 保持平铺：
/// `{ seq, connection_id, type, ...变体字段 }`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: u64,
    pub connection_id: String,
    #[serde(flatten)]
    pub payload: AcpEvent,
}

/// Events pushed from Rust backend to frontend via Tauri event system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpEvent {
    /// Agent returned text content (streaming delta)
    ContentDelta { text: String },
    /// Agent thinking/reasoning
    Thinking { text: String },
    /// Raw SDK message forwarded from Claude ACP extension notification
    ClaudeSdkMessage {
        session_id: String,
        message: serde_json::Value,
    },
    /// Agent initiated a tool call
    ToolCall {
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
        /// Images attached to this tool call (e.g. codex image generation).
        /// `None` when the agent didn't supply any.
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<ToolCallImageInfo>>,
    },
    /// Tool call status/content updated
    ToolCallUpdate {
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
        /// Replace-on-update semantics: `Some(v)` replaces the prior `images`
        /// vec on `ToolCallState`, `None` preserves it.
        #[serde(skip_serializing_if = "Option::is_none")]
        images: Option<Vec<ToolCallImageInfo>>,
    },
    /// Agent requests permission
    PermissionRequest {
        request_id: String,
        tool_call: serde_json::Value,
        options: Vec<PermissionOptionInfo>,
    },
    /// User responded to (or the connection drained) a previously-pending
    /// permission request. The responder.respond() side of the SACP exchange
    /// is RPC-only, so without this event downstream consumers (pet snapshot,
    /// session_state for snapshot recovery) would have to wait until
    /// TurnComplete to learn that the permission is no longer outstanding —
    /// keeping the pet pinned on `Waiting` through whatever work the agent
    /// does after the approval (which, for ExitPlanMode, is the entire
    /// implementation phase).
    PermissionResolved { request_id: String },
    /// Turn completed
    TurnComplete {
        session_id: String,
        stop_reason: String,
        agent_type: String,
    },
    /// Session established with agent-assigned session ID
    SessionStarted { session_id: String },
    /// Backend has bound this connection to a conversation row. Emitted exactly
    /// once per connection lifetime, on first prompt that creates the row.
    /// Frontend uses this to associate the connection_id with conversation_id
    /// without polling the DB.
    ///
    /// `parent_conversation_id` / `parent_tool_use_id` are set when the row was
    /// created as a delegation child (see `DelegationLink` in
    /// `acp::delegation`); they are `None` for normal top-level conversations.
    ConversationLinked {
        conversation_id: i32,
        folder_id: i32,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        parent_conversation_id: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        parent_tool_use_id: Option<String>,
    },
    /// Backend has transitioned the conversation row's `status` column.
    /// Emitted by `send_prompt_linked` (`InProgress`) and the lifecycle
    /// subscriber on `TurnComplete` (`PendingReview`). The frontend mirrors
    /// the new status onto its sidebar/list state without re-querying the DB.
    /// `completed` / `cancelled` transitions remain frontend-driven and are
    /// NOT emitted via this event.
    ConversationStatusChanged {
        conversation_id: i32,
        status: crate::db::entities::conversation::ConversationStatus,
    },
    /// Session modes are available for this connection
    SessionModes { modes: SessionModeStateInfo },
    /// Session configuration options are available/updated for this connection
    SessionConfigOptions {
        config_options: Vec<SessionConfigOptionInfo>,
    },
    /// Initial selector payloads (modes/config options) have been emitted
    SelectorsReady,
    /// Prompt capabilities for this connection
    PromptCapabilities {
        prompt_capabilities: PromptCapabilitiesInfo,
    },
    /// Whether the agent supports session/fork
    ForkSupported { supported: bool },
    /// Current session mode changed
    ModeChanged { mode_id: String },
    /// Agent reported plan update for current turn
    PlanUpdate { entries: Vec<PlanEntryInfo> },
    /// Connection status changed
    StatusChanged { status: ConnectionStatus },
    /// Error occurred
    Error {
        message: String,
        agent_type: String,
        /// Stable machine-readable identifier (e.g. "initialize_timeout").
        /// When present, the frontend renders a localized message keyed on
        /// this code; otherwise it falls back to `message`.
        code: Option<String>,
        /// Whether this Error signals connection-level death — i.e. the
        /// `run_connection` task is about to emit `Disconnected` and tear
        /// the session down. Non-terminal Errors (turn failure, `SetMode`
        /// failure, `session/load` fallback, empty-prompt rejection)
        /// leave the connection alive and the next prompt will still work.
        ///
        /// Skipped from serialization — the wire-format payload sent to
        /// the frontend (Tauri / WebSocket) is unchanged. This is purely
        /// an in-process signal between `connection.rs` and the lifecycle
        /// worker so the worker can avoid wrongly cancelling the
        /// conversation row or polluting the broker's cancel reason with
        /// a stale, non-terminal error detail. (Stays `false` after any
        /// JSON round-trip; only the original emitter sees `true`.)
        #[serde(skip, default)]
        terminal: bool,
    },
    /// `session/load` failed in a non-recoverable way (e.g. the agent has no
    /// record of this `session_id`). Emitted instead of silently falling back
    /// to `session/new`, so the frontend can surface the failure with reload
    /// / new-conversation actions.
    SessionLoadFailed {
        session_id: String,
        message: String,
        /// Stable machine-readable identifier — currently
        /// `"resource_not_found"` for JSON-RPC -32002.
        code: String,
    },
    /// Available slash commands updated
    AvailableCommands { commands: Vec<AvailableCommandInfo> },
    /// Session usage/context window updated during conversation
    UsageUpdate { used: u64, size: u64 },
    /// A `delegate_to_agent` MCP tool call from the parent agent has spawned a
    /// child sub-session and the child's prompt is in flight. Emitted as soon
    /// as the broker registers the pending call. The frontend uses this to
    /// build the parent ↔ child mapping for inline rendering.
    DelegationStarted {
        parent_connection_id: String,
        parent_tool_use_id: String,
        child_connection_id: String,
        child_conversation_id: i32,
        agent_type: crate::models::agent::AgentType,
    },
    /// The child sub-session has finished (or errored / timed out / been
    /// canceled). The MCP tool_result has been delivered to the parent agent.
    DelegationCompleted {
        parent_connection_id: String,
        parent_tool_use_id: String,
        child_connection_id: String,
        child_conversation_id: i32,
        /// Child agent type. Carried so a frontend that missed the
        /// `DelegationStarted` event (context mounted mid-flight, reconnect,
        /// or web/server snapshot replay that only re-delivered the completion)
        /// can synthesize the binding with the correct agent instead of a
        /// hardcoded default. Mirrors `DelegationStarted.agent_type`.
        agent_type: crate::models::agent::AgentType,
        result: DelegationResultSummary,
    },
    /// A human submitted a prompt from the Codeg conversation UI (desktop or
    /// web). Synthetic, notification-only event: it mutates no `SessionState`
    /// field and exists purely to drive the chat-channel "user message" push.
    /// Emitted by `send_prompt_linked` on the genuine UI path only
    /// (`delegation.is_none()`), after the prompt reached the agent, and only
    /// when the message carried text. `text_preview` is already bounded by the
    /// emitter so a large paste can't bloat the event payload / ring buffer /
    /// webhook body.
    UserPromptSent { text_preview: String },
    /// The user's submitted prompt, broadcast on the connection stream so OTHER
    /// clients viewing this conversation can synthesize the user turn in real
    /// time. The sending client adds its own optimistic turn and ignores this
    /// echo (it dedups against having an in-flight optimistic turn). Also
    /// captured into `SessionState.pending_user_message` so a client attaching
    /// mid-turn receives it in the snapshot. Emitted only for root sends
    /// (delegation children synthesize their kickoff text separately).
    UserMessage {
        message_id: String,
        blocks: Vec<UserMessageBlock>,
    },
    /// The user submitted a live-feedback note while the agent is mid-turn (the
    /// `check_user_feedback` MCP-tool steering path). Broadcast so every client
    /// viewing this conversation renders the pending note, and captured into
    /// `SessionState.feedback` so a mid-turn snapshot attach recovers it.
    /// Idempotent by `item.id` on apply (replay-safe).
    FeedbackSubmitted {
        item: crate::acp::feedback::FeedbackItem,
    },
    /// The agent read one or more pending feedback notes via
    /// `check_user_feedback`. Carries only the note ids + the delivery instant;
    /// clients already hold the note text (from `FeedbackSubmitted` / snapshot)
    /// and just flip those ids to `Delivered`. Idempotent on apply.
    FeedbackConsumed {
        ids: Vec<String>,
        delivered_at: chrono::DateTime<chrono::Utc>,
    },
}

/// A block of the user's submitted prompt, broadcast via [`AcpEvent::UserMessage`]
/// and stored in the live snapshot. Intentionally narrower than
/// [`PromptInputBlock`]: only what a viewer needs to render the user turn.
/// `Resource` / `ResourceLink` prompt blocks are folded into `Text` markdown
/// links by [`user_blocks_from_prompt`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserMessageBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
}

/// Project the wire `PromptInputBlock`s the sender submitted into the lean
/// [`UserMessageBlock`]s broadcast to viewers: text and images pass through;
/// resources/resource-links collapse to a `[label](uri)` markdown line so a
/// viewer still sees what was attached without shipping blob bytes twice.
pub fn user_blocks_from_prompt(blocks: &[PromptInputBlock]) -> Vec<UserMessageBlock> {
    blocks
        .iter()
        .map(|b| match b {
            PromptInputBlock::Text { text } => UserMessageBlock::Text { text: text.clone() },
            PromptInputBlock::Image {
                data, mime_type, ..
            } => UserMessageBlock::Image {
                data: data.clone(),
                mime_type: mime_type.clone(),
            },
            PromptInputBlock::Resource { uri, .. } => UserMessageBlock::Text {
                text: format!("[{uri}]({uri})"),
            },
            PromptInputBlock::ResourceLink { uri, name, .. } => UserMessageBlock::Text {
                text: format!("[{name}]({uri})"),
            },
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DelegationResultSummary {
    Ok {
        duration_ms: u64,
        /// Bounded preview (≤ ~2 KiB) of the child's final assistant text, so
        /// the parent UI can render the result inline on the live
        /// `delegation_completed` event without re-fetching the child session,
        /// and the chat-channel relay can echo it. `None` for older payloads /
        /// when the child produced no text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_preview: Option<String>,
    },
    Err {
        error_code: String,
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

/// Read-only snapshot of the modes + config_options an agent advertises
/// when it opens a new session. Used by `ConnectionManager::probe_agent_options`
/// to give the delegation settings UI an authoritative view of what an
/// agent will accept (no reliance on chat-side caches).
///
/// Both fields mirror `SessionState`: `modes` is `None` when the agent
/// reports no mode catalog (e.g. some thin wrappers); `config_options` is
/// empty when the agent advertises no configurable options.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentOptionsSnapshot {
    pub modes: Option<SessionModeStateInfo>,
    pub config_options: Vec<SessionConfigOptionInfo>,
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
}

/// The live connection currently bound to a conversation, returned by
/// `acp_find_connection_for_conversation`. The endpoint returns `None` when no
/// live connection owns the conversation (the client reads the persisted detail
/// instead of attaching). `event_seq` is the connection's progress at discovery
/// time — informational only; the viewer always does a COLD snapshot attach
/// (no cursor), since it has applied no prior events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationConnectionInfo {
    pub connection_id: String,
    pub event_seq: u64,
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
    /// Best-effort `description:` extracted from the SKILL.md YAML
    /// frontmatter. `None` when there is no frontmatter or no key.
    pub description: Option<String>,
    /// True for skills bundled by the agent CLI itself (e.g. Codex's
    /// `~/.codex/skills/.system/*`). Surfaced so the UI can show them but
    /// refuse to edit or delete; the backend also refuses such writes.
    pub read_only: bool,
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

/// Internal reply shape from the connection loop back to `manager.fork_session`
/// — protocol-only, before any DB writes. The manager combines this with the
/// freshly-created sibling row id to produce the wire-level `ForkResultInfo`.
#[derive(Debug, Clone)]
pub struct ForkProtocolResult {
    pub forked_session_id: String,
    pub original_session_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkResultInfo {
    pub forked_session_id: String,
    pub original_session_id: String,
    /// DB id of the sibling conversation row that backend created to preserve
    /// the pre-fork (S1) history. The current connection's conversation row
    /// (still bound in `SessionState`) gets re-pointed to S2 in the same call.
    pub sibling_conversation_id: i32,
}

#[cfg(test)]
mod envelope_tests {
    use super::*;

    #[test]
    fn event_envelope_serializes_with_flat_payload() {
        let env = EventEnvelope {
            seq: 5,
            connection_id: "conn-1".to_string(),
            payload: AcpEvent::ContentDelta {
                text: "hello".to_string(),
            },
        };
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["seq"], 5);
        assert_eq!(json["connection_id"], "conn-1");
        assert_eq!(json["type"], "content_delta");
        assert_eq!(json["text"], "hello");
        assert!(
            json.get("payload").is_none(),
            "flatten means no nested 'payload' key in JSON"
        );
    }

    #[test]
    fn conversation_status_changed_round_trips_with_flat_payload() {
        use crate::db::entities::conversation::ConversationStatus;
        let env = EventEnvelope {
            seq: 12,
            connection_id: "conn-x".to_string(),
            payload: AcpEvent::ConversationStatusChanged {
                conversation_id: 99,
                status: ConversationStatus::PendingReview,
            },
        };
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["seq"], 12);
        assert_eq!(json["connection_id"], "conn-x");
        assert_eq!(json["type"], "conversation_status_changed");
        assert_eq!(json["conversation_id"], 99);
        assert_eq!(json["status"], "pending_review");
        assert!(
            json.get("payload").is_none(),
            "flatten means no nested 'payload' key in JSON"
        );

        // Round-trip back to verify Deserialize matches Serialize.
        let back: EventEnvelope = serde_json::from_value(json).unwrap();
        match back.payload {
            AcpEvent::ConversationStatusChanged {
                conversation_id,
                status,
            } => {
                assert_eq!(conversation_id, 99);
                assert_eq!(status, ConversationStatus::PendingReview);
            }
            other => panic!("expected ConversationStatusChanged, got {other:?}"),
        }
    }
}
