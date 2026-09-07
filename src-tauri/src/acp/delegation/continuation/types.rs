//! Domain / protocol types for the strict recovery boundary.
//!
//! `SessionRecoveryPolicy` names the two recovery strategies; the
//! [`StrictAttachGate`] carries the `RequireExisting` verdict channel through
//! the spawn path so `run_connection` can report a REAL readiness (or a typed
//! failure) instead of the implicit "a connection id exists" signal.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::oneshot;

/// Which recovery behavior a connection launch uses.
///
/// * `AllowNewFallback` — the ordinary spawn chain (resume → load → **new**):
///   a failed recovery of a session the caller merely suggested may legally
///   produce a brand-new agent session. This is the production default for
///   every pre-existing entry point and must stay untouched.
/// * `RequireExisting` — the strict boundary: only `session/resume` /
///   `session/load` against the caller-supplied external session id may
///   establish the session; a failure is a typed error and **never** falls
///   through to `session/new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecoveryPolicy {
    AllowNewFallback,
    RequireExisting,
}

/// The launch-time recovery strategy handed to `run_connection`: the policy
/// plus, for the strict one, the gate that receives the readiness verdict.
#[derive(Debug)]
pub enum SessionRecovery {
    AllowNewFallback,
    RequireExisting(StrictAttachGate),
}

impl SessionRecovery {
    /// Whether this launch is allowed to mint a new agent session when
    /// recovery fails. Every `session/new` call site must consult this BEFORE
    /// sending the request — that ordering is the whole strict guarantee.
    pub fn allows_new_fallback(&self) -> bool {
        matches!(self, SessionRecovery::AllowNewFallback)
    }

    pub(crate) fn continuation_identity(&self) -> Option<&ContinuationConnectionIdentity> {
        match self {
            SessionRecovery::AllowNewFallback => None,
            SessionRecovery::RequireExisting(gate) => gate.continuation_identity.as_ref(),
        }
    }
}

/// Trusted, backend-only ownership copied into a strict connection at the
/// same instant it becomes visible through the manager. This closes the gap
/// before the coordinator can move the round into its execution registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContinuationConnectionIdentity {
    pub turn_id: String,
    pub execution_id: String,
}

/// Typed strict-recovery failures. Any of these means NO usable session was
/// established and nothing may be sent over the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrictAttachErrorCode {
    /// The recorded binding (cwd / execution-config identity) does not match
    /// what this launch would actually run, or is not reliably verifiable.
    BindingMismatch,
    /// The agent (or its recorded capabilities) supports neither
    /// `session/resume` nor `session/load` — there is no existing session to
    /// attach to.
    ResumeUnsupported,
    /// Both recovery methods were tried and failed, or a required setup step
    /// (config application) failed after recovery.
    ResumeFailed,
    /// The recovery handshake never completed within the allowed time.
    ResumeTimeout,
}

/// A successful strict attach: the connection is bound to the requested
/// external session, recovery + replay drain + config application all
/// succeeded, and the connection is ready for the caller's FIRST prompt.
/// Returned readiness is the ONLY signal a strict caller may act on —
/// `Connected` / `SessionStarted` events fire earlier and prove nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrictReady {
    pub connection_id: String,
    pub external_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code:?}: {message}")]
pub struct StrictAttachError {
    pub code: StrictAttachErrorCode,
    pub message: String,
}

impl StrictAttachError {
    pub fn new(code: StrictAttachErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// One-shot verdict channel for a strict launch. The sender half lives with
/// the connection; the receiver is what the strict caller awaits (bounded by
/// its own timeout → [`StrictAttachErrorCode::ResumeTimeout`]).
///
/// Both [`Self::ready`] and [`Self::fail`] are idempotent: whichever verdict
/// lands first wins and later calls are logged no-ops, mirroring the
/// first-terminal-wins rule the rest of the delegation machinery follows.
#[derive(Clone)]
pub struct StrictAttachGate {
    tx: Arc<tokio::sync::Mutex<Option<oneshot::Sender<StrictOutcome>>>>,
    policy: SessionRecoveryPolicy,
    /// The canonical cwd the outcome's binding recorded. A launch resolving to
    /// any other directory is refused BEFORE the agent process starts.
    expected_cwd: PathBuf,
    /// The execution-config identity the outcome's binding recorded (the
    /// canonical config fingerprint captured at the original success). A
    /// launch whose freshly computed fingerprint differs is refused — codeg
    /// must not silently move a continued session onto a changed environment.
    expected_config_fingerprint: String,
    continuation_identity: Option<ContinuationConnectionIdentity>,
}

/// What the gate's receiver resolves to.
#[derive(Debug)]
pub enum StrictOutcome {
    Ready(StrictReady),
    Failed(StrictAttachError),
}

impl std::fmt::Debug for StrictAttachGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrictAttachGate")
            .field("policy", &self.policy)
            .field("expected_cwd", &self.expected_cwd)
            // The fingerprint is a non-sensitive identity hash, but it is
            // still a config identifier — show only its presence/length.
            .field(
                "expected_config_fingerprint",
                &self.expected_config_fingerprint.len(),
            )
            .finish()
    }
}

