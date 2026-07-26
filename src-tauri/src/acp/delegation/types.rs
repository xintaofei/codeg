//! Broker-facing request / outcome types.
//!
//! These cross two boundaries:
//! 1. The MCP companion serializes `DelegationRequest` → JSON-RPC params and
//!    deserializes `DelegationOutcome` → MCP `tool_result`.
//! 2. The broker emits a structured outcome the listener can persist and
//!    forward to the parent's tool_use_id.
//!
//! DB ids are `i32` to match the actual `conversation.id` / `conversation.parent_id`
//! column types — keeping them strongly typed here saves us a parse-or-die step
//! at every DB boundary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::models::AgentType;

/// Per-agent defaults applied when codeg-mcp spawns a subagent on behalf of a
/// `delegate_to_agent` call. Mirrors the two knobs `ConnectionManager::spawn_agent`
/// already accepts:
///   * `mode_id` → forwarded as `preferred_mode_id`
///   * `config_values` → forwarded as `preferred_config_values`
///
/// All fields are optional / may be empty; an absent entry means "no override —
/// use whatever the agent advertises as the default."
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDelegationDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config_values: BTreeMap<String, String>,
}

impl AgentDelegationDefaults {
    pub fn is_empty(&self) -> bool {
        self.mode_id.is_none() && self.config_values.is_empty()
    }
}

/// Everything the broker needs to dispatch a single delegation call.
///
/// `parent_connection_id` is the codeg-internal ACP connection UUID for the
/// parent session (NOT the agent-assigned ACP session id). The broker uses it
/// to inherit the parent's EventEmitter/working_dir and to scope
/// `cancel_by_parent`.
///
/// `external_handle` is a companion-minted opaque token (per MCP `tools/call`)
/// that the broker stores alongside the pending entry so an MCP-side
/// `notifications/cancelled` can target this specific delegation without the
/// companion having to know the broker-internal `call_id`. `None` for non-MCP
/// callers and tests that don't exercise the cancel path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationRequest {
    pub parent_connection_id: String,
    pub parent_conversation_id: i32,
    pub parent_tool_use_id: String,
    pub agent_type: AgentType,
    pub task: String,
    pub working_dir: Option<String>,
    /// The `working_dir` exactly as the LLM passed it in the
    /// `delegate_to_agent` arguments, BEFORE the listener defaults a missing
    /// value to the parent's launch directory. Used only as part of the
    /// `(agent_type, task, requested_working_dir)` correlation key so two
    /// parallel calls sharing an agent and task but targeting different
    /// explicit directories don't bind to each other's `tool_call_id`.
    /// `None` when the LLM omitted it — symmetric with the ACP `raw_input`,
    /// which also omits it then. Distinct from `working_dir` above, which is
    /// the defaulted value the child is actually spawned in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_handle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationSuccess {
    pub text: String,
    pub child_conversation_id: i32,
    pub child_agent_type: AgentType,
    pub turn_count: u32,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
}

/// Broker-internal failure modes. Serialized via the wrapping
/// [`DelegationOutcome::Err`] variant — the broker maps each into a stable
/// `code` string so the frontend / MCP consumer can pattern-match without
/// caring about the inner shape.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "code", content = "detail", rename_all = "snake_case")]
pub enum DelegationError {
    #[error("depth limit exceeded ({current_depth} >= {limit})")]
    DepthLimitExceeded { current_depth: u32, limit: u32 },
    #[error("invalid agent type")]
    InvalidAgentType,
    #[error("invalid working dir: {0}")]
    InvalidWorkingDir(String),
    #[error("spawn failed: {0}")]
    SpawnFailed(String),
    #[error("subagent runtime error: {0}")]
    SubagentRuntimeError(String),
    /// Child agent ended its turn via `refusal`. Often a backend / gateway
    /// error masquerading as a refusal per the ACP spec gap.
    #[error("subagent refused to continue")]
    ChildRefusal,
    #[error("subagent reached max token budget")]
    ChildMaxTokens,
    #[error("subagent reached max turn request budget")]
    ChildMaxTurnRequests,
    /// Child reported `end_turn` without producing any output (synthesized
    /// as `empty` by the connection loop's "silent EndTurn" guard).
    #[error("subagent produced no output")]
    ChildEmpty,
    #[error("subagent ended with unrecognized stop reason: {0}")]
    ChildUnknown(String),
    #[error("canceled: {reason}")]
    Canceled { reason: String },
    #[error("parent session is gone")]
    ParentSessionGone,
    /// `continue_with_session` targeted a task whose current turn is still
    /// running (Requirement 2.4).
    #[error("subagent session is still running; wait for it or cancel first")]
    SessionStillRunning,
    /// The session was RELEASED by `close_session` / parent-conversation
    /// deletion (R2-B4 release semantics — not a permanent close; a process
    /// restart naturally expires the lease). Upstream PR #375 calls this
    /// `session_closed`; see [`canonical_continuation_code`].
    #[error("subagent session was released; start a new delegation for further work")]
    SessionReleased,
    /// The task exists but cannot be continued: empty message, missing
    /// resume credential after process death, binding mismatch, etc.
    /// (Requirements 3.2, 7.6).
    #[error("subagent session cannot be continued: {0}")]
    NotContinuable(String),
    /// The resume chain cannot restore the prior context. Returned BEFORE any
    /// prompt side effect — never silently degraded to a context-losing
    /// `session/new` (Requirements 3.3, 7.7).
    #[error("resume unavailable: {0}")]
    ResumeUnavailable(String),
    /// The same `continuation_id` was reused with a different payload
    /// (Requirement 2.13 — idempotency keys bind to one exact request).
    #[error("continuation_id was already used with a different payload")]
    ContinuationConflict,
    /// Retryable startup state: the broker is still rebuilding its session
    /// index (Requirement 7.2) — answering `Unknown` would read as "your
    /// session is lost", so this distinct code tells callers to retry.
    #[error("the broker is rebuilding its session index after startup; retry shortly")]
    Rebuilding,
}

