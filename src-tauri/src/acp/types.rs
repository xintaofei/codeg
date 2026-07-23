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
    /// Out-of-turn activity surfaced from the agent's own session transcript
    /// by the background watcher (`acp::background_watch`; Claude-only today).
    /// Covers everything that happens OUTSIDE a codeg-driven prompt turn:
    /// async sub-agent / background-shell `<task-notification>` completions,
    /// the agent's continued work after them, and cron//loop autonomous turns
    /// (which never produce ACP wire events at all — see issue #270). The
    /// transcript is the single render source for out-of-turn content; wire
    /// updates arriving out-of-turn are dropped by the frontend.
    BackgroundActivity {
        session_id: String,
        /// Out-of-turn turns parsed from the transcript tail. UPSERT semantics
        /// keyed by `MessageTurn.id` (`bg-<episode-offset>-<idx>`) — a still-
        /// growing turn is re-emitted whole on each poll tick that changed it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        turns: Vec<crate::models::message::MessageTurn>,
        /// Launched-but-unresolved background tasks (async sub-agents +
        /// background shell tasks) accounted from transcript acks. Mirrored
        /// into `SessionState` to exempt the connection from both idle sweeps
        /// while work is pending.
        outstanding: u32,
        /// Tasks settled by `<task-notification>` records in this batch — the
        /// frontend raises one OS notification per entry.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        settled: Vec<BackgroundSettledInfo>,
        /// Byte offset of the transcript parsed through at emission. The
        /// frontend retires overlay turns once a detail fetch's
        /// `transcript_watermark` catches up (`>=`), closing the emit/refetch
        /// race without cross-namespace id dedup.
        watermark: u64,
    },
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
        /// Bounded preview of the delegated task text (broker's
        /// `TASK_PREVIEW_CAP`). Lets the live card show WHAT was delegated even
        /// when the parent tool call's `raw_input` never carries the arguments
        /// (Cursor announces MCP calls identity-less and never re-sends them).
        task_preview: String,
        /// Broker-minted task id — the same id the running ack embeds as
        /// `task_id=<id>` — so the live card can label the delegation before
        /// the ack text lands on the tool output.
        task_id: String,
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
    /// An agent called the `ask_user_question` MCP tool: one or more
    /// multiple-choice questions the user must answer before the (blocked) tool
    /// call returns. Broadcast so every client viewing this conversation renders
    /// the interactive card above the input box, and captured into
    /// `SessionState.pending_question` so a client attaching mid-turn (cold
    /// attach, reconnect, another window) recovers it from the snapshot. The
    /// backend parks a one-shot per `question_id` waiting for the answer.
    QuestionRequest {
        question_id: String,
        questions: Vec<crate::acp::question::QuestionSpec>,
    },
    /// A previously-pending question was answered (from any client) or canceled
    /// (the tool call was aborted / the connection drained). Carries only the
    /// `question_id`; clients clear the matching card. Idempotent on apply.
    QuestionResolved { question_id: String },
    /// The agent's effective settings (env vars / model provider / native config
    /// files) changed AFTER this connection was spawned, so the running process
    /// is still using its launch-time config. Emitted by
    /// `ConnectionManager::refresh_connection_staleness` when a settings save
    /// drifts a running session's freshly-recomputed config fingerprint away
    /// from its spawn-time snapshot. `stale = false` means a prior drift was
    /// reverted (the user changed the setting back) and the frontend should
    /// clear its "restart to apply" banner. Carried into `SessionState` so a
    /// snapshot attach (web reconnect, window refresh, new tile) recovers the
    /// staleness the one-shot event won't replay for it.
    SessionConfigStale {
        stale: bool,
        kind: ConfigStaleKind,
    },
}