impl StrictAttachGate {
    /// Create the gate + receiver pair. `expected_cwd` /
    /// `expected_config_fingerprint` come from the frozen outcome's resume
    /// binding and are verified against the actual launch before the agent
    /// process starts.
    pub fn channel(
        expected_cwd: PathBuf,
        expected_config_fingerprint: String,
    ) -> (Self, oneshot::Receiver<StrictOutcome>) {
        let (tx, rx) = oneshot::channel();
        (
            Self {
                tx: Arc::new(tokio::sync::Mutex::new(Some(tx))),
                policy: SessionRecoveryPolicy::RequireExisting,
                expected_cwd,
                expected_config_fingerprint,
                continuation_identity: None,
            },
            rx,
        )
    }

    pub(crate) fn channel_for_continuation(
        expected_cwd: PathBuf,
        expected_config_fingerprint: String,
        turn_id: String,
        execution_id: String,
    ) -> (Self, oneshot::Receiver<StrictOutcome>) {
        let (mut gate, rx) = Self::channel(expected_cwd, expected_config_fingerprint);
        gate.continuation_identity = Some(ContinuationConnectionIdentity {
            turn_id,
            execution_id,
        });
        (gate, rx)
    }

    /// Verify the launch parameters against the recorded binding. Runs in
    /// `spawn_agent_connection` BEFORE the agent process starts, so a binding
    /// mismatch never even boots a binary — and therefore can never fall back
    /// to anything.
    pub fn verify_launch(
        &self,
        launch_cwd: &std::path::Path,
        config_fingerprint: &str,
    ) -> Result<(), StrictAttachError> {
        if !self.expected_cwd.as_path().eq(launch_cwd) {
            return Err(StrictAttachError::new(
                StrictAttachErrorCode::BindingMismatch,
                format!(
                    "recorded cwd {:?} does not match the launch cwd {:?}; refusing to \
                     continue the session elsewhere",
                    self.expected_cwd, launch_cwd
                ),
            ));
        }
        if self.expected_config_fingerprint != config_fingerprint {
            return Err(StrictAttachError::new(
                StrictAttachErrorCode::BindingMismatch,
                "the execution configuration (environment / model provider config) \
                 changed since the recorded success; refusing to continue the session \
                 under a different configuration"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Deliver the Ready verdict (after recovery + drain + config all truly
    /// succeeded). A no-op when a verdict already landed.
    pub async fn ready(&self, ready: StrictReady) {
        let mut slot = self.tx.lock().await;
        if let Some(tx) = slot.take() {
            let _ = tx.send(StrictOutcome::Ready(ready));
        } else {
            tracing::warn!("[strict-attach] ready verdict arrived after resolution; ignored");
        }
    }

    /// Deliver a failure verdict. A no-op when a verdict already landed.
    pub async fn fail(&self, err: StrictAttachError) {
        let mut slot = self.tx.lock().await;
        if let Some(tx) = slot.take() {
            let _ = tx.send(StrictOutcome::Failed(err));
        } else {
            tracing::warn!("[strict-attach] failure verdict arrived after resolution; ignored");
        }
    }
}

// ---------------------------------------------------------------------------
// Wire contract (v2 design §6) — schema_version = 1
// ---------------------------------------------------------------------------

pub const CONTINUATION_SCHEMA_VERSION: i32 = 1;

/// The rework message bound stored on a turn / returned by a send. The SAME
/// bounded-text policy as the delegation outcome store applies to turn
/// results.
pub const TURN_RESULT_TEXT_CAP: usize = 256 * 1024;

/// Lifecycle states of a collaboration turn. Wire-stable snake_case; the
/// five ACTIVE states must stay in sync with
/// `collaboration_service::ACTIVE_TURN_STATES` (and the partial unique index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Accepted,
    Preparing,
    Dispatching,
    Running,
    CancelRequested,
    Completed,
    Failed,
    Canceled,
    Interrupted,
    OutcomeUnknown,
}

impl TurnState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TurnState::Accepted => "accepted",
            TurnState::Preparing => "preparing",
            TurnState::Dispatching => "dispatching",
            TurnState::Running => "running",
            TurnState::CancelRequested => "cancel_requested",
            TurnState::Completed => "completed",
            TurnState::Failed => "failed",
            TurnState::Canceled => "canceled",
            TurnState::Interrupted => "interrupted",
            TurnState::OutcomeUnknown => "outcome_unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "accepted" => TurnState::Accepted,
            "preparing" => TurnState::Preparing,
            "dispatching" => TurnState::Dispatching,
            "running" => TurnState::Running,
            "cancel_requested" => TurnState::CancelRequested,
            "completed" => TurnState::Completed,
            "failed" => TurnState::Failed,
            "canceled" => TurnState::Canceled,
            "interrupted" => TurnState::Interrupted,
            "outcome_unknown" => TurnState::OutcomeUnknown,
            _ => return None,
        })
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            TurnState::Accepted
                | TurnState::Preparing
                | TurnState::Dispatching
                | TurnState::Running
                | TurnState::CancelRequested
        )
    }
}

