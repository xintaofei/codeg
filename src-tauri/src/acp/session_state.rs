//! 会话级状态结构。后端权威：流式累积、in-flight tool calls、待处理 permission 等
//! 全部住在这里。Phase 2 的 snapshot 端点直接从此处读取 live 部分。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::acp::event_stream::{ConnectionEventStream, RecentEventsBuffer};
use crate::acp::feedback::{FeedbackItem, FeedbackStatus};
use crate::acp::types::{
    AcpEvent, AvailableCommandInfo, ConnectionStatus, EventEnvelope, PromptCapabilitiesInfo,
    SessionConfigOptionInfo, SessionModeStateInfo, ToolCallImageInfo,
};
use crate::models::agent::AgentType;
use crate::models::message::MessageRole;

/// 当前 streaming 中的 turn 的累积内容。turn 完成后清空。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: Vec<LiveContentBlock>,
    pub started_at: DateTime<Utc>,
}

/// 流式 turn 的内容块。事件按到达顺序追加。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveContentBlock {
    Text { text: String },
    Thinking { text: String },
    ToolCallRef { tool_call_id: String },
    Plan { entries: serde_json::Value },
}

/// 工具调用的运行态。turn 完成时统一 clear。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallState {
    pub id: String,
    pub kind: ToolKind,
    pub label: String,
    pub status: ToolCallStatus,
    pub input: Option<serde_json::Value>,
    pub output: Option<ToolCallOutput>,
    /// Latest rendered content blocks reported by the agent (markdown / text).
    /// Distinct from `output` (which is the parsed `raw_output`); kept as the
    /// most recent value (replace-on-update, not append) for snapshot fidelity.
    pub content: Option<String>,
    /// File locations affected by this tool call (e.g. paths of edits).
    /// Forwarded verbatim from the agent's ToolCall/ToolCallUpdate event.
    /// `None` if the agent didn't supply it. Partial-update preservation:
    /// an incoming `None` from a `ToolCallUpdate` (which typically carries
    /// only changed fields) must NOT clobber a previously-set value.
    pub locations: Option<serde_json::Value>,
    /// ACP extensibility metadata. Used by frontend Phase 1 parent
    /// extraction. `None` if the agent didn't supply it. Same partial-update
    /// preservation semantic as `locations`.
    ///
    /// Convention used by codeg's multi-agent delegation (the `delegate_to_agent`
    /// MCP tool) — `DelegationBroker` writes the following object under
    /// `meta["codeg.delegation"]` on the parent's active tool call:
    ///
    /// ```jsonc
    /// {
    ///   "child_connection_id": "<uuid>",
    ///   "child_conversation_id": <i32>,
    ///   "status": "pending" | "running" | "completed" | "failed"
    /// }
    /// ```
    ///
    /// The frontend reads this to render "Delegating to <agent>…" on the live
    /// tool-call, and to anchor the inline `<DelegatedSubThread>` to the
    /// correct child conversation.
    pub meta: Option<serde_json::Value>,
    /// Latest images attached to this tool call (e.g. codex-acp v0.14+
    /// image generation). Replace-on-update semantics matching `content`:
    /// a fresh `ToolCallUpdate` carrying `Some(images)` replaces the prior
    /// vec, `None` preserves it. Persisted on snapshot so a frontend
    /// reconnecting mid-turn or after refresh sees the same image that was
    /// streamed live. ⚠ base64 image data can be multi-MB per entry; the
    /// snapshot endpoint payload grows accordingly. This is the cost of
    /// surviving page refresh without re-fetching from JSONL.
    #[serde(default)]
    pub images: Vec<ToolCallImageInfo>,
    /// 流式拼接的 input chunks（serde 不输出，仅运行时用）
    #[serde(skip)]
    pub raw_input_chunks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// 工具种类。沿用 ACP 协议层枚举。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

/// 工具调用输出。可能是文本、错误、结构化结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolCallOutput {
    Text { content: String },
    Error { message: String },
    Json { value: serde_json::Value },
}

/// 待处理的权限请求。重连后从 SessionState 恢复，跨 UI 关闭不丢。
/// 注意：与 chat_channel::PendingPermission 不同（后者有 sent_message_id）。
///
/// `tool_call` 是 agent 原样转发的 JSON——保留 rawInput / content / locations /
/// patch / plan 等所有结构，前端 `parsePermissionToolCall` 依赖它来渲染 diff、
/// shell 命令、plan 列表等审批必备信息。压成 `description: String` 那种摘要
/// 字符串会让"刷新后继续审批"变成"盲签"。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPermissionState {
    pub request_id: String,
    pub tool_call_id: String,
    pub tool_call: serde_json::Value,
    pub options: Vec<crate::acp::types::PermissionOptionInfo>,
    pub created_at: DateTime<Utc>,
}

/// 上下文 / 模型用量。
/// Snapshot of the most recent `AcpEvent::Error`. Carried on
/// `SessionState` so post-mortem readers (e.g. the delegation-settings
/// probe) can surface the agent's own error after the connection task
/// has already cleaned up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionLastError {
    pub message: String,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageInfo {
    pub used: u64,
    pub size: u64,
}

/// Snapshot-recoverable record of an IN-FLIGHT (running) sub-agent delegation,
/// keyed (in `SessionState.active_delegations`) by the parent's
/// `parent_tool_use_id`.
///
/// This is the live "currently delegating" SET, not a history log:
/// `DelegationStarted` inserts an entry; `DelegationCompleted` REMOVES it. So
/// its size tracks live concurrency (bounded by what the machine actually runs)
/// — there is no cap and no cumulative growth over the parent connection's
/// lifetime.
///
/// Completed delegations are recovered without this field: a live page keeps the
/// binding in `DelegationProvider` for its lifetime, and a cold load / refresh
/// rebuilds `meta["codeg.delegation"]` (status + child id) from the child's
/// persisted DB row via `commands::conversations::inject_delegation_meta`
/// (authoritative, uncapped). The snapshot only has to recover the *running*
/// binding, which the transient `DelegationStarted` event cannot supply on the
/// snapshot attach path (cold attach, lagged re-attach, refresh) — that gap is
/// exactly what this field closes.
///
/// UNLIKE `active_tool_calls`, entries are NOT cleared on `TurnComplete`: an
/// async delegation's child runs in the background long after the parent's
/// `delegate_to_agent` tool call returns and the parent turn completes. The
/// broker emits `DelegationStarted`/`DelegationCompleted` only for a REAL
/// (non-synthetic) `parent_tool_use_id`, so synthetic-fallback cards never
/// create a phantom entry here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveDelegationState {
    pub parent_tool_use_id: String,
    pub child_connection_id: String,
    pub child_conversation_id: i32,
    pub agent_type: AgentType,
}

/// The in-flight user prompt for the current turn. Captured from
/// `AcpEvent::UserMessage` into `SessionState.pending_user_message` and carried
/// on `to_snapshot()` so a client attaching mid-turn can render the user turn
/// even though the one-shot `UserMessage` event won't replay for it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingUserMessage {
    pub message_id: String,
    pub blocks: Vec<crate::acp::types::UserMessageBlock>,
}

/// 后端权威的会话状态。每个 AgentConnection 持有一个 Arc<RwLock<SessionState>>。
///
/// 字段范围：仅当前 turn 的 in-flight 数据 + 元信息 + 协商出的能力。
/// 已完成的 turn 不存在这里——它们由 parser 从 agent JSONL 读。
#[derive(Debug)]
pub struct SessionState {
    // 身份
    pub connection_id: String,
    pub conversation_id: Option<i32>,
    pub external_id: Option<String>,
    pub agent_type: AgentType,
    pub working_dir: Option<PathBuf>,
    pub owner_window_label: String,
    pub folder_id: Option<i32>,

    // 状态
    pub status: ConnectionStatus,
    pub live_message: Option<LiveMessage>,
    pub active_tool_calls: BTreeMap<String, ToolCallState>,
    pub pending_permission: Option<PendingPermissionState>,

    /// In-flight (running) sub-agent delegations keyed by `parent_tool_use_id`.
    /// `DelegationStarted` inserts; `DelegationCompleted` removes. UNLIKE
    /// `active_tool_calls`, NOT cleared on `TurnComplete` (an async delegation
    /// outlives the parent turn). Carried on `to_snapshot()` so a web/server
    /// attach on the snapshot path (cold attach, lagged re-attach, refresh) can
    /// recover the running parent↔child binding the transient `DelegationStarted`
    /// event can't supply there. Size tracks live concurrency — no cap, no
    /// cumulative growth; completed delegations are recovered from the child's
    /// persisted DB row, not from here. See `ActiveDelegationState`.
    pub active_delegations: BTreeMap<String, ActiveDelegationState>,

    /// Live user-feedback ("steering") notes for the current turn. Appended by
    /// `FeedbackSubmitted` (a user note while the agent works), flipped to
    /// `Delivered` by `FeedbackConsumed` (the agent read them via the
    /// `check_user_feedback` MCP tool), and cleared on the next turn's
    /// `UserMessage` (notes are turn-scoped steering, not durable history).
    /// Carried on `to_snapshot()` so a client attaching mid-turn renders the
    /// pending notes the one-shot `FeedbackSubmitted` event won't replay for it.
    /// Size is human-bounded (one entry per note the user types this turn).
    pub feedback: Vec<FeedbackItem>,

    // ACP 协商出的能力
    pub modes: Option<SessionModeStateInfo>,
    pub current_mode: Option<String>,
    pub config_options: Option<Vec<SessionConfigOptionInfo>>,
    pub prompt_capabilities: Option<PromptCapabilitiesInfo>,
    pub fork_supported: bool,
    pub available_commands: Vec<AvailableCommandInfo>,
    pub usage: Option<UsageInfo>,
    /// True once the agent's initial selectors handshake (modes +
    /// config_options) has finished and `SelectorsReady` has fired. Persisted
    /// on the snapshot so a frontend that reconnects after refresh can see
    /// "init complete" without waiting for an event that already fired.
    pub selectors_ready: bool,

    /// Most recent `AcpEvent::Error` payload, or `None` if no error has
    /// landed since the connection started. The probe path reads this
    /// after `wait_for_session_options` errors so it can fold the
    /// agent's own error message into the returned `AcpError` instead
    /// of surfacing a generic "connection not found" once the
    /// connection task has cleaned up its map entry.
    ///
    /// Not exposed on `to_snapshot()` today — chat-side error UX already
    /// flows through the live `AcpEvent::Error` channel.
    pub last_error: Option<SessionLastError>,

    /// Single-fire signal that fires when `SessionStarted` applies (i.e.
    /// `external_id` transitioned from None → Some). `ConnectionManager::
    /// spawn_agent` holds the per-(agent, working_dir, session_id) dedup
    /// lock until this fires (or times out), so a concurrent acp_connect
    /// for the same logical session sees the populated `external_id` and
    /// reuses instead of spawning a duplicate. `Some` immediately after
    /// `install_session_started_signal()`; `take()`'d in `apply_event::
    /// SessionStarted`; `None` thereafter (the signal is one-shot per
    /// connection). Lives only on the in-memory `SessionState`; not
    /// transmitted on the wire (`LiveSessionSnapshot` doesn't include it).
    pub(crate) session_started_tx: Option<tokio::sync::oneshot::Sender<()>>,

    // 事件锚点
    pub event_seq: u64,
    pub last_activity_at: DateTime<Utc>,

    /// Per-connection event broadcaster used by the WS attach protocol.
    /// New subscribers register receivers here while holding the SessionState
    /// read lock; `emit_with_state` broadcasts after releasing the write
    /// lock. Wrapped in `Arc` so subscriber tasks can hold a reference
    /// independent of the SessionState lock.
    pub(crate) event_stream: Arc<ConnectionEventStream>,

    /// Bounded ring buffer of recent envelopes (most-recent-last). Pushed
    /// by `emit_with_state` inside the write-lock critical section, kept in
    /// strict lockstep with `event_seq`. Read by attach handlers under the
    /// read lock to decide between sending a snapshot or a batched replay.
    /// See `event_stream` module for size limits.
    pub(crate) recent_events: RecentEventsBuffer,

    /// Per-launch token registered with the delegation broker's
    /// `TokenRegistry` when `codeg-mcp` is injected at init.
    /// Revoked when the connection tears down so a leaked binary can't
    /// keep round-tripping after the parent session ends.
    pub delegation_token: Option<String>,