/// One background task settled by a `<task-notification>` transcript record,
/// carried on [`AcpEvent::BackgroundActivity`]. `task_id` is the launch ack's
/// `agentId` (async sub-agent) or `backgroundTaskId` (background shell);
/// `status` is the notification's `<status>` passed through verbatim
/// (`"completed"` on success). The same task id may settle more than once —
/// a completed sub-agent can be resumed via `SendMessage` and re-notify.
///
/// `tool_use_id` and `result` come from the same `<task-notification>` record's
/// `<tool-use-id>`/`<result>` tags. They let the frontend flip the LAUNCH card
/// (`AgentToolCallPart`) from "running in background" to its terminal state
/// entirely in-memory — rewriting the launching tool call's own
/// `[[codeg-background-task]]` marker — WITHOUT a `refetchDetail`. That refetch
/// path used to be the only card-flip trigger, but it re-parses the still-open
/// transcript mid-`#870`-hold and both double-renders the held turn and races
/// the file's own last write.
/// `tool_use_id` is the launching `tool_use`/`tool_result` block's id (Claude's
/// SDK-level `toolu_…`), NOT `task_id`; `None` for a background shell (its
/// notification carries no tool-use-id and it has no marker card to flip).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundSettledInfo {
    pub task_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The launching tool call's `tool_use_id` (from the notification's
    /// `<tool-use-id>`), so the frontend can locate the exact card to flip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// The notification's `<result>` markdown (capped at
    /// [`crate::parsers::claude::BACKGROUND_RESULT_MAX_CHARS`], matching the
    /// cold-parse fold), so the live path renders identically to a cold detail
    /// parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Whether this task's reply is/was rendered on the ACP wire as the tail of
    /// a turn `#870` (claude-agent-acp v0.59.0) held open for it — i.e. the
    /// settling task's id was still in `current_turn_launched_ids` when the
    /// watcher read the notification. The frontend uses this to decide whether
    /// to arm the "syncing results" hint: for a wire-visible settle the reply
    /// is already on screen (no gap to bridge), whereas a genuinely out-of-turn
    /// settle's reply arrives later as a separate overlay turn. Derived from the
    /// backend set (which persists until the next turn's rising edge), NOT from
    /// the connection's current status — so it's correct even when the watcher
    /// reads the settlement AFTER the turn already fell back to `Connected`.
    #[serde(default)]
    pub wire_visible: bool,
}

/// Which settings surface drifted, so the frontend can word the
/// "restart to apply" banner precisely ("agent config" vs "model provider").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigStaleKind {
    /// Agent env vars / enabled / model-provider binding / native config file.
    AgentConfig,
    /// A model provider row this agent is bound to (url / key / model) changed.
    ModelProvider,
}

/// A block of the user's submitted prompt, broadcast via [`AcpEvent::UserMessage`]
/// and stored in the live snapshot. Intentionally narrower than
/// [`PromptInputBlock`]: only what a viewer needs to render the user turn.
/// Non-image `Resource` / `ResourceLink` prompt blocks are folded into `Text`
/// markdown links by [`user_blocks_from_prompt`]; an image-mime embedded
/// `Resource` (how an `image:false` / `embedded_context:true` agent like Grok
/// carries a pasted image) is promoted to `Image` so the viewer renders a
/// thumbnail, not a link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserMessageBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
}