/// Session states: `open` (normal), `blocked` (an outcome is unknown — the
/// host cannot prove what the agent did; no new turns), `closed` (explicitly
/// ended; ordinary chat resumes, the source is never continuable again).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationSessionState {
    Open,
    Blocked,
    Closed,
}

impl CollaborationSessionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CollaborationSessionState::Open => "open",
            CollaborationSessionState::Blocked => "blocked",
            CollaborationSessionState::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "open" => CollaborationSessionState::Open,
            "blocked" => CollaborationSessionState::Blocked,
            "closed" => CollaborationSessionState::Closed,
            _ => return None,
        })
    }
}

/// Stable error codes for the continuation tools. Every rejection maps to one
/// of these — raw SQL errors and internal details never leak to the tool
/// surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationErrorCode {
    FeatureDisabled,
    InvalidRequest,
    SourceNotResumable,
    SourceMissing,
    SourceBusy,
    NotFoundOrForbidden,
    SessionBusy,
    SessionBlocked,
    SessionClosed,
    RequestConflict,
    BindingMismatch,
    ResumeUnsupported,
    ResumeFailed,
    ResumeTimeout,
    StorageUnavailable,
    SessionReservedForDelegation,
}

impl ContinuationErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContinuationErrorCode::FeatureDisabled => "feature_disabled",
            ContinuationErrorCode::InvalidRequest => "invalid_request",
            ContinuationErrorCode::SourceNotResumable => "source_not_resumable",
            ContinuationErrorCode::SourceMissing => "source_missing",
            ContinuationErrorCode::SourceBusy => "source_busy",
            ContinuationErrorCode::NotFoundOrForbidden => "not_found_or_forbidden",
            ContinuationErrorCode::SessionBusy => "session_busy",
            ContinuationErrorCode::SessionBlocked => "session_blocked",
            ContinuationErrorCode::SessionClosed => "session_closed",
            ContinuationErrorCode::RequestConflict => "request_conflict",
            ContinuationErrorCode::BindingMismatch => "binding_mismatch",
            ContinuationErrorCode::ResumeUnsupported => "resume_unsupported",
            ContinuationErrorCode::ResumeFailed => "resume_failed",
            ContinuationErrorCode::ResumeTimeout => "resume_timeout",
            ContinuationErrorCode::StorageUnavailable => "storage_unavailable",
            ContinuationErrorCode::SessionReservedForDelegation => {
                "session_reserved_for_delegation"
            }
        }
    }
}

/// The typed rejection every continuation entry returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationError {
    pub schema_version: SchemaVersion1,
    pub error_code: ContinuationErrorCode,
    pub message: String,
    /// Filled only when the rejection is not an existence leak: a caller that
    /// must not learn whether the target exists gets `null` for both.
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
}

impl ContinuationError {
    pub fn new(code: ContinuationErrorCode, message: impl Into<String>) -> Self {
        Self {
            schema_version: SchemaVersion1,
            error_code: code,
            message: message.into(),
            session_id: None,
            turn_id: None,
        }
    }

    pub fn with_ids(mut self, session_id: Option<&str>, turn_id: Option<&str>) -> Self {
        self.session_id = session_id.map(str::to_string);
        self.turn_id = turn_id.map(str::to_string);
        self
    }
}

impl std::fmt::Display for ContinuationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_code.as_str(), self.message)
    }
}