    /// Whether the `check_user_feedback` MCP tool was exposed to THIS agent at
    /// launch (the `feedback` feature was on when its companion was injected).
    /// Fixed for the connection's lifetime — tool exposure can't change after
    /// launch. The authoritative gate for both the submit path and the UI: a
    /// session started before the feature was enabled has no tool, so notes
    /// would strand; one started after has it. Carried on `to_snapshot()` so the
    /// frontend gates the feedback bar on the agent's actual capability, not the
    /// (possibly later-toggled) global setting.
    pub feedback_tool_available: bool,

    /// Concatenated text content of the just-completed turn's assistant
    /// message. Captured at TurnComplete (just before live_message is
    /// cleared) so the lifecycle subscriber can surface it as the
    /// `delegation_call_id`-bound child outcome. Cleared on the next prompt.
    pub last_assistant_text: Option<String>,

    /// The in-flight user prompt for the current turn, captured from
    /// `AcpEvent::UserMessage` and cleared on `TurnComplete` (alongside
    /// `live_message`). Carried on `to_snapshot()` so a client attaching
    /// mid-turn renders the user turn even though no `UserMessage` event will
    /// replay for it. `None` outside an active turn.
    pub pending_user_message: Option<PendingUserMessage>,

    /// Backend wall-clock instant the in-flight turn started, captured alongside
    /// `pending_user_message` from `AcpEvent::UserMessage` and cleared on
    /// `TurnComplete`. The detail endpoint uses it to tell the in-flight prompt
    /// — persisted at/after this instant by the agent CLI, a local subprocess
    /// sharing this machine's clock — apart from a prior identical prompt
    /// persisted during an earlier turn (see `apply_in_flight_message_id`). Not
    /// serialized: backend-internal, like `turn_in_flight`. `None` outside an
    /// active turn.
    pub pending_user_message_started_at: Option<DateTime<Utc>>,

    /// True between a prompt being accepted (enqueued to the connection loop)
    /// and that turn completing. Set by the manager BEFORE the enqueue (so it
    /// is guaranteed set before the loop can dequeue) and cleared on
    /// `TurnComplete`. The manager rejects a second prompt with
    /// `AcpError::TurnInProgress` while this is set — otherwise the second
    /// `Prompt` would queue behind the active turn and be silently dropped by
    /// the loop's in-turn command handler (`_ => {}`), with the caller still
    /// seeing success. Not serialized: it is a connection-loop liveness flag,
    /// not part of the client-visible snapshot.
    pub turn_in_flight: bool,
}

impl SessionState {
    pub fn new(
        connection_id: String,
        agent_type: AgentType,
        working_dir: Option<PathBuf>,
        owner_window_label: String,
        folder_id: Option<i32>,
    ) -> Self {
        Self {
            connection_id,
            conversation_id: None,
            external_id: None,
            agent_type,
            working_dir,
            owner_window_label,
            folder_id,
            status: ConnectionStatus::Connecting,
            live_message: None,
            active_tool_calls: BTreeMap::new(),
            pending_permission: None,
            active_delegations: BTreeMap::new(),
            feedback: Vec::new(),
            modes: None,
            current_mode: None,
            config_options: None,
            prompt_capabilities: None,
            fork_supported: false,
            available_commands: Vec::new(),
            usage: None,
            selectors_ready: false,
            last_error: None,
            session_started_tx: None,
            event_seq: 0,
            last_activity_at: Utc::now(),
            event_stream: Arc::new(ConnectionEventStream::new()),
            recent_events: RecentEventsBuffer::new(),
            delegation_token: None,
            feedback_tool_available: false,
            last_assistant_text: None,
            pending_user_message: None,
            pending_user_message_started_at: None,
            turn_in_flight: false,
        }
    }

    /// Clone the broadcaster handle so attach handlers and subscriber tasks
    /// can hold an independent reference. Cheap (Arc clone).
    pub fn event_stream(&self) -> Arc<ConnectionEventStream> {
        Arc::clone(&self.event_stream)
    }

    /// Return events buffered after `since_seq`, or `None` if the cursor is
    /// older than what the ring buffer holds (caller must fall back to a
    /// snapshot). See `RecentEventsBuffer::range_after`.
    pub fn recent_events_after(&self, since_seq: u64) -> Option<Vec<Arc<EventEnvelope>>> {
        self.recent_events.range_after(since_seq)
    }

    /// Push an envelope into the ring buffer. Must be called under the
    /// write lock from `emit_with_state`, immediately after `event_seq`
    /// is incremented, so the buffer's tail seq matches `event_seq`.
    ///
    /// Returns the eviction count (events dropped from the buffer's head to
    /// stay within count/byte caps, plus any wholesale clear triggered by an
    /// oversized event). Caller propagates this into the
    /// `EventBusMetrics::ring_buffer_evict_count` counter.
    #[must_use = "evicted count feeds the ring_buffer_evict_count metric"]
    pub(crate) fn push_recent_event(&mut self, envelope: Arc<EventEnvelope>) -> usize {
        self.recent_events.push(envelope)
    }