/// Project the wire `PromptInputBlock`s the sender submitted into the lean
/// [`UserMessageBlock`]s broadcast to viewers: text and images pass through; an
/// image-mime embedded resource (Grok's pasted-image encoding) is promoted to an
/// `Image`; other resources/resource-links collapse to a `[label](uri)` markdown
/// line so a viewer still sees what was attached without shipping blob bytes
/// twice.
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
            // An image-mime embedded resource carries a pasted image for agents
            // that reject native image blocks (Grok: `image:false` +
            // `embedded_context:true`). Promote it to `Image` so viewers render
            // the thumbnail; non-image resources still collapse to a link.
            PromptInputBlock::Resource {
                uri,
                mime_type,
                blob,
                ..
            } => match (mime_type, blob) {
                (Some(mt), Some(b)) if mt.starts_with("image/") => UserMessageBlock::Image {
                    data: b.clone(),
                    mime_type: mt.clone(),
                },
                _ => UserMessageBlock::Text {
                    text: format!("[{uri}]({uri})"),
                },
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

/// Grok's per-model reasoning-effort capability, parsed from a session
/// response's top-level `models.availableModels[]._meta` (only reachable via the
/// raw JSON, since the `unstable_session_model` feature that would surface the
/// typed `models` field is intentionally off). Drives the model-reactive
/// composer effort selector: `supports == false` ⇒ the model shows NO effort
/// selector. Backend-internal — NOT serialized onto the wire.
#[derive(Debug, Clone, Default)]
pub struct GrokEffortSpec {
    /// Switchable efforts the model advertises: `(id, label, description)`.
    pub options: Vec<(String, String, Option<String>)>,
    /// The model's default/current effort. MAY fall outside `options`
    /// (e.g. grok-4.5 defaults to `xhigh` while only listing `high/medium/low`).
    pub default: Option<String>,
    /// Whether the model advertises `supportsReasoningEffort`.
    pub supports: bool,
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
    /// Slash commands the agent advertised during the probe's session, captured
    /// from the same transient connection used for modes/config so callers (e.g.
    /// the automation editor's `/` menu) get them without a live session. Empty
    /// when the agent publishes none within the probe's ready+grace window.
    /// `#[serde(default)]` keeps older snapshots deserializable.
    #[serde(default)]
    pub available_commands: Vec<AvailableCommandInfo>,
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
    /// Compact structured codex model-catalog source (the `codeg` custom-model
    /// list) round-tripped into the settings editor. Only populated for
    /// `AgentType::Codex`, and only in api-key mode (no bound provider).
    pub codex_model_catalog: Option<String>,
    pub cline_secrets_json: Option<String>,
    /// Raw `~/.hermes/config.yaml` text, attached for the Hermes settings panel's
    /// advanced editor. Only populated for `AgentType::Hermes`.
    pub hermes_config_yaml: Option<String>,
    /// Raw `~/.grok/config.toml` text, attached for the Grok settings panel's
    /// config-file editor. Only populated for `AgentType::Grok`.
    pub grok_config_toml: Option<String>,
    /// Parsed scalar settings from `~/.grok/config.toml` that back the Grok
    /// settings panel's structured controls (permission mode / reasoning
    /// effort). Only populated for `AgentType::Grok`. `None` fields mean the key
    /// is absent from the config. Derived from `grok_config_toml`.
    pub grok_settings: Option<GrokSettings>,
    /// Raw `~/.cursor/cli-config.json` text, attached for the Cursor settings
    /// panel's advanced view. Only populated for `AgentType::Cursor`.
    pub cursor_cli_config_json: Option<String>,
    /// Parsed scalar settings from cli-config.json backing the Cursor panel's
    /// structured controls (sandbox / permission rules; the Run Everything
    /// permission mode is a launch flag, not a config key). Only populated
    /// for `AgentType::Cursor`. Derived from `cursor_cli_config_json`.
    pub cursor_settings: Option<CursorSettings>,
    pub model_provider_id: Option<i32>,
}

/// The subset of `~/.grok/config.toml` keys surfaced as structured controls in
/// the Grok settings panel. Each field mirrors one documented key (see
/// docs.x.ai/build/settings/reference); `None` means the key is absent.
///
/// The *stock* per-session model is NOT surfaced here — it is chosen from the
/// composer's model selector. But a codeg-managed **custom (BYO endpoint) model**
/// IS: codeg writes a `[model.<id>]` block and points `[models].default` at it,
/// then reads it back through `custom_*` below. The managed block is anchored as
/// "the `[model.<id>]` whose id equals `[models].default`", giving clean
/// edit/rename/remove without leaving orphans.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrokSettings {
    /// `[models].default_reasoning_effort` — one of low/medium/high/xhigh.
    pub default_reasoning_effort: Option<String>,
    /// `[ui].permission_mode` — grok's real enum
    /// (default/acceptEdits/auto/dontAsk/bypassPermissions/plan). Legacy codeg
    /// markers (`ask`/`always-approve`) are migrated to `default`/`bypassPermissions`
    /// on read (see `migrate_grok_permission_mode`).
    pub permission_mode: Option<String>,
    /// The codeg-managed custom model id: the `[model.<id>]` block whose id
    /// equals `[models].default`. `None` when there is no such managed block.
    pub custom_model_id: Option<String>,
    /// `[model.<id>].base_url` — the custom endpoint. `None` ⇒ Grok's official
    /// xAI API (`https://api.x.ai/v1`).
    pub custom_base_url: Option<String>,
    /// `[model.<id>].api_key` — inline key scoped to the custom endpoint
    /// (distinct from the global `XAI_API_KEY` env credential).
    pub custom_api_key: Option<String>,
    /// `[model.<id>].api_backend` — chat_completions | responses | messages.
    pub custom_api_backend: Option<String>,
    /// `[model.<id>].context_window` — context size in tokens.
    pub custom_context_window: Option<i64>,
    /// `[session].auto_compact_threshold_percent` — auto-compact trigger, 0–100
    /// (Grok's default is 85).
    pub auto_compact_threshold_percent: Option<i64>,
}