/// Historical alias accepted on the facade (R2-B4): upstream PR #375 named the
/// released state `session_closed`; this build reports `session_released`.
/// Anything comparing / routing on continuation error codes should normalize
/// through [`canonical_continuation_code`] instead of matching the alias.
pub const SESSION_CLOSED_LEGACY_ALIAS: &str = "session_closed";

/// Map a wire error code to its canonical form: the upstream `session_closed`
/// alias folds into `session_released`; every other code passes through.
pub fn canonical_continuation_code(code: &str) -> &str {
    if code == SESSION_CLOSED_LEGACY_ALIAS {
        "session_released"
    } else {
        code
    }
}

/// The single value the broker hands back to the listener / MCP companion.
/// `child_conversation_id` on the `Err` arm is best-effort — it's `Some` once
/// the broker successfully created the child DB row, even if the run later
/// fails or times out.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DelegationOutcome {
    Ok(DelegationSuccess),
    Err {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        child_conversation_id: Option<i32>,
    },
}

/// Lifecycle status of an asynchronous delegation task. Surfaced by the
/// three delegation tools — `delegate_to_agent` (returns a `Running` ack, or
/// a terminal status when the child finished during setup / setup failed),
/// `get_delegation_status`, and `cancel_delegation`. Wire-stable snake_case
/// strings: they ship to LLM context and to the frontend, so don't rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Child is running in the background; no terminal result yet.
    Running,
    /// Child ended its turn cleanly; `text` carries the result (possibly
    /// truncated — open the child session for the full output).
    Completed,
    /// Child ended in a non-cancel failure; `error_code` / `message` describe it.
    Failed,
    /// Task was canceled (by `cancel_delegation`, parent teardown, or a
    /// non-`end_turn` parent turn end).
    Canceled,
    /// Task id is not known to this parent — never existed, belonged to a
    /// different parent, or its result was evicted from the cache and no DB
    /// row backs it.
    Unknown,
}

/// Unified response the broker hands the listener for every delegation tool
/// (`delegate_to_agent` / `get_delegation_status` / `cancel_delegation`). The
/// listener serializes it into `BrokerResponse.outcome`; the companion renders
/// it into the MCP `CallToolResult` (with `structuredContent` carrying this
/// whole shape so the frontend can read `status` and distinguish a running ack
/// from a terminal outcome).
///
/// Fields are all optional except `status` so one type can describe a running
/// ack (ids + `Running`), a completed result (`text` + `duration_ms`), a
/// failure (`error_code` + `message`), and a setup failure (`task_id: None`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationTaskReport {
    /// Broker `call_id` (UUID) identifying the task. `None` only when setup
    /// failed before a task was registered (no id to track).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_conversation_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
    /// Completed result text (capped; open the child session for the full
    /// output). Only set for `Completed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Wire-stable error code for `Failed` / `Canceled` (mirrors
    /// `DelegationOutcome::Err.code`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Human-readable note: the failure message, or a hint like
    /// "running in background" / "result not cached; open child session N".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl DelegationTaskReport {
    /// Attach a known `task_id` onto a setup-style error report that was built
    /// without one (e.g. continue/close failures after the id is already
    /// known). Never clobbers an id the report already carries.
    pub fn with_task_id(mut self, task_id: &str) -> Self {
        if self.task_id.is_none() {
            self.task_id = Some(task_id.to_string());
        }
        self
    }
}