impl From<StrictAttachError> for ContinuationError {
    fn from(e: StrictAttachError) -> Self {
        use StrictAttachErrorCode as S;
        let code = match e.code {
            S::BindingMismatch => ContinuationErrorCode::BindingMismatch,
            S::ResumeUnsupported => ContinuationErrorCode::ResumeUnsupported,
            S::ResumeFailed => ContinuationErrorCode::ResumeFailed,
            S::ResumeTimeout => ContinuationErrorCode::ResumeTimeout,
        };
        ContinuationError::new(code, e.message)
    }
}

/// A single schema-version field with a closed accepted value: the contract
/// tests (Rust AND TypeScript, sharing the same fixtures) reject any version
/// the parser does not know, instead of guessing at new shapes. Serializes
/// as the bare number `1` — matching the frozen fixtures byte-for-byte.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchemaVersion1;

impl SchemaVersion1 {
    pub fn value(&self) -> i32 {
        CONTINUATION_SCHEMA_VERSION
    }
}

impl Serialize for SchemaVersion1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i32(CONTINUATION_SCHEMA_VERSION)
    }
}

impl<'de> Deserialize<'de> for SchemaVersion1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = i32::deserialize(deserializer)?;
        if v == CONTINUATION_SCHEMA_VERSION {
            Ok(SchemaVersion1)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported schema_version {v}; this build understands                  {CONTINUATION_SCHEMA_VERSION}"
            )))
        }
    }
}

/// Acceptance receipt for `continue_with_session`: the turn exists and is
/// tracked, but nothing about it is complete yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnAck {
    pub schema_version: SchemaVersion1,
    pub session_id: String,
    pub turn_id: String,
    pub source_task_id: String,
    pub ordinal: i32,
    pub state: TurnState,
}

/// The full report for one specific turn. Nullable fields are ALWAYS present
/// (serialized as `null`) so consumers can rely on the shape; `blocked_on` is
/// a live snapshot (cleared on restart), not a resumable authorization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnReport {
    pub schema_version: SchemaVersion1,
    pub session_id: String,
    pub turn_id: String,
    pub source_task_id: String,
    pub ordinal: i32,
    pub state: TurnState,
    pub message: String,
    pub initiator_kind: String,
    pub initiator_parent_conversation_id: i32,
    pub result_text: Option<String>,
    pub text_truncated: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub blocked_on: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub version: i32,
}

/// Session-level summary for `close_session` and the read-only UI snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub schema_version: SchemaVersion1,
    pub session_id: String,
    pub source_task_id: String,
    pub child_conversation_id: i32,
    pub state: CollaborationSessionState,
}

/// The terminal outcome a lifecycle event (or a test double) delivers to
/// `Coordinator::settle`. Mirrors the delegation outcome vocabulary: exactly
/// one of success / failure / cancel; late duplicates lose to the first
/// settle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnTerminal {
    Completed { text: String },
    Failed { code: String, message: String },
    Canceled,
}

/// Cap a turn result under the SAME UTF-8-safe bound as the outcome store
/// (256 KiB, ellipsis counted inside the budget).
pub fn cap_turn_result_text(text: &str) -> String {
    if text.len() <= TURN_RESULT_TEXT_CAP {
        return text.to_string();
    }
    const ELLIPSIS: &str = "…";
    let budget = TURN_RESULT_TEXT_CAP.saturating_sub(ELLIPSIS.len());
    let mut end = budget.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELLIPSIS}", &text[..end])
}

/// Request-field validation shared by every entry: `request_id` is 1–128
/// bytes of printable ASCII; `message` is non-blank UTF-8 of at most 64 KiB.
pub fn validate_turn_request(request_id: &str, message: &str) -> Result<(), ContinuationError> {
    let id_bytes = request_id.as_bytes();
    if id_bytes.is_empty()
        || id_bytes.len() > 128
        || !id_bytes
            .iter()
            .all(|b| (0x20..=0x7e).contains(b))
    {
        return Err(ContinuationError::new(
            ContinuationErrorCode::InvalidRequest,
            "request_id must be 1-128 printable ASCII bytes",
        ));
    }
    if message.trim().is_empty() {
        return Err(ContinuationError::new(
            ContinuationErrorCode::InvalidRequest,
            "message must not be blank",
        ));
    }
    if message.len() > 64 * 1024 {
        return Err(ContinuationError::new(
            ContinuationErrorCode::InvalidRequest,
            "message must be at most 64 KiB",
        ));
    }
    Ok(())
}