/// The structured-control values the Grok settings panel sends on save. Each
/// `Some(value)` sets the corresponding key; each `None` removes it. Merged
/// (format-preserving, via `toml_edit`) onto the current on-disk config.toml so
/// unmanaged keys/comments are preserved. camelCase on the wire to match the
/// enclosing request body (`AcpUpdateAgentConfigParams`).
///
/// The custom-model group is driven by `custom_model_id`: a non-empty id writes
/// (or renames to) `[model.<id>]` + `[models].default = "<id>"`; an empty/`None`
/// id removes the codeg-managed block and its default. Within an active model,
/// each empty sub-field omits its key (e.g. empty `custom_base_url` ⇒ Grok falls
/// back to the official endpoint).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokStructuredConfig {
    pub default_reasoning_effort: Option<String>,
    pub permission_mode: Option<String>,
    pub custom_model_id: Option<String>,
    pub custom_base_url: Option<String>,
    pub custom_api_key: Option<String>,
    pub custom_api_backend: Option<String>,
    pub custom_context_window: Option<i64>,
    pub auto_compact_threshold_percent: Option<i64>,
}

/// The subset of `~/.cursor/cli-config.json` surfaced as structured controls
/// in the Cursor settings panel. The file is shared with the Cursor CLI's own
/// `/config` UI, so codeg only projects the keys it manages; everything else
/// is preserved verbatim on write. `None` means the key is absent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CursorSettings {
    /// `sandbox.mode` — "enabled" | "disabled".
    pub sandbox_mode: Option<String>,
    /// `permissions.allow` rules, e.g. `Shell(ls)`.
    pub permissions_allow: Vec<String>,
    /// `permissions.deny` rules.
    pub permissions_deny: Vec<String>,
}

/// The structured-control values the Cursor settings panel sends on save.
/// `None` fields leave the corresponding key untouched; `Some` fields replace
/// it (lists are replaced wholesale). Merged onto the current on-disk
/// cli-config.json so unmanaged keys are preserved. camelCase on the wire to
/// match the enclosing request body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorStructuredConfig {
    pub sandbox_mode: Option<String>,
    pub permissions_allow: Option<Vec<String>>,
    pub permissions_deny: Option<Vec<String>>,
}

/// Result of probing `cursor-agent status --format json` for the Cursor
/// settings panel's auth card. Parsed defensively: unknown shapes surface as
/// `raw_status` so the panel can still show something useful.
#[derive(Debug, Clone, Serialize)]
pub struct CursorAuthStatus {
    /// A launchable cursor-agent binary was found (cache or system install).
    pub installed: bool,
    pub is_authenticated: bool,
    /// The CLI's own `status` string (e.g. "unauthenticated").
    pub raw_status: Option<String>,
    /// Account email when logged in. The CLI nests it under `userInfo.email`
    /// (a top-level `email` is also accepted as a fallback).
    pub email: Option<String>,
    /// Membership/plan label when the CLI reports one. Current `status --format
    /// json` output carries no such field, so this is usually `None`.
    pub membership: Option<String>,
    /// Probe failure detail (spawn error / timeout / non-JSON output).
    pub error: Option<String>,
    /// Absolute path to the cursor-agent binary codeg would launch (managed
    /// cache or system install). The settings panel builds a copy-pasteable
    /// `"<binary_path>" login` command from it — the managed binary lives in
    /// codeg's cache and is NOT on the user's PATH, so a bare `cursor-agent
    /// login` fails. `None` when no binary is installed.
    pub binary_path: Option<String>,
}

/// One entry from `cursor-agent models`, whose lines are `<id> - <label>
/// [(default)]` (e.g. `claude-opus-4-8-high - Opus 4.8 1M`). The panel shows
/// `label` (falling back to `id`) and passes `id` to the CLI as `--model`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CursorModelInfo {
    /// The model id (`--model` value), e.g. `claude-opus-4-8-high`.
    pub id: String,
    /// Human-readable label from the CLI, e.g. `Opus 4.8 1M`. Empty when the
    /// CLI emitted a bare id with no ` - <label>` suffix.
    pub label: String,
    /// The account default (the CLI marks it `(default)`, e.g. `auto`).
    pub is_default: bool,
}