impl DelegationOutcome {
    /// Project a `DelegationError` onto the wire-stable `code` string used by
    /// the frontend and MCP companion. Keep these strings stable — they ship
    /// to LLM context.
    pub fn from_err(err: DelegationError, child_conversation_id: Option<i32>) -> Self {
        let code = match &err {
            DelegationError::DepthLimitExceeded { .. } => "depth_limit",
            DelegationError::InvalidAgentType => "invalid_agent_type",
            DelegationError::InvalidWorkingDir(_) => "invalid_working_dir",
            DelegationError::SpawnFailed(_) => "spawn_failed",
            DelegationError::SubagentRuntimeError(_) => "subagent_error",
            DelegationError::ChildRefusal => "child_refusal",
            DelegationError::ChildMaxTokens => "child_max_tokens",
            DelegationError::ChildMaxTurnRequests => "child_max_turn_requests",
            DelegationError::ChildEmpty => "child_empty",
            DelegationError::ChildUnknown(_) => "child_unknown",
            DelegationError::Canceled { .. } => "canceled",
            DelegationError::ParentSessionGone => "canceled",
            DelegationError::SessionStillRunning => "session_still_running",
            DelegationError::SessionReleased => "session_released",
            DelegationError::NotContinuable(_) => "not_continuable",
            DelegationError::ResumeUnavailable(_) => "resume_unavailable",
            DelegationError::ContinuationConflict => "continuation_conflict",
            DelegationError::Rebuilding => "rebuilding",
        };
        DelegationOutcome::Err {
            code: code.to_string(),
            message: err.to_string(),
            child_conversation_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T4.1 red: the five continuation error codes (plus `rebuilding`) must be
    /// typed `DelegationError` variants with wire-stable `from_err` codes —
    /// not broker-local string constants.
    #[test]
    fn from_err_maps_continuation_variants_to_wire_codes() {
        let cases: Vec<(DelegationError, &str)> = vec![
            (
                DelegationError::SessionStillRunning,
                "session_still_running",
            ),
            (DelegationError::SessionReleased, "session_released"),
            (
                DelegationError::NotContinuable("x".into()),
                "not_continuable",
            ),
            (
                DelegationError::ResumeUnavailable("y".into()),
                "resume_unavailable",
            ),
            (
                DelegationError::ContinuationConflict,
                "continuation_conflict",
            ),
            (DelegationError::Rebuilding, "rebuilding"),
        ];
        for (err, want) in cases {
            match DelegationOutcome::from_err(err, None) {
                DelegationOutcome::Err { code, .. } => assert_eq!(code, want),
                other => panic!("expected Err outcome, got {other:?}"),
            }
        }
    }

    /// R2-B4: the facade accepts upstream PR #375's `session_closed` as a
    /// historical alias for `session_released`; every other code is passed
    /// through canonical unchanged.
    #[test]
    fn session_closed_is_accepted_as_alias_for_session_released() {
        assert_eq!(
            canonical_continuation_code("session_closed"),
            "session_released"
        );
        assert_eq!(
            canonical_continuation_code("session_released"),
            "session_released"
        );
        assert_eq!(
            canonical_continuation_code("not_continuable"),
            "not_continuable"
        );
    }

    /// `with_task_id` attaches a known id onto a setup-style report built
    /// without one, and never clobbers an existing id.
    #[test]
    fn with_task_id_fills_only_missing_ids() {
        let report = DelegationTaskReport {
            task_id: None,
            status: TaskStatus::Failed,
            child_conversation_id: None,
            agent_type: None,
            text: None,
            error_code: Some("not_continuable".into()),
            message: None,
            duration_ms: None,
        };
        let filled = report.with_task_id("t-1");
        assert_eq!(filled.task_id.as_deref(), Some("t-1"));
        let kept = filled.with_task_id("t-2");
        assert_eq!(kept.task_id.as_deref(), Some("t-1"));
    }

    /// The detail payloads ride the Display string so per-site context
    /// survives the typed funnel (broker call sites embed the reason).
    #[test]
    fn detail_variants_carry_reason_in_message() {
        let out = DelegationOutcome::from_err(
            DelegationError::NotContinuable("message is empty".into()),
            None,
        );
        match out {
            DelegationOutcome::Err { message, .. } => {
                assert!(message.contains("message is empty"), "got: {message}");
            }
            other => panic!("expected Err outcome, got {other:?}"),
        }
    }
}