    /// Install a one-shot signal that fires when `SessionStarted` applies.
    /// Returns the receiver; caller (typically `spawn_agent_connection`)
    /// passes it back to the dedup waiter in `spawn_agent`. Calling this
    /// more than once on the same state replaces the previous sender,
    /// silently dropping it — the contract is "exactly one install per
    /// connection lifetime" and that's what `spawn_agent_connection` does.
    pub fn install_session_started_signal(&mut self) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.session_started_tx = Some(tx);
        rx
    }

    /// 单一分发器：把一个 AcpEvent 应用到 self。注意此方法**不**自增 event_seq——
    /// seq 由 emit_with_state 在外层管理（这样 apply_event 可独立单元测试）。
    pub fn apply_event(&mut self, payload: &AcpEvent) {
        match payload {
            AcpEvent::SessionStarted { session_id } => {
                self.external_id = Some(session_id.clone());
                self.status = ConnectionStatus::Connected;
                // Fire the dedup waiter (if any). Take()-and-send is
                // single-shot: a duplicate SessionStarted (replay, agent
                // re-init) finds None here and is a no-op, which is
                // exactly the desired idempotent behavior. send returns
                // Err only when the receiver dropped (timeout already
                // fired in spawn_agent) — also a no-op.
                if let Some(tx) = self.session_started_tx.take() {
                    let _ = tx.send(());
                }
            }
            AcpEvent::StatusChanged { status } => {
                self.status = status.clone();
            }
            AcpEvent::SessionModes { modes } => {
                self.current_mode = Some(modes.current_mode_id.clone());
                self.modes = Some(modes.clone());
            }
            AcpEvent::ModeChanged { mode_id } => {
                self.current_mode = Some(mode_id.clone());
                // Keep `modes.current_mode_id` consistent with the latched
                // `current_mode`. Snapshot consumers read `modes.current_mode_id`
                // directly (the frontend's `denormalizeSnapshot` does not look
                // at the separate `current_mode` field), so without this sync
                // a session that has switched modes would hydrate post-refresh
                // showing the original default — even though the live event
                // stream has long since corrected it.
                if let Some(modes) = self.modes.as_mut() {
                    modes.current_mode_id = mode_id.clone();
                }
            }
            AcpEvent::SessionConfigOptions { config_options } => {
                self.config_options = Some(config_options.clone());
            }
            AcpEvent::PromptCapabilities {
                prompt_capabilities,
            } => {
                self.prompt_capabilities = Some(prompt_capabilities.clone());
            }
            AcpEvent::ForkSupported { supported } => {
                self.fork_supported = *supported;
            }
            AcpEvent::AvailableCommands { commands } => {
                self.available_commands = commands.clone();
            }
            AcpEvent::UsageUpdate { used, size } => {
                self.usage = Some(UsageInfo {
                    used: *used,
                    size: *size,
                });
            }
            AcpEvent::ContentDelta { text } => {
                self.append_text_delta(text);
            }
            AcpEvent::Thinking { text } => {
                self.append_thinking_delta(text);
            }
            AcpEvent::ToolCall {
                tool_call_id,
                title,
                kind,
                status,
                content,
                raw_input,
                raw_output,
                locations,
                meta,
                images,
            } => {
                self.upsert_tool_call(
                    tool_call_id,
                    Some(kind),
                    Some(title),
                    Some(status),
                    content.as_deref(),
                    raw_input.as_deref(),
                    raw_output.as_deref(),
                    locations.as_ref(),
                    meta.as_ref(),
                    images.as_deref(),
                );
                // Anchor the tool call in `live_message.content` so snapshot
                // reload preserves position relative to surrounding text /
                // thinking blocks. Idempotent by id: a second ToolCall (or a
                // ToolCallUpdate, see below) for the same id must not push a
                // duplicate ref. Mirrors text/thinking deltas in lazily
                // creating `live_message` if absent.
                self.push_tool_call_ref_if_absent(tool_call_id);
            }
            AcpEvent::ToolCallUpdate {
                tool_call_id,
                title,
                status,
                content,
                raw_input,
                raw_output,
                locations,
                meta,
                images,
                ..
            } => {
                self.upsert_tool_call(
                    tool_call_id,
                    None,
                    title.as_deref(),
                    status.as_deref(),
                    content.as_deref(),
                    raw_input.as_deref(),
                    raw_output.as_deref(),
                    locations.as_ref(),
                    meta.as_ref(),
                    images.as_deref(),
                );
                // Defensive: if a ToolCallUpdate arrives before its initial
                // ToolCall (unusual ordering / replay), ensure the ref block
                // still gets anchored. Idempotent so the normal-flow case is
                // a no-op here.
                self.push_tool_call_ref_if_absent(tool_call_id);
            }
            AcpEvent::PermissionRequest {
                request_id,
                tool_call,
                options,
            } => {
                let tc_id = extract_tool_call_id(tool_call);
                self.pending_permission = Some(PendingPermissionState {
                    request_id: request_id.clone(),
                    tool_call_id: tc_id,
                    tool_call: tool_call.clone(),
                    options: options.clone(),
                    created_at: Utc::now(),
                });
            }
            AcpEvent::PermissionResolved { request_id } => {
                // Drop the snapshot's pending_permission iff the resolved
                // request matches the current one. Without the id check, a
                // late-arriving resolved event for an already-replaced
                // request could wipe the live dialog out from under the
                // user.
                if matches!(
                    &self.pending_permission,
                    Some(p) if p.request_id == *request_id,
                ) {
                    self.pending_permission = None;
                }
            }
            AcpEvent::TurnComplete { .. } => {
                // Snapshot the just-finished turn's FINAL assistant text — what
                // `get_delegation_status` returns as the child result. We take
                // the Text blocks that follow the LAST tool call (the agent's
                // concluding answer), skipping any trailing Thinking/Plan blocks:
                // a `PlanUpdate` is always re-appended at the end of content, so a
                // trailing-only scan would wrongly drop the answer sitting before
                // it. No tool calls → all the turn's text. A turn ending on a tool
                // call (no concluding text) → empty, which CLEARS the field so a
                // prior turn's text can't leak as this turn's result; the LLM
                // reads the full result by opening the child session instead.
                if let Some(live) = self.live_message.as_ref() {
                    let after_last_tool_call = live
                        .content
                        .iter()
                        .rposition(|b| matches!(b, LiveContentBlock::ToolCallRef { .. }))
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let assembled: String = live.content[after_last_tool_call..]
                        .iter()
                        .filter_map(|b| match b {
                            LiveContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<&str>>()
                        .join("");
                    self.last_assistant_text = if assembled.trim().is_empty() {
                        None
                    } else {
                        Some(assembled)
                    };
                }
                self.live_message = None;
                self.active_tool_calls.clear();
                // The turn's user prompt is no longer "in flight" — the
                // assistant reply is done and the transcript is the source of
                // truth. Clear it so a post-turn snapshot doesn't carry a stale
                // pending user message into a fresh attach.
                self.pending_user_message = None;
                self.pending_user_message_started_at = None;
                // Turn finished: release the concurrency gate so the next prompt
                // is accepted. (All connection-alive turn endings — normal,
                // cancel, stop-reason — emit TurnComplete; disconnect/error
                // discard the state entirely, so no stale flag can outlive them.)
                self.turn_in_flight = false;
                // NOTE: `active_delegations` is intentionally NOT cleared here.
                // A running delegation's child runs in the background long after
                // the parent's `delegate_to_agent` tool call returns and this
                // turn completes; clearing it would drop the running binding from
                // the snapshot the instant the parent turn ends (the original
                // web-only bug). It's removed per-entry by `DelegationCompleted`.
                self.pending_permission = None;
                self.status = ConnectionStatus::Connected;
            }
            AcpEvent::UserMessage { message_id, blocks } => {
                // Capture the in-flight user prompt so a client attaching
                // mid-turn renders the user turn from the snapshot (the
                // one-shot event won't replay for it). Cleared on TurnComplete.
                self.pending_user_message = Some(PendingUserMessage {
                    message_id: message_id.clone(),
                    blocks: blocks.clone(),
                });
                // Reference instant for the in-flight prompt's recency check in
                // `apply_in_flight_message_id`. Set here (not at manager enqueue)
                // so it tracks `pending_user_message` exactly.
                self.pending_user_message_started_at = Some(Utc::now());
                // Live-feedback notes are turn-scoped steering: a new user turn
                // starts with a clean slate. The previous turn's notes (read or
                // not) are history at this point; the frontend's "agent didn't
                // read your note → resend" fallback already had its post-turn
                // window before this next prompt arrives.
                self.feedback.clear();
            }
            AcpEvent::ConversationLinked {
                conversation_id,
                folder_id,
                ..
            } => {
                self.conversation_id = Some(*conversation_id);
                self.folder_id = Some(*folder_id);
            }
            AcpEvent::PlanUpdate { entries } => {
                // Replace any existing Plan block, then append at end.
                // Mirrors the frontend's PLAN_UPDATE reducer semantic: there
                // is at most one plan block, always at the current end of
                // content. `Vec<PlanEntryInfo>` is converted to
                // `serde_json::Value` because the wire-side `Plan` variant
                // stores it opaquely (frontend casts back to PlanEntryInfo[]).
                let live = self.ensure_live_message();
                live.content
                    .retain(|b| !matches!(b, LiveContentBlock::Plan { .. }));
                live.content.push(LiveContentBlock::Plan {
                    entries: serde_json::to_value(entries).unwrap_or(serde_json::Value::Null),
                });
            }
            AcpEvent::ConversationStatusChanged { .. } => {
                // No-op on purpose. Conversation row `status` is row-level
                // metadata persisted by the lifecycle subscriber / send_prompt
                // path, not in-flight session state — snapshot consumers read
                // status via the conversation list endpoints, not via
                // `LiveSessionSnapshot`. Listed explicitly (rather than swept
                // up by the catchall) so the no-op is intentional and grep-able.
            }
            AcpEvent::SelectorsReady => {
                // Latches once. Snapshot exposes this so a fresh frontend (e.g.
                // after browser refresh) can tell the initial handshake is
                // already done — the event fires only once per connection.
                self.selectors_ready = true;
            }
            AcpEvent::Error { message, code, .. } => {
                // Capture so post-mortem readers (probe path, debug
                // snapshots) can surface the agent's own error message
                // after the connection task has cleaned up its map
                // entry. The same payload is independently emitted
                // through the event channel for live chat-side UX.
                self.last_error = Some(SessionLastError {
                    message: message.clone(),
                    code: code.clone(),
                });
            }
            AcpEvent::DelegationStarted {
                parent_tool_use_id,
                child_connection_id,
                child_conversation_id,
                agent_type,
                ..
            } => {
                // Record the running delegation so the binding is snapshot-
                // recoverable (survives this connection's TurnComplete and any
                // re-attach on the snapshot path). The broker only emits this for
                // a REAL (non-synthetic) parent_tool_use_id, so synthetic-fallback
                // cards never create a phantom entry here — they rely on the
                // parent tool output (see DelegatedSubThread's ack fallback).
                self.active_delegations.insert(
                    parent_tool_use_id.clone(),
                    ActiveDelegationState {
                        parent_tool_use_id: parent_tool_use_id.clone(),
                        child_connection_id: child_connection_id.clone(),
                        child_conversation_id: *child_conversation_id,
                        agent_type: *agent_type,
                    },
                );
            }
            AcpEvent::DelegationCompleted {
                parent_tool_use_id, ..
            } => {
                // A running delegation finished: drop it from the live set. Its
                // terminal status/result reaches the LLM via
                // `get_delegation_status` and the UI via the live
                // `DelegationCompleted` event (DelegationProvider) or, on a cold
                // load, the child's persisted DB row (`inject_delegation_meta`).
                // Retaining it would turn this map into an unbounded history log;
                // it is deliberately only the in-flight set.
                self.active_delegations.remove(parent_tool_use_id);
            }
            AcpEvent::FeedbackSubmitted { item } => {
                // Idempotent by id (replay / double-attach safe): append only if
                // this note isn't already tracked. The authoritative append is
                // here so snapshot replay reconstructs the same list the live
                // node holds.
                if !self.feedback.iter().any(|f| f.id == item.id) {
                    self.feedback.push(item.clone());
                }
            }
            AcpEvent::FeedbackConsumed { ids, delivered_at } => {
                // Flip the named pending notes to Delivered. Idempotent: an id
                // already Delivered (the emitting node marked it directly under
                // the write lock; this re-apply is for replay/attach nodes) is
                // skipped. Order-independent and safe to apply more than once.
                for f in self.feedback.iter_mut() {
                    if f.status == FeedbackStatus::Pending && ids.contains(&f.id) {
                        f.status = FeedbackStatus::Delivered;
                        f.delivered_at = Some(*delivered_at);
                    }
                }
            }
            AcpEvent::ClaudeSdkMessage { .. }
            | AcpEvent::SessionLoadFailed { .. }
            | AcpEvent::UserPromptSent { .. } => {
                // 这些事件不直接修改 SessionState 的可见字段。
                // UserPromptSent 是纯通知事件，仅供 chat-channel 推送消费。
            }
        }
        self.last_activity_at = Utc::now();
    }

    /// A single-line "what the sub-agent is doing right now" hint, used by the
    /// delegation broker so `get_delegation_status` can prove a running child is
    /// genuinely making progress instead of returning a bare "Running.".
    ///
    /// Reads the still-streaming `live_message` — unlike `last_assistant_text`,
    /// which is only snapshotted at `TurnComplete` and so is empty/stale while a
    /// turn is in flight. Preference order, each reduced to one trimmed line
    /// capped at `max_chars` chars (char-based → never splits a UTF-8 codepoint;
    /// an `…` marks truncation):
    ///
    /// 1. the answer-in-progress — `Text` after the last `ToolCallRef`, mirroring
    ///    the `TurnComplete` answer extraction;
    /// 2. else the latest `Thinking` block (`thinking: …`);
    /// 3. else the most recent tool call's label (`running tool: …`).
    ///
    /// `None` when the turn hasn't produced anything renderable yet.
    pub fn latest_live_reply(&self, max_chars: usize) -> Option<String> {
        let live = self.live_message.as_ref()?;

        // (1) Answer-in-progress: the `Text` after the last tool call.
        //
        // Consecutive text deltas merge into a single block (see
        // `append_text_delta`), so this is almost always ONE block — borrow it
        // and take its last non-empty line without copying a potentially large
        // streaming answer on every poll (this runs under the `SessionState`
        // read lock on the `get_delegation_status` path). Only when the answer
        // is split across multiple `Text` blocks (a `Thinking` block interleaved
        // mid-answer) do we stitch them, which is rare.
        let after_last_tool_call = live
            .content
            .iter()
            .rposition(|b| matches!(b, LiveContentBlock::ToolCallRef { .. }))
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut texts = live.content[after_last_tool_call..]
            .iter()
            .filter_map(|b| match b {
                LiveContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            });
        match (texts.next(), texts.next()) {
            (None, _) => {}
            (Some(only), None) => {
                if let Some(line) = last_nonempty_line(only) {
                    return Some(truncate_one_line(line, max_chars));
                }
            }
            (Some(first), Some(second)) => {
                let mut joined = String::with_capacity(first.len() + second.len());
                joined.push_str(first);
                joined.push_str(second);
                for rest in texts {
                    joined.push_str(rest);
                }
                if let Some(line) = last_nonempty_line(&joined) {
                    return Some(truncate_one_line(line, max_chars));
                }
            }
        }

        // (2) Latest thinking block — the agent is reasoning, not silent.
        if let Some(line) = live
            .content
            .iter()
            .rev()
            .find_map(|b| match b {
                LiveContentBlock::Thinking { text } => Some(text.as_str()),
                _ => None,
            })
            .and_then(last_nonempty_line)
        {
            return Some(format!("thinking: {}", truncate_one_line(line, max_chars)));
        }

        // (3) Most recent tool call's label — work is happening in a tool.
        if let Some(label) = live
            .content
            .iter()
            .rev()
            .find_map(|b| match b {
                LiveContentBlock::ToolCallRef { tool_call_id } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .and_then(|id| self.active_tool_calls.get(id))
            .map(|tc| tc.label.trim())
            .filter(|l| !l.is_empty())
        {
            return Some(format!(
                "running tool: {}",
                truncate_one_line(label, max_chars)
            ));
        }

        None
    }

    /// Lazily initialize `self.live_message` and return a mutable reference
    /// to it. Centralizes the "create-if-absent" pattern shared by the
    /// text/thinking delta appenders, the tool-call ref pusher, and the
    /// plan-update applier.
    fn ensure_live_message(&mut self) -> &mut LiveMessage {
        if self.live_message.is_none() {
            self.live_message = Some(LiveMessage {
                id: format!("live-{}", uuid::Uuid::new_v4()),
                role: MessageRole::Assistant,
                content: Vec::new(),
                started_at: Utc::now(),
            });
        }
        self.live_message
            .as_mut()
            .expect("live_message just initialized")
    }

    fn append_text_delta(&mut self, text: &str) {
        let live = self.ensure_live_message();
        if let Some(LiveContentBlock::Text { text: existing }) = live.content.last_mut() {
            existing.push_str(text);
        } else {
            live.content.push(LiveContentBlock::Text {
                text: text.to_string(),
            });
        }
    }

    fn append_thinking_delta(&mut self, text: &str) {
        let live = self.ensure_live_message();
        if let Some(LiveContentBlock::Thinking { text: existing }) = live.content.last_mut() {
            existing.push_str(text);
        } else {
            live.content.push(LiveContentBlock::Thinking {
                text: text.to_string(),
            });
        }
    }

    /// Push a `ToolCallRef` block onto `live_message.content` for the given
    /// tool-call id, but only if no existing block in `content` already
    /// references that id. Called by both `ToolCall` and `ToolCallUpdate`
    /// arms so a tool's position survives any event-ordering edge case
    /// without ever duplicating.
    fn push_tool_call_ref_if_absent(&mut self, tool_call_id: &str) {
        let live = self.ensure_live_message();
        let already_present = live.content.iter().any(|b| {
            matches!(
                b,
                LiveContentBlock::ToolCallRef { tool_call_id: id } if id == tool_call_id
            )
        });
        if !already_present {
            live.content.push(LiveContentBlock::ToolCallRef {
                tool_call_id: tool_call_id.to_string(),
            });
        }
    }

    /// Insert-or-update a tool call entry. Used by both `ToolCall` (initial) and
    /// `ToolCallUpdate` events. `kind` is `Some` only on the initial event;
    /// title/status/content/raw_input/raw_output/locations/meta are merged
    /// when present. Partial-update preservation: a `None` value passed in
    /// from a `ToolCallUpdate` (which typically carries only the fields that
    /// changed) must NOT clobber a previously-set value on the entry.
    #[allow(clippy::too_many_arguments)]
    fn upsert_tool_call(
        &mut self,
        id: &str,
        kind: Option<&str>,
        title: Option<&str>,
        status: Option<&str>,
        content: Option<&str>,
        raw_input: Option<&str>,
        raw_output: Option<&str>,
        locations: Option<&serde_json::Value>,
        meta: Option<&serde_json::Value>,
        images: Option<&[ToolCallImageInfo]>,
    ) {
        let entry = self
            .active_tool_calls
            .entry(id.to_string())
            .or_insert_with(|| ToolCallState {
                id: id.to_string(),
                kind: ToolKind::Other,
                label: String::new(),
                status: ToolCallStatus::Pending,
                input: None,
                output: None,
                content: None,
                locations: None,
                meta: None,
                images: Vec::new(),
                raw_input_chunks: Vec::new(),
            });
        if let Some(k) = kind {
            entry.kind = parse_tool_kind(k);
        }
        if let Some(t) = title {
            entry.label = t.to_string();
        }
        if let Some(s) = status {
            entry.status = parse_tool_call_status(s);
        }
        if let Some(c) = content {
            entry.content = Some(c.to_string());
        }
        if let Some(chunk) = raw_input {
            entry.raw_input_chunks.push(chunk.to_string());
            // 后端目前发送的是已序列化的 JSON 文本（完整或正在累积）。
            // 对最新片段做尽力解析；解析失败则尝试拼接历史片段。
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(chunk) {
                entry.input = Some(value);
            } else if let Ok(value) =
                serde_json::from_str::<serde_json::Value>(&entry.raw_input_chunks.join(""))
            {
                entry.input = Some(value);
            }
        }
        if let Some(text) = raw_output {
            entry.output = Some(parse_tool_call_output_text(text));
        }
        if let Some(loc) = locations {
            entry.locations = Some(loc.clone());
        }
        if let Some(m) = meta {
            entry.meta = Some(m.clone());
        }
        if let Some(imgs) = images {
            // Replace-on-update: the agent re-sends the full image list on
            // every ToolCallUpdate that carries content (see
            // extract_tool_call_images in connection.rs). Absent images
            // (None at the AcpEvent layer) preserve the prior vec.
            entry.images = imgs.to_vec();
        }
    }

    /// 拷贝出对外可见的 wire-friendly snapshot。Phase 2 snapshot 端点直接调用此方法。
    pub fn to_snapshot(&self) -> LiveSessionSnapshot {
        LiveSessionSnapshot {
            connection_id: self.connection_id.clone(),
            conversation_id: self.conversation_id,
            folder_id: self.folder_id,
            status: self.status.clone(),
            external_id: self.external_id.clone(),
            live_message: self.live_message.clone(),
            active_tool_calls: self.active_tool_calls.values().cloned().collect(),
            pending_permission: self.pending_permission.clone(),
            pending_user_message: self.pending_user_message.clone(),
            active_delegations: self.active_delegations.values().cloned().collect(),
            feedback: self.feedback.clone(),
            feedback_tool_available: self.feedback_tool_available,
            modes: self.modes.clone(),
            current_mode: self.current_mode.clone(),
            config_options: self.config_options.clone(),
            prompt_capabilities: self.prompt_capabilities.clone(),
            usage: self.usage.clone(),
            fork_supported: self.fork_supported,
            available_commands: self.available_commands.clone(),
            selectors_ready: self.selectors_ready,
            event_seq: self.event_seq,
        }
    }
}

/// `to_snapshot()` 的输出——前端可消费的 wire shape。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSessionSnapshot {
    pub connection_id: String,
    pub conversation_id: Option<i32>,
    pub folder_id: Option<i32>,
    pub status: ConnectionStatus,
    pub external_id: Option<String>,
    pub live_message: Option<LiveMessage>,
    pub active_tool_calls: Vec<ToolCallState>,
    pub pending_permission: Option<PendingPermissionState>,
    /// The in-flight user prompt for the current turn (see
    /// `SessionState.pending_user_message`). `#[serde(default)]` so older
    /// payloads still deserialize; `skip_serializing_if` so the no-pending case
    /// keeps the wire shape byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_user_message: Option<PendingUserMessage>,
    /// Running sub-agent delegations recoverable from the snapshot (see
    /// `SessionState.active_delegations`). `#[serde(default)]` so older server
    /// payloads without this field still deserialize; `skip_serializing_if` so
    /// the common no-delegation case keeps the wire shape byte-identical and
    /// doesn't bloat every snapshot with an empty array.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_delegations: Vec<ActiveDelegationState>,
    /// Live user-feedback notes for the current turn (see `SessionState.feedback`).
    /// `#[serde(default)]` so older server payloads without this field still
    /// deserialize; `skip_serializing_if` keeps the common empty case off the
    /// wire so every snapshot stays byte-identical with the pre-feature shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feedback: Vec<FeedbackItem>,
    /// Whether this agent has the `check_user_feedback` tool (see
    /// `SessionState.feedback_tool_available`). `#[serde(default)]` so older
    /// payloads deserialize to `false`; the frontend gates the feedback bar on
    /// it. Always serialized (a plain bool) so the frontend can rely on it.
    #[serde(default)]
    pub feedback_tool_available: bool,
    pub modes: Option<SessionModeStateInfo>,
    pub current_mode: Option<String>,
    pub config_options: Option<Vec<SessionConfigOptionInfo>>,
    pub prompt_capabilities: Option<PromptCapabilitiesInfo>,
    pub usage: Option<UsageInfo>,
    pub fork_supported: bool,
    pub available_commands: Vec<AvailableCommandInfo>,
    pub selectors_ready: bool,
    pub event_seq: u64,
}

/// Last non-empty line of `s`, trimmed. `None` if every line is blank.
fn last_nonempty_line(s: &str) -> Option<&str> {
    s.lines().map(str::trim).rev().find(|l| !l.is_empty())
}

/// Cap `line` at `max_chars` characters, appending `…` when truncated. Operates
/// on `char`s so multi-byte text never splits mid-codepoint. Expects an
/// already single, trimmed line (see [`last_nonempty_line`]). Single-pass: takes
/// at most `max_chars + 1` chars total, so a huge (e.g. MB) input line never
/// triggers a second full scan to decide whether to mark truncation.
fn truncate_one_line(line: &str, max_chars: usize) -> String {
    let mut chars = line.chars();
    let mut out: String = (&mut chars).take(max_chars).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

fn parse_tool_kind(s: &str) -> ToolKind {
    match s {
        "read" => ToolKind::Read,
        "edit" => ToolKind::Edit,
        "delete" => ToolKind::Delete,
        "move" => ToolKind::Move,
        "search" => ToolKind::Search,
        "execute" => ToolKind::Execute,
        "think" => ToolKind::Think,
        "fetch" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

fn parse_tool_call_status(s: &str) -> ToolCallStatus {
    match s {
        "in_progress" => ToolCallStatus::InProgress,
        "completed" => ToolCallStatus::Completed,
        "failed" => ToolCallStatus::Failed,
        _ => ToolCallStatus::Pending,
    }
}

/// `raw_output` 是已序列化的 JSON 文本。尽力解析为结构化 JSON；解析失败时回退为
/// 文本。如果解析后的 JSON 顶层有 `"error"` 字段，提升为 `Error` 变体。
fn parse_tool_call_output_text(text: &str) -> ToolCallOutput {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => {
            if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
                ToolCallOutput::Error {
                    message: err.to_string(),
                }
            } else if let Some(s) = value.as_str() {
                ToolCallOutput::Text {
                    content: s.to_string(),
                }
            } else {
                ToolCallOutput::Json { value }
            }
        }
        Err(_) => ToolCallOutput::Text {
            content: text.to_string(),
        },
    }
}

/// Permission 事件的 `tool_call` 字段是 ACP 的 ToolCall JSON。提取 id 用作
/// `PendingPermissionState.tool_call_id`——快查路径（match by id 时不必每次重
/// 解析整个 tool_call value）。完整 tool_call value 由调用方另行保留，前端
/// 依赖它做 diff / 命令 / plan 渲染。同时兼容 camelCase / snake_case。
fn extract_tool_call_id(tool_call: &serde_json::Value) -> String {
    tool_call
        .as_object()
        .and_then(|o| {
            o.get("toolCallId")
                .or_else(|| o.get("tool_call_id"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::types::{
        AcpEvent, ConnectionStatus, DelegationResultSummary, EventEnvelope, PromptCapabilitiesInfo,
        SessionConfigKindInfo, SessionConfigOptionInfo, SessionConfigSelectInfo, SessionModeInfo,
        SessionModeStateInfo, UserMessageBlock,
    };

    fn fresh_state() -> SessionState {
        SessionState::new(
            "conn-test".to_string(),
            AgentType::ClaudeCode,
            None,
            "win-test".to_string(),
            None,
        )
    }

    #[test]
    fn new_session_starts_with_seq_zero_and_connecting_status() {
        let s = fresh_state();
        assert_eq!(s.event_seq, 0);
        assert_eq!(s.status, ConnectionStatus::Connecting);
        assert!(s.external_id.is_none());
        assert!(s.live_message.is_none());
        assert!(s.active_tool_calls.is_empty());
        assert!(s.pending_permission.is_none());
        assert!(!s.fork_supported);
        assert!(s.available_commands.is_empty());
        assert!(!s.selectors_ready);
        assert!(s.pending_user_message.is_none());
    }

    fn text_user_message(id: &str, text: &str) -> AcpEvent {
        AcpEvent::UserMessage {
            message_id: id.to_string(),
            blocks: vec![UserMessageBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    #[test]
    fn user_message_event_captures_pending_user_message() {
        // The in-flight user prompt is captured so a mid-turn attacher renders
        // the user turn from the snapshot (the one-shot event won't replay).
        let mut s = fresh_state();
        s.apply_event(&text_user_message("user-1", "hello agent"));
        let pending = s.pending_user_message.as_ref().expect("pending set");
        assert_eq!(pending.message_id, "user-1");
        assert_eq!(
            pending.blocks,
            vec![UserMessageBlock::Text {
                text: "hello agent".into()
            }]
        );
        assert!(
            s.pending_user_message_started_at.is_some(),
            "the turn-start instant is captured alongside the pending prompt"
        );
    }

    #[test]
    fn turn_complete_clears_pending_user_message() {
        let mut s = fresh_state();
        s.apply_event(&text_user_message("user-1", "hi"));
        assert!(s.pending_user_message.is_some());
        s.apply_event(&AcpEvent::TurnComplete {
            session_id: "sess".into(),
            stop_reason: "end_turn".into(),
            agent_type: "claude_code".into(),
        });
        assert!(
            s.pending_user_message.is_none(),
            "a completed turn must clear the pending user message (no stale snapshot)"
        );
        assert!(
            s.pending_user_message_started_at.is_none(),
            "the turn-start instant is cleared in lockstep with the pending prompt"
        );
    }

    #[test]
    fn to_snapshot_carries_pending_user_message() {
        let mut s = fresh_state();
        s.apply_event(&text_user_message("user-7", "snapshot me"));
        let pending = s
            .to_snapshot()
            .pending_user_message
            .expect("snapshot carries pending");
        assert_eq!(pending.message_id, "user-7");
    }

    #[test]
    fn snapshot_round_trips_pending_user_message_and_omits_when_absent() {
        let mut s = fresh_state();
        s.apply_event(&text_user_message("user-9", "round trip"));
        let snap = s.to_snapshot();
        let json = serde_json::to_string(&snap).expect("serialize");
        let back: LiveSessionSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.pending_user_message, snap.pending_user_message);
        // No-pending snapshot keeps the field off the wire (byte-identical with
        // the pre-feature shape).
        let empty_json = serde_json::to_string(&fresh_state().to_snapshot()).expect("serialize");
        assert!(
            !empty_json.contains("pending_user_message"),
            "no-pending snapshot must omit the field"
        );
    }

    #[test]
    fn latest_live_reply_prefers_answer_after_last_tool_call() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ContentDelta {
            text: "let me check".into(),
        });
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "ls".into(),
            kind: "execute".into(),
            status: "in_progress".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        s.apply_event(&AcpEvent::ContentDelta {
            text: "Found 3 files.\nDetails here".into(),
        });
        // Last non-empty line of the text that follows the final tool call.
        assert_eq!(s.latest_live_reply(100).as_deref(), Some("Details here"));
    }

    #[test]
    fn latest_live_reply_falls_back_to_thinking_then_tool() {
        // Thinking only → `thinking:` prefix.
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::Thinking {
            text: "pondering options".into(),
        });
        assert_eq!(
            s.latest_live_reply(100).as_deref(),
            Some("thinking: pondering options")
        );

        // A tool call with no trailing text / thinking → `running tool:` prefix.
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-9".into(),
            title: "grep files".into(),
            kind: "search".into(),
            status: "in_progress".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        assert_eq!(
            s.latest_live_reply(100).as_deref(),
            Some("running tool: grep files")
        );
    }

    #[test]
    fn latest_live_reply_truncates_to_char_budget_and_handles_empty() {
        // No live message yet → nothing to report.
        assert_eq!(fresh_state().latest_live_reply(100), None);

        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ContentDelta {
            text: "0123456789abcdef".into(),
        });
        assert_eq!(s.latest_live_reply(10).as_deref(), Some("0123456789…"));
    }

    #[test]
    fn latest_live_reply_extracts_last_line_from_large_multiline_and_truncates_utf8() {
        let mut s = fresh_state();
        // A large multi-line streamed answer, a final multi-byte line, then
        // trailing blank lines (which must be skipped). The tail extraction must
        // not copy the whole answer, and truncation must land on a codepoint
        // boundary.
        let huge = "x".repeat(5000);
        let last = "résumé 完成 ▸ 配置已更新";
        s.apply_event(&AcpEvent::ContentDelta {
            text: format!("{huge}\nintermediate\n{last}\n   \n"),
        });
        let out = s.latest_live_reply(8).unwrap();
        // First 8 chars of `last` are r é s u m é <space> 完, then a truncation
        // marker — codepoint-safe (8 multi-byte chars + the ellipsis), proving
        // the cap counts chars, not bytes.
        assert_eq!(out, "résumé 完…");
        assert_eq!(out.chars().count(), 9);
    }

    #[test]
    fn latest_live_reply_stitches_text_split_by_interleaved_thinking() {
        // A Thinking block between two text deltas yields two separate Text
        // blocks; their concatenation forms the single answer line.
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ContentDelta {
            text: "Answer ".into(),
        });
        s.apply_event(&AcpEvent::Thinking { text: "hmm".into() });
        s.apply_event(&AcpEvent::ContentDelta {
            text: "continues here".into(),
        });
        assert_eq!(
            s.latest_live_reply(100).as_deref(),
            Some("Answer continues here")
        );
    }

    #[test]
    fn selectors_ready_event_latches_state_and_snapshot() {
        let mut s = fresh_state();
        assert!(!s.selectors_ready);
        assert!(!s.to_snapshot().selectors_ready);
        s.apply_event(&AcpEvent::SelectorsReady);
        assert!(s.selectors_ready);
        assert!(s.to_snapshot().selectors_ready);
        // Idempotent — staying true on a second apply.
        s.apply_event(&AcpEvent::SelectorsReady);
        assert!(s.selectors_ready);
    }

    #[test]
    fn conversation_status_changed_event_is_a_visible_field_noop() {
        use crate::db::entities::conversation::ConversationStatus;
        // Seed a fully-populated state so we can verify nothing visible mutates
        // when ConversationStatusChanged is applied.
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::SessionStarted {
            session_id: "ext-1".into(),
        });
        s.apply_event(&AcpEvent::ContentDelta {
            text: "hello".into(),
        });
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "ls".into(),
            kind: "execute".into(),
            status: "pending".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        s.apply_event(&AcpEvent::ConversationLinked {
            conversation_id: 7,
            folder_id: 3,
            parent_conversation_id: None,
            parent_tool_use_id: None,
        });
        let before = s.to_snapshot();
        let before_status = s.status.clone();
        let before_conversation_id = s.conversation_id;
        let before_external_id = s.external_id.clone();

        s.apply_event(&AcpEvent::ConversationStatusChanged {
            conversation_id: 7,
            status: ConversationStatus::InProgress,
        });

        // Visible state fields unchanged.
        assert_eq!(s.status, before_status);
        assert_eq!(s.conversation_id, before_conversation_id);
        assert_eq!(s.external_id, before_external_id);
        assert!(
            s.live_message.is_some(),
            "live_message must be preserved across status-changed event"
        );
        assert_eq!(s.active_tool_calls.len(), 1);
        assert!(s.active_tool_calls.contains_key("tc-1"));

        // Snapshot output unchanged (modulo last_activity_at which is internal).
        let after = s.to_snapshot();
        assert_eq!(
            serde_json::to_value(&before).unwrap(),
            serde_json::to_value(&after).unwrap(),
            "snapshot must be byte-identical after no-op event"
        );
    }

    #[test]
    fn conversation_linked_event_writes_ids_into_state_and_snapshot() {
        let mut s = fresh_state();
        assert_eq!(s.conversation_id, None);
        assert_eq!(s.folder_id, None);
        s.apply_event(&AcpEvent::ConversationLinked {
            conversation_id: 42,
            folder_id: 7,
            parent_conversation_id: None,
            parent_tool_use_id: None,
        });
        assert_eq!(s.conversation_id, Some(42));
        assert_eq!(s.folder_id, Some(7));
        let snap = s.to_snapshot();
        assert_eq!(snap.conversation_id, Some(42));
        assert_eq!(snap.folder_id, Some(7));
    }

    #[test]
    fn session_started_sets_external_id_and_connected_status() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::SessionStarted {
            session_id: "ext-42".into(),
        });
        assert_eq!(s.external_id.as_deref(), Some("ext-42"));
        assert_eq!(s.status, ConnectionStatus::Connected);
    }

    #[tokio::test]
    async fn session_started_signal_fires_when_session_started_applies() {
        let mut s = fresh_state();
        let rx = s.install_session_started_signal();
        // Pre-fire: rx not ready.
        assert!(s.session_started_tx.is_some());

        s.apply_event(&AcpEvent::SessionStarted {
            session_id: "ext-1".into(),
        });

        // tx was take()'d.
        assert!(s.session_started_tx.is_none());
        // rx resolves with Ok(()) — bounded timeout because the test must
        // never hang if the signal logic regresses.
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), rx).await;
        assert!(
            matches!(result, Ok(Ok(()))),
            "rx must fire on SessionStarted; got {result:?}"
        );
    }

    #[tokio::test]
    async fn session_started_signal_is_single_shot_safe_against_replay() {
        let mut s = fresh_state();
        let rx = s.install_session_started_signal();
        s.apply_event(&AcpEvent::SessionStarted {
            session_id: "ext-1".into(),
        });
        // Replay (or any second SessionStarted) must not panic / double-fire.
        s.apply_event(&AcpEvent::SessionStarted {
            session_id: "ext-2".into(),
        });
        // The first send delivered; rx is consumed.
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), rx).await;
        assert!(matches!(result, Ok(Ok(()))));
    }

    #[tokio::test]
    async fn session_started_rx_aborts_when_state_drops_before_session_started() {
        // Mirrors the production "agent died before SessionStarted" path:
        // SessionState owns tx, gets dropped → rx receives RecvError. The
        // dedup waiter in `spawn_agent` treats this as "abort, release
        // dedup_lock, let next caller proceed".
        let rx = {
            let mut s = fresh_state();
            s.install_session_started_signal()
            // s drops here, taking tx with it.
        };
        let result = tokio::time::timeout(std::time::Duration::from_millis(50), rx).await;
        assert!(
            matches!(result, Ok(Err(_))),
            "rx must receive Err when sender drops without sending; got {result:?}"
        );
    }

    #[test]
    fn content_delta_creates_live_message_then_appends() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ContentDelta {
            text: "hello ".into(),
        });
        s.apply_event(&AcpEvent::ContentDelta {
            text: "world".into(),
        });
        let live = s.live_message.as_ref().expect("live_message expected");
        assert_eq!(
            live.content.len(),
            1,
            "consecutive text deltas merge into one block"
        );
        match &live.content[0] {
            LiveContentBlock::Text { text } => assert_eq!(text, "hello world"),
            _ => panic!("expected text block"),
        }
        assert!(matches!(live.role, MessageRole::Assistant));
    }

    #[test]
    fn thinking_delta_creates_separate_block_from_text() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ContentDelta { text: "T".into() });
        s.apply_event(&AcpEvent::Thinking { text: "X".into() });
        s.apply_event(&AcpEvent::ContentDelta { text: "Y".into() });
        let live = s.live_message.as_ref().unwrap();
        assert_eq!(live.content.len(), 3);
        match &live.content[0] {
            LiveContentBlock::Text { text } => assert_eq!(text, "T"),
            _ => panic!("expected text"),
        }
        match &live.content[1] {
            LiveContentBlock::Thinking { text } => assert_eq!(text, "X"),
            _ => panic!("expected thinking"),
        }
        match &live.content[2] {
            LiveContentBlock::Text { text } => assert_eq!(text, "Y"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn tool_call_inserts_pending_entry() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "ls".into(),
            kind: "execute".into(),
            status: "pending".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        let entry = s.active_tool_calls.get("tc-1").expect("tc-1 inserted");
        assert_eq!(entry.status, ToolCallStatus::Pending);
        assert_eq!(entry.kind, ToolKind::Execute);
        assert_eq!(entry.label, "ls");
        assert!(entry.input.is_none());
        assert!(entry.output.is_none());
    }

    #[test]
    fn snapshot_active_tool_calls_are_sorted_by_id() {
        let mut s = fresh_state();
        for id in ["tc-z", "tc-a", "tc-m"] {
            s.apply_event(&AcpEvent::ToolCall {
                tool_call_id: id.into(),
                title: id.into(),
                kind: "read".into(),
                status: "pending".into(),
                content: None,
                raw_input: None,
                raw_output: None,
                locations: None,
                meta: None,
                images: None,
            });
        }
        let snap = s.to_snapshot();
        let ids: Vec<&str> = snap
            .active_tool_calls
            .iter()
            .map(|tc| tc.id.as_str())
            .collect();
        assert_eq!(ids, vec!["tc-a", "tc-m", "tc-z"]);
    }

    #[test]
    fn tool_call_content_field_is_preserved_on_state() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "ls".into(),
            kind: "execute".into(),
            status: "pending".into(),
            content: Some("line one\nline two".into()),
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        let entry = s.active_tool_calls.get("tc-1").expect("tc-1 inserted");
        assert_eq!(entry.content.as_deref(), Some("line one\nline two"));

        s.apply_event(&AcpEvent::ToolCallUpdate {
            tool_call_id: "tc-1".into(),
            title: None,
            status: None,
            content: Some("line three".into()),
            raw_input: None,
            raw_output: None,
            raw_output_append: None,
            locations: None,
            meta: None,
            images: None,
        });
        let entry = s.active_tool_calls.get("tc-1").unwrap();
        // Phase 2 chooses replace-on-update semantics: update == latest known content.
        assert_eq!(entry.content.as_deref(), Some("line three"));
    }

    #[test]
    fn tool_call_update_merges_status_and_output() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "cat foo.txt".into(),
            kind: "read".into(),
            status: "in_progress".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        // raw_output text "\"file contents\"" — i.e. JSON-encoded string.
        s.apply_event(&AcpEvent::ToolCallUpdate {
            tool_call_id: "tc-1".into(),
            title: None,
            status: Some("completed".into()),
            content: None,
            raw_input: None,
            raw_output: Some("\"file contents\"".into()),
            raw_output_append: None,
            locations: None,
            meta: None,
            images: None,
        });
        let entry = s.active_tool_calls.get("tc-1").unwrap();
        assert_eq!(entry.status, ToolCallStatus::Completed);
        assert_eq!(entry.kind, ToolKind::Read);
        assert_eq!(entry.label, "cat foo.txt");
        match &entry.output {
            Some(ToolCallOutput::Text { content }) => assert_eq!(content, "file contents"),
            other => panic!("expected text output, got {:?}", other),
        }
    }

    #[test]
    fn turn_complete_clears_live_and_tool_calls_and_pending_permission() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ContentDelta { text: "hi".into() });
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "x".into(),
            kind: "read".into(),
            status: "pending".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        s.apply_event(&AcpEvent::PermissionRequest {
            request_id: "p-1".into(),
            tool_call: serde_json::json!({"toolCallId": "tc-1", "title": "danger"}),
            options: vec![],
        });
        assert!(s.live_message.is_some());
        assert!(s.pending_permission.is_some());
        assert_eq!(s.active_tool_calls.len(), 1);
        s.apply_event(&AcpEvent::TurnComplete {
            session_id: "ext".into(),
            stop_reason: "end_turn".into(),
            agent_type: "claude_code".into(),
        });
        assert!(s.live_message.is_none());
        assert!(s.active_tool_calls.is_empty());
        assert!(s.pending_permission.is_none());
        assert_eq!(s.status, ConnectionStatus::Connected);
    }

    // --- active_delegations: running-only, snapshot-recoverable binding ---

    fn delegation_started(parent_tool_use_id: &str, child_conv: i32) -> AcpEvent {
        AcpEvent::DelegationStarted {
            parent_connection_id: "conn-test".into(),
            parent_tool_use_id: parent_tool_use_id.into(),
            child_connection_id: "child-conn-1".into(),
            child_conversation_id: child_conv,
            agent_type: AgentType::Codex,
        }
    }

    fn delegation_completed(parent_tool_use_id: &str, child_conv: i32) -> AcpEvent {
        AcpEvent::DelegationCompleted {
            parent_connection_id: "conn-test".into(),
            parent_tool_use_id: parent_tool_use_id.into(),
            child_connection_id: "child-conn-1".into(),
            child_conversation_id: child_conv,
            agent_type: AgentType::Codex,
            result: DelegationResultSummary::Ok {
                duration_ms: 1,
                text_preview: None,
            },
        }
    }

    #[test]
    fn delegation_started_populates_active_delegations_and_snapshot() {
        let mut s = fresh_state();
        s.apply_event(&delegation_started("pt-1", 99));

        let d = s
            .active_delegations
            .get("pt-1")
            .expect("active delegation recorded");
        assert_eq!(d.child_conversation_id, 99);
        assert_eq!(d.child_connection_id, "child-conn-1");
        assert_eq!(d.agent_type, AgentType::Codex);

        // Surfaced on the snapshot, and survives the JSON round-trip the web
        // client hydrates from.
        let snap = s.to_snapshot();
        assert_eq!(snap.active_delegations.len(), 1);
        let json = serde_json::to_string(&snap).unwrap();
        let back: LiveSessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.active_delegations.len(), 1);
        assert_eq!(back.active_delegations[0].parent_tool_use_id, "pt-1");
        assert_eq!(back.active_delegations[0].child_conversation_id, 99);
    }

    #[test]
    fn active_delegations_survives_turn_complete() {
        // Core regression for the web-only bug: an async delegation's child runs
        // in the background AFTER the parent's `delegate_to_agent` tool call
        // returns and the parent turn completes. TurnComplete clears
        // live_message / active_tool_calls but MUST NOT clear active_delegations
        // — otherwise the running binding vanishes from the snapshot the instant
        // the parent turn ends, and a web/server attach (snapshot path) can't
        // recover it.
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "pt-1".into(),
            title: "delegate_to_agent".into(),
            kind: "other".into(),
            status: "in_progress".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        s.apply_event(&delegation_started("pt-1", 99));
        assert!(s.active_tool_calls.contains_key("pt-1"));
        assert!(s.active_delegations.contains_key("pt-1"));

        s.apply_event(&AcpEvent::TurnComplete {
            session_id: "ext".into(),
            stop_reason: "end_turn".into(),
            agent_type: "claude_code".into(),
        });

        assert!(
            s.active_tool_calls.is_empty(),
            "TurnComplete still clears in-flight tool calls"
        );
        assert!(
            s.active_delegations.contains_key("pt-1"),
            "running delegation binding must survive TurnComplete"
        );
        assert_eq!(
            s.to_snapshot().active_delegations.len(),
            1,
            "binding still on the snapshot a post-turn attach would receive"
        );
    }

    #[test]
    fn delegation_completed_removes_entry() {
        // Completed delegations are NOT retained here — their terminal state is
        // recovered from the child's persisted DB row (inject_delegation_meta)
        // and the live DelegationProvider binding, not from this in-flight set.
        let mut s = fresh_state();
        s.apply_event(&delegation_started("pt-1", 99));
        assert!(s.active_delegations.contains_key("pt-1"));
        s.apply_event(&delegation_completed("pt-1", 99));
        assert!(
            !s.active_delegations.contains_key("pt-1"),
            "completed delegation removed from the in-flight set"
        );
        assert!(s.to_snapshot().active_delegations.is_empty());
    }

    #[test]
    fn delegation_completed_without_started_is_noop() {
        // A stream that only delivered the completion (started never observed on
        // this connection) must not synthesize a phantom entry: removing an
        // absent key is a no-op, and there is no running child to bind.
        let mut s = fresh_state();
        s.apply_event(&delegation_completed("pt-unknown", 7));
        assert!(s.active_delegations.is_empty());
    }

    #[test]
    fn active_delegations_unbounded_by_running_fanout() {
        // No cap: a parent fanning out far past any old soft bound keeps every
        // running binding (size tracks live concurrency, not an artificial
        // limit). Completing them drains the set back to empty.
        let mut s = fresh_state();
        let n: i32 = 200;
        for i in 0..n {
            s.apply_event(&delegation_started(&format!("pt-{i}"), 1000 + i));
        }
        assert_eq!(s.active_delegations.len(), n as usize);
        assert_eq!(s.to_snapshot().active_delegations.len(), n as usize);
        for i in 0..n {
            s.apply_event(&delegation_completed(&format!("pt-{i}"), 1000 + i));
        }
        assert!(s.active_delegations.is_empty());
    }

    #[test]
    fn delegation_binding_survives_snapshot_split_like_live() {
        // Path A (live): apply started + completed straight through.
        // Path B (reconnect): apply started, snapshot round-trip mid-flight,
        // then apply completed. Both must converge — proving a running
        // delegation recovered from the snapshot ends identically to one tracked
        // live. This is the exact web-attach path the original bug broke.
        let mut a = fresh_state();
        a.apply_event(&delegation_started("tc-1", 99));
        a.apply_event(&delegation_completed("tc-1", 99));

        let mut b = fresh_state();
        b.apply_event(&delegation_started("tc-1", 99));
        // Snapshot round-trip while the child is still running: the running
        // binding must ride along on the wire shape the web client hydrates from.
        let snap = b.to_snapshot();
        assert_eq!(snap.active_delegations.len(), 1);
        assert_eq!(snap.active_delegations[0].parent_tool_use_id, "tc-1");
        let wire = serde_json::to_string(&snap).unwrap();
        let _back: LiveSessionSnapshot = serde_json::from_str(&wire).unwrap();
        b.apply_event(&delegation_completed("tc-1", 99));

        assert_eq!(
            serde_json::to_value(a.to_snapshot().active_delegations).unwrap(),
            serde_json::to_value(b.to_snapshot().active_delegations).unwrap(),
            "snapshot-recovered delegation must match the live-tracked one"
        );
    }

    #[test]
    fn turn_complete_captures_only_trailing_text_block() {
        // last_assistant_text (the delegation result text surfaced by
        // get_delegation_status) keeps only the final text run — the answer
        // after the last tool call — not intermediate narration.
        let mut s = fresh_state();
        s.live_message = Some(LiveMessage {
            id: "m1".into(),
            role: MessageRole::Assistant,
            content: vec![
                LiveContentBlock::Text {
                    text: "let me check ".into(),
                },
                LiveContentBlock::ToolCallRef {
                    tool_call_id: "tc".into(),
                },
                LiveContentBlock::Text {
                    text: "the answer is 42".into(),
                },
            ],
            started_at: Utc::now(),
        });
        s.apply_event(&AcpEvent::TurnComplete {
            session_id: "ext".into(),
            stop_reason: "end_turn".into(),
            agent_type: "codex".into(),
        });
        assert_eq!(s.last_assistant_text.as_deref(), Some("the answer is 42"));
    }

    #[test]
    fn turn_complete_no_tool_calls_captures_full_text() {
        // With no tool call to split on, the trailing run is the whole answer.
        let mut s = fresh_state();
        s.live_message = Some(LiveMessage {
            id: "m1".into(),
            role: MessageRole::Assistant,
            content: vec![
                LiveContentBlock::Text {
                    text: "part 1 ".into(),
                },
                LiveContentBlock::Text {
                    text: "part 2".into(),
                },
            ],
            started_at: Utc::now(),
        });
        s.apply_event(&AcpEvent::TurnComplete {
            session_id: "ext".into(),
            stop_reason: "end_turn".into(),
            agent_type: "codex".into(),
        });
        assert_eq!(s.last_assistant_text.as_deref(), Some("part 1 part 2"));
    }

    #[test]
    fn turn_complete_trailing_tool_call_captures_no_text() {
        // A turn ending on a tool call has no concluding text block; the result
        // text stays unset (the LLM opens the child session for detail).
        let mut s = fresh_state();
        s.live_message = Some(LiveMessage {
            id: "m1".into(),
            role: MessageRole::Assistant,
            content: vec![
                LiveContentBlock::Text {
                    text: "running a tool".into(),
                },
                LiveContentBlock::ToolCallRef {
                    tool_call_id: "tc".into(),
                },
            ],
            started_at: Utc::now(),
        });
        s.apply_event(&AcpEvent::TurnComplete {
            session_id: "ext".into(),
            stop_reason: "end_turn".into(),
            agent_type: "codex".into(),
        });
        assert_eq!(s.last_assistant_text, None);
    }

    #[test]
    fn turn_complete_keeps_final_text_before_a_trailing_plan_block() {
        // `PlanUpdate` re-appends a Plan block at the END of content, so the
        // agent's concluding answer often sits BEFORE a trailing Plan. The
        // result must still be the text after the last tool call, not empty.
        let mut s = fresh_state();
        s.live_message = Some(LiveMessage {
            id: "m1".into(),
            role: MessageRole::Assistant,
            content: vec![
                LiveContentBlock::Text {
                    text: "let me check".into(),
                },
                LiveContentBlock::ToolCallRef {
                    tool_call_id: "tc".into(),
                },
                LiveContentBlock::Text {
                    text: "the answer is 42".into(),
                },
                LiveContentBlock::Plan {
                    entries: serde_json::json!([]),
                },
            ],
            started_at: Utc::now(),
        });
        s.apply_event(&AcpEvent::TurnComplete {
            session_id: "ext".into(),
            stop_reason: "end_turn".into(),
            agent_type: "codex".into(),
        });
        assert_eq!(s.last_assistant_text.as_deref(), Some("the answer is 42"));
    }

    #[test]
    fn turn_complete_clears_stale_last_assistant_text() {
        // A turn that ends with no concluding text must CLEAR any prior value
        // rather than leak it as this turn's delegation result.
        let mut s = fresh_state();
        s.last_assistant_text = Some("stale text from an earlier turn".into());
        s.live_message = Some(LiveMessage {
            id: "m1".into(),
            role: MessageRole::Assistant,
            content: vec![
                LiveContentBlock::Text {
                    text: "working".into(),
                },
                LiveContentBlock::ToolCallRef {
                    tool_call_id: "tc".into(),
                },
            ],
            started_at: Utc::now(),
        });
        s.apply_event(&AcpEvent::TurnComplete {
            session_id: "ext".into(),
            stop_reason: "end_turn".into(),
            agent_type: "codex".into(),
        });
        assert_eq!(s.last_assistant_text, None);
    }

    #[test]
    fn permission_resolved_clears_matching_request() {
        // Mirrors the pet snapshot semantics: when the user (or auto-approve)
        // responds, the snapshot's pending_permission must drop *before*
        // TurnComplete, otherwise a snapshot-recovering frontend (WS attach
        // after a refresh) would re-render a dialog the user has already
        // answered.
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::PermissionRequest {
            request_id: "p-1".into(),
            tool_call: serde_json::json!({"toolCallId": "tc-1"}),
            options: vec![],
        });
        assert!(s.pending_permission.is_some());

        s.apply_event(&AcpEvent::PermissionResolved {
            request_id: "p-1".into(),
        });
        assert!(
            s.pending_permission.is_none(),
            "matching PermissionResolved must clear the pending permission"
        );
    }

    #[test]
    fn permission_resolved_stale_request_is_noop() {
        // A late `PermissionResolved` for an already-replaced request must
        // not wipe out the *new* outstanding permission — id mismatch is
        // the only thing distinguishing the two, since the snapshot only
        // tracks one pending permission at a time.
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::PermissionRequest {
            request_id: "p-2".into(),
            tool_call: serde_json::json!({"toolCallId": "tc-2"}),
            options: vec![],
        });

        s.apply_event(&AcpEvent::PermissionResolved {
            request_id: "p-stale".into(),
        });
        let p = s
            .pending_permission
            .as_ref()
            .expect("stale PermissionResolved must not clear a non-matching pending permission");
        assert_eq!(p.request_id, "p-2");
    }

    #[test]
    fn permission_request_preserves_full_tool_call_value() {
        let mut s = fresh_state();
        // Realistic permission payload: title + kind + rawInput (used by the
        // frontend's permission parser to extract command / diff / plan).
        // After the refresh-survives-permission fix, all of this must round
        // trip via the snapshot — losing rawInput would force the user to
        // approve blind.
        let raw_tool_call = serde_json::json!({
            "toolCallId": "tc-9",
            "title": "Run rm -rf /",
            "kind": "execute",
            "rawInput": { "command": "rm -rf /" },
            "locations": [{ "path": "/", "line": 1 }],
        });
        s.apply_event(&AcpEvent::PermissionRequest {
            request_id: "p-1".into(),
            tool_call: raw_tool_call.clone(),
            options: vec![],
        });
        let p = s.pending_permission.as_ref().expect("permission set");
        assert_eq!(p.request_id, "p-1");
        assert_eq!(p.tool_call_id, "tc-9");
        assert_eq!(
            p.tool_call, raw_tool_call,
            "full tool_call JSON must round-trip into PendingPermissionState"
        );

        // Snapshot round-trip preserves it byte-for-byte (the load-bearing
        // property — frontend re-renders the approval dialog from this).
        let snap = s.to_snapshot();
        let snap_perm = snap.pending_permission.as_ref().unwrap();
        assert_eq!(snap_perm.tool_call, raw_tool_call);
    }

    #[test]
    fn mode_changed_updates_current_mode_and_session_modes_seeds_state() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::SessionModes {
            modes: SessionModeStateInfo {
                current_mode_id: "default".into(),
                available_modes: vec![SessionModeInfo {
                    id: "default".into(),
                    name: "Default".into(),
                    description: None,
                }],
            },
        });
        assert_eq!(s.current_mode.as_deref(), Some("default"));
        assert!(s.modes.is_some());
        s.apply_event(&AcpEvent::ModeChanged {
            mode_id: "edit".into(),
        });
        assert_eq!(s.current_mode.as_deref(), Some("edit"));
        // Snapshot consistency invariant: ModeChanged must keep
        // `modes.current_mode_id` in sync with the scalar `current_mode`.
        // The frontend's `denormalizeSnapshot` reads `modes.current_mode_id`
        // exclusively; without this sync a post-refresh hydration would
        // show the stale default even though the live event stream had
        // long since switched modes.
        assert_eq!(
            s.modes.as_ref().unwrap().current_mode_id,
            "edit",
            "ModeChanged must keep modes.current_mode_id consistent for snapshot consumers"
        );
    }

    #[test]
    fn snapshot_excludes_internal_chunk_buffers_and_carries_negotiated_caps() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::PromptCapabilities {
            prompt_capabilities: PromptCapabilitiesInfo {
                image: true,
                audio: false,
                embedded_context: true,
            },
        });
        s.apply_event(&AcpEvent::ForkSupported { supported: true });
        s.apply_event(&AcpEvent::SessionConfigOptions {
            config_options: vec![SessionConfigOptionInfo {
                id: "model".into(),
                name: "Model".into(),
                description: None,
                category: None,
                kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                    current_value: "sonnet".into(),
                    options: vec![],
                    groups: vec![],
                }),
            }],
        });
        s.apply_event(&AcpEvent::UsageUpdate {
            used: 1234,
            size: 200_000,
        });
        // Two raw_input fragments; the second is a complete JSON object
        // and should overwrite `entry.input` with the parsed value.
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "edit".into(),
            kind: "edit".into(),
            status: "pending".into(),
            content: None,
            raw_input: Some("{\"a\":".into()),
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        });
        s.apply_event(&AcpEvent::ToolCallUpdate {
            tool_call_id: "tc-1".into(),
            title: None,
            status: None,
            content: None,
            raw_input: Some("{\"a\":1}".into()),
            raw_output: None,
            raw_output_append: None,
            locations: None,
            meta: None,
            images: None,
        });
        let entry = s.active_tool_calls.get("tc-1").unwrap();
        assert_eq!(entry.input, Some(serde_json::json!({"a": 1})));
        assert_eq!(entry.raw_input_chunks.len(), 2);

        let snapshot = s.to_snapshot();
        assert_eq!(snapshot.connection_id, "conn-test");
        assert!(snapshot.fork_supported);
        assert_eq!(
            snapshot.usage,
            Some(UsageInfo {
                used: 1234,
                size: 200_000,
            })
        );
        assert!(snapshot.prompt_capabilities.is_some());
        assert_eq!(snapshot.config_options.as_ref().map(|v| v.len()), Some(1));
        assert_eq!(snapshot.active_tool_calls.len(), 1);

        // Wire shape: raw_input_chunks must NOT be serialized.
        let json = serde_json::to_value(&snapshot).unwrap();
        let tc_json = json["active_tool_calls"][0].clone();
        assert!(
            tc_json.get("raw_input_chunks").is_none(),
            "raw_input_chunks must be #[serde(skip)] (got {})",
            tc_json
        );
        assert_eq!(tc_json["input"], serde_json::json!({"a": 1}));
    }

    fn scripted_event_sequence() -> Vec<AcpEvent> {
        vec![
            AcpEvent::SessionStarted {
                session_id: "ext-1".into(),
            },
            AcpEvent::ContentDelta {
                text: "Hello ".into(),
            },
            AcpEvent::ContentDelta {
                text: "world".into(),
            },
            AcpEvent::ToolCall {
                tool_call_id: "tc-1".into(),
                title: "ls".into(),
                kind: "execute".into(),
                status: "pending".into(),
                content: None,
                raw_input: None,
                raw_output: None,
                locations: None,
                meta: None,
                images: None,
            },
            AcpEvent::ToolCallUpdate {
                tool_call_id: "tc-1".into(),
                title: None,
                status: Some("completed".into()),
                content: None,
                raw_input: None,
                raw_output: Some("\"done\"".into()),
                raw_output_append: None,
                locations: None,
                meta: None,
                images: None,
            },
            AcpEvent::Thinking {
                text: "considering".into(),
            },
            AcpEvent::ContentDelta {
                text: " More text".into(),
            },
            AcpEvent::UsageUpdate {
                used: 1234,
                size: 200_000,
            },
        ]
    }

    #[test]
    fn full_turn_lifecycle_increments_seq_monotonically() {
        let mut s = fresh_state();
        let events = scripted_event_sequence();
        let mut seq = 0u64;
        for e in &events {
            s.apply_event(e);
            seq += 1;
            s.event_seq = seq;
        }
        assert_eq!(s.event_seq, events.len() as u64);
    }

    /// Strip volatile fields that legitimately differ between Path A and Path B
    /// (e.g. `LiveMessage.id` is generated via `uuid::new_v4()` and `started_at`
    /// uses `Utc::now()`) but don't matter for snapshot/live consistency.
    fn normalize_snapshot(snap: &LiveSessionSnapshot) -> serde_json::Value {
        let mut v = serde_json::to_value(snap).unwrap();
        if let Some(lm) = v.get_mut("live_message") {
            if let Some(obj) = lm.as_object_mut() {
                obj.remove("id");
                obj.remove("started_at");
            }
        }
        v
    }

    /// 对账测试：从初始状态全程 apply 到 N 个事件 == 从 snapshot
    /// (apply 完前 K 个) + apply 剩下 N-K 个事件，最终状态等价。
    #[test]
    fn snapshot_filtered_events_yield_same_state_as_live_subscriber() {
        let events = scripted_event_sequence();
        let split = events.len() / 2;

        // Path A: live subscriber——全程 apply
        let mut a = fresh_state();
        for (i, e) in events.iter().enumerate() {
            a.apply_event(e);
            a.event_seq = (i + 1) as u64;
        }

        // Path B: snapshot 重连
        // 1) apply 前 split 个事件
        let mut b = fresh_state();
        for (i, e) in events.iter().take(split).enumerate() {
            b.apply_event(e);
            b.event_seq = (i + 1) as u64;
        }
        // 2) snapshot round-trip 通过 JSON
        let snapshot = b.to_snapshot();
        let _wire = serde_json::to_string(&snapshot).unwrap();
        // 3) 继续 apply 剩下事件
        for (i, e) in events.iter().enumerate().skip(split) {
            b.apply_event(e);
            b.event_seq = (i + 1) as u64;
        }

        let snap_a = a.to_snapshot();
        let snap_b = b.to_snapshot();

        assert_eq!(snap_a.event_seq, snap_b.event_seq);
        assert_eq!(snap_a.status, snap_b.status);
        assert_eq!(snap_a.external_id, snap_b.external_id);
        assert_eq!(snap_a.usage, snap_b.usage);

        // Full structural equivalence (with volatile fields stripped + tool
        // calls sorted by id). This is the load-bearing consistency check.
        assert_eq!(normalize_snapshot(&snap_a), normalize_snapshot(&snap_b));
    }

    // ---------- Phase 3c-3: snapshot fidelity ----------

    /// Helper: returns the kind discriminator + payload-id of each block in
    /// `live_message.content`, suitable for asserting block ordering.
    fn live_block_summary(s: &SessionState) -> Vec<(&'static str, String)> {
        s.live_message
            .as_ref()
            .map(|lm| {
                lm.content
                    .iter()
                    .map(|b| match b {
                        LiveContentBlock::Text { text } => ("text", text.clone()),
                        LiveContentBlock::Thinking { text } => ("thinking", text.clone()),
                        LiveContentBlock::ToolCallRef { tool_call_id } => {
                            ("tool_call_ref", tool_call_id.clone())
                        }
                        LiveContentBlock::Plan { entries } => {
                            ("plan", serde_json::to_string(entries).unwrap_or_default())
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn tool_call_event(id: &str, title: &str) -> AcpEvent {
        AcpEvent::ToolCall {
            tool_call_id: id.into(),
            title: title.into(),
            kind: "execute".into(),
            status: "pending".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: None,
        }
    }

    #[test]
    fn tool_call_pushes_ref_block_at_current_position() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ContentDelta {
            text: "before ".into(),
        });
        s.apply_event(&tool_call_event("tc-1", "ls"));
        s.apply_event(&AcpEvent::ContentDelta {
            text: "between".into(),
        });
        s.apply_event(&tool_call_event("tc-2", "pwd"));

        let summary = live_block_summary(&s);
        assert_eq!(
            summary,
            vec![
                ("text", "before ".to_string()),
                ("tool_call_ref", "tc-1".to_string()),
                ("text", "between".to_string()),
                ("tool_call_ref", "tc-2".to_string()),
            ],
            "tool-call refs must anchor at the position they arrived in the stream"
        );
    }

    #[test]
    fn tool_call_ref_push_is_idempotent() {
        let mut s = fresh_state();
        s.apply_event(&tool_call_event("tc-1", "ls"));
        // Defensive: second ToolCall with the same id (replay/unusual ordering)
        // must NOT push a duplicate ref block.
        s.apply_event(&tool_call_event("tc-1", "ls (retry)"));

        let summary = live_block_summary(&s);
        let ref_count = summary
            .iter()
            .filter(|(kind, id)| *kind == "tool_call_ref" && id == "tc-1")
            .count();
        assert_eq!(ref_count, 1, "duplicate ToolCall must not duplicate ref");
    }

    #[test]
    fn tool_call_update_does_not_duplicate_ref() {
        let mut s = fresh_state();
        s.apply_event(&tool_call_event("tc-1", "ls"));
        s.apply_event(&AcpEvent::ToolCallUpdate {
            tool_call_id: "tc-1".into(),
            title: None,
            status: Some("completed".into()),
            content: None,
            raw_input: None,
            raw_output: Some("\"done\"".into()),
            raw_output_append: None,
            locations: None,
            meta: None,
            images: None,
        });

        let summary = live_block_summary(&s);
        let ref_count = summary
            .iter()
            .filter(|(kind, id)| *kind == "tool_call_ref" && id == "tc-1")
            .count();
        assert_eq!(
            ref_count, 1,
            "ToolCall + ToolCallUpdate for same id yields exactly one ref"
        );
    }

    #[test]
    fn tool_call_state_carries_locations_and_meta() {
        let mut s = fresh_state();
        let locs = serde_json::json!([{ "path": "/tmp/foo.rs", "line": 12 }]);
        let meta = serde_json::json!({ "parent_tool_use_id": "abc", "session": "ext-1" });
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "edit".into(),
            kind: "edit".into(),
            status: "in_progress".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: Some(locs.clone()),
            meta: Some(meta.clone()),
            images: None,
        });
        let entry = s.active_tool_calls.get("tc-1").expect("tc-1 inserted");
        assert_eq!(entry.locations.as_ref(), Some(&locs));
        assert_eq!(entry.meta.as_ref(), Some(&meta));

        // Snapshot round-trip preserves both.
        let snap = s.to_snapshot();
        let tc = snap
            .active_tool_calls
            .iter()
            .find(|t| t.id == "tc-1")
            .unwrap();
        assert_eq!(tc.locations.as_ref(), Some(&locs));
        assert_eq!(tc.meta.as_ref(), Some(&meta));
    }

    #[test]
    fn tool_call_update_preserves_locations_when_omitted() {
        let mut s = fresh_state();
        let locs = serde_json::json!([{ "path": "/tmp/foo.rs" }]);
        let meta = serde_json::json!({ "k": "v" });
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "tc-1".into(),
            title: "edit".into(),
            kind: "edit".into(),
            status: "in_progress".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: Some(locs.clone()),
            meta: Some(meta.clone()),
            images: None,
        });
        // Subsequent partial update without locations/meta — must not clobber.
        s.apply_event(&AcpEvent::ToolCallUpdate {
            tool_call_id: "tc-1".into(),
            title: None,
            status: Some("completed".into()),
            content: None,
            raw_input: None,
            raw_output: Some("\"ok\"".into()),
            raw_output_append: None,
            locations: None,
            meta: None,
            images: None,
        });
        let entry = s.active_tool_calls.get("tc-1").unwrap();
        assert_eq!(entry.status, ToolCallStatus::Completed);
        assert_eq!(
            entry.locations.as_ref(),
            Some(&locs),
            "ToolCallUpdate without locations must NOT clobber previously-set value"
        );
        assert_eq!(
            entry.meta.as_ref(),
            Some(&meta),
            "ToolCallUpdate without meta must NOT clobber previously-set value"
        );
    }

    #[test]
    fn tool_call_images_replace_or_preserve_on_update() {
        let mut s = fresh_state();
        let img_v1 = ToolCallImageInfo {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
            uri: Some("/tmp/v1.png".into()),
        };
        let img_v2 = ToolCallImageInfo {
            data: "BBBB".into(),
            mime_type: "image/jpeg".into(),
            uri: None,
        };

        // Initial ToolCall carries one image — should be persisted.
        s.apply_event(&AcpEvent::ToolCall {
            tool_call_id: "ig-1".into(),
            title: "Image generation".into(),
            kind: "other".into(),
            status: "in_progress".into(),
            content: None,
            raw_input: None,
            raw_output: None,
            locations: None,
            meta: None,
            images: Some(vec![img_v1.clone()]),
        });
        let entry = s.active_tool_calls.get("ig-1").unwrap();
        assert_eq!(entry.images.len(), 1);
        assert_eq!(entry.images[0].data, "AAAA");

        // Update without images field — must preserve prior images.
        s.apply_event(&AcpEvent::ToolCallUpdate {
            tool_call_id: "ig-1".into(),
            title: None,
            status: Some("in_progress".into()),
            content: None,
            raw_input: None,
            raw_output: None,
            raw_output_append: None,
            locations: None,
            meta: None,
            images: None,
        });
        let entry = s.active_tool_calls.get("ig-1").unwrap();
        assert_eq!(
            entry.images.len(),
            1,
            "ToolCallUpdate with images=None must preserve prior images"
        );
        assert_eq!(entry.images[0].data, "AAAA");

        // Update with Some(new_vec) — must replace.
        s.apply_event(&AcpEvent::ToolCallUpdate {
            tool_call_id: "ig-1".into(),
            title: None,
            status: Some("completed".into()),
            content: None,
            raw_input: None,
            raw_output: None,
            raw_output_append: None,
            locations: None,
            meta: None,
            images: Some(vec![img_v2.clone()]),
        });
        let entry = s.active_tool_calls.get("ig-1").unwrap();
        assert_eq!(entry.images.len(), 1, "Some(vec) replaces prior images");
        assert_eq!(entry.images[0].data, "BBBB");
        assert_eq!(entry.images[0].mime_type, "image/jpeg");
        assert!(entry.images[0].uri.is_none());

        // Snapshot round-trip preserves images.
        let snap = s.to_snapshot();
        let tc = snap
            .active_tool_calls
            .iter()
            .find(|t| t.id == "ig-1")
            .unwrap();
        assert_eq!(tc.images.len(), 1);
        assert_eq!(tc.images[0].data, "BBBB");

        // Update with Some(empty) — must clear images (allows the agent to
        // explicitly drop a prior image if needed).
        s.apply_event(&AcpEvent::ToolCallUpdate {
            tool_call_id: "ig-1".into(),
            title: None,
            status: None,
            content: None,
            raw_input: None,
            raw_output: None,
            raw_output_append: None,
            locations: None,
            meta: None,
            images: Some(vec![]),
        });
        let entry = s.active_tool_calls.get("ig-1").unwrap();
        assert!(
            entry.images.is_empty(),
            "Some(empty vec) clears prior images"
        );
    }

    #[test]
    fn plan_update_appends_at_end_replacing_existing() {
        use crate::acp::types::PlanEntryInfo;
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ContentDelta { text: "A".into() });
        s.apply_event(&AcpEvent::PlanUpdate {
            entries: vec![PlanEntryInfo {
                content: "step v1".into(),
                priority: "high".into(),
                status: "pending".into(),
            }],
        });
        s.apply_event(&AcpEvent::ContentDelta { text: "B".into() });
        s.apply_event(&AcpEvent::PlanUpdate {
            entries: vec![PlanEntryInfo {
                content: "step v2".into(),
                priority: "high".into(),
                status: "in_progress".into(),
            }],
        });

        let summary = live_block_summary(&s);
        // Expect: text("A"), text("B"), plan(v2). The old plan block is
        // removed and the fresh one is appended at end (after all current
        // text), matching the frontend reducer's replace-then-append.
        assert_eq!(summary.len(), 3, "summary was: {:?}", summary);
        assert_eq!(summary[0], ("text", "A".to_string()));
        assert_eq!(summary[1], ("text", "B".to_string()));
        assert_eq!(summary[2].0, "plan");
        assert!(
            summary[2].1.contains("step v2"),
            "plan block must be the v2 entries, not v1; got: {}",
            summary[2].1
        );
        assert!(
            !summary[2].1.contains("step v1"),
            "old plan block must be removed; got: {}",
            summary[2].1
        );
    }

    #[test]
    fn plan_update_creates_live_message_when_absent() {
        use crate::acp::types::PlanEntryInfo;
        let mut s = fresh_state();
        assert!(s.live_message.is_none());
        s.apply_event(&AcpEvent::PlanUpdate {
            entries: vec![PlanEntryInfo {
                content: "first step".into(),
                priority: "medium".into(),
                status: "pending".into(),
            }],
        });
        let live = s
            .live_message
            .as_ref()
            .expect("PlanUpdate must lazily create live_message");
        assert_eq!(live.content.len(), 1);
        match &live.content[0] {
            LiveContentBlock::Plan { entries } => {
                assert!(
                    entries.to_string().contains("first step"),
                    "plan must carry the entries payload; got: {}",
                    entries
                );
            }
            other => panic!("expected Plan block, got {:?}", other),
        }
    }

    #[test]
    fn turn_complete_clears_plan_and_tool_refs() {
        use crate::acp::types::PlanEntryInfo;
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::ContentDelta { text: "x".into() });
        s.apply_event(&tool_call_event("tc-1", "ls"));
        s.apply_event(&AcpEvent::PlanUpdate {
            entries: vec![PlanEntryInfo {
                content: "step".into(),
                priority: "low".into(),
                status: "pending".into(),
            }],
        });
        // Sanity precondition: live now has text, ref, plan.
        assert_eq!(live_block_summary(&s).len(), 3);
        assert_eq!(s.active_tool_calls.len(), 1);

        s.apply_event(&AcpEvent::TurnComplete {
            session_id: "ext".into(),
            stop_reason: "end_turn".into(),
            agent_type: "claude_code".into(),
        });
        // The existing `live_message = None` clear handles the new block kinds
        // automatically — they live inside live_message, not as siblings.
        assert!(s.live_message.is_none());
        assert!(s.active_tool_calls.is_empty());
    }

    /// 验证 envelope 序列化 + 反序列化 round-trip
    #[test]
    fn event_envelope_round_trips_through_json() {
        let env = EventEnvelope {
            seq: 7,
            connection_id: "conn-x".into(),
            payload: AcpEvent::ContentDelta { text: "abc".into() },
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: EventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.seq, 7);
        assert_eq!(back.connection_id, "conn-x");
        match back.payload {
            AcpEvent::ContentDelta { text } => assert_eq!(text, "abc"),
            _ => panic!("expected ContentDelta"),
        }
    }

    // --- live feedback: apply_event + snapshot --------------------------

    fn feedback_note(id: &str, text: &str) -> FeedbackItem {
        FeedbackItem::new_pending(id.into(), text.into(), Utc::now())
    }

    #[test]
    fn feedback_submitted_appends_idempotently() {
        let mut s = fresh_state();
        let item = feedback_note("f1", "use UserService");
        s.apply_event(&AcpEvent::FeedbackSubmitted { item: item.clone() });
        assert_eq!(s.feedback.len(), 1);
        // Replay / double-attach: a second apply with the same id is a no-op.
        s.apply_event(&AcpEvent::FeedbackSubmitted { item });
        assert_eq!(s.feedback.len(), 1, "duplicate id must not append twice");
        assert_eq!(s.feedback[0].status, FeedbackStatus::Pending);
        // A different id appends.
        s.apply_event(&AcpEvent::FeedbackSubmitted {
            item: feedback_note("f2", "skip the migration"),
        });
        assert_eq!(s.feedback.len(), 2);
    }

    #[test]
    fn feedback_consumed_marks_named_notes_delivered() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::FeedbackSubmitted {
            item: feedback_note("f1", "a"),
        });
        s.apply_event(&AcpEvent::FeedbackSubmitted {
            item: feedback_note("f2", "b"),
        });
        let at = Utc::now();
        s.apply_event(&AcpEvent::FeedbackConsumed {
            ids: vec!["f1".into()],
            delivered_at: at,
        });
        let f1 = s.feedback.iter().find(|f| f.id == "f1").unwrap();
        let f2 = s.feedback.iter().find(|f| f.id == "f2").unwrap();
        assert_eq!(f1.status, FeedbackStatus::Delivered);
        assert_eq!(f1.delivered_at, Some(at));
        assert_eq!(f2.status, FeedbackStatus::Pending, "unnamed note untouched");
        // Idempotent: re-applying the same consumption leaves f1 delivered and
        // does not flip its delivered_at to a new instant.
        s.apply_event(&AcpEvent::FeedbackConsumed {
            ids: vec!["f1".into()],
            delivered_at: Utc::now(),
        });
        let f1 = s.feedback.iter().find(|f| f.id == "f1").unwrap();
        assert_eq!(f1.delivered_at, Some(at), "delivered_at must not change");
    }

    #[test]
    fn user_message_clears_feedback_for_new_turn() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::FeedbackSubmitted {
            item: feedback_note("f1", "a"),
        });
        assert_eq!(s.feedback.len(), 1);
        // A new turn's user prompt resets the turn-scoped feedback set.
        s.apply_event(&text_user_message("user-1", "next prompt"));
        assert!(
            s.feedback.is_empty(),
            "feedback is turn-scoped; a new user_message clears it"
        );
    }

    #[test]
    fn snapshot_carries_feedback_and_omits_when_empty() {
        let mut s = fresh_state();
        s.apply_event(&AcpEvent::FeedbackSubmitted {
            item: feedback_note("f1", "snapshot me"),
        });
        let snap = s.to_snapshot();
        assert_eq!(snap.feedback.len(), 1);
        assert_eq!(snap.feedback[0].id, "f1");
        // Round-trips through the wire shape the web client hydrates from.
        let json = serde_json::to_string(&snap).unwrap();
        let back: LiveSessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.feedback.len(), 1);
        // The empty case keeps the NOTES array off the wire (the always-present
        // `feedback_tool_available` bool is a separate field).
        let empty = serde_json::to_string(&fresh_state().to_snapshot()).unwrap();
        assert!(
            !empty.contains("\"feedback\":"),
            "no-feedback snapshot must omit the notes array"
        );
    }
}