/// Result of `cursor-agent models` for the Cursor settings panel's model
/// picker. `models` is best-effort parsed CLI output; `error` carries the
/// failure reason when the probe could not run (e.g. not logged in).
#[derive(Debug, Clone, Serialize)]
pub struct CursorModelsResult {
    pub models: Vec<CursorModelInfo>,
    pub default_model: Option<String>,
    pub error: Option<String>,
}

/// Lightweight status info for a single agent, used by connect() pre-check.
#[derive(Debug, Clone, Serialize)]
pub struct AcpAgentStatus {
    pub agent_type: crate::models::agent::AgentType,
    pub available: bool,
    pub enabled: bool,
    pub installed_version: Option<String>,
}

/// Severity of a single diagnostics check / the overall verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagLevel {
    /// Healthy / expected.
    Ok,
    /// Suspicious but not necessarily broken (e.g. slow `npm prefix -g`).
    Warn,
    /// A concrete problem that explains a failure.
    Fail,
    /// Neutral information (not a pass/fail signal).
    Info,
}

/// One labelled probe result inside a [`DiagSection`]. `value` and `hint` carry
/// dynamic data (paths, versions) and are rendered as plain text in the UI —
/// they are NEVER fed through i18n/ICU (see `label`, which is a language-neutral
/// technical string emitted by the backend).
#[derive(Debug, Clone, Serialize)]
pub struct DiagCheck {
    pub label: String,
    pub value: String,
    pub status: DiagLevel,
    pub hint: Option<String>,
}

/// A titled group of [`DiagCheck`]s.
#[derive(Debug, Clone, Serialize)]
pub struct DiagSection {
    pub title: String,
    pub checks: Vec<DiagCheck>,
}

/// The one-line "likely cause" conclusion. `code` is a stable identifier the
/// frontend localizes via `DiagnosticsSettings.verdict.<code>`; `summary` is a
/// pre-formatted English sentence used only inside [`AgentDiagnosticsReport::plain_text`]
/// so a copied report reads the same regardless of UI locale.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsVerdict {
    pub level: DiagLevel,
    pub code: String,
    pub summary: String,
}

/// Full environment-diagnostics report returned by `acp_env_diagnostics`.
///
/// Plain `Serialize` with snake_case fields (the repo convention for response
/// DTOs), mirrored field-for-field by the `AgentDiagnosticsReport` TS interface.
#[derive(Debug, Clone, Serialize)]
pub struct AgentDiagnosticsReport {
    pub generated_at: String,
    pub agent_type: Option<crate::models::agent::AgentType>,
    pub verdict: DiagnosticsVerdict,
    pub sections: Vec<DiagSection>,
    pub plain_text: String,
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

    #[test]
    fn user_blocks_promote_image_resource_and_fold_other_resources() {
        let blocks = vec![
            PromptInputBlock::Text { text: "hi".into() },
            // Grok's pasted image: an embedded resource with an image mime + blob.
            PromptInputBlock::Resource {
                uri: "clipboard://image.png-abc".into(),
                mime_type: Some("image/png".into()),
                text: None,
                blob: Some("QUJD".into()),
            },
            // A non-image embedded resource still folds to a link.
            PromptInputBlock::Resource {
                uri: "clipboard://notes.txt".into(),
                mime_type: Some("text/plain".into()),
                text: Some("note".into()),
                blob: None,
            },
            PromptInputBlock::ResourceLink {
                uri: "file:///a/app.ts".into(),
                name: "app.ts".into(),
                mime_type: None,
                description: None,
            },
        ];
        let out = user_blocks_from_prompt(&blocks);
        assert_eq!(
            out,
            vec![
                UserMessageBlock::Text { text: "hi".into() },
                UserMessageBlock::Image {
                    data: "QUJD".into(),
                    mime_type: "image/png".into(),
                },
                UserMessageBlock::Text {
                    text: "[clipboard://notes.txt](clipboard://notes.txt)".into(),
                },
                UserMessageBlock::Text {
                    text: "[app.ts](file:///a/app.ts)".into(),
                },
            ]
        );
    }
}
