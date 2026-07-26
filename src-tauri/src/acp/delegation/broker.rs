//! `DelegationBroker` — the coordination unit for multi-agent delegation.
//!
//! Delegation is **asynchronous**: `delegate_to_agent` returns a `task_id`
//! ack as soon as setup finishes; the LLM collects the result later with
//! `get_delegation_status` (optionally long-polling) or stops it with
//! `cancel_delegation`. There is no blocking `oneshot` — a running task is just
//! an entry in the `running` map, and a terminal event migrates it into the
//! `completed` cache (atomically, under one lock) and wakes any long-poll via
//! `result_notify`.
//!
//! Lifecycle of a single task:
//!
//! 1. [`DelegationBroker::start_delegation`] is the broker's entry point. The
//!    MCP listener feeds it the LLM-issued `delegate_to_agent` payload.
//! 2. Pre-checks: feature enabled? depth limit ok? Both failures return a
//!    terminal report immediately, no child session created.
//! 3. Spawn the child via [`ConnectionSpawner::spawn`].
//! 4. Send the delegation task as the first prompt via
//!    [`ConnectionSpawner::send_prompt_linked_for_delegation`]. The trailing
//!    [`DelegationLink`] carries the parent's `tool_use_id` and a
//!    broker-internal `call_id` (UUID = `task_id`) — persisted onto the new
//!    conversation row so the lifecycle resolver can find it.
//! 5. Register a [`RunningTask`] keyed by `call_id` and return a `Running` ack
//!    [`DelegationTaskReport`] (or a terminal report when the child finished
//!    during setup / a cancel reached it mid-setup / setup itself failed).
//! 6. Later, a terminal event resolves the task — migrating it `running` →
//!    `completed` and tearing the child down:
//!       - the lifecycle calling [`DelegationBroker::complete_call`] on
//!         `TurnComplete` (happy path), or
//!       - a cancel — MCP-side (`notifications/cancelled` →
//!         [`DelegationBroker::cancel_by_external_handle`]), child-side
//!         ([`DelegationBroker::cancel_by_child_connection`]), parent-side
//!         ([`DelegationBroker::cancel_by_parent`] /
//!         [`DelegationBroker::cancel_by_parent_turn`]), or the LLM's own
//!         [`DelegationBroker::cancel_task_by_id`].
//!
//! v1 is explicitly one-shot — no session reuse.
//!
//! Result durability: child output is NOT stored in codeg's DB, so the broker
//! caches the completed text in `completed` (parent-scoped, FIFO-capped). Once
//! evicted, [`DelegationBroker::get_task_status`] falls back to the DB for the
//! task's terminal STATUS (via [`ChildStatusLookup`]); the full output is always
//! viewable in the child's own session.
//!
//! Cancellation cascade: when a parent session goes away (user-initiated
//! cancel, parent disconnect), the lifecycle subscriber calls
//! [`DelegationBroker::cancel_by_parent`] which fans out cancel + disconnect
//! to every running child of that parent. A normal `end_turn` does NOT cancel
//! children — they keep running in the background (the whole point of async).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify};

use crate::acp::delegation::event_emitter::{DelegationEventEmitter, NoopEventEmitter};
use crate::acp::delegation::live_reply::{ChildLiveReplyLookup, NoopChildLiveReplyLookup};
use crate::acp::delegation::meta_writer::{
    build_delegation_meta, is_synthetic_parent_tool_use_id, DelegationMetaWriter, NoopMetaWriter,
};
use crate::acp::delegation::spawner::{ConnectionSpawner, DelegationLink};
use crate::acp::delegation::types::{
    AgentDelegationDefaults, DelegationError, DelegationOutcome, DelegationRequest,
    DelegationTaskReport, TaskStatus,
};
use crate::acp::types::DelegationResultSummary;
use crate::models::AgentType;

/// Default per-parent byte budget for cached completed-task result text. The
/// completed-cache lets `get_delegation_status` / `cancel_delegation` return a
/// finished task's result after the lifecycle resolved it; once a parent's
/// retained result text exceeds this budget the OLDEST results are FIFO-evicted
/// (evicted tasks fall back to the DB status lookup, which carries status only).
/// This is the seed value baked into `DelegationConfig::default()`; the live
/// value is user-configurable from the settings page (in MB) and `0` disables
/// eviction entirely. See `PendingInner::completed_cap_bytes`.
const DEFAULT_COMPLETED_CACHE_CAP_BYTES: usize = 512 * 1024 * 1024;

/// Default cap on kept-alive settled child connections (D3). Each kept-alive
/// child is a resident agent CLI process, so the count — not bytes — is the
/// resource that must be bounded (`completed_cache_cap_bytes` only budgets
/// result TEXT and can be bypassed by many short results). Applied at global
/// scope and per-parent-conversation scope; FIFO-evicts the oldest settled
/// connection. `0` disables the cap.
const DEFAULT_KEPT_ALIVE_CAP: usize = 8;

// The continuation error codes (`session_still_running` / `session_released`
// / `not_continuable` / `resume_unavailable` / `continuation_conflict` /
// `rebuilding`) are typed `DelegationError` variants in `types.rs` (T4.1),
// projected onto wire codes by `DelegationOutcome::from_err`; the facade
// accepts upstream PR #375's `session_closed` as a historical alias
// (`types::canonical_continuation_code`). Release semantics (R2-B4): the
// wire code is `session_released` — child process freed + no further
// continuation IN THIS PROCESS, not a permanent close.
/// Fixed payload identity for `close_delegation_session` ledger entries —
/// distinguishes a replayed close from a continuation reusing the same
/// `continuation_id` (which is a conflict).
const CLOSE_OP_PAYLOAD: &str = "<close_session>";

/// Per-result cap on cached completed text. The full child output always lives
/// in the child's own session (viewable via the frontend's child-session
/// sheet); this only bounds the broker's in-memory copy of a SINGLE result.
/// Because it is far below the per-parent byte budget
/// (`DEFAULT_COMPLETED_CACHE_CAP_BYTES`), the newest result always fits and is
/// never the eviction victim in `insert_completed`.
const COMPLETED_TEXT_CAP: usize = 256 * 1024;

/// Cap on the `task_preview` carried by the `DelegationStarted` event and the
/// parent-card meta writes. The full task text lives in the MCP call (and, on
/// most hosts, in the parent tool call's own `raw_input`); the preview only
/// has to label the delegation card, so it shares the status-preview budget
/// rather than the multi-KiB result cap.
const TASK_PREVIEW_CAP: usize = 2 * 1024;

/// Cap on the inline `text_preview` carried by the `DelegationCompleted` event
/// and the terminal meta, so the parent card can render the result inline
/// without re-fetching the child session.
const STATUS_PREVIEW_CAP: usize = 2 * 1024;

/// Lookup the `parent_id` for a conversation. Abstracted so the broker can be
/// unit-tested against an in-memory chain without touching SeaORM.
#[async_trait]
pub trait ConversationDepthLookup: Send + Sync {
    async fn parent_of(&self, conversation_id: i32) -> Result<Option<i32>, DelegationError>;
}

/// Status-level facts the broker recovers from a child conversation row when a
/// task's in-memory completed-cache entry was evicted. Carries NO result text —
/// child output isn't stored in codeg's DB; the full result lives in the
/// child's own session (viewable via the frontend's child-session sheet).
#[derive(Debug, Clone)]
pub struct ChildStatusRecord {
    pub child_conversation_id: i32,
    pub status: TaskStatus,
    pub agent_type: AgentType,
    /// The parent conversation id this child was spawned under. Used to scope
    /// the DB fallback to the calling parent so one parent can't read another's
    /// task by guessing a UUID.
    pub parent_id: Option<i32>,
    /// Folder the child conversation lives in — required for Branch A row
    /// adoption when a continuation sends a follow-up prompt.
    pub folder_id: i32,
    /// Agent-side session id (`conversation.external_id`): the resume
    /// credential handed to `spawn_for_resume` once the process is gone.
    pub external_id: Option<String>,
    /// Workspace path for the resume spawn (folder path when available).
    pub working_dir: Option<String>,
}

/// DB fallback for `get_delegation_status` / `cancel_delegation` once a task's
/// result has aged out of the broker's in-memory completed-cache. Abstracted
/// so broker unit tests can run without SeaORM; production wires
/// [`DbChildStatusLookup`] via [`DelegationBroker::with_status_lookup`].
#[async_trait]
pub trait ChildStatusLookup: Send + Sync {
    async fn find_by_call_id(&self, call_id: &str) -> Option<ChildStatusRecord>;

    /// How many live (non-deleted) conversation rows reference this
    /// `external_id`. A count above 1 means the credential is ambiguous and
    /// must not be handed to an agent for resume (Requirement 7.7). Defaults
    /// to 0 so in-memory test lookups (and the Noop) stay unambiguous.
    async fn external_id_ref_count(&self, _external_id: &str) -> usize {
        0
    }

    /// Every child conversation row eligible for the startup session rebuild
    /// (Requirement 7.1): `kind = Delegate`, not deleted, carrying an
    /// `external_id` and a `delegation_call_id`. Defaults to empty so lookups
    /// that don't model persistence rebuild nothing.
    async fn list_rebuildable(&self) -> Vec<RebuildCandidate> {
        Vec::new()
    }
}

/// One child conversation row as seen by the startup rebuild protocol
/// (Requirement 7.1). `parent_alive` is resolved by the lookup (parent row
/// exists and is not soft-deleted) so the broker's ownership validation stays
/// storage-agnostic.
#[derive(Debug, Clone)]
pub struct RebuildCandidate {
    /// The persisted `delegation_call_id` — the stable `task_id`.
    pub task_id: String,
    pub child_conversation_id: i32,
    pub parent_conversation_id: Option<i32>,
    pub parent_alive: bool,
    pub parent_tool_use_id: Option<String>,
    pub agent_type: AgentType,
    pub status: TaskStatus,
    pub folder_id: i32,
    pub external_id: Option<String>,
    pub working_dir: Option<String>,
}

/// Default lookup — always "unknown". Used by `DelegationBroker::new` /
/// `with_writers` (tests that don't exercise the DB-fallback path); production
/// replaces it via `with_status_lookup`.
#[derive(Default, Clone)]
pub struct NoopChildStatusLookup;

#[async_trait]
impl ChildStatusLookup for NoopChildStatusLookup {
    async fn find_by_call_id(&self, _call_id: &str) -> Option<ChildStatusRecord> {
        None
    }
}

/// User-facing continuability verdict for one child session (design §D4 five
/// tiers). Serialized snake_case onto the user-side query wire. The design
/// table's `Closed` label predates the R2-B4 release rename — the wire and
/// UI expose it as `released` (the child process is freed; a restart expires
/// the lease, this is not a permanent close).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationAvailability {
    /// A turn is in flight — input disabled, "sub-agent is working".
    Running,
    /// A kept-alive connection is up: a continuation sends directly.
    ContinuableLive,
    /// The connection is gone but a resume credential exists: continuable,
    /// first round will be slower (resume spawn).
    ContinuableResume,
    /// Released by `close_session` / parent-conversation deletion.
    Released,
    /// No resume credential, the agent's capability rules out a resume, or
    /// the id is unknown to the broker (one verdict — existence is never
    /// disclosed).
    NotContinuable,
}

#[derive(Debug, Clone)]
pub struct DelegationConfig {
    pub enabled: bool,
    /// Max chain depth a *new* delegation may exist at. With `depth_limit = 2`
    /// the chain root → child → grandchild is allowed; the grandchild trying
    /// to spawn a great-grandchild is rejected. See spec §5.
    pub depth_limit: u32,
    /// Per-agent overrides applied when spawning a delegation child. Keyed by
    /// the target `agent_type`; missing entries mean "no override." Forwarded
    /// to `ConnectionSpawner::spawn` as `preferred_mode_id` /
    /// `preferred_config_values`.
    pub agent_defaults: BTreeMap<AgentType, AgentDelegationDefaults>,
    /// Per-parent byte budget for cached completed-task result text. `0`
    /// disables eviction (unlimited). Surfaced from the settings page in MB and
    /// converted to bytes in `into_broker_config`. Pushed into the pending-calls
    /// bucket by `set_config` so `insert_completed` reads it lock-free.
    pub completed_cache_cap_bytes: usize,
    /// Max settled child connections kept alive for `continue_with_session`,
    /// enforced at global scope and per-parent-conversation scope (D3 /
    /// Requirements 5.4-5.6). `0` disables the cap. FIFO-evicts the oldest
    /// settled connection; the evicted task's result text and resume credential
    /// survive, so a later continuation goes through the resume path.
    pub kept_alive_cap: usize,
}

impl Default for DelegationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            depth_limit: 1,
            agent_defaults: BTreeMap::new(),
            completed_cache_cap_bytes: DEFAULT_COMPLETED_CACHE_CAP_BYTES,
            kept_alive_cap: DEFAULT_KEPT_ALIVE_CAP,
        }
    }
}

/// A delegation task running in the background after `start_delegation`
/// returned its `Running` ack. The async redesign drops the parked
/// `oneshot::Sender` the old `PendingCall` carried: the parent's
/// `delegate_to_agent` no longer blocks on a channel, so there is nothing to
/// signal. A terminal event instead migrates the entry into `completed` (same
/// lock) and wakes any `get_delegation_status` long-poll via the broker's
/// `result_notify`.
struct RunningTask {
    child_connection_id: String,
    child_conversation_id: i32,
    parent_connection_id: String,
    parent_tool_use_id: String,
    /// Target agent — surfaced in status reports.
    agent_type: AgentType,
    /// Bounded preview of the delegated task text ([`TASK_PREVIEW_CAP`]).
    /// Carried so TERMINAL meta writes can keep labeling the parent card —
    /// meta is replace-wholesale on the ToolCallState, so a terminal write
    /// that dropped the task text would erase what the running write supplied.
    task_preview: String,
    /// The broker-minted task id — duplicates this entry's key in `running`
    /// because the cancel/teardown paths hand around the drained
    /// `RunningTask` by value without its map key.
    task_id: String,
    /// MCP-side opaque handle minted by the companion per `tools/call`. The
    /// listener forwards it through `DelegationRequest`; we keep it here so
    /// `cancel_by_external_handle` can find the entry. `None` for delegations
    /// that didn't come through MCP (tests, future internal callers).
    external_handle: Option<String>,
    /// When the child started running (after `send_prompt` succeeded). Used to
    /// compute a real `duration_ms` at terminal time.
    started_at: Instant,
    /// The session's dispatch counter for THIS turn (1 = the original
    /// delegation). Gates completion-event emission: a continued turn
    /// (`> 1`) must NOT re-emit `DelegationCompleted` against a
    /// `parent_tool_use_id` whose tool call already went terminal
    /// (Requirement 2.8a; the session-scoped update replaces it — T4.3).
    turn_version: u64,
    /// Broker-internal id of THIS turn's [`TurnRecord`], carried onto the
    /// session-scoped update at settle time (Requirement 8.2). `None` for the
    /// original turn (version 1 — its terminal event is the tool-scoped
    /// `DelegationCompleted`, which needs no turn id).
    turn_id: Option<String>,
    /// Who dispatched this turn (Requirement 8.2).
    origin: TurnOrigin,
}

/// A terminal delegation result retained so `get_delegation_status` /
/// `cancel_delegation` can answer after the lifecycle resolved the task.
/// Parent-scoped, FIFO-evicted once the parent's retained result text exceeds
/// `PendingInner::completed_cap_bytes`, and dropped wholesale when the parent
/// connection tears down.
#[derive(Clone)]
struct CompletedTask {
    parent_connection_id: String,
    child_conversation_id: i32,
    agent_type: AgentType,
    status: TaskStatus,
    /// Result text for `Completed` (capped at [`COMPLETED_TEXT_CAP`]). `None`
    /// for failures/cancels.
    text: Option<String>,
    error_code: Option<String>,
    message: Option<String>,
    duration_ms: u64,
}

/// Who dispatched a turn (Requirement 8.2 origin): the MCP companion tool
/// (`ParentAgent`) or the user-side entry point (`User`). Recorded on each
/// [`TurnRecord`]; never on the wire REPORT — but it does ride the
/// session-scoped `AcpEvent::DelegationSessionUpdate` event (T4.3), hence the
/// serde derives (snake_case: `parent_agent` / `user`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOrigin {
    ParentAgent,
    User,
}

/// One dispatched execution of a child session — the middle layer of the D2
/// split. `turn_id` is broker-internal (event correlation / audit, never on
/// the wire); `turn_version` is the session's monotonic dispatch counter
/// (Requirement 8.1).
struct TurnRecord {
    /// Consumed by the session-scoped update events (T4.3: payload
    /// `(task_id, turn_id, turn_version, origin)`); until that lands the only
    /// readers live in tests, so the non-test build sees no read.
    #[allow(dead_code)]
    turn_id: String,
    #[allow(dead_code)]
    turn_version: u64,
    #[allow(dead_code)]
    origin: TurnOrigin,
}

/// Bound on the per-session turn history (D2 "bounded queue") — audit depth,
/// not correctness: `turn_version` keeps counting past it.
const TURNS_CAP: usize = 32;

/// One continuation operation tracked for idempotency — the bottom layer of
/// the D2 split. Keyed by the caller-generated `continuation_id` in
/// `PendingInner::operations`, independent of `completed` / `running`; lives
/// until the session is released or the process restarts (Requirement 7.4).
struct OperationRecord {
    task_id: String,
    /// Payload identity: the same `continuation_id` with a different message
    /// is a caller bug (`continuation_conflict`), never silently resolved.
    message: String,
    /// First accepted report. `None` while the dispatch is still in flight —
    /// a duplicate arriving then gets an equivalent Running ack.
    report: Option<DelegationTaskReport>,
}

/// Session-level domain state — the top layer of the D2 three-layer split
/// (Session / Turn / Operation). One entry per delegation `task_id`, created at
/// dispatch and retained for the whole session lifetime (running, settled,
/// released). Owns identity, ownership, and the kept-alive child connection.
///
/// This is deliberately SEPARATE from [`CompletedTask`]: the completed-cache
/// only caches result TEXT (byte-capped, evictable), while this entry carries
/// the domain facts that must never be lost to a cache eviction. Emptying the
/// completed-cache may only make the system slower (DB fallback for text),
/// never wrong.
struct SessionEntry {
    /// Persistent ownership key (D5): the parent CONVERSATION row id, not the
    /// volatile parent connection id — so a parent reconnect can't orphan the
    /// session.
    parent_conversation_id: i32,
    /// Latest run lease — the parent connection that dispatched the most
    /// recent turn. Transient; refreshed on every dispatch.
    parent_connection_id: String,
    /// Parent `delegate_to_agent` tool_use_id — reused verbatim by follow-up
    /// meta writes / started events (NEVER re-claimed; see `44415f56`).
    parent_tool_use_id: String,
    agent_type: AgentType,
    child_conversation_id: i32,
    /// Latest known child ACP connection. `Some` while running or kept alive
    /// after a completed/failed settle; `None` once torn down (cancel) — a
    /// later continuation must then go through the resume path.
    child_connection_id: Option<String>,
    /// Session status = the latest turn's result (`Running` while a turn is in
    /// flight). Mirrors what `get_delegation_status` reports.
    status: TaskStatus,
    /// Release semantics (R2-B4): the child process is freed and this session
    /// takes no further continuation IN THIS PROCESS. Not persisted — a
    /// restart naturally expires the lease (that is correct behavior, not a
    /// gap). Permanent disposal = deleting the conversation row.
    released: bool,
    /// Set when a release-time disconnect FAILED: the child process may still
    /// be alive (observable for diagnosis; a background retry + the
    /// `--parent-pid` watchdog are the recovery paths).
    orphan_suspect: bool,
    /// Monotonic dispatch counter — +1 per dispatched turn (Requirement 8.1).
    turn_version: u64,
    /// Bounded turn history, newest at the back (D2 middle layer).
    turns: VecDeque<TurnRecord>,
    /// Resume metadata, cached lazily from the child conversation row on the
    /// first continuation (folder → Branch A row adoption; `external_id` →
    /// resume credential; `working_dir` → resume spawn cwd).
    folder_id: Option<i32>,
    external_id: Option<String>,
    working_dir: Option<String>,
}

impl SessionEntry {
    /// Register a newly dispatched turn: bump the monotonic version and append
    /// a bounded [`TurnRecord`]. Returns the new turn's `(turn_id, version)`
    /// for event payloads.
    fn push_turn(&mut self, origin: TurnOrigin) -> (String, u64) {
        self.turn_version += 1;
        let turn_id = uuid::Uuid::new_v4().to_string();
        self.turns.push_back(TurnRecord {
            turn_id: turn_id.clone(),
            turn_version: self.turn_version,
            origin,
        });
        while self.turns.len() > TURNS_CAP {
            self.turns.pop_front();
        }
        (turn_id, self.turn_version)
    }
}

#[derive(Default)]
struct PendingCalls {
    inner: Mutex<PendingInner>,
}

/// Everything guarded by the single pending-calls mutex. Co-locating the parked
/// calls with the early-terminal bookkeeping under ONE lock is what makes the
/// terminal-vs-registration race safe: a terminal event for a delegation that
/// is still mid-setup (its `handle_request` hasn't parked the [`PendingCall`]
/// yet) and the matching registration are serialized on this lock, so the
/// terminal event either finds the parked entry (resolves via `tx`) or buffers
/// its outcome (and `handle_request` drains it the instant it parks) — never
/// both, never neither. Without this, a terminal that fires in the spawn→park
/// window would no-op the resolver and then strand the parked `rx.await`.
///
/// Both CHILD-terminal pre-park resolvers are covered, because either can win
/// the race against the parent `write_meta` await between `send_prompt` and the
/// park:
///   * `complete_call` — a fast/empty turn's `TurnComplete` (the prompt is only
///     *enqueued* by `send_prompt`; the child loop emits `TurnComplete`
///     independently). Keyed by `call_id`.
///   * `cancel_by_child_connection` — a freshly-spawned child connection dying
///     before its first prompt is answered. Keyed by `child_connection_id`.
///
/// Parent-side cancels (`cancel_by_parent` / `cancel_by_parent_turn`) are
/// covered symmetrically by the `inflight` registry: `handle_request` registers
/// each setup at entry, and `mark_inflight_canceled_for_parent` runs in the SAME
/// lock acquisition that drains the parked `calls`. A parent cancel landing
/// while a child is still mid-setup therefore flags the in-flight record, and
/// `handle_request` observes the flag at its next checkpoint (or atomically at
/// park) and tears the child down itself — it is no longer left to the child's
/// own terminal / connection-teardown cascade.
///
/// The reservation records the `child_connection_id` each resolver gates on;
/// `handle_request` drains both buffers at park.
#[derive(Default)]
struct PendingInner {
    /// Session registry — the domain-state layer (see [`SessionEntry`]). Keyed
    /// by `task_id`. Entries are created at dispatch and OUTLIVE both `running`
    /// (turn scheduling) and `completed` (result-text cache): cancel paths
    /// settle them in place, and parent-connection teardown does NOT remove
    /// them (ownership is the parent conversation, not the connection — D5).
    sessions: HashMap<String, SessionEntry>,
    /// Kept-alive FIFO — task ids whose session currently retains a settled
    /// child connection, oldest-settled first. Drives the `kept_alive_cap`
    /// eviction (global + per-parent scope). Invariant: `task ∈
    /// kept_alive_order` ⇔ `sessions[task].child_connection_id.is_some()` and
    /// the session is settled (maintained by `settle_session`).
    kept_alive_order: VecDeque<String>,
    /// Global cap on kept-alive settled connections. `0` = unlimited. Seeded
    /// by `set_config` from `DelegationConfig::kept_alive_cap` (same scheme as
    /// `completed_cap_bytes`).
    kept_alive_cap_global: usize,
    /// Per-parent-conversation cap. Seeded to the SAME configured value as the
    /// global cap today — with equal values the global check fires first, so
    /// this layer is a structural safeguard that becomes load-bearing the day
    /// the two values diverge (Requirement 5.5 mandates both scopes).
    kept_alive_cap_per_parent: usize,
    /// Tasks running in the background after their `Running` ack, keyed by
    /// broker `call_id` (= `task_id`). A terminal event migrates an entry from
    /// here into `completed` under THIS lock (atomic `running` → `completed`
    /// transition), so a concurrent `get_delegation_status` never observes a
    /// task as neither running nor completed.
    running: HashMap<String, RunningTask>,
    /// Terminal results retained for `get_delegation_status` / `cancel_delegation`,
    /// keyed by `task_id`. Bounded by the per-parent byte valve
    /// (`completed_cap_bytes` over `completed_bytes`, FIFO-evicted via
    /// `completed_order`) and dropped per-parent on connection teardown.
    /// Evicted/unknown tasks fall back to the DB status lookup.
    completed: HashMap<String, CompletedTask>,
    /// Per-parent FIFO index over `completed` for byte-valve eviction and
    /// per-parent teardown. Keyed by `parent_connection_id`; each deque holds
    /// that parent's completed `task_id`s oldest-first.
    completed_order: HashMap<String, VecDeque<String>>,
    /// Per-parent running total of retained completed result-text bytes (the
    /// `CompletedTask::text` lengths). Drives the `completed_cap_bytes` valve in
    /// `insert_completed`; kept in sync on insert/evict and cleared per-parent
    /// on teardown.
    completed_bytes: HashMap<String, usize>,
    /// Per-parent byte budget for retained completed result text. `0` =
    /// unlimited (no eviction). Seeded by `set_config` from the live
    /// `DelegationConfig` (default until then: `0`, but `set_config` always runs
    /// at startup via `apply_persisted_config`). Read lock-free by
    /// `insert_completed`, which already holds THIS mutex — so the cap is
    /// consulted WITHOUT nesting the `config` lock under the pending lock.
    completed_cap_bytes: usize,
    /// In-setup delegations (spawned + id minted, not yet parked), mapping
    /// `call_id` → `child_connection_id`. Gating the early buffers on membership
    /// here distinguishes a genuine pre-registration race (still reserved →
    /// buffer) from the normal post-resolution teardown that fires on every
    /// completion (no longer reserved → ignore). Removed at park / on the
    /// send-failure path.
    setups: HashMap<String, String>,
    /// Completion outcomes captured by a `TurnComplete` that beat registration
    /// (gated by `setups`), keyed by `call_id`. Each carries the `seq` arrival
    /// stamp taken when it buffered, so the park can order it against a racing
    /// parent cancel (first-terminal-wins). Drained at park.
    early_completes: HashMap<String, (u64, DelegationOutcome)>,
    /// Cancel reasons captured by a child failure that beat registration (gated
    /// by `setups`), keyed by `child_connection_id`. The value pairs the `seq`
    /// arrival stamp (for the park's first-terminal-wins ordering against a
    /// racing parent cancel) with the pre-computed `Canceled { reason }` text
    /// (same wording the parked `cancel_by_child_connection` path produces);
    /// `handle_request` rebuilds the full outcome at park with the real
    /// `child_conversation_id` (which the resolver, finding no entry, lacked).
    early_cancels: HashMap<String, (u64, String)>,
    /// Continuation idempotency ledger (D2 bottom layer), keyed by the
    /// caller-generated `continuation_id`. Deliberately independent of
    /// `completed` / `running` so dedup covers the whole
    /// dispatching → running → settled lifetime (R2-B5).
    operations: HashMap<String, OperationRecord>,
    /// True while the startup rebuild scan is in flight (Requirement 7.2):
    /// a session-miss then answers the retryable `rebuilding` code instead of
    /// `Unknown`. Set/cleared by `rebuild_sessions_from_db`.
    rebuilding: bool,
    /// In-flight `handle_request` setups, keyed by a unique per-call id and
    /// registered at entry (BEFORE the claim poll, so the whole claim→park
    /// window is covered). This is the parent-cancel counterpart to `setups`:
    /// `setups` lets a *child* terminal reach a not-yet-parked delegation,
    /// while `inflight` lets a *parent* cancel reach one. `cancel_by_parent*`
    /// flags every entry it owns (`mark_inflight_canceled_for_parent`);
    /// `handle_request` consults the flag after claim, after spawn, and
    /// atomically at park, tearing the spawned child down itself when set.
    /// Removed at park and on every early-return (no Drop guard — see
    /// `register_inflight`).
    inflight: HashMap<u64, InflightSetup>,
    /// Monotonic arrival clock (see `tick`). Hands out the unique `inflight`
    /// keys AND the arrival stamps on buffered child terminals / parent cancels,
    /// so the park can resolve a setup-window race by true first-terminal-wins
    /// order. Keys and stamps share this sequence but are never cross-compared
    /// (keys match by identity, stamps only by `<` against other stamps).
    seq: u64,
}

/// One in-flight `handle_request` setup tracked for parent-cancel coverage.
struct InflightSetup {
    parent_connection_id: String,
    /// `Some(stamp)` once a parent cancel lands while this delegation is
    /// mid-setup (spawned / sending, not yet parked), where `stamp` is the `seq`
    /// arrival-clock value at that moment. First-write-wins and never cleared,
    /// so a cancel can't be lost between `handle_request`'s checkpoints, and its
    /// stamp lets the park order it against a racing child terminal.
    canceled_at: Option<u64>,
    /// The task a CONTINUATION dispatch is acting on (`None` for the original
    /// `start_delegation` setup, whose task id isn't minted yet at
    /// registration). Lets `close_delegation_session` cancel exactly the
    /// in-dispatch continuation of ITS task (Requirement 2.12) without
    /// flagging the parent's unrelated setups.
    task_id: Option<String>,
}

impl PendingInner {
    /// Mark a delegation as setting-up (spawned + id minted, not yet parked) so
    /// a terminal event racing the park is buffered rather than dropped.
    ///
    /// No cap: a reservation lives only for the brief spawn→park window and is
    /// always released by `unreserve` on every `handle_request` exit (park, or
    /// the send-failure path), so `setups` is bounded by the count of
    /// concurrently-in-setup delegations — it never accumulates stale entries.
    /// A cap here would be actively unsafe: every reservation is live, so
    /// evicting one to make room would drop a real in-flight delegation's race
    /// guard and reopen the very hang this machinery exists to prevent.
    fn reserve(&mut self, call_id: &str, child_connection_id: &str) {
        self.setups
            .insert(call_id.to_string(), child_connection_id.to_string());
    }

    /// Release a delegation's reservation and discard any un-drained buffered
    /// terminal — called once the entry is parked (the buffers were already
    /// drained, so the removals are no-ops then) or when setup errors out
    /// (discarding a buffer no `handle_request` will pick up).
    fn unreserve(&mut self, call_id: &str, child_connection_id: &str) {
        self.setups.remove(call_id);
        self.early_completes.remove(call_id);
        self.early_cancels.remove(child_connection_id);
    }

    /// Whether a child connection belongs to a still-in-setup delegation. O(n)
    /// over `setups`, but n is the (tiny) count of concurrently-in-setup
    /// delegations.
    fn is_child_reserved(&self, child_connection_id: &str) -> bool {
        self.setups
            .values()
            .any(|child| child == child_connection_id)
    }

    /// Buffer a completion for a still-reserved delegation, stamped with the
    /// current arrival clock so the park can order it against a racing parent
    /// cancel. No-op when the `call_id` isn't reserved (already resolved by
    /// another terminal path), so the buffer only ever holds genuine
    /// pre-registration races.
    fn buffer_early_complete(&mut self, call_id: &str, outcome: DelegationOutcome) {
        if self.setups.contains_key(call_id) {
            let stamp = self.tick();
            self.early_completes
                .insert(call_id.to_string(), (stamp, outcome));
        }
    }

    /// Buffer a child failure for a still-reserved delegation, stamped with the
    /// current arrival clock so the park can order it against a racing parent
    /// cancel. No-op when the child isn't reserved (normal post-resolution
    /// teardown). Stores the pre-computed cancel reason so the park rebuilds the
    /// same wording the parked `cancel_by_child_connection` path produces.
    fn buffer_child_failure(&mut self, child_connection_id: &str, detail: Option<String>) {
        if self.is_child_reserved(child_connection_id) {
            let stamp = self.tick();
            self.early_cancels.insert(
                child_connection_id.to_string(),
                (stamp, child_canceled_reason(detail.as_deref())),
            );
        }
    }

    /// Drain a buffered completion with its arrival stamp (by `call_id`) — used
    /// by `handle_request` at park.
    fn take_early_complete(&mut self, call_id: &str) -> Option<(u64, DelegationOutcome)> {
        self.early_completes.remove(call_id)
    }

    /// Drain a buffered cancel reason with its arrival stamp (by
    /// `child_connection_id`) — used by `handle_request` at park.
    fn take_early_cancel(&mut self, child_connection_id: &str) -> Option<(u64, String)> {
        self.early_cancels.remove(child_connection_id)
    }

    /// Advance the monotonic arrival clock, returning the pre-increment value.
    /// Strictly increasing (wraps only after 2^64 calls — unreachable), so two
    /// events stamped under this lock always compare in their true arrival
    /// order. Backs both `inflight` keys and terminal/cancel arrival stamps; the
    /// two uses never cross-compare (keys match by identity, stamps by `<`).
    fn tick(&mut self) -> u64 {
        let v = self.seq;
        self.seq = self.seq.wrapping_add(1);
        v
    }

    /// Register an in-flight setup at `handle_request` entry, returning its
    /// unique id. The caller MUST `deregister_inflight` on every exit path
    /// (each early-return, and at park). There is deliberately NO Drop guard:
    /// the park hand-off — `calls.insert` followed by `deregister_inflight` —
    /// has to be atomic under this lock so a concurrent parent cancel sees the
    /// entry in exactly one of `inflight` or `calls`, and a guard firing after
    /// the lock releases would reopen that window.
    fn register_inflight(&mut self, parent_connection_id: &str) -> u64 {
        let id = self.tick();
        self.inflight.insert(
            id,
            InflightSetup {
                parent_connection_id: parent_connection_id.to_string(),
                canceled_at: None,
                task_id: None,
            },
        );
        id
    }

    /// Like [`Self::register_inflight`], but for a CONTINUATION dispatch —
    /// records the task id so a `close_delegation_session` racing the dispatch
    /// can flag exactly this setup (Requirement 2.12).
    fn register_inflight_for_task(&mut self, parent_connection_id: &str, task_id: &str) -> u64 {
        let id = self.tick();
        self.inflight.insert(
            id,
            InflightSetup {
                parent_connection_id: parent_connection_id.to_string(),
                canceled_at: None,
                task_id: Some(task_id.to_string()),
            },
        );
        id
    }

    /// Flag the in-flight continuation dispatch(es) of ONE task as canceled —
    /// the close-vs-continue serialization point (Requirement 2.12). Same
    /// first-write-wins stamping as `mark_inflight_canceled_for_parent`.
    fn mark_inflight_canceled_for_task(&mut self, task_id: &str) {
        let stamp = self.tick();
        for setup in self.inflight.values_mut() {
            if setup.task_id.as_deref() == Some(task_id) && setup.canceled_at.is_none() {
                setup.canceled_at = Some(stamp);
            }
        }
    }

    /// Drop an in-flight setup record (idempotent).
    fn deregister_inflight(&mut self, id: u64) {
        self.inflight.remove(&id);
    }

    /// Whether a parent cancel flagged this in-flight setup. False once the
    /// record is gone (already parked / deregistered). Used by the pre-spawn /
    /// post-spawn checkpoints, which only need the boolean.
    fn inflight_canceled(&self, id: u64) -> bool {
        self.inflight
            .get(&id)
            .map(|s| s.canceled_at.is_some())
            .unwrap_or(false)
    }

    /// Arrival stamp of the parent cancel that flagged this in-flight setup, if
    /// any (`None` when not canceled, or the record is already gone). Used at
    /// park to order the cancel against a buffered child terminal.
    fn inflight_canceled_at(&self, id: u64) -> Option<u64> {
        self.inflight.get(&id).and_then(|s| s.canceled_at)
    }

    /// Flag every in-flight setup owned by `parent_connection_id` as canceled,
    /// stamping each with one shared arrival-clock value (this cancel is a
    /// single event). First-write-wins per setup, so a later cancel can't push
    /// an earlier one's stamp forward. Called from `drain_for_parent_cancel` in
    /// the SAME lock acquisition that drains the parked `calls`, so each of the
    /// parent's delegations is caught either here (still in-flight → flagged;
    /// `handle_request` tears its child down at the next checkpoint) or by the
    /// parked-call drain (already parked) — never neither.
    fn mark_inflight_canceled_for_parent(&mut self, parent_connection_id: &str) {
        let stamp = self.tick();
        for setup in self.inflight.values_mut() {
            if setup.parent_connection_id == parent_connection_id && setup.canceled_at.is_none() {
                setup.canceled_at = Some(stamp);
            }
        }
    }

    /// Insert a terminal result into the completed-cache, then FIFO-evict this
    /// parent's OLDEST results until its retained result-text bytes fit
    /// `completed_cap_bytes` (`0` = unlimited). Evicted tasks fall back to the
    /// DB status lookup (status only — child text lives in the child session).
    /// The just-inserted entry is never the victim: a single result is capped
    /// at [`COMPLETED_TEXT_CAP`] (256 KiB), far below any MB-scale budget, so
    /// the newest result always survives for the LLM's immediate
    /// `get_delegation_status`. The caller does the atomic `running.remove` +
    /// this insert under one lock, then notifies long-poll waiters AFTER
    /// releasing the lock.
    fn insert_completed(&mut self, call_id: &str, task: CompletedTask) {
        let parent = task.parent_connection_id.clone();
        let task_bytes = task.text.as_ref().map_or(0, |t| t.len());
        self.completed.insert(call_id.to_string(), task);
        *self.completed_bytes.entry(parent.clone()).or_insert(0) += task_bytes;
        self.completed_order
            .entry(parent.clone())
            .or_default()
            .push_back(call_id.to_string());
        self.evict_completed_over_cap(&parent);
    }

    /// Evict `parent`'s OLDEST completed results until its retained result-text
    /// bytes fit `completed_cap_bytes` (`0` = unlimited). Evicted tasks fall
    /// back to the DB status lookup (status only — child text lives in the child
    /// session). The newest entry is never evicted: a single result is capped at
    /// [`COMPLETED_TEXT_CAP`] (256 KiB), far below any MB-scale budget, so the
    /// LLM's immediate `get_delegation_status` always hits.
    fn evict_completed_over_cap(&mut self, parent: &str) {
        let cap = self.completed_cap_bytes;
        if cap == 0 {
            return;
        }
        loop {
            if self.completed_bytes.get(parent).copied().unwrap_or(0) <= cap {
                break;
            }
            let evicted = match self.completed_order.get_mut(parent) {
                Some(order) if order.len() > 1 => order.pop_front(),
                _ => None,
            };
            let Some(evicted) = evicted else {
                break;
            };
            if let Some(removed) = self.completed.remove(&evicted) {
                let freed = removed.text.as_ref().map_or(0, |t| t.len());
                if let Some(slot) = self.completed_bytes.get_mut(parent) {
                    *slot = slot.saturating_sub(freed);
                }
            }
        }
    }

    /// Re-apply the current `completed_cap_bytes` to EVERY parent. Called by
    /// `set_config` when the cap may have been LOWERED at runtime, so
    /// already-retained results are pruned promptly — insert-time eviction alone
    /// would otherwise strand them until a parent's next completion (which may
    /// never arrive).
    fn enforce_completed_cap_all_parents(&mut self) {
        if self.completed_cap_bytes == 0 {
            return;
        }
        let parents: Vec<String> = self.completed_bytes.keys().cloned().collect();
        for parent in parents {
            self.evict_completed_over_cap(&parent);
        }
    }

    /// Settle the session entry for `task_id`: record the terminal status and
    /// either retain the child connection (keep-alive, completed/failed) or
    /// drop it (cancel-coded outcomes — the process is being torn down).
    /// No-op for an unknown task (e.g. pure-unit tests that drive
    /// `insert_completed` directly without a dispatch).
    ///
    /// Returns the connection ids EVICTED by the `kept_alive_cap` to make room
    /// for a newly retained connection (Property 1 conservation: every id
    /// cleared from a session is handed back for a disconnect call). The
    /// caller MUST disconnect them after releasing the pending lock.
    #[must_use]
    fn settle_session(
        &mut self,
        task_id: &str,
        status: TaskStatus,
        keep_conn: Option<String>,
    ) -> Vec<String> {
        let parent_conversation_id = match self.sessions.get_mut(task_id) {
            Some(s) => {
                // Release is terminal (R2-B4): a close that raced this settle
                // has already recorded the session's final status and taken
                // its connection pointer — for an in-dispatch continuation it
                // deferred the teardown to the dispatch's compensation paths
                // (S3 checkpoint / `abort_continuation`), which settle through
                // HERE. Restoring `keep_conn` onto the released session would
                // leave `released = true` plus a connection nobody will ever
                // disconnect (leaked child process), and overwriting `status`
                // would corrupt the close's recorded terminal. Freeze the
                // session and hand the connection back to the caller for a
                // disconnect instead (Property 1 conservation: every id not
                // retained by a session is returned for teardown).
                if s.released {
                    self.kept_alive_order.retain(|t| t != task_id);
                    return keep_conn.into_iter().collect();
                }
                s.status = status;
                s.child_connection_id = keep_conn.clone();
                s.parent_conversation_id
            }
            None => return Vec::new(),
        };
        // Maintain the kept-alive FIFO invariant: drop any stale slot for this
        // task (re-settle after a continuation), then re-register when a
        // connection is retained.
        self.kept_alive_order.retain(|t| t != task_id);
        if keep_conn.is_none() {
            return Vec::new();
        }
        self.kept_alive_order.push_back(task_id.to_string());
        self.enforce_kept_alive_caps(parent_conversation_id)
    }

    /// Enforce the kept-alive caps after a new retention: global scope first
    /// (FIFO over ALL kept connections), then the per-parent scope for the
    /// parent that just retained one. Clears each victim's session connection
    /// pointer and returns the ids for the caller to disconnect out of lock.
    /// The just-retained newest entry sits at the back of the FIFO, so it is
    /// only evicted when the cap is smaller than 1 slot of headroom.
    fn enforce_kept_alive_caps(&mut self, parent_conversation_id: i32) -> Vec<String> {
        let mut evicted = Vec::new();
        if self.kept_alive_cap_global > 0 {
            while self.kept_alive_order.len() > self.kept_alive_cap_global {
                let Some(victim) = self.kept_alive_order.pop_front() else {
                    break;
                };
                if let Some(cid) = self
                    .sessions
                    .get_mut(&victim)
                    .and_then(|s| s.child_connection_id.take())
                {
                    evicted.push(cid);
                }
            }
        }
        if self.kept_alive_cap_per_parent > 0 {
            loop {
                let parent_tasks: Vec<String> = self
                    .kept_alive_order
                    .iter()
                    .filter(|t| {
                        self.sessions
                            .get(t.as_str())
                            .is_some_and(|s| s.parent_conversation_id == parent_conversation_id)
                    })
                    .cloned()
                    .collect();
                if parent_tasks.len() <= self.kept_alive_cap_per_parent {
                    break;
                }
                let victim = parent_tasks[0].clone();
                self.kept_alive_order.retain(|t| t != &victim);
                if let Some(cid) = self
                    .sessions
                    .get_mut(&victim)
                    .and_then(|s| s.child_connection_id.take())
                {
                    evicted.push(cid);
                }
            }
        }
        evicted
    }

    /// Re-apply the kept-alive caps to EVERY parent — called by `set_config`
    /// when the cap may have been LOWERED at runtime, mirroring
    /// `enforce_completed_cap_all_parents`. Returns the evicted connection ids
    /// for the caller to disconnect after releasing the lock.
    fn enforce_kept_alive_caps_all(&mut self) -> Vec<String> {
        let parents: Vec<i32> = self
            .kept_alive_order
            .iter()
            .filter_map(|t| self.sessions.get(t).map(|s| s.parent_conversation_id))
            .collect();
        let mut evicted = Vec::new();
        for parent in parents {
            evicted.extend(self.enforce_kept_alive_caps(parent));
        }
        evicted
    }

    /// Remove ONE completed entry, keeping the byte counter and per-parent
    /// order index consistent. Used when a continuation moves a settled task
    /// back to `running` (the old result text is no longer the current answer)
    /// and by the dispatch-window cancel compensation.
    fn remove_completed_entry(&mut self, task_id: &str) {
        if let Some(c) = self.completed.remove(task_id) {
            let freed = c.text.as_ref().map_or(0, |t| t.len());
            if let Some(slot) = self.completed_bytes.get_mut(&c.parent_connection_id) {
                *slot = slot.saturating_sub(freed);
            }
            if let Some(order) = self.completed_order.get_mut(&c.parent_connection_id) {
                order.retain(|t| t != task_id);
            }
        }
    }

    /// Forget every completed result for a parent. Called on connection
    /// teardown (the parent is gone — nothing left to query). A turn cancel
    /// deliberately does NOT call this: the connection stays alive and the LLM
    /// may still query its just-canceled tasks.
    fn drop_completed_for_parent(&mut self, parent_connection_id: &str) {
        self.completed_bytes.remove(parent_connection_id);
        if let Some(ids) = self.completed_order.remove(parent_connection_id) {
            for id in ids {
                self.completed.remove(&id);
            }
        }
    }
}

/// Cap result text retained in the completed-cache. The full output always
/// lives in the child session; this only bounds the broker's copy.
fn cap_completed_text(text: &str) -> String {
    truncate_on_char_boundary(text, COMPLETED_TEXT_CAP)
}

/// Build the bounded inline preview carried by the `DelegationCompleted` event
/// and terminal meta. `None` for empty text.
fn build_text_preview(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    Some(truncate_on_char_boundary(text, STATUS_PREVIEW_CAP))
}

/// Truncate `s` so the RESULT (including the appended ellipsis) is at most `cap`
/// bytes, cut on a UTF-8 char boundary. Reserving the ellipsis bytes keeps the
/// output within the advertised cap rather than `cap + 3`.
fn truncate_on_char_boundary(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    const ELLIPSIS: &str = "…";
    // Leave room for the ellipsis; clamp at 0 for pathologically small caps.
    let budget = cap.saturating_sub(ELLIPSIS.len());
    let mut end = budget.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELLIPSIS}", &s[..end])
}

/// Derive the completed-cache fields (status / text / error_code / message)
/// from a resolved [`DelegationOutcome`]. `Canceled`-coded errors map to
/// [`TaskStatus::Canceled`]; every other error maps to [`TaskStatus::Failed`].
fn terminal_fields(
    outcome: &DelegationOutcome,
) -> (TaskStatus, Option<String>, Option<String>, Option<String>) {
    match outcome {
        DelegationOutcome::Ok(ok) => (
            TaskStatus::Completed,
            Some(cap_completed_text(&ok.text)),
            None,
            None,
        ),
        DelegationOutcome::Err { code, message, .. } => {
            let status = if code == "canceled" {
                TaskStatus::Canceled
            } else {
                TaskStatus::Failed
            };
            (status, None, Some(code.clone()), Some(message.clone()))
        }
    }
}

/// Build a [`CompletedTask`] from a resolved outcome for the completed-cache.
fn build_completed(
    parent_connection_id: &str,
    child_conversation_id: i32,
    agent_type: AgentType,
    duration_ms: u64,
    outcome: &DelegationOutcome,
) -> CompletedTask {
    let (status, text, error_code, message) = terminal_fields(outcome);
    CompletedTask {
        parent_connection_id: parent_connection_id.to_string(),
        child_conversation_id,
        agent_type,
        status,
        text,
        error_code,
        message,
        duration_ms,
    }
}

/// A `canceled`-coded [`DelegationOutcome`] carrying the child conversation id.
fn canceled_outcome(child_conversation_id: i32, reason: &str) -> DelegationOutcome {
    DelegationOutcome::from_err(
        DelegationError::Canceled {
            reason: reason.to_string(),
        },
        Some(child_conversation_id),
    )
}

/// Remove `keys` from `running`, recording each as a `Canceled` completed entry
/// (so a `get_delegation_status` still answers) and returning the drained tasks
/// — each paired with the `duration_ms` captured at this drain point — for I/O
/// teardown. MUST be called with the pending lock held so the running →
/// completed migration is atomic.
///
/// The duration is captured ONCE here and returned so the slow teardown
/// (parent-card meta, report) reuses the exact value recorded into the
/// completed-cache, rather than recomputing `started_at.elapsed()` later — which
/// would inflate it for the backgrounded `cancel_by_parent_turn` teardown and
/// disagree with the `get_delegation_status` / `cancel_delegation` cards.
fn drain_and_record_canceled(
    inner: &mut PendingInner,
    keys: Vec<String>,
    reason: &str,
) -> Vec<(RunningTask, u64)> {
    let mut out = Vec::with_capacity(keys.len());
    for k in keys {
        let task = inner.running.remove(&k).expect("key just observed");
        let outcome = canceled_outcome(task.child_conversation_id, reason);
        let duration_ms = task.started_at.elapsed().as_millis() as u64;
        inner.insert_completed(
            &k,
            build_completed(
                &task.parent_connection_id,
                task.child_conversation_id,
                task.agent_type,
                duration_ms,
                &outcome,
            ),
        );
        // Cancel tears the child process down — clear the session's connection
        // so a later continuation must go through the resume path instead of
        // talking to a dead connection. `keep_conn: None` never evicts, so the
        // returned list is structurally empty here.
        let evicted = inner.settle_session(&k, TaskStatus::Canceled, None);
        debug_assert!(evicted.is_empty());
        out.push((task, duration_ms));
    }
    out
}

/// Project a `DelegationOutcome` + broker-measured `duration_ms` onto the
/// wire-stable `DelegationResultSummary` carried by `DelegationCompleted`.
/// Keeps the mapping (and the bounded `text_preview`) in one place.
fn outcome_to_summary(outcome: &DelegationOutcome, duration_ms: u64) -> DelegationResultSummary {
    match outcome {
        DelegationOutcome::Ok(ok) => DelegationResultSummary::Ok {
            duration_ms,
            text_preview: build_text_preview(&ok.text),
        },
        DelegationOutcome::Err { code, .. } => DelegationResultSummary::Err {
            error_code: code.clone(),
        },
    }
}

/// Project a resolved outcome onto a terminal [`DelegationTaskReport`] (used by
/// the setup-window terminal dispositions and the test shim).
fn report_from_outcome(
    task_id: Option<String>,
    agent_type: Option<AgentType>,
    outcome: &DelegationOutcome,
    duration_ms: Option<u64>,
) -> DelegationTaskReport {
    let (status, text, error_code, message) = terminal_fields(outcome);
    let child_conversation_id = match outcome {
        DelegationOutcome::Ok(ok) => Some(ok.child_conversation_id),
        DelegationOutcome::Err {
            child_conversation_id,
            ..
        } => *child_conversation_id,
    };
    DelegationTaskReport {
        task_id,
        status,
        child_conversation_id,
        agent_type,
        text,
        error_code,
        message,
        duration_ms,
    }
}

/// Build a `Failed` report carrying a known task id and a typed continuation
/// error — the continue / close rejection shape (the task exists, the
/// operation on it was refused). The wire `error_code` / `message` are
/// projected by `DelegationOutcome::from_err`, keeping `types.rs` the single
/// source of the continuation error contract (T4.1).
fn continuation_err_report(
    task_id: &str,
    err: DelegationError,
    child_conversation_id: Option<i32>,
    agent_type: Option<AgentType>,
) -> DelegationTaskReport {
    let outcome = DelegationOutcome::from_err(err, child_conversation_id);
    report_from_outcome(Some(task_id.to_string()), agent_type, &outcome, None)
}

/// Build a `Failed`/`Canceled` report for a setup error (no task id — setup
/// failed before/around registration, so the LLM has no task to track).
fn report_err(
    agent_type: AgentType,
    err: DelegationError,
    child_conversation_id: Option<i32>,
) -> DelegationTaskReport {
    let outcome = DelegationOutcome::from_err(err, child_conversation_id);
    report_from_outcome(None, Some(agent_type), &outcome, None)
}

/// The `Running` ack returned by `start_delegation` for a backgrounded task.
fn running_ack(
    call_id: String,
    child_conversation_id: i32,
    agent_type: AgentType,
) -> DelegationTaskReport {
    // Embed the literal task_id in the message so it survives clients that only
    // surface the MCP `content` text (not `structuredContent`) — without it the
    // LLM couldn't call get_delegation_status / cancel_delegation.
    let message = format!(
        "Delegation successful. task_id={call_id}. Call get_delegation_status \
         with this id in the task_ids array (optionally wait_ms) to collect the \
         result, or cancel_delegation to stop it."
    );
    DelegationTaskReport {
        task_id: Some(call_id),
        status: TaskStatus::Running,
        child_conversation_id: Some(child_conversation_id),
        agent_type: Some(agent_type),
        text: None,
        error_code: None,
        message: Some(message),
        duration_ms: None,
    }
}

/// How long [`DelegationBroker::get_task_status`] may block before returning the
/// current (possibly still-running) snapshot. Derived by the listener from the
/// MCP tool's `wait_ms`: omitted → [`Immediate`], an explicit `0` → [`Infinite`],
/// any positive value → [`Bounded`] (clamped to the listener's hard ceiling).
///
/// [`Immediate`]: StatusWait::Immediate
/// [`Bounded`]: StatusWait::Bounded
/// [`Infinite`]: StatusWait::Infinite
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusWait {
    /// Return the current snapshot right away — the default poll.
    Immediate,
    /// Block up to this many milliseconds, then return whatever snapshot we have
    /// (the child keeps running past the deadline; the caller re-issues to wait
    /// more).
    Bounded(u64),
    /// Block until the task reaches a terminal state — never time out. Lets a
    /// long-running child be awaited in a single call. A parent disconnect or
    /// cancel also drives the task terminal (and fires the completion signal),
    /// so this never outlives the task itself.
    Infinite,
}

/// Status report for a still-running task.
fn running_report(task_id: &str, task: &RunningTask) -> DelegationTaskReport {
    DelegationTaskReport {
        task_id: Some(task_id.to_string()),
        status: TaskStatus::Running,
        child_conversation_id: Some(task.child_conversation_id),
        agent_type: Some(task.agent_type),
        text: None,
        error_code: None,
        // Bare baseline; `get_task_status` upgrades this to a two-line
        // "Running.\nLatest sub-agent reply: …" when the child has live output.
        message: Some("Running.".to_string()),
        duration_ms: None,
    }
}

/// Status report from a cached completed result.
fn completed_report(task_id: &str, c: &CompletedTask) -> DelegationTaskReport {
    DelegationTaskReport {
        task_id: Some(task_id.to_string()),
        status: c.status,
        child_conversation_id: Some(c.child_conversation_id),
        agent_type: Some(c.agent_type),
        text: c.text.clone(),
        error_code: c.error_code.clone(),
        message: c.message.clone(),
        duration_ms: Some(c.duration_ms),
    }
}

/// Last-known-status report for a released session: the cached completed
/// result when the text cache still holds it, else a status-only shell from
/// the session's domain facts. The message states the release explicitly.
fn last_known_report_locked(
    inner: &PendingInner,
    task_id: &str,
    status: TaskStatus,
    child_conversation_id: i32,
    agent_type: AgentType,
) -> DelegationTaskReport {
    let mut report = if let Some(c) = inner.completed.get(task_id) {
        completed_report(task_id, c)
    } else {
        DelegationTaskReport {
            task_id: Some(task_id.to_string()),
            status,
            child_conversation_id: Some(child_conversation_id),
            agent_type: Some(agent_type),
            text: None,
            error_code: None,
            message: None,
            duration_ms: None,
        }
    };
    report.message = Some(
        "Session released. The child process is freed and this session cannot \
         be continued in this process. Start a new delegate_to_agent for \
         further work."
            .to_string(),
    );
    report
}

/// Status report when a task id isn't known to the caller (never existed,
/// owned by a different parent, or evicted with no DB record).
fn unknown_report(task_id: &str) -> DelegationTaskReport {
    DelegationTaskReport {
        task_id: Some(task_id.to_string()),
        status: TaskStatus::Unknown,
        child_conversation_id: None,
        agent_type: None,
        text: None,
        error_code: None,
        message: Some(
            "Unknown task id — it never existed, isn't owned by this session, \
             or its result was evicted with no stored record."
                .to_string(),
        ),
        duration_ms: None,
    }
}

/// Status report recovered from the DB after the in-memory result was evicted.
/// Carries status only — the full output lives in the child session.
fn db_report(task_id: &str, rec: &ChildStatusRecord) -> DelegationTaskReport {
    DelegationTaskReport {
        task_id: Some(task_id.to_string()),
        status: rec.status,
        child_conversation_id: Some(rec.child_conversation_id),
        agent_type: Some(rec.agent_type),
        text: None,
        error_code: (rec.status == TaskStatus::Canceled).then(|| "canceled".to_string()),
        message: Some(format!(
            "Result no longer cached; open child session {} for the full output.",
            rec.child_conversation_id
        )),
        duration_ms: None,
    }
}

/// Per-id classification captured under the pending lock during a (possibly
/// batched) status query. The async resolution that can't run under the lock —
/// `attach_live_reply` (a different lock) for a running task, `status_from_db`
/// (a DB round-trip) for one not in memory — is deferred to `assemble_reports`
/// AFTER the lock is released, so a status query never nests the pending lock
/// inside another await. This is the same lock-ordering the single-task path
/// has always used; batching just captures it per id.
enum StatusClass {
    /// Terminal/owned-cached, or a cross-parent `unknown` — the report is final.
    Settled(DelegationTaskReport),
    /// Running and owned — the bare running snapshot plus its child connection
    /// id, so `assemble_reports` can attach the latest live reply out of lock.
    Running {
        report: DelegationTaskReport,
        child_connection_id: String,
    },
    /// Neither running nor completed in memory — resolve via the DB fallback in
    /// `assemble_reports`. A not-in-memory id is, for wait purposes, already
    /// settled: it can never transition back to running, so a batch wait need
    /// not park on it (and must not hit the DB on every wake).
    NotInMemory,
}

/// Classify one task id against the in-memory maps while the pending lock is
/// held. Mirrors the single-task resolution order — completed cache (parent
/// scoped) → running set (parent scoped) → not-in-memory — and yields a
/// cross-parent hit as `unknown` so a task owned by another parent never leaks.
fn classify_locked(inner: &PendingInner, parent_connection_id: &str, task_id: &str) -> StatusClass {
    if let Some(c) = inner.completed.get(task_id) {
        if c.parent_connection_id == parent_connection_id {
            return StatusClass::Settled(completed_report(task_id, c));
        }
        return StatusClass::Settled(unknown_report(task_id));
    }
    match inner.running.get(task_id) {
        Some(r) if r.parent_connection_id == parent_connection_id => StatusClass::Running {
            report: running_report(task_id, r),
            child_connection_id: r.child_connection_id.clone(),
        },
        Some(_) => StatusClass::Settled(unknown_report(task_id)),
        None => StatusClass::NotInMemory,
    }
}

/// Map a terminal [`DelegationTaskReport`] back to a [`DelegationOutcome`] for
/// the test-only `handle_request` shim (so pre-async tests keep asserting on
/// the old outcome shape).
#[cfg(any(test, feature = "test-utils"))]
fn report_to_outcome(report: &DelegationTaskReport) -> DelegationOutcome {
    use crate::acp::delegation::types::DelegationSuccess;
    match report.status {
        TaskStatus::Completed => DelegationOutcome::Ok(DelegationSuccess {
            text: report.text.clone().unwrap_or_default(),
            child_conversation_id: report.child_conversation_id.unwrap_or(0),
            child_agent_type: report.agent_type.unwrap_or(AgentType::ClaudeCode),
            turn_count: 1,
            duration_ms: report.duration_ms.unwrap_or(0),
            token_usage: None,
        }),
        // Running never reaches here (the shim loops until terminal); the other
        // states all project onto Err.
        _ => DelegationOutcome::Err {
            code: report
                .error_code
                .clone()
                .unwrap_or_else(|| "canceled".to_string()),
            message: report.message.clone().unwrap_or_default(),
            child_conversation_id: report.child_conversation_id,
        },
    }
}

/// Build the `Canceled { reason }` string for a child that ended without a
/// clean `TurnComplete`, optionally stitching in the terminal `Error` detail.
/// Shared by `cancel_by_child_connection` and `handle_request`'s early-terminal
/// pickup so both surface the same wording.
fn child_canceled_reason(terminal_error: Option<&str>) -> String {
    match terminal_error {
        Some(detail) if !detail.trim().is_empty() => {
            format!("child session ended without TurnComplete: {detail}")
        }
        _ => "child session ended without TurnComplete".to_string(),
    }
}

/// Set of MCP-side `external_handle` tokens for which the companion
/// already received `notifications/cancelled` BEFORE the matching
/// `handle_request` reached the pending-registration phase. Without
/// this pre-cancel buffer, a fast cancel that lands during the
/// pre-check / spawn window would find no entry in `pending`, drop
/// silently, and let the broker proceed to spawn a child the caller
/// no longer wants. `handle_request` consults this set both at entry
/// (so we never even spawn) and immediately after parking the pending
/// entry (so a cancel landing mid-spawn still wins).
///
/// Capped at [`PRE_CANCELED_CAP`] so a misbehaving MCP client (or a
/// pathological cancel-for-unknown-id storm) can't grow the set
/// without bound. Eviction is FIFO via the parallel `order` deque,
/// which is fine because pre-cancels only matter for the short window
/// between the cancel and the late-arriving `handle_request`.
#[derive(Default)]
struct PreCanceledHandles {
    inner: Mutex<PreCanceledState>,
}

#[derive(Default)]
struct PreCanceledState {
    set: HashSet<String>,
    order: VecDeque<String>,
}

const PRE_CANCELED_CAP: usize = 256;

/// Per-parent tracking of `tool_call_id`s that the ACP lifecycle
/// observed firing `delegate_to_agent`. MCP clients (Codex, Claude
/// Code) generally do NOT populate `_meta.tool_use_id` when invoking
/// an MCP tool, so the broker can't read the LLM-issued
/// `tool_use_id` from the wire — we capture it from the parallel ACP
/// `tool_call` event stream instead.
///
/// Each bucket holds two FIFOs under the SAME mutex:
///
/// * `pending` — ids the lifecycle has registered but the matching
///   broker round-trip has not yet claimed. UNKEYED entries are subject
///   to [`PENDING_TOOL_CALL_TTL`] eviction so an anonymous ACP id whose
///   MCP round-trip never arrives can't linger and FIFO-mis-bind a later
///   delegation. KEYED entries carry no count cap: they are drained only
///   by their exact-match claim, by terminal tombstoning
///   (`tombstone_pending_tool_call`), or by per-parent teardown — because
///   the host may serialize a delegation's round-trip arbitrarily far
///   behind earlier long-running ones, so a count cap would drop a
///   still-pending keyed id and orphan its card.
/// * `consumed` — ids that were already claimed by a prior
///   round-trip, or terminal-tombstoned (`tombstone_pending_tool_call`).
///   NEITHER subject to TTL eviction NOR to a per-bucket
///   cap: a delegated child agent may run for minutes to hours, and
///   the host can re-emit the same `tool_call` (e.g. as a `completed`
///   status flip) at the end of that run, so the consumed memory
///   must outlast the entire parent-side tool call lifetime. It is
///   scoped to the parent connection's lifetime instead, cleared by
///   `drop_pending_tool_calls_for_parent` on disconnect. The growth
///   is bounded by how many entries ever register for the parent:
///   `delegate_to_agent` calls (typically tens at most) plus, on
///   Cursor connections, one identity-less "MCP: tool" candidate per
///   MCP call the session makes (see `acp::lifecycle`'s
///   `CURSOR_IDENTITYLESS_MCP_TITLE` registration) — with each
///   `(String, Instant)` entry costing well under 100 bytes, an
///   unbounded set stays comfortable for realistic sessions without
///   OOM risk in the typical operating envelope.
///
/// Co-locating the two halves under one lock makes the
/// claim → mark-consumed pair atomic. A host re-emit racing with the
/// claim cannot observe an empty pending queue AND a consumed memory
/// that does not yet remember the id; consequently it cannot inject
/// a stale duplicate that would mis-bind the next delegation.
#[derive(Default)]
struct ToolCallTracker {
    inner: Mutex<HashMap<String, ToolCallTrackerBucket>>,
}

/// The arguments that uniquely identify a `delegate_to_agent` invocation,
/// used to correlate a parent-side ACP `tool_call` to the matching MCP
/// `tools/call` round-trip. All three fields are values the LLM passed
/// identically to both wire paths, so the triple is the deterministic key
/// when a parent fires several `delegate_to_agent` calls in parallel —
/// matching on `task` alone would swap two calls targeting different agents
/// with the same task, and adding `agent_type` alone would still swap two
/// same-agent/same-task calls aimed at different directories (e.g. "run
/// tests" against `/repo-a` vs `/repo-b`).
///
/// `working_dir` here is the value the LLM EXPLICITLY passed (`None` when
/// omitted), NOT the listener-defaulted spawn directory: the listener
/// defaults a missing MCP `working_dir` to the parent's launch dir, but the
/// ACP `raw_input` omits it then too, so keying on the explicit value keeps
/// both sides symmetric (`None == None`) for the common omitted case while
/// still distinguishing two calls that name different directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationMatchKey {
    pub agent_type: AgentType,
    pub task: String,
    pub working_dir: Option<String>,
}

/// One captured parent-side `delegate_to_agent` tool_call awaiting its
/// matching MCP round-trip.
struct PendingToolCall {
    tool_call_id: String,
    /// The `(agent_type, task, working_dir)` correlation key parsed from the ACP
    /// tool_call's `raw_input`. Matched against the MCP round-trip's own
    /// key so parallel `delegate_to_agent` calls each bind to their own
    /// `tool_call_id` regardless of arrival order — pure arrival-order FIFO
    /// can mis-assign them (or, when one MCP round-trip out-races the
    /// matching ACP event, orphan to a synthetic id). `None` when the host
    /// shipped no parseable `raw_input` at ToolCall time; such entries are
    /// claimable ONLY via the post-budget FIFO fallback
    /// (`take_pending_tool_call`), never the in-loop key-match path.
    match_key: Option<DelegationMatchKey>,
    /// True when the entry came from an identity-less MCP announcement
    /// (Cursor's `"MCP: tool"` + empty input — see
    /// `lifecycle::CURSOR_IDENTITYLESS_MCP_TITLE`). Such an id belongs to
    /// *some* codeg-mcp call whose identity only the companion round-trip can
    /// reveal, so it is additionally claimable by
    /// [`DelegationBroker::rewrite_identityless_tool_call`] (the
    /// `get_delegation_status` / `cancel_delegation` call-time rename), and a
    /// `delegate_to_agent` claim that lands on one triggers the identity
    /// write. Plain unkeyed entries (a delegation invocation whose args were
    /// unparseable) keep `false` and are never renamed.
    identityless: bool,
    registered_at: Instant,
}

#[derive(Default)]
struct ToolCallTrackerBucket {
    pending: VecDeque<PendingToolCall>,
    consumed: VecDeque<(String, Instant)>,
    /// Sticky "this parent has EVER announced an identity-less MCP call" flag.
    /// Gates [`DelegationBroker::rewrite_identityless_tool_call`]'s poll: on
    /// hosts that ship real identities (Claude/Codex/…) no identity-less
    /// entry ever registers, the flag stays `false`, and every status/cancel
    /// round-trip skips the rename at zero cost instead of burning the poll
    /// budget. Never cleared on claim — only with the bucket on teardown.
    identityless_seen: bool,
}

/// Maximum age before a `pending` entry is discarded as stale — but ONLY for
/// UNKEYED entries (anonymous, arrival-order correlated). KEYED entries are
/// retained regardless of age: each is claimed solely by an exact key match,
/// so it can't mis-bind a later delegation, and its MCP round-trip may be
/// serialized arbitrarily far behind earlier long-running delegations (Claude
/// Code runs parallel `delegate_to_agent` calls one-at-a-time — observed gap
/// 77 s). See the retain block in `take_matching_tool_call_at`.
/// 60 s comfortably covers the ACP→MCP race for the unkeyed case (<5 ms
/// typical) while still GC'ing a forgotten anonymous id before it can
/// FIFO-mis-bind a subsequent unkeyed delegation.
///
/// The `consumed` side has no TTL — see [`ToolCallTrackerBucket`] — because
/// long-running delegations can re-emit the parent-side `tool_call` well past
/// this window.
const PENDING_TOOL_CALL_TTL: Duration = Duration::from_secs(60);

/// Poll cadence and budget used by `claim_pending_tool_call_with_brief_wait`
/// to correlate an MCP `delegate_to_agent` round-trip to its parent-side
/// ACP `tool_call_id`. The exact-match path returns instantly; this budget is
/// spent while waiting for THIS delegation's own `tool_call` to register (or to
/// backfill its key onto an already-registered entry) so we bind by exact match
/// instead of stealing a parallel sibling's id, or while no claimable id has
/// arrived yet. Unkeyed entries are never claimed in-loop — arrival-order FIFO
/// is deferred to the post-budget last resort, which runs only after the caller
/// has waited the full budget (the correct clock for "this delegation has no
/// key coming"), so a round-trip can't grab a sibling's not-yet-keyed id
/// mid-race.
///
/// 200 × 10 ms = 2 s. This budget only matters when the MCP round-trip
/// out-races its own ACP `tool_call` registration — i.e. the `tools/call`
/// reaches the broker before the in-process `session/update(tool_call)` (and
/// any slightly-later `ToolCallUpdate` carrying the `agent_type`/`task` args)
/// has registered the key. That race is sub-5ms locally; the headroom covers
/// busier hosts and split arg streaming. The wait is invisible in the happy
/// path (it returns the instant the key matches) and negligible against the
/// multi-second-to-minutes child run it precedes.
///
/// NOTE: the budget is NOT what protects a *serialized* second delegation
/// whose round-trip lands many seconds after its tool_call registered (Claude
/// Code runs parallel `delegate_to_agent` calls one-at-a-time, so the 2nd may
/// arrive minutes later). That id is already registered and waiting — the
/// thing that used to orphan it was age-eviction, now fixed by retaining keyed
/// entries indefinitely (see `take_matching_tool_call_at`'s retain
/// block). A host that emits no observable ACP `tool_call` at all still falls
/// through to the synthetic id after the budget, exactly as before.
const CLAIM_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CLAIM_POLL_ATTEMPTS: usize = 200;

/// Poll budget for the status/cancel call-time rename
/// ([`DelegationBroker::rewrite_identityless_tool_call`]): 30 × 10 ms = 300 ms.
/// Much shorter than the delegate claim budget because the rename is
/// best-effort (a miss falls back to the completion-time result sniff) and the
/// poll delays the status snapshot the LLM is waiting on. It only has to
/// absorb the in-process ordering race between the ACP announcement landing in
/// the lifecycle dispatcher and the companion's UDS round-trip reaching the
/// listener — sub-5 ms in practice; the sticky `identityless_seen` gate keeps
/// hosts that never announce identity-less calls at zero cost.
const IDENTITYLESS_RENAME_POLL_ATTEMPTS: usize = 30;

/// The broker is intentionally `Clone` (cheap — only `Arc`s inside) so
/// listener/handler code can hand copies to spawned tasks without lifetime
/// gymnastics.
#[derive(Clone)]
pub struct DelegationBroker {
    spawner: Arc<dyn ConnectionSpawner>,
    depth_lookup: Arc<dyn ConversationDepthLookup>,
    /// Writer for `meta["codeg.delegation"]` on the parent's active
    /// `delegate_to_agent` ToolCallState. Defaults to a no-op so tests
    /// that aren't exercising the meta lifecycle don't need to wire
    /// anything; production constructs the broker with the
    /// `ConnectionManagerMetaWriter` via `with_writers`.
    meta_writer: Arc<dyn DelegationMetaWriter>,
    /// Emitter for `AcpEvent::DelegationCompleted` against the parent
    /// connection's event stream. Same Noop/Mock/Production scheme as
    /// the meta writer — production wires `ConnectionManagerEventEmitter`
    /// via `with_writers`; tests that don't observe the event lifecycle
    /// take the default Noop.
    event_emitter: Arc<dyn DelegationEventEmitter>,
    /// DB fallback for `get_delegation_status` / `cancel_delegation` once a
    /// task's result aged out of the in-memory completed-cache. Defaults to a
    /// no-op ("unknown"); production wires `DbChildStatusLookup` via
    /// `with_status_lookup`.
    status_lookup: Arc<dyn ChildStatusLookup>,
    /// Peeks a still-running child's live session for a one-line progress hint,
    /// used to enrich `get_delegation_status`'s running report. Defaults to a
    /// no-op ("no hint"); production wires `ConnectionManagerLiveReplyLookup` via
    /// `with_live_reply_lookup`.
    live_reply_lookup: Arc<dyn ChildLiveReplyLookup>,
    pending: Arc<PendingCalls>,
    tool_calls: Arc<ToolCallTracker>,
    pre_canceled_handles: Arc<PreCanceledHandles>,
    config: Arc<Mutex<DelegationConfig>>,
    /// Woken after every terminal `record_completed` so a `get_delegation_status`
    /// long-poll wakes the instant its task finishes instead of busy-polling.
    result_notify: Arc<Notify>,
}

impl DelegationBroker {
    pub fn new(
        spawner: Arc<dyn ConnectionSpawner>,
        depth_lookup: Arc<dyn ConversationDepthLookup>,
    ) -> Self {
        Self::with_writers(
            spawner,
            depth_lookup,
            Arc::new(NoopMetaWriter) as Arc<dyn DelegationMetaWriter>,
            Arc::new(NoopEventEmitter) as Arc<dyn DelegationEventEmitter>,
        )
    }

    /// Test-only constructor that injects a meta writer but keeps the
    /// default Noop event emitter. Retained so existing meta-focused
    /// tests don't have to mention the emitter parameter. New callsites
    /// (and production wiring) should prefer `with_writers`.
    pub fn with_meta_writer(
        spawner: Arc<dyn ConnectionSpawner>,
        depth_lookup: Arc<dyn ConversationDepthLookup>,
        meta_writer: Arc<dyn DelegationMetaWriter>,
    ) -> Self {
        Self::with_writers(
            spawner,
            depth_lookup,
            meta_writer,
            Arc::new(NoopEventEmitter) as Arc<dyn DelegationEventEmitter>,
        )
    }

    /// Production-grade constructor wiring the broker to both a real
    /// meta writer (`ConnectionManagerMetaWriter`) AND an event emitter
    /// (`ConnectionManagerEventEmitter`). Tests that observe the full
    /// lifecycle (meta writes + DelegationCompleted emits) should use
    /// this with `MockMetaWriter` + `MockEventEmitter`.
    pub fn with_writers(
        spawner: Arc<dyn ConnectionSpawner>,
        depth_lookup: Arc<dyn ConversationDepthLookup>,
        meta_writer: Arc<dyn DelegationMetaWriter>,
        event_emitter: Arc<dyn DelegationEventEmitter>,
    ) -> Self {
        Self {
            spawner,
            depth_lookup,
            meta_writer,
            event_emitter,
            status_lookup: Arc::new(NoopChildStatusLookup),
            live_reply_lookup: Arc::new(NoopChildLiveReplyLookup),
            pending: Arc::new(PendingCalls::default()),
            tool_calls: Arc::new(ToolCallTracker::default()),
            pre_canceled_handles: Arc::new(PreCanceledHandles::default()),
            config: Arc::new(Mutex::new(DelegationConfig::default())),
            result_notify: Arc::new(Notify::new()),
        }
    }

    /// Replace the DB status fallback used by `get_delegation_status` /
    /// `cancel_delegation` for tasks evicted from the in-memory completed-cache.
    /// Builder-style so the production wiring can layer it onto `with_writers`
    /// without growing that constructor's arity, and tests can opt in.
    pub fn with_status_lookup(mut self, status_lookup: Arc<dyn ChildStatusLookup>) -> Self {
        self.status_lookup = status_lookup;
        self
    }

    /// Replace the live-reply lookup used to enrich `get_delegation_status`'s
    /// running report with the child's latest one-line progress. Builder-style,
    /// layered onto `with_writers` by the production wiring; tests opt in with a
    /// `MockChildLiveReplyLookup`.
    pub fn with_live_reply_lookup(
        mut self,
        live_reply_lookup: Arc<dyn ChildLiveReplyLookup>,
    ) -> Self {
        self.live_reply_lookup = live_reply_lookup;
        self
    }

    /// Record a parent ACP `tool_call_id` whose title indicates the LLM is
    /// invoking `delegate_to_agent`. The next broker round-trip from the
    /// same `parent_connection_id` will claim this id as its
    /// `parent_tool_use_id`. Bounded FIFO per connection.
    ///
    /// Two-tier dedupe against host re-emits of `sessionUpdate(tool_call)`
    /// (some hosts use the non-update variant to ship status flips and
    /// late-arriving `raw_input` chunks):
    ///
    /// 1. **In-queue**: if the id is still waiting to be claimed, drop
    ///    the re-emit — the first push will be consumed by the matching
    ///    MCP round-trip.
    /// 2. **Recently consumed**: if the id was already claimed for an
    ///    earlier delegation on the same parent, drop the re-emit —
    ///    otherwise it would sit in the queue as a stale id and mis-
    ///    bind the **next** delegation's MCP round-trip. The consumed
    ///    memory persists for the parent connection's lifetime (no
    ///    TTL, no cap) so a host re-emit at terminal status flip is
    ///    still rejected even if the delegation ran for hours.
    pub async fn register_pending_tool_call(
        &self,
        parent_connection_id: &str,
        tool_call_id: String,
    ) {
        self.register_pending_tool_call_with_key_at(
            parent_connection_id,
            tool_call_id,
            None,
            false,
            Instant::now(),
        )
        .await;
    }

    /// Register an id announced with NO identity at all — Cursor's
    /// `"MCP: tool"` + empty-input shape, where the wire never reveals which
    /// MCP tool the call is. The entry is unkeyed (claimable by the
    /// post-budget FIFO fallback, exactly like a keyless delegation
    /// invocation) and additionally marked `identityless`, which (a) makes it
    /// claimable by the status/cancel call-time rename
    /// ([`Self::rewrite_identityless_tool_call`]) and (b) triggers the
    /// identity write when a `delegate_to_agent` claim lands on it.
    pub async fn register_identityless_tool_call(
        &self,
        parent_connection_id: &str,
        tool_call_id: String,
    ) {
        self.register_pending_tool_call_with_key_at(
            parent_connection_id,
            tool_call_id,
            None,
            true,
            Instant::now(),
        )
        .await;
    }

    /// `register_pending_tool_call` that also records the
    /// `(agent_type, task, working_dir)` correlation key parsed from the
    /// tool_call's `raw_input`. The key lets
    /// the broker bind this id to its matching MCP round-trip deterministically
    /// for parallel `delegate_to_agent` calls that pure arrival-order FIFO can
    /// mis-assign. Production registration (from the ACP lifecycle dispatcher)
    /// goes through here.
    pub async fn register_pending_tool_call_with_key(
        &self,
        parent_connection_id: &str,
        tool_call_id: String,
        match_key: Option<DelegationMatchKey>,
    ) {
        self.register_pending_tool_call_with_key_at(
            parent_connection_id,
            tool_call_id,
            match_key,
            false,
            Instant::now(),
        )
        .await;
    }

    /// Core registration. Holds the [`ToolCallTracker`] mutex across both
    /// dedupe tiers AND the push so no concurrent `take` can split the
    /// "queue empty + not yet recorded as consumed" window where a host
    /// re-emit could otherwise inject a stale duplicate.
    ///
    /// Two-tier dedupe against host re-emits of `sessionUpdate(tool_call)`
    /// (some hosts use the non-update variant to ship status flips and
    /// late-arriving `raw_input` chunks):
    ///
    /// 1. **Recently consumed**: if the id was already claimed for an
    ///    earlier delegation on the same parent, drop the re-emit —
    ///    otherwise it would sit in the queue as a stale id and mis-bind
    ///    the **next** delegation's MCP round-trip. The consumed memory
    ///    persists for the parent connection's lifetime (no TTL, no cap)
    ///    so a host re-emit at terminal status flip is still rejected
    ///    even if the delegation ran for hours.
    /// 2. **In-queue**: if the id is still waiting to be claimed, drop the
    ///    re-emit rather than push a duplicate — EXCEPT we backfill the
    ///    `match_key` onto an entry registered without one. This is the common
    ///    case for hosts that emit an arg-less initial `ToolCall` and ship the
    ///    `agent_type`/`task` arguments on a following `ToolCallUpdate`: the
    ///    lifecycle dispatcher registers BOTH variants (see
    ///    `register_delegation_tool_call_from_event`), so the first call lands
    ///    here unkeyed and the later update re-enters and back-fills the key.
    ///    Keying the entry this way is what lets it survive past the unkeyed
    ///    GC TTL (see `take_matching_tool_call_at`'s retain block).
    async fn register_pending_tool_call_with_key_at(
        &self,
        parent_connection_id: &str,
        tool_call_id: String,
        match_key: Option<DelegationMatchKey>,
        identityless: bool,
        now: Instant,
    ) {
        let mut map = self.tool_calls.inner.lock().await;
        let bucket = map.entry(parent_connection_id.to_string()).or_default();
        if identityless {
            bucket.identityless_seen = true;
        }
        // Tier 1: recently consumed. No TTL — the consumed memory must
        // outlast the entire parent-side tool call lifetime (minutes
        // to hours) so a host re-emit at terminal status flip is
        // still rejected. See `ToolCallTrackerBucket` docs.
        if bucket.consumed.iter().any(|(id, _)| id == &tool_call_id) {
            tracing::info!(
                "[delegation] dropping ACP tool_call_id={tool_call_id} on conn={parent_connection_id} (already consumed by an earlier delegation)"
            );
            return;
        }
        // Tier 2: in-queue. A re-emit of an already-queued id: adopt the
        // LATEST parseable key rather than only back-filling a missing one.
        // Hosts stream `raw_input` incrementally and the MCP side keys on the
        // FINAL arguments, so a later `ToolCallUpdate` that completes the key
        // (e.g. adds an explicit `working_dir` the first parse lacked) must
        // REPLACE the earlier `(agent, task, None)` key — otherwise the MCP
        // claim keys on `(agent, task, Some(dir))`, fails to match the stale
        // `None`, refuses the keyed fallback, and orphans to a synthetic id
        // (the very dead-card failure this whole change fixes). An arg-less or
        // identical re-emit changes nothing and is dropped as a duplicate.
        if let Some(existing) = bucket
            .pending
            .iter_mut()
            .find(|p| p.tool_call_id == tool_call_id)
        {
            match match_key {
                Some(key) if existing.match_key.as_ref() != Some(&key) => {
                    existing.match_key = Some(key);
                }
                _ => {
                    tracing::info!(
                        "[delegation] dropping duplicate ACP tool_call_id={tool_call_id} on conn={parent_connection_id}"
                    );
                }
            }
            return;
        }
        bucket.pending.push_back(PendingToolCall {
            tool_call_id,
            match_key,
            identityless,
            registered_at: now,
        });
    }

    /// Pop the oldest pending `tool_call_id` for the given parent, if any.
    /// Skips entries older than [`PENDING_TOOL_CALL_TTL`] so an ACP id whose
    /// matching MCP round-trip never arrived cannot mis-bind a later
    /// delegation. Mutates the queue in-place; the bucket is removed once
    /// drained.
    pub async fn take_pending_tool_call(&self, parent_connection_id: &str) -> Option<String> {
        self.take_pending_tool_call_at(parent_connection_id, Instant::now())
            .await
            .map(|(id, _)| id)
    }

    /// `take_pending_tool_call` with an injected "as of" instant. The
    /// public entry point pins it to `Instant::now()`; tests can supply
    /// a future instant to exercise TTL eviction without sleeping past
    /// [`PENDING_TOOL_CALL_TTL`].
    ///
    /// Anonymous claim: returns the oldest *unkeyed* pending id (plus its
    /// `identityless` mark, so the delegate-claim path knows to restore the
    /// call's identity), GC'ing stale unkeyed entries along the way. KEYED
    /// entries are stepped over and left in place — they're reserved for
    /// their exact-key-match round-trip and must never be handed out by this
    /// arrival-order path (doing so would steal an in-flight delegation's
    /// id). Returns `None` when no unkeyed entry is claimable, even if keyed
    /// entries remain.
    async fn take_pending_tool_call_at(
        &self,
        parent_connection_id: &str,
        now: Instant,
    ) -> Option<(String, bool)> {
        self.take_unkeyed_tool_call_at(parent_connection_id, now, false)
            .await
    }

    /// Claim the oldest pending entry that was registered as IDENTITY-LESS
    /// (Cursor's `"MCP: tool"` announcements) — used by the status/cancel
    /// call-time rename. Unlike `take_pending_tool_call_at` this NEVER hands
    /// out a plain unkeyed entry: that one is a delegation invocation whose
    /// args merely didn't parse, and renaming it to a status tool would
    /// mislabel a real delegate card.
    async fn take_identityless_tool_call(&self, parent_connection_id: &str) -> Option<String> {
        self.take_unkeyed_tool_call_at(parent_connection_id, Instant::now(), true)
            .await
            .map(|(id, _)| id)
    }

    /// Shared FIFO claim over unkeyed entries. `identityless_only` restricts
    /// eligibility to entries carrying the identity-less mark; keyed entries
    /// are always stepped over (reserved for their exact-key-match
    /// round-trip), and stale unkeyed entries are GC'd along the way
    /// regardless of the filter so the queue stays bounded from either
    /// caller.
    async fn take_unkeyed_tool_call_at(
        &self,
        parent_connection_id: &str,
        now: Instant,
        identityless_only: bool,
    ) -> Option<(String, bool)> {
        let mut map = self.tool_calls.inner.lock().await;
        let bucket = map.get_mut(parent_connection_id)?;
        // Anonymous claim (post-budget last resort + legacy single-delegation
        // path): only UNKEYED entries are eligible. A keyed entry identifies a
        // specific in-flight delegation and is claimable ONLY by its
        // exact-key-match round-trip; grabbing it here would steal that
        // delegation's id and make IT the dead card. Walk oldest→newest,
        // GC'ing stale unkeyed entries and stepping over keyed ones, until we
        // find the oldest fresh eligible id. When only keyed siblings remain we
        // return `None` — the delegate caller then mints a synthetic id rather
        // than mis-binding a sibling.
        let mut claimed: Option<(String, bool)> = None;
        let mut idx = 0;
        while idx < bucket.pending.len() {
            if bucket.pending[idx].match_key.is_some() {
                idx += 1; // keyed: leave it for its exact-match round-trip
                continue;
            }
            if now.duration_since(bucket.pending[idx].registered_at) > PENDING_TOOL_CALL_TTL {
                if let Some(stale) = bucket.pending.remove(idx) {
                    let age_secs = now.duration_since(stale.registered_at).as_secs();
                    tracing::info!(
                        "[delegation] evicting stale UNKEYED ACP tool_call_id={} (age={age_secs}s) on conn={parent_connection_id}",
                        stale.tool_call_id
                    );
                }
                // `remove` shifted later entries left into `idx`; re-check it.
                continue;
            }
            if identityless_only && !bucket.pending[idx].identityless {
                idx += 1; // plain unkeyed: reserved for the delegate FIFO path
                continue;
            }
            claimed = bucket
                .pending
                .remove(idx)
                .map(|p| (p.tool_call_id, p.identityless));
            break;
        }
        // Same mutex span: record the claim into the consumed memory so
        // a concurrent re-register cannot observe "pending empty AND
        // consumed missing" and inject a stale duplicate. Consumed
        // entries persist for the whole parent connection lifetime
        // (no TTL, no cap — see `ToolCallTrackerBucket`) and are only
        // released when the parent disconnects.
        if let Some((id, _)) = &claimed {
            bucket.consumed.push_back((id.clone(), now));
        }
        if bucket.pending.is_empty() && bucket.consumed.is_empty() && !bucket.identityless_seen {
            map.remove(parent_connection_id);
        }
        claimed
    }

    /// Whether this parent has EVER registered an identity-less candidate —
    /// the zero-cost gate for [`Self::rewrite_identityless_tool_call`]'s poll.
    async fn has_seen_identityless_tool_calls(&self, parent_connection_id: &str) -> bool {
        self.tool_calls
            .inner
            .lock()
            .await
            .get(parent_connection_id)
            .is_some_and(|b| b.identityless_seen)
    }

    /// Restore the identity of a pending identity-less MCP tool call at
    /// companion round-trip time: claim the oldest such candidate for this
    /// parent and rewrite its live `title` + `raw_input` to the actual
    /// codeg-mcp tool (`get_delegation_status` / `cancel_delegation`) with
    /// its real arguments — flipping the card from the generic "MCP: tool"
    /// WHILE the call runs, instead of only at the completion-time result
    /// sniff (which, for a `wait_ms`-blocked status long-poll, can be minutes
    /// away).
    ///
    /// Best-effort by design: on hosts that ship real identities the sticky
    /// `identityless_seen` gate skips everything at zero cost; on an
    /// identity-less host whose announcement hasn't registered yet, a short
    /// bounded poll (well under the ACP-vs-companion race in practice)
    /// absorbs the ordering, and a miss simply leaves the rename to the
    /// completion sniff. Mis-pairing under PARALLEL identity-less calls in
    /// one burst is the same accepted arrival-order residual as the delegate
    /// FIFO fallback — Cursor executes tool calls sequentially, so announce
    /// order matches round-trip order in practice.
    pub async fn rewrite_identityless_tool_call(
        &self,
        parent_connection_id: &str,
        title: &str,
        raw_input: serde_json::Value,
    ) -> Option<String> {
        if !self
            .has_seen_identityless_tool_calls(parent_connection_id)
            .await
        {
            return None;
        }
        for attempt in 0..IDENTITYLESS_RENAME_POLL_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(CLAIM_POLL_INTERVAL).await;
            }
            if let Some(id) = self.take_identityless_tool_call(parent_connection_id).await {
                tracing::info!(
                    "[delegation] renaming identity-less tool_call_id={id} on conn={parent_connection_id} to {title}"
                );
                self.meta_writer
                    .write_tool_call_identity(parent_connection_id, &id, title, raw_input)
                    .await;
                return Some(id);
            }
        }
        None
    }

    /// Claim the pending `tool_call_id` for `parent_connection_id` whose
    /// recorded key matches `key` (exact `(agent_type, task, working_dir)`
    /// match). This is the ONLY claim this method makes — it never hands out an
    /// unkeyed entry, because an unkeyed entry may belong to a *different*
    /// parallel delegation whose round-trip simply hasn't registered (or keyed)
    /// its `tool_call` yet, and claiming it by arrival order would steal that
    /// sibling's id. Returns `None` (so the caller keeps polling) whenever no
    /// entry's key matches — whether keyed siblings or only unkeyed entries are
    /// present.
    ///
    /// Arrival-order FIFO for genuinely keyless hosts is deferred to the
    /// post-budget last resort `take_pending_tool_call`, which runs only after
    /// the caller has waited its full budget (see
    /// `claim_pending_tool_call_with_brief_wait`) — the correct clock for "no
    /// key is coming", since a host can serialize a round-trip arbitrarily far
    /// behind its `tool_call` registration, so the entry's own age can never
    /// prove a key won't still arrive. Evicts stale *unkeyed* entries along the
    /// way; keyed entries are retained regardless of age (their round-trip may
    /// be serialized far behind earlier delegations — see the retain block) and
    /// an exact key match claims them at any age.
    pub async fn take_matching_tool_call(
        &self,
        parent_connection_id: &str,
        key: &DelegationMatchKey,
    ) -> Option<String> {
        self.take_matching_tool_call_at(parent_connection_id, key, Instant::now())
            .await
    }

    /// `take_matching_tool_call` with an injected "as of"
    /// instant for TTL tests.
    async fn take_matching_tool_call_at(
        &self,
        parent_connection_id: &str,
        key: &DelegationMatchKey,
        now: Instant,
    ) -> Option<String> {
        let mut map = self.tool_calls.inner.lock().await;
        let bucket = map.get_mut(parent_connection_id)?;

        // Evict every stale UNKEYED entry up front. The key-match scan below
        // ignores unkeyed entries anyway (they carry no key to match), but
        // GC'ing here keeps the queue bounded during the poll loop and
        // consistent with `take_pending_tool_call_at`'s view, so the
        // post-budget last resort never hands out an aged-out id. Mirrors that
        // TTL skip but covers entries at any position (not just the front).
        bucket.pending.retain(|p| {
            // Keyed entries are NEVER aged out. Each identifies one specific
            // `delegate_to_agent` invocation and is claimable ONLY by an exact
            // key match (never by FIFO — see below), so it cannot mis-bind a
            // different delegation no matter how old it gets. And it MUST
            // survive until its MCP round-trip arrives, which the host may
            // serialize arbitrarily far behind earlier long-running
            // delegations: Claude Code runs parallel `delegate_to_agent` calls
            // SEQUENTIALLY, so the 2nd call's round-trip only fires after the
            // 1st child finishes. Observed in the wild — a 2nd delegation whose
            // tool_call registered, then waited 77s (past the old 60s TTL) for
            // its round-trip while the 1st ran; age-evicting it here orphaned
            // it to a synthetic id and left the parent card stuck on
            // "sub-agent running…". Only UNKEYED (anonymous, arrival-order
            // correlated) entries keep the age-based GC, since a stale one
            // could be mis-claimed via the FIFO path. Keyed memory stays bounded
            // by exact-match claim, terminal tombstoning, and
            // `drop_pending_tool_calls_for_parent` on connection teardown — not
            // by this TTL.
            if p.match_key.is_some() {
                return true;
            }
            let fresh = now.duration_since(p.registered_at) <= PENDING_TOOL_CALL_TTL;
            if !fresh {
                let age_secs = now.duration_since(p.registered_at).as_secs();
                tracing::info!(
                    "[delegation] evicting stale UNKEYED ACP tool_call_id={} (age={age_secs}s) on conn={parent_connection_id}",
                    p.tool_call_id
                );
            }
            fresh
        });

        let claimed = if let Some(pos) = bucket
            .pending
            .iter()
            .position(|p| p.match_key.as_ref() == Some(key))
        {
            // Exact (agent_type, task) match: deterministic correlation
            // regardless of ACP-vs-MCP arrival order or how many delegations
            // are in flight.
            bucket.pending.remove(pos).map(|p| p.tool_call_id)
        } else {
            // No exact key match. We deliberately do NOT claim an unkeyed entry
            // here — not even the oldest, not even the only one. An unkeyed
            // pending entry may belong to a DIFFERENT parallel delegation whose
            // own round-trip hasn't yet registered (or keyed) its `tool_call`,
            // and claiming it by arrival order would steal that sibling's id —
            // the mis-bind this machinery exists to prevent.
            //
            // Crucially, the ENTRY's age is the wrong clock for "no key is
            // coming": a host can serialize a round-trip arbitrarily far behind
            // its `tool_call` registration (see the retain block / the
            // `keyed_entry_survives_past_ttl` case), so even an old lone unkeyed
            // entry can still be a sibling's. The CALLER's own wait is the right
            // clock. So return `None` and let
            // `claim_pending_tool_call_with_brief_wait` poll: if this
            // delegation's key lands (initial register or a later backfill) we
            // bind by the exact match above; only after the caller has spent the
            // FULL budget does its post-budget last resort
            // (`take_pending_tool_call`) claim the oldest unkeyed id in arrival
            // order — the best a genuinely keyless host allows, and the point at
            // which waiting longer cannot improve correlation.
            None
        };

        if let Some(id) = &claimed {
            bucket.consumed.push_back((id.clone(), now));
        }
        if bucket.pending.is_empty() && bucket.consumed.is_empty() {
            map.remove(parent_connection_id);
        }
        claimed
    }

    /// Consume an explicit `parent_tool_use_id` that the MCP client supplied
    /// directly via `_meta.tool_use_id` (the precise-binding path; most clients
    /// omit it). In that case `handle_request` does NOT run the claim path, so
    /// the matching pending entry the lifecycle dispatcher registered off the
    /// parent's ACP stream would otherwise never be consumed — and because
    /// keyed entries are now retained indefinitely, it would linger and could
    /// be mis-claimed by a *later* delegation sharing the same
    /// `(agent_type, task, working_dir)` key, retargeting that delegation's
    /// writes/events at the wrong (already-handled) card.
    ///
    /// Remove the entry from the pending queue AND record the id as consumed.
    /// Recording consumed also covers the MCP-before-ACP race: a later ACP
    /// registration for the same id is dropped by the Tier-1 consumed check in
    /// `register_pending_tool_call_with_key_at`, so the entry can't reappear
    /// regardless of arrival order.
    async fn consume_explicit_tool_call(&self, parent_connection_id: &str, tool_call_id: &str) {
        let mut map = self.tool_calls.inner.lock().await;
        let bucket = map.entry(parent_connection_id.to_string()).or_default();
        bucket.pending.retain(|p| p.tool_call_id != tool_call_id);
        if !bucket.consumed.iter().any(|(id, _)| id == tool_call_id) {
            bucket
                .consumed
                .push_back((tool_call_id.to_string(), Instant::now()));
        }
    }

    /// Tombstone a parent `tool_call_id` whose `delegate_to_agent` reached a
    /// TERMINAL ACP status (`completed`/`failed`) so a stale keyed pending entry
    /// can't mis-bind a later delegation. The lifecycle dispatcher calls this
    /// from its terminal-`ToolCallUpdate` branch, keyed on `tool_call_id`
    /// (a bare terminal update carries no parseable key).
    ///
    /// The hazard: keyed pending entries are retained regardless of age (see the
    /// retain block in `take_matching_tool_call_at`), so if a `delegate_to_agent`
    /// tool call goes terminal without its MCP round-trip ever reaching the
    /// broker (the call failed, the turn was interrupted, the companion never
    /// dispatched), its entry would linger forever and a LATER delegation sharing
    /// the same `(agent_type, task, working_dir)` key would claim this dead id,
    /// retargeting its writes/events at the wrong card. Same hazard
    /// `consume_explicit_tool_call` guards on the explicit-id path; this is its
    /// terminal-status sibling.
    ///
    /// Safe synchronously, no grace window: a terminal `completed` can only
    /// arrive AFTER the round-trip's claim already removed the entry (the ack
    /// that claim produces is what lets the parent's tool call return), and a
    /// serialized sibling still awaiting its (observed 77s-late) round-trip is
    /// NON-terminal while it waits — so this never evicts a live entry.
    ///
    /// Records `consumed` ONLY when an entry was actually removed: this runs for
    /// EVERY terminal tool-call update (the vast majority are non-delegations),
    /// and `consumed` has no TTL/cap, so recording unconditionally would grow it
    /// with every completed tool call. Recording on a real removal still drops an
    /// out-of-order re-registration of the same id via the Tier-1 consumed check
    /// in `register_pending_tool_call_with_key_at`. Returns whether an entry was
    /// removed (for the dispatcher's gated log).
    pub async fn tombstone_pending_tool_call(
        &self,
        parent_connection_id: &str,
        tool_call_id: &str,
    ) -> bool {
        let mut map = self.tool_calls.inner.lock().await;
        // No bucket → nothing registered for this parent; nothing to tombstone
        // and nothing to record (unlike `consume_explicit_tool_call`, no
        // MCP-before-ACP race can land a terminal status before registration on
        // the single ordered ACP stream, so we never pre-create a bucket here).
        let Some(bucket) = map.get_mut(parent_connection_id) else {
            return false;
        };
        let before = bucket.pending.len();
        bucket.pending.retain(|p| p.tool_call_id != tool_call_id);
        let removed = bucket.pending.len() != before;
        if removed && !bucket.consumed.iter().any(|(id, _)| id == tool_call_id) {
            bucket
                .consumed
                .push_back((tool_call_id.to_string(), Instant::now()));
        }
        removed
    }

    /// Correlate an MCP `delegate_to_agent` round-trip to the parent's
    /// real ACP `tool_call_id`, polling briefly to absorb the race between
    /// two independent arrival paths for the same invocation:
    ///
    ///   * ACP `session/update(tool_call)` → in-process bus → lifecycle
    ///     dispatcher → `register_pending_tool_call_with_key`
    ///   * MCP `tools/call` → stdio round-trip → companion → `handle_request`
    ///
    /// Correlation is by the `(agent_type, task, working_dir)` key (carried in
    /// both the ACP `raw_input` and the MCP call), so several `delegate_to_agent`
    /// calls firing in parallel each bind to their own `tool_call_id`
    /// regardless of arrival order — pure FIFO mis-assigned them (swapping
    /// the child shown under each card) or, when one MCP round-trip out-raced
    /// its ACP event, orphaned the loser to a synthetic `delegation-<uuid>`
    /// (the parent UI then never paints "view session" and the card hangs on
    /// "sub-agent running…", because the frontend keys its binding map by
    /// the agent's real `tool_call_id`).
    ///
    /// As a last resort after the budget — and the ONLY place arrival-order
    /// FIFO is applied — claim the oldest unkeyed id, so a sibling whose
    /// registration was unusually delayed, or a genuinely keyless host, still
    /// yields a *real* id rather than a synthetic one. Deferring FIFO until the
    /// full budget has elapsed is what makes it safe: in-loop we bind ONLY by
    /// exact key match, so a round-trip can't FIFO-steal a sibling's
    /// not-yet-keyed id while that sibling's own registration is still in
    /// flight (the entry's age is no proof a key won't still arrive). A
    /// synthetic id only results when no unkeyed id is claimable for the whole
    /// budget — only keyed siblings remain, or the queue stays genuinely empty.
    /// Returns the claimed id plus its `identityless` mark — `true` only for
    /// the post-budget FIFO claim of an identity-less announcement (Cursor),
    /// which tells `start_delegation` to restore the call's identity on the
    /// live tool call. Exact-key claims are by definition NOT identity-less
    /// (the key was parsed from the wire's own `raw_input`).
    async fn claim_pending_tool_call_with_brief_wait(
        &self,
        parent_connection_id: &str,
        key: &DelegationMatchKey,
    ) -> Option<(String, bool)> {
        if let Some(id) = self
            .take_matching_tool_call(parent_connection_id, key)
            .await
        {
            return Some((id, false));
        }
        for _ in 0..CLAIM_POLL_ATTEMPTS {
            tokio::time::sleep(CLAIM_POLL_INTERVAL).await;
            if let Some(id) = self
                .take_matching_tool_call(parent_connection_id, key)
                .await
            {
                return Some((id, false));
            }
        }
        // Budget exhausted with no key match. As a last resort claim the
        // oldest UNKEYED pending id (a host that shipped no parseable
        // `raw_input`, or a mixed-shape race) — a real id beats a synthetic
        // placeholder that orphans the parent UI binding. Crucially this
        // never claims a KEYED entry: those belong to specific in-flight
        // delegations and are reserved for their own exact-key-match
        // round-trip, so when only keyed siblings remain the caller falls
        // through to a synthetic id rather than stealing a sibling's binding
        // (which would just move the dead card from one delegation to another).
        self.take_pending_tool_call_at(parent_connection_id, Instant::now())
            .await
    }

    /// Remove `handle` from the pre-cancel set, returning whether it was
    /// present. Used by `handle_request` at two checkpoints (entry + just
    /// after pending registration) so a cancel that lost the race with the
    /// MCP round-trip still wins. The set is single-shot per handle —
    /// taking it here means a subsequent `cancel_by_external_handle` will
    /// have to find the pending entry on its own.
    async fn take_pre_canceled_handle(&self, handle: &str) -> bool {
        let mut state = self.pre_canceled_handles.inner.lock().await;
        if state.set.remove(handle) {
            // Best-effort companion-side cleanup of `order` so a later
            // FIFO eviction doesn't burn a slot. Linear scan is fine —
            // PRE_CANCELED_CAP is small.
            if let Some(pos) = state.order.iter().position(|h| h == handle) {
                state.order.remove(pos);
            }
            true
        } else {
            false
        }
    }

    /// Insert `handle` into the pre-cancel set with FIFO eviction at
    /// [`PRE_CANCELED_CAP`]. Idempotent — re-inserting an existing handle
    /// is a no-op.
    async fn buffer_pre_canceled_handle(&self, handle: String) {
        let mut state = self.pre_canceled_handles.inner.lock().await;
        if !state.set.insert(handle.clone()) {
            return;
        }
        state.order.push_back(handle);
        while state.order.len() > PRE_CANCELED_CAP {
            if let Some(evicted) = state.order.pop_front() {
                state.set.remove(&evicted);
            }
        }
    }

    /// Forget every pending and recently-consumed tool_call id for the
    /// given parent. Called when the parent connection tears down so
    /// stale ids don't bind to a future reuse of the same connection_id
    /// (UUIDs make that unlikely but cheap to defend against), and so a
    /// fresh connection on the reused id is not blocked by the
    /// consumed memory of the previous one.
    pub async fn drop_pending_tool_calls_for_parent(&self, parent_connection_id: &str) {
        self.drop_tool_calls_for_parent(parent_connection_id, false)
            .await;
    }

    /// Core of the tool_call-tracker drop, shared by the two cancel scopes.
    ///
    /// * `keep_consumed == false` — genuine connection teardown: remove the
    ///   whole bucket (`pending` + `consumed`). The connection is going away,
    ///   so nothing it remembered can mis-bind a future delegation, and a
    ///   reused connection_id must start clean.
    /// * `keep_consumed == true` — turn/prompt cancel with the parent
    ///   connection STILL ALIVE: TOMBSTONE the cancelled turn's unclaimed
    ///   `pending` ids into `consumed` and RETAIN the existing `consumed`. Both
    ///   the already-claimed ids AND the just-cancelled turn's unclaimed ids
    ///   must keep rejecting a host re-emit (e.g. a terminal status-flip): the
    ///   Tier-1 consumed check in `register_pending_tool_call_with_key_at` drops
    ///   the re-emit, so a stale id can't re-register as fresh `pending` and
    ///   mis-bind the next same-key delegation on this live connection. Merely
    ///   CLEARING the unclaimed ids would leave them re-registerable, reopening
    ///   that hole for the unclaimed half (the claimed half was already safe via
    ///   `consumed`). Retention is connection-scoped and released on teardown —
    ///   the same unbounded-but-bounded-by-delegation-count envelope `consumed`
    ///   already lives in for normal end_turn delegations (see
    ///   [`ToolCallTrackerBucket`]).
    ///
    /// Tombstoning ALL of `pending` here is safe (no turn/generation tag
    /// needed): `run_conversation_loop` drives at most ONE `session/prompt`
    /// future per connection at a time (see `acp/connection.rs`), and a
    /// parent-side `tool_call` only streams while its prompt future is in
    /// flight, so every `pending` id belongs to the single active turn — the one
    /// being cancelled — or is a stale leftover from an earlier turn that should
    /// be tombstoned regardless. (The per-connection `prompt_lock` only
    /// serializes the prompt-SEND handshake, not the turn, so it is NOT the
    /// source of this invariant.) The cancelled turn's serialized MCP round-trip
    /// won't arrive after cancel, so nothing legitimate is lost.
    async fn drop_tool_calls_for_parent(&self, parent_connection_id: &str, keep_consumed: bool) {
        let mut map = self.tool_calls.inner.lock().await;
        if !keep_consumed {
            map.remove(parent_connection_id);
            return;
        }
        if let Some(bucket) = map.get_mut(parent_connection_id) {
            // Tombstone the cancelled turn's unclaimed pending ids into
            // `consumed` rather than just dropping them, so a later host re-emit
            // of one is rejected by the Tier-1 consumed check instead of
            // re-registering as a claimable stale entry. `drain` empties
            // `pending` first so the subsequent `consumed` borrow is disjoint.
            let now = Instant::now();
            let cleared: Vec<String> = bucket.pending.drain(..).map(|p| p.tool_call_id).collect();
            for id in cleared {
                if !bucket.consumed.iter().any(|(c, _)| c == &id) {
                    bucket.consumed.push_back((id, now));
                }
            }
            // Drop the now-empty bucket only when nothing consumed remains —
            // otherwise keep it so the retained `consumed` ids keep rejecting
            // re-emits for the rest of this connection's lifetime.
            if bucket.consumed.is_empty() {
                map.remove(parent_connection_id);
            }
        }
    }

    pub async fn set_config(&self, cfg: DelegationConfig) {
        let cap_bytes = cfg.completed_cache_cap_bytes;
        let kept_cap = cfg.kept_alive_cap;
        *self.config.lock().await = cfg;
        // Seed the caps into the pending-calls bucket so the settle paths read
        // them lock-free (they already hold the pending lock). Acquired AFTER
        // the config guard above is dropped — sequential, never nested — so no
        // path locks `config` under `pending` or vice-versa (deadlock-free).
        // Then prune existing retentions: a LOWERED cap must free memory /
        // processes now, not lazily on the next completion (which may never
        // arrive for an idle parent).
        let evicted = {
            let mut inner = self.pending.inner.lock().await;
            inner.completed_cap_bytes = cap_bytes;
            // TODO(delegation-settings): one `kept_alive_cap` setting feeds
            // BOTH scopes, so the per-parent layer is same-value redundant
            // with the global FIFO until the settings surface lets the two
            // caps diverge. Keep the two-scope enforcement (the eviction
            // semantics differ) but don't read meaning into the equal values.
            inner.kept_alive_cap_global = kept_cap;
            inner.kept_alive_cap_per_parent = kept_cap;
            inner.enforce_completed_cap_all_parents();
            inner.enforce_kept_alive_caps_all()
        };
        // Disconnect displaced kept-alive children out of lock (Property 1).
        for cid in evicted {
            let _ = self.spawner.disconnect(&cid).await;
        }
    }

    pub async fn config_snapshot(&self) -> DelegationConfig {
        self.config.lock().await.clone()
    }

    /// If this in-flight setup has been flagged canceled by a parent cancel,
    /// deregister it and return true. One lock acquisition; used at the
    /// pre-spawn / post-spawn checkpoints in `handle_request`.
    async fn take_inflight_cancel(&self, inflight_id: u64) -> bool {
        let mut inner = self.pending.inner.lock().await;
        if inner.inflight_canceled(inflight_id) {
            inner.deregister_inflight(inflight_id);
            true
        } else {
            false
        }
    }

    /// Drop this setup's in-flight record. Called on each `handle_request`
    /// early-return that isn't a park hand-off (the park region deregisters
    /// inline, atomically with `calls.insert`).
    async fn drop_inflight(&self, inflight_id: u64) {
        self.pending
            .inner
            .lock()
            .await
            .deregister_inflight(inflight_id);
    }

    /// Async entry point for `delegate_to_agent`. Does the bounded setup
    /// (claim/depth checks → spawn → send first prompt), registers the task in
    /// `running`, and returns a `Running` ack [`DelegationTaskReport`] WITHOUT
    /// waiting for the child to finish. The child resolves later via the
    /// lifecycle → [`complete_call`] (or a cancel path), which migrates the task
    /// into `completed` and wakes any `get_delegation_status` long-poll.
    ///
    /// Returns a terminal report instead of a `Running` ack in three cases: the
    /// child finished during setup (fast/empty turn), a parent cancel reached it
    /// mid-setup, or setup itself failed (disabled / depth / spawn / send).
    ///
    /// All the setup-window race machinery (`setups` / `early_*` / `inflight`)
    /// is unchanged — it governs terminals that beat registration, which is
    /// orthogonal to whether the caller then blocks. The only change vs. the old
    /// `handle_request` is that "park a `oneshot` and await it" becomes "insert a
    /// [`RunningTask`] and return the ack."
    #[tracing::instrument(
        name = "delegation_task",
        skip_all,
        fields(
            parent_connection_id = %req.parent_connection_id,
            parent_tool_use_id = %req.parent_tool_use_id,
            agent_type = ?req.agent_type,
            working_dir = ?req.working_dir,
            child_connection_id = tracing::field::Empty,
            task_id = tracing::field::Empty,
        )
    )]
    pub async fn start_delegation(&self, mut req: DelegationRequest) -> DelegationTaskReport {
        // Register this setup as the VERY FIRST thing — before the pre-cancel
        // check's `.await` and the (possibly multi-second) claim poll — so a
        // parent cancel landing ANYWHERE from here to park reaches it, not just
        // after park (which is all the `cancel_by_parent*` parked-call drain
        // covers on its own). The only residual gap is a cancel firing before
        // the broker is even invoked for this request, which no
        // in-`handle_request` mechanism can observe. Deregistered on every exit
        // path below: each early-return via `drop_inflight` /
        // `take_inflight_cancel`, or inline at park (atomically with
        // `calls.insert`).
        let inflight_id = self
            .pending
            .inner
            .lock()
            .await
            .register_inflight(&req.parent_connection_id);
        // Pre-cancel short-circuit. If the MCP companion already received
        // `notifications/cancelled` for this `tools/call` before we even
        // started processing (cancel ran ahead of the UDS round-trip), we
        // claim the handle from the pre-cancel set and bail without
        // spawning anything — the caller will not be receiving our
        // response either way (the companion suppresses it per MCP spec).
        if let Some(handle) = req.external_handle.as_deref() {
            if self.take_pre_canceled_handle(handle).await {
                self.drop_inflight(inflight_id).await;
                // Bailing here BEFORE the claim path means this delegation never
                // consumes the ACP `tool_call_id` the lifecycle keyed for it. As
                // keyed entries are retained indefinitely, a leftover would let a
                // *later* same-`(agent_type, task, working_dir)` delegation claim
                // this canceled call's id and bind its writes/events to the wrong
                // card. Drain it now (idempotent; the turn-end tombstone is the
                // backstop if the ACP event hasn't registered yet).
                if req.parent_tool_use_id.is_empty() {
                    let key = DelegationMatchKey {
                        agent_type: req.agent_type,
                        task: req.task.clone(),
                        working_dir: req.requested_working_dir.clone(),
                    };
                    let _ = self
                        .take_matching_tool_call(&req.parent_connection_id, &key)
                        .await;
                } else {
                    self.consume_explicit_tool_call(
                        &req.parent_connection_id,
                        &req.parent_tool_use_id,
                    )
                    .await;
                }
                return report_err(
                    req.agent_type,
                    DelegationError::Canceled {
                        reason: "canceled before spawn".into(),
                    },
                    None,
                );
            }
        }
        // MCP clients usually don't populate `_meta.tool_use_id`, so the
        // listener will pass through an empty string. Claim the matching
        // ACP-side `tool_call_id` for this parent by task text — with a brief
        // poll loop so an MCP round-trip that out-races the in-process ACP
        // `session/update` doesn't fall back to a synthetic id (which breaks
        // the parent UI's `parent_tool_use_id` binding). Falls back to a UUID
        // placeholder only when no id arrives within the wait budget.
        if req.parent_tool_use_id.is_empty() {
            let match_key = DelegationMatchKey {
                agent_type: req.agent_type,
                task: req.task.clone(),
                working_dir: req.requested_working_dir.clone(),
            };
            let claimed = self
                .claim_pending_tool_call_with_brief_wait(&req.parent_connection_id, &match_key)
                .await;
            let claimed_identityless = matches!(claimed, Some((_, true)));
            req.parent_tool_use_id = claimed.map(|(id, _)| id).unwrap_or_else(|| {
                tracing::warn!(
                    "[delegation] synthetic fallback for parent_tool_use_id on conn={} (no ACP tool_call_id arrived within claim budget)",
                    req.parent_connection_id
                );
                format!("delegation-{}", uuid::Uuid::new_v4())
            });
            // The claimed announcement never carried an identity (Cursor's
            // "MCP: tool" + "{}" — the wire will not re-send title/arguments):
            // restore it NOW, before the multi-second spawn, so the live card
            // shows the canonical delegate tool with its real arguments while
            // the child is still starting. Hosts whose wire carried the
            // identity (keyed or explicit-id claims) are left untouched — a
            // reconstructed `raw_input` would clobber host-specific fields.
            if claimed_identityless {
                let mut raw_input = serde_json::Map::new();
                raw_input.insert("task".into(), serde_json::Value::String(req.task.clone()));
                if let Ok(at) = serde_json::to_value(req.agent_type) {
                    raw_input.insert("agent_type".into(), at);
                }
                if let Some(dir) = req.requested_working_dir.as_deref() {
                    raw_input.insert("working_dir".into(), serde_json::Value::String(dir.into()));
                }
                self.meta_writer
                    .write_tool_call_identity(
                        &req.parent_connection_id,
                        &req.parent_tool_use_id,
                        crate::acp::delegation::DELEGATE_TOOL_REWRITE_TITLE,
                        serde_json::Value::Object(raw_input),
                    )
                    .await;
            }
        } else {
            // The client gave us the real ACP tool_call_id directly
            // (`_meta.tool_use_id`), so we skip the claim path — but the
            // lifecycle dispatcher may already have registered that same id as
            // a (now indefinitely-retained) keyed pending entry. Consume it so
            // it can't linger and be mis-claimed by a later same-key
            // delegation. Idempotent and order-independent (see the method).
            self.consume_explicit_tool_call(&req.parent_connection_id, &req.parent_tool_use_id)
                .await;
        }
        let cfg = self.config_snapshot().await;
        if !cfg.enabled {
            self.drop_inflight(inflight_id).await;
            return report_err(
                req.agent_type,
                DelegationError::Canceled {
                    reason: "delegation disabled".into(),
                },
                None,
            );
        }

        // --- Depth pre-check ----------------------------------------------------
        // We walk up to `limit + 1` so we know whether the *new* child would
        // sit at >= limit. Cycles/dead chains saturate at the cap.
        let lookup = self.depth_lookup.clone();
        let parent_depth = match crate::acp::delegation::depth::compute_depth(
            req.parent_conversation_id,
            |id| {
                let lookup = lookup.clone();
                async move { lookup.parent_of(id).await }
            },
            cfg.depth_limit + 1,
        )
        .await
        {
            Ok(d) => d,
            Err(e) => {
                self.drop_inflight(inflight_id).await;
                return report_err(req.agent_type, e, None);
            }
        };
        // The child the broker is about to create would sit at `parent_depth + 1`.
        // Reject only when the *child* depth would strictly exceed the limit;
        // a child sitting exactly at `depth_limit` is allowed.
        if parent_depth + 1 > cfg.depth_limit {
            self.drop_inflight(inflight_id).await;
            return report_err(
                req.agent_type,
                DelegationError::DepthLimitExceeded {
                    current_depth: parent_depth,
                    limit: cfg.depth_limit,
                },
                None,
            );
        }

        // --- Spawn child connection --------------------------------------------
        // Pull per-agent overrides from the broker config (defaults to empty).
        // Cloning is cheap — `AgentDelegationDefaults` is at most one Option<String>
        // and a small BTreeMap, and the spawner consumes both fields by value.
        let (preferred_mode_id, preferred_config_values) = cfg
            .agent_defaults
            .get(&req.agent_type)
            .map(|d: &AgentDelegationDefaults| (d.mode_id.clone(), d.config_values.clone()))
            .unwrap_or((None, BTreeMap::new()));
        // Checkpoint #1 (opportunistic): if a parent cancel already landed
        // during the claim/depth phase, bail before spawning a child the parent
        // has abandoned. No child exists yet, so there's nothing to tear down.
        if self.take_inflight_cancel(inflight_id).await {
            return report_err(
                req.agent_type,
                DelegationError::Canceled {
                    reason: "parent canceled".into(),
                },
                None,
            );
        }
        let child_connection_id = match self
            .spawner
            .spawn(
                &req.parent_connection_id,
                req.agent_type,
                req.working_dir.clone(),
                preferred_mode_id,
                preferred_config_values,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                self.drop_inflight(inflight_id).await;
                return report_err(
                    req.agent_type,
                    DelegationError::SpawnFailed(e.to_string()),
                    None,
                );
            }
        };

        // Checkpoint #2: a parent cancel that landed during spawn() — the child
        // now exists but no prompt has been sent, so disconnect it (mirroring
        // the send-failure path's disconnect-only teardown) and bail. This is
        // the primary guard for the spawn window, which can block while the
        // agent process starts up.
        if self.take_inflight_cancel(inflight_id).await {
            let _ = self.spawner.disconnect(&child_connection_id).await;
            return report_err(
                req.agent_type,
                DelegationError::Canceled {
                    reason: "parent canceled".into(),
                },
                None,
            );
        }

        // --- Send linked prompt ------------------------------------------------
        let call_id = uuid::Uuid::new_v4().to_string();
        // Bounded task label used by the started event and every meta write —
        // the frontend card's fallback when the parent tool call's `raw_input`
        // never carried the arguments (Cursor's identity-less announcements).
        let task_preview = truncate_on_char_boundary(&req.task, TASK_PREVIEW_CAP);
        // Now that the child connection and task id exist, fill the span's empty
        // fields so every subsequent log line in this delegation carries the
        // parent→child linkage (see the `delegation_task` span on this fn).
        tracing::Span::current().record("child_connection_id", child_connection_id.as_str());
        tracing::Span::current().record("task_id", call_id.as_str());
        let link = DelegationLink {
            parent_conversation_id: req.parent_conversation_id,
            parent_tool_use_id: req.parent_tool_use_id.clone(),
            delegation_call_id: call_id.clone(),
        };

        // Reserve this delegation (both ids) BEFORE sending its first prompt.
        // `send_prompt_linked_for_delegation` persists the delegation link onto
        // the child row (arming the lifecycle resolver) AND dispatches the
        // prompt — after which a fast/empty turn's `TurnComplete` OR an
        // immediate child-connection failure can fire before we park the pending
        // entry below. The reservation lets those terminal events buffer their
        // outcome (see `PendingInner`) for the park to drain, rather than
        // no-oping and stranding `rx.await`. There is no `.await` between this
        // reservation and `send_prompt` (so nothing the child does can be
        // observed before the reservation is in place); it's cleared at park or
        // on the send-failure path. Reserving by `call_id` AND
        // `child_connection_id` lets each resolver gate on the id it holds —
        // `complete_call` the `call_id`, `cancel_by_child_connection` the
        // `child_connection_id`.
        self.pending
            .inner
            .lock()
            .await
            .reserve(&call_id, &child_connection_id);

        let child_conversation_id = match self
            .spawner
            .send_prompt_linked_for_delegation(&child_connection_id, req.task.clone(), link)
            .await
        {
            Ok(cid) => cid,
            Err(e) => {
                // Setup failed before parking — release the reservation (and
                // discard any terminal that buffered against this delegation in
                // the window) so nothing lingers or mis-binds a future id, and
                // drop the in-flight record in the same lock acquisition.
                {
                    let mut inner = self.pending.inner.lock().await;
                    inner.unreserve(&call_id, &child_connection_id);
                    inner.deregister_inflight(inflight_id);
                }
                let _ = self.spawner.disconnect(&child_connection_id).await;
                return report_err(
                    req.agent_type,
                    DelegationError::SpawnFailed(e.to_string()),
                    None,
                );
            }
        };

        // The child is now running. Stamp the start so terminal paths can
        // report a real `duration_ms`.
        let started_at = Instant::now();

        // --- Mark the parent's tool call as in-flight -------------------------
        // The frontend's DelegationContext seeds its `parent_tool_use_id`-keyed
        // binding map from this meta on snapshot replay, so a page refresh
        // mid-delegation can reconstruct the child connection / conversation
        // ids without depending on the live `delegation_started` event having
        // been received.
        self.write_meta_if_real(
            &req.parent_connection_id,
            &req.parent_tool_use_id,
            build_delegation_meta(
                "running",
                Some(&child_connection_id),
                Some(child_conversation_id),
                None,
                None,
                // No meaningful elapsed yet — the child just started.
                None,
                Some(&task_preview),
                Some(&call_id),
            ),
        )
        .await;

        // Announce the live delegation on the PARENT's event stream so the
        // frontend `DelegationContext` binds the child inline and attaches its
        // live sub-thread. Symmetric with the terminal `emit_completed_if_real`,
        // and — unlike the removed child-stream emit in `send_prompt_linked` —
        // delivered on a stream the parent is already attached to in web/server
        // mode, carrying the real `parent_connection_id`.
        self.emit_started_if_real(
            &req.parent_connection_id,
            &req.parent_tool_use_id,
            &child_connection_id,
            child_conversation_id,
            req.agent_type,
            &task_preview,
            &call_id,
        )
        .await;

        // --- Register pending, or resolve a terminal that beat us -------------
        // Under a single lock, decide this delegation's fate atomically against
        // everything a concurrent resolver may have recorded while we were
        // setting up:
        //   * a child terminal buffered against the reservation — a
        //     `TurnComplete` via `complete_call` (keyed by `call_id`) OR a child
        //     failure via `cancel_by_child_connection` (keyed by
        //     `child_connection_id`); either can race ahead of this park; or
        //   * a parent cancel that flagged this in-flight setup
        //     (`mark_inflight_canceled_for_parent`, which runs in the SAME lock
        //     acquisition that drains the parked `calls`).
        // Precedence: strict first-terminal-wins by arrival stamp. Both a child
        // terminal and a parent cancel carry the `seq` clock value they were
        // recorded at, so whichever landed FIRST wins — a child that completed
        // before the cancel keeps its result; a cancel that beat the completion
        // discards it (the parent had already abandoned the turn). Ties are
        // impossible: every event draws a distinct stamp under this one lock.
        // Only when NOTHING beat us do we park for a future resolver,
        // deregistering the in-flight record adjacent to `calls.insert` with no
        // `.await` between — so a parent cancel serialized AFTER us finds the
        // entry in `calls` and drains it, while one serialized BEFORE us is seen
        // here via its stamp. When a terminal/cancel DID beat us we deliberately
        // DON'T park: resolving inline (never leaving an entry for a second
        // resolver to grab) rules out a double-finalize.
        enum Disposition {
            ChildTerminal(DelegationOutcome),
            ParentCanceled,
            Running,
        }
        // Near-zero elapsed for these setup-window races, but measured for
        // consistency with the normal terminal paths.
        let setup_duration_ms = started_at.elapsed().as_millis() as u64;
        let disposition = {
            let mut inner = self.pending.inner.lock().await;
            // Create the session entry (domain layer) for this dispatch. Done
            // unconditionally under the disposition lock: the terminal arms
            // below settle it in place, and the Running arm leaves it live.
            let mut session_entry = SessionEntry {
                parent_conversation_id: req.parent_conversation_id,
                parent_connection_id: req.parent_connection_id.clone(),
                parent_tool_use_id: req.parent_tool_use_id.clone(),
                agent_type: req.agent_type,
                child_conversation_id,
                child_connection_id: Some(child_connection_id.clone()),
                status: TaskStatus::Running,
                released: false,
                orphan_suspect: false,
                turn_version: 0,
                turns: VecDeque::new(),
                folder_id: None,
                external_id: None,
                working_dir: None,
            };
            // Turn 1 = the original delegation dispatch.
            let _ = session_entry.push_turn(TurnOrigin::ParentAgent);
            inner.sessions.insert(call_id.clone(), session_entry);
            // Each buffered child terminal carries (arrival_stamp, outcome).
            let child_terminal: Option<(u64, DelegationOutcome)> =
                if let Some((stamp, outcome)) = inner.take_early_complete(&call_id) {
                    Some((stamp, outcome))
                } else {
                    inner
                        .take_early_cancel(&child_connection_id)
                        .map(|(stamp, reason)| {
                            (
                                stamp,
                                DelegationOutcome::from_err(
                                    DelegationError::Canceled { reason },
                                    Some(child_conversation_id),
                                ),
                            )
                        })
                };
            let parent_canceled_at = inner.inflight_canceled_at(inflight_id);
            inner.unreserve(&call_id, &child_connection_id);
            // For both terminal dispositions we record the completed result
            // INSIDE this lock (atomically with unreserve/deregister) so a
            // concurrent `get_delegation_status` can never observe the task as
            // neither running nor completed. The `Running` arm inserts the live
            // task instead of parking a `oneshot` — the caller returns the ack.
            let record = |inner: &mut PendingInner, outcome: &DelegationOutcome| {
                inner.insert_completed(
                    &call_id,
                    build_completed(
                        &req.parent_connection_id,
                        child_conversation_id,
                        req.agent_type,
                        setup_duration_ms,
                        outcome,
                    ),
                );
                // Settle the session created above: cancel-coded outcomes drop
                // the connection (about to be torn down); a child that finished
                // during setup keeps it alive for continue_with_session.
                let is_canceled = matches!(
                    outcome,
                    DelegationOutcome::Err { code, .. } if code == "canceled"
                );
                let keep_conn = (!is_canceled).then(|| child_connection_id.clone());
                inner.settle_session(&call_id, terminal_fields(outcome).0, keep_conn)
            };
            match (child_terminal, parent_canceled_at) {
                // Both raced in the setup window: the earlier arrival stamp wins.
                (Some((child_stamp, outcome)), Some(cancel_stamp)) => {
                    inner.deregister_inflight(inflight_id);
                    if child_stamp < cancel_stamp {
                        let evicted = record(&mut inner, &outcome);
                        (Disposition::ChildTerminal(outcome), evicted)
                    } else {
                        let evicted = record(
                            &mut inner,
                            &canceled_outcome(child_conversation_id, "parent canceled"),
                        );
                        (Disposition::ParentCanceled, evicted)
                    }
                }
                // Only a child terminal fired.
                (Some((_, outcome)), None) => {
                    inner.deregister_inflight(inflight_id);
                    let evicted = record(&mut inner, &outcome);
                    (Disposition::ChildTerminal(outcome), evicted)
                }
                // Only a parent cancel fired.
                (None, Some(_)) => {
                    inner.deregister_inflight(inflight_id);
                    let evicted = record(
                        &mut inner,
                        &canceled_outcome(child_conversation_id, "parent canceled"),
                    );
                    (Disposition::ParentCanceled, evicted)
                }
                // Nothing beat us — register the running task for a future
                // resolver, deregistering the in-flight record adjacent to the
                // insert with no `.await` between (so a parent cancel serialized
                // AFTER us finds it in `running` and drains it).
                (None, None) => {
                    inner.running.insert(
                        call_id.clone(),
                        RunningTask {
                            child_connection_id: child_connection_id.clone(),
                            child_conversation_id,
                            parent_connection_id: req.parent_connection_id.clone(),
                            parent_tool_use_id: req.parent_tool_use_id.clone(),
                            agent_type: req.agent_type,
                            task_preview: task_preview.clone(),
                            task_id: call_id.clone(),
                            external_handle: req.external_handle.clone(),
                            started_at,
                            turn_version: 1,
                            turn_id: None,
                            origin: TurnOrigin::ParentAgent,
                        },
                    );
                    inner.deregister_inflight(inflight_id);
                    (Disposition::Running, Vec::new())
                }
            }
        };
        let (disposition, cap_evicted) = disposition;
        // Kept-alive cap evictions decided under the disposition lock: release
        // the displaced connections now that the lock is gone (Property 1).
        for cid in cap_evicted {
            let _ = self.spawner.disconnect(&cid).await;
        }

        match disposition {
            // A child terminal beat registration. Finalize (terminal meta +
            // DelegationCompleted event + child teardown) and return the
            // terminal report directly. The completed entry was recorded under
            // the disposition lock above; wake any long-poll waiter.
            Disposition::ChildTerminal(outcome) => {
                self.finalize_delegation(
                    &req.parent_connection_id,
                    &req.parent_tool_use_id,
                    &child_connection_id,
                    child_conversation_id,
                    req.agent_type,
                    setup_duration_ms,
                    &outcome,
                    &task_preview,
                    &call_id,
                    // Setup-window terminal is always the FIRST turn.
                    true,
                )
                .await;
                self.result_notify.notify_waiters();
                return report_from_outcome(
                    Some(call_id),
                    Some(req.agent_type),
                    &outcome,
                    Some(setup_duration_ms),
                );
            }
            // A parent cancel reached this delegation mid-setup — after the
            // prompt was sent, before we registered. Tear the child down
            // ourselves (cancel + disconnect, since a turn is in flight) and
            // return a canceled report. The canceled result was recorded above.
            Disposition::ParentCanceled => {
                self.write_meta_if_real(
                    &req.parent_connection_id,
                    &req.parent_tool_use_id,
                    build_delegation_meta(
                        "failed",
                        Some(&child_connection_id),
                        Some(child_conversation_id),
                        Some("canceled"),
                        None,
                        Some(setup_duration_ms),
                        Some(&task_preview),
                        Some(&call_id),
                    ),
                )
                .await;
                self.emit_completed_if_real(
                    &req.parent_connection_id,
                    &req.parent_tool_use_id,
                    &child_connection_id,
                    child_conversation_id,
                    req.agent_type,
                    DelegationResultSummary::Err {
                        error_code: "canceled".to_string(),
                    },
                )
                .await;
                let _ = self.spawner.cancel(&child_connection_id).await;
                let _ = self.spawner.disconnect(&child_connection_id).await;
                self.result_notify.notify_waiters();
                return report_from_outcome(
                    Some(call_id),
                    Some(req.agent_type),
                    &canceled_outcome(child_conversation_id, "parent canceled"),
                    Some(setup_duration_ms),
                );
            }
            // Registered in `running` — fall through to the second pre-cancel
            // check, then return the ack.
            Disposition::Running => {}
        }

        // Second pre-cancel check: a `notifications/cancelled` may have landed
        // between the entry-side check and the `running` registration above. If
        // so, drain the task ourselves (so a racing `cancel_by_external_handle`
        // doesn't double-finalize), record the canceled result, and return a
        // canceled report instead of the Running ack.
        if let Some(handle) = req.external_handle.as_deref() {
            if self.take_pre_canceled_handle(handle).await {
                // Capture the elapsed ONCE at terminalization (under the lock,
                // when the running task is removed) so the completed-cache, the
                // parent-card meta, and the returned report all report the same
                // duration. `None` when nothing was drained.
                let canceled_duration_ms = {
                    let mut inner = self.pending.inner.lock().await;
                    if inner.running.remove(&call_id).is_some() {
                        let outcome =
                            canceled_outcome(child_conversation_id, "canceled before await");
                        let duration_ms = started_at.elapsed().as_millis() as u64;
                        inner.insert_completed(
                            &call_id,
                            build_completed(
                                &req.parent_connection_id,
                                child_conversation_id,
                                req.agent_type,
                                duration_ms,
                                &outcome,
                            ),
                        );
                        let evicted = inner.settle_session(&call_id, TaskStatus::Canceled, None);
                        debug_assert!(evicted.is_empty());
                        Some(duration_ms)
                    } else {
                        None
                    }
                };
                if let Some(duration_ms) = canceled_duration_ms {
                    self.write_meta_if_real(
                        &req.parent_connection_id,
                        &req.parent_tool_use_id,
                        build_delegation_meta(
                            "failed",
                            Some(&child_connection_id),
                            Some(child_conversation_id),
                            Some("canceled"),
                            None,
                            Some(duration_ms),
                            Some(&task_preview),
                            Some(&call_id),
                        ),
                    )
                    .await;
                    self.emit_completed_if_real(
                        &req.parent_connection_id,
                        &req.parent_tool_use_id,
                        &child_connection_id,
                        child_conversation_id,
                        req.agent_type,
                        DelegationResultSummary::Err {
                            error_code: "canceled".to_string(),
                        },
                    )
                    .await;
                    let _ = self.spawner.cancel(&child_connection_id).await;
                    let _ = self.spawner.disconnect(&child_connection_id).await;
                    self.result_notify.notify_waiters();
                    return report_from_outcome(
                        Some(call_id),
                        Some(req.agent_type),
                        &canceled_outcome(child_conversation_id, "canceled before await"),
                        Some(duration_ms),
                    );
                }
            }
        }

        // Registered and running in the background — return the ack. The child
        // resolves later via the lifecycle → `complete_call` (or a cancel path).
        running_ack(call_id, child_conversation_id, req.agent_type)
    }

    /// Called by the child-session lifecycle subscriber on `TurnComplete`
    /// (success path) or by error mappers (failure path).
    ///
    /// Migrates the task from `running` into `completed` (atomically, under one
    /// lock) and then finalizes (terminal meta + `DelegationCompleted` event +
    /// child teardown) and wakes any `get_delegation_status` long-poll.
    ///
    /// If no entry is in `running` under `call_id`, the outcome is buffered for
    /// a racing `start_delegation` to drain at registration — but ONLY while the
    /// delegation is still reserved (mid-setup). This closes the window where a
    /// fast/empty turn's `TurnComplete` propagates through the lifecycle while
    /// `start_delegation` is still between `send_prompt` and the `running`
    /// insert: the prompt is only *enqueued* by `send_prompt`, and the child
    /// loop emits `TurnComplete` independently, so a completion CAN beat it. When
    /// the `call_id` is no longer reserved the call was already resolved by
    /// another terminal path, so the buffer is skipped (silent no-op).
    pub async fn complete_call(&self, call_id: &str, outcome: DelegationOutcome) {
        let is_canceled = matches!(
            &outcome,
            DelegationOutcome::Err { code, .. } if code == "canceled"
        );
        let task = {
            let mut inner = self.pending.inner.lock().await;
            match inner.running.remove(call_id) {
                Some(task) => {
                    // Atomic running → completed so a concurrent status query
                    // never sees the task as neither running nor completed.
                    let duration_ms = task.started_at.elapsed().as_millis() as u64;
                    inner.insert_completed(
                        call_id,
                        build_completed(
                            &task.parent_connection_id,
                            task.child_conversation_id,
                            task.agent_type,
                            duration_ms,
                            &outcome,
                        ),
                    );
                    // Keep the child process for continue_with_session unless
                    // this terminal is cancel-coded (Property 3).
                    let keep_conn = (!is_canceled).then(|| task.child_connection_id.clone());
                    let evicted =
                        inner.settle_session(call_id, terminal_fields(&outcome).0, keep_conn);
                    Some((task, duration_ms, evicted))
                }
                None => {
                    // Buffer for the racing `start_delegation` to drain iff still
                    // reserved (mid-setup); a no-op otherwise, so the clone only
                    // materializes on the genuine pre-registration race.
                    inner.buffer_early_complete(call_id, outcome.clone());
                    None
                }
            }
        };
        if let Some((task, duration_ms, evicted)) = task {
            // Kept-alive cap evictions decided under the settle lock: release
            // the displaced connections now that the lock is gone (Property 1).
            for cid in evicted {
                let _ = self.spawner.disconnect(&cid).await;
            }
            self.finalize_delegation(
                &task.parent_connection_id,
                &task.parent_tool_use_id,
                &task.child_connection_id,
                task.child_conversation_id,
                task.agent_type,
                duration_ms,
                &outcome,
                &task.task_preview,
                &task.task_id,
                // Requirement 2.8a: a continued turn (version > 1) must not
                // re-emit completion against the already-terminal tool call.
                task.turn_version <= 1,
            )
            .await;
            // T4.3 (Requirements 2.8 / 8.2): a CONTINUED turn's terminal is
            // announced by the session-scoped update carrying
            // (task_id, turn_id, turn_version, origin) — the replacement for
            // the suppressed completion above.
            self.emit_session_update_for_settled_turn(&task).await;
            self.result_notify.notify_waiters();
        }
    }

    /// Write the terminal meta, emit `DelegationCompleted`, and tear down the
    /// child for a resolved delegation. Shared by `complete_call` and
    /// `start_delegation`'s early-terminal pickup. Mirrors the resolution onto
    /// the parent's `delegate_to_agent` ToolCallState meta (including a bounded
    /// `text_preview` on the completed path so a post-refresh snapshot renders
    /// the result inline) so snapshot recovery shows the final state without the
    /// live `delegation_completed` event. Does not touch the pending maps — the
    /// caller owns the `running` → `completed` migration.
    ///
    /// `duration_ms` is the broker-measured elapsed time (from `started_at`),
    /// carried onto the event summary so the parent UI shows a real duration.
    #[allow(clippy::too_many_arguments)]
    async fn finalize_delegation(
        &self,
        parent_connection_id: &str,
        parent_tool_use_id: &str,
        child_connection_id: &str,
        child_conversation_id: i32,
        agent_type: AgentType,
        duration_ms: u64,
        outcome: &DelegationOutcome,
        task_preview: &str,
        task_id: &str,
        emit_completion: bool,
    ) {
        let meta = match outcome {
            DelegationOutcome::Ok(ok) => build_delegation_meta(
                "completed",
                Some(child_connection_id),
                Some(child_conversation_id),
                None,
                build_text_preview(&ok.text).as_deref(),
                Some(duration_ms),
                Some(task_preview),
                Some(task_id),
            ),
            DelegationOutcome::Err { code, .. } => build_delegation_meta(
                "failed",
                Some(child_connection_id),
                Some(child_conversation_id),
                Some(code),
                None,
                Some(duration_ms),
                Some(task_preview),
                Some(task_id),
            ),
        };
        self.write_meta_if_real(parent_connection_id, parent_tool_use_id, meta)
            .await;
        if emit_completion {
            self.emit_completed_if_real(
                parent_connection_id,
                parent_tool_use_id,
                child_connection_id,
                child_conversation_id,
                agent_type,
                outcome_to_summary(outcome, duration_ms),
            )
            .await;
        }
        // Terminal-outcome routing (Property 3): completed / failed children
        // stay alive so continue_with_session can follow up with the child's
        // context intact. Only cancel-coded outcomes tear the child down here
        // (the cancel_by_* paths run their own teardown).
        let should_disconnect = matches!(
            outcome,
            DelegationOutcome::Err { code, .. } if code == "canceled"
        );
        if should_disconnect {
            let _ = self.spawner.disconnect(child_connection_id).await;
        }
    }

    /// Internal helper — apply the meta write iff the parent's
    /// `tool_use_id` refers to a real ACP `tool_call_id`. The
    /// broker-synthesized `"delegation-<uuid>"` placeholder targets no
    /// ToolCallState, so emitting a `ToolCallUpdate` against it would be
    /// noise that the frontend would route through `apply_tool_call_update`
    /// to a non-existent entry. See `meta_writer::is_synthetic_parent_tool_use_id`.
    async fn write_meta_if_real(
        &self,
        parent_connection_id: &str,
        parent_tool_use_id: &str,
        meta: serde_json::Value,
    ) {
        if is_synthetic_parent_tool_use_id(parent_tool_use_id) {
            return;
        }
        self.meta_writer
            .write_meta(parent_connection_id, parent_tool_use_id, meta)
            .await;
    }

    /// Internal helper — emit `AcpEvent::DelegationStarted` on the parent's
    /// stream iff the `parent_tool_use_id` refers to a real ACP tool_call.
    /// Mirror of `emit_completed_if_real`: same synthetic-id skip, and the
    /// event rides the parent's stream so the frontend `DelegationContext`
    /// receives it via the parent's per-connection attach stream in
    /// web/server mode (not only via the desktop firehose).
    #[allow(clippy::too_many_arguments)]
    async fn emit_started_if_real(
        &self,
        parent_connection_id: &str,
        parent_tool_use_id: &str,
        child_connection_id: &str,
        child_conversation_id: i32,
        agent_type: AgentType,
        task_preview: &str,
        task_id: &str,
    ) {
        if is_synthetic_parent_tool_use_id(parent_tool_use_id) {
            return;
        }
        self.event_emitter
            .emit_started(
                parent_connection_id,
                parent_tool_use_id,
                child_connection_id,
                child_conversation_id,
                agent_type,
                task_preview,
                task_id,
            )
            .await;
    }

    /// Internal helper — emit the session-scoped
    /// `AcpEvent::DelegationSessionUpdate` for a settled CONTINUED turn
    /// (Requirements 2.8 / 8.2). No-op for the original turn (version 1 — its
    /// terminal is the tool-scoped `DelegationCompleted`) and for the
    /// defensive case of a continued turn without a recorded turn id. Unlike
    /// the completed/started emits this is NOT gated on a real tool_use_id:
    /// the event is session-addressed (task_id), not tool-call-addressed.
    async fn emit_session_update_for_settled_turn(&self, task: &RunningTask) {
        if task.turn_version <= 1 {
            return;
        }
        let Some(turn_id) = task.turn_id.as_deref() else {
            debug_assert!(false, "continued turns always carry a turn_id");
            return;
        };
        // USER-originated continuations carry the synthetic
        // `USER_ENTRY_CONNECTION_ID` as `parent_connection_id`, which never
        // resolves on the parent fan-out — the emitter then falls back to
        // the CHILD connection's stream so the sub-agent dialog (composer
        // availability + transcript refresh) still receives the settle push.
        // The parent AI side of a user-origin turn remains poll-only
        // (design §2.8b): it collects results via `get_delegation_status`.
        self.event_emitter
            .emit_session_update(
                &task.parent_connection_id,
                &task.child_connection_id,
                task.child_conversation_id,
                &task.task_id,
                turn_id,
                task.turn_version,
                task.origin,
            )
            .await;
    }

    /// Internal helper — emit `AcpEvent::DelegationCompleted` on the parent's
    /// stream iff the `parent_tool_use_id` refers to a real ACP tool_call.
    /// Synthetic ids (the `"delegation-<uuid>"` UUID fallback) map to no
    /// live UI binding, so the emit would be wasted noise — same skip
    /// criterion as `write_meta_if_real`.
    async fn emit_completed_if_real(
        &self,
        parent_connection_id: &str,
        parent_tool_use_id: &str,
        child_connection_id: &str,
        child_conversation_id: i32,
        agent_type: AgentType,
        result: DelegationResultSummary,
    ) {
        if is_synthetic_parent_tool_use_id(parent_tool_use_id) {
            return;
        }
        self.event_emitter
            .emit_completed(
                parent_connection_id,
                parent_tool_use_id,
                child_connection_id,
                child_conversation_id,
                agent_type,
                result,
            )
            .await;
    }

    /// Cancel the pending delegation whose `external_handle` matches.
    /// Called by the MCP listener on receipt of `notifications/cancelled`
    /// from a companion. When no matching pending entry exists (the
    /// cancel arrived before `handle_request` reached the
    /// pending-registration phase) the handle is stashed in
    /// `pre_canceled_handles` so the in-flight request can drain itself
    /// when it tries to register or shortly after.
    pub async fn cancel_by_external_handle(&self, external_handle: &str, reason: String) {
        let drained = {
            let mut inner = self.pending.inner.lock().await;
            let keys: Vec<String> = inner
                .running
                .iter()
                .filter(|(_, v)| v.external_handle.as_deref() == Some(external_handle))
                .map(|(k, _)| k.clone())
                .collect();
            drain_and_record_canceled(&mut inner, keys, &reason)
        };
        if drained.is_empty() {
            // Race: the cancel beat the handle's `running` registration. Buffer
            // it (capped, FIFO-evicted) so `start_delegation` can drain itself on
            // the next checkpoint instead of proceeding to spawn the child.
            self.buffer_pre_canceled_handle(external_handle.to_string())
                .await;
            return;
        }
        for (task, duration_ms) in drained {
            // A turn is in flight, so cancel + disconnect.
            self.teardown_canceled_child(&task, duration_ms, true).await;
        }
        self.result_notify.notify_waiters();
    }

    /// Resolve the pending delegation whose child matches
    /// `child_connection_id` with a `canceled` outcome. Used when a child
    /// session disconnects or errors out without firing a clean
    /// TurnComplete — the parent's `tool_use_id` shouldn't dangle.
    /// No-op when no matching entry exists.
    ///
    /// `terminal_error` carries the child connection's last `AcpEvent::Error`
    /// detail when the lifecycle worker is dispatching off an `Error` event
    /// (vs. a bare `Disconnected`). When present, it gets appended to the
    /// `Canceled { reason }` string so the parent agent's tool-call result
    /// surfaces the real cause (e.g. "Authentication required",
    /// "transport closed") instead of the opaque default. Falls back to
    /// the default reason when `None`.
    pub async fn cancel_by_child_connection(
        &self,
        child_connection_id: &str,
        terminal_error: Option<&str>,
    ) {
        let reason = child_canceled_reason(terminal_error);
        let drained = {
            let mut inner = self.pending.inner.lock().await;
            let keys: Vec<String> = inner
                .running
                .iter()
                .filter(|(_, v)| v.child_connection_id == child_connection_id)
                .map(|(k, _)| k.clone())
                .collect();
            if keys.is_empty() {
                // No running entry. If the child is still reserved,
                // `start_delegation` is mid-setup and this failure beat the
                // `running` insert — buffer its detail for it to drain at
                // registration instead of no-oping. `buffer_child_failure` is a
                // no-op when the child isn't reserved, so a normal
                // post-resolution child teardown accumulates nothing.
                inner.buffer_child_failure(
                    child_connection_id,
                    terminal_error.map(|s| s.to_string()),
                );
                Vec::new()
            } else {
                drain_and_record_canceled(&mut inner, keys, &reason)
            }
        };
        for (task, duration_ms) in drained {
            // The child already disconnected/errored — disconnect-only teardown
            // (no spawner `cancel`, there's no live turn to interrupt).
            self.teardown_canceled_child(&task, duration_ms, false)
                .await;
        }
        self.result_notify.notify_waiters();
    }

    /// Cascade-cancel every pending delegation owned by `parent_connection_id`
    /// when the parent **connection tears down** (disconnect / `run_connection`
    /// exit). Drops the parent's entire tool_call tracker bucket (`pending` +
    /// `consumed`) since the connection is going away. Runs fully inline — the
    /// connection is already exiting, so there is no next prompt to unblock.
    pub async fn cancel_by_parent(&self, parent_connection_id: &str) {
        let drained = self
            .drain_for_parent_cancel(parent_connection_id, false)
            .await;
        self.finalize_parent_cancel(drained).await;
    }

    /// Cascade-cancel every pending delegation owned by `parent_connection_id`
    /// for a **turn/prompt cancel** where the parent connection STAYS ALIVE
    /// (a non-`end_turn` turn end, or a user Cancel between/within prompts).
    ///
    /// The fast, turn-scoped part — tombstoning the tool_call tracker and
    /// removing this parent's parked calls — runs SYNCHRONOUSLY: the caller
    /// awaits it before the connection loop accepts the next prompt, so it can't
    /// race a next-turn registration and tombstone/cancel that turn's legitimate
    /// entries (the safety the `drop_tool_calls_for_parent` invariant relies
    /// on). Only the slow child teardown (meta/emit + spawner `cancel` /
    /// `disconnect`, which can block on slow agents) is backgrounded, so the
    /// user-visible Cancel path stays responsive.
    ///
    /// RETAINS the parent's `consumed` tool_call memory (and tombstones the
    /// cancelled turn's unclaimed `pending` ids into it): dropping it would let
    /// a host re-emit of an already-handled `tool_call_id` re-register and
    /// mis-bind the next same-key delegation on this live connection — see
    /// `drop_tool_calls_for_parent`.
    pub async fn cancel_by_parent_turn(&self, parent_connection_id: &str) {
        let drained = self
            .drain_for_parent_cancel(parent_connection_id, true)
            .await;
        // The fast drain above already ran inline (scoped to the just-ended
        // turn); background only the slow child teardown.
        let broker = self.clone();
        tokio::spawn(async move {
            broker.finalize_parent_cancel(drained).await;
        });
    }

    /// Fast, lock-guarded part of a parent cancel: drop/tombstone this parent's
    /// tool_call tracker (per `keep_consumed`, see `drop_tool_calls_for_parent`)
    /// and remove every running task it owns, returning them for the (slow)
    /// child teardown. Touches only the two broker mutexes — no spawner I/O — so
    /// it is safe to await inline in the connection loop before the next prompt
    /// is accepted.
    ///
    /// `keep_consumed` also governs the completed-cache: a **turn** cancel
    /// (`true`) records each drained task as `Canceled` so the still-alive
    /// connection's LLM can still query it; a **connection teardown** (`false`)
    /// drops the parent's whole completed-cache instead — the parent is gone, so
    /// nothing will query it.
    async fn drain_for_parent_cancel(
        &self,
        parent_connection_id: &str,
        keep_consumed: bool,
    ) -> Vec<(RunningTask, u64)> {
        // Also drain any tool_call ids captured ahead of an MCP round-trip that
        // never arrived — keeps the map bounded across parent reconnects.
        // Teardown drops the whole bucket; a turn cancel keeps `consumed` so a
        // later re-emit can't mis-bind the next delegation.
        self.drop_tool_calls_for_parent(parent_connection_id, keep_consumed)
            .await;
        let drained = {
            let mut inner = self.pending.inner.lock().await;
            // Flag every still-in-flight setup this parent owns in the SAME lock
            // acquisition that drains its running tasks: a delegation is then
            // caught either here (mid-setup → `start_delegation` tears its child
            // down at the next checkpoint) or by the running drain below (already
            // registered) — there is no interleaving where both miss it.
            inner.mark_inflight_canceled_for_parent(parent_connection_id);
            let keys: Vec<String> = inner
                .running
                .iter()
                .filter(|(_, v)| v.parent_connection_id == parent_connection_id)
                .map(|(k, _)| k.clone())
                .collect();
            if keep_consumed {
                // Turn cancel: connection stays alive → keep each canceled
                // result queryable.
                drain_and_record_canceled(&mut inner, keys, "parent canceled")
            } else {
                // Connection teardown: just remove the running tasks and drop the
                // whole completed-cache for this parent. No completed entry to
                // match, but still capture the elapsed once (at drain time) so
                // the teardown meta doesn't recompute it later.
                let drained: Vec<(RunningTask, u64)> = keys
                    .into_iter()
                    .map(|k| {
                        let task = inner.running.remove(&k).expect("key just observed");
                        let duration_ms = task.started_at.elapsed().as_millis() as u64;
                        // The in-flight turn is canceled and its child torn
                        // down; settle the session accordingly. The session
                        // entry itself SURVIVES the teardown (ownership is the
                        // parent conversation, not the connection — D5).
                        // `keep_conn: None` never evicts.
                        let evicted = inner.settle_session(&k, TaskStatus::Canceled, None);
                        debug_assert!(evicted.is_empty());
                        (task, duration_ms)
                    })
                    .collect();
                inner.drop_completed_for_parent(parent_connection_id);
                drained
            }
        };
        self.result_notify.notify_waiters();
        drained
    }

    /// Slow part of a parent cancel: for each drained task, patch the parent
    /// meta, emit `DelegationCompleted`, and tear the child down. The canceled
    /// result was already recorded into `completed` (turn cancel) by
    /// `drain_for_parent_cancel` under the lock, so this is pure I/O. Split out
    /// so a turn cancel can background it without delaying the fast, turn-scoped
    /// drain.
    async fn finalize_parent_cancel(&self, drained: Vec<(RunningTask, u64)>) {
        for (task, duration_ms) in drained {
            // A turn was in flight → cancel + disconnect.
            self.teardown_canceled_child(&task, duration_ms, true).await;
        }
    }

    /// Shared canceled-child teardown: best-effort `failed`/`canceled` meta
    /// patch (so a parent-side snapshot post-cancel shows the delegation as
    /// canceled rather than stuck on "running"), a `DelegationCompleted` err
    /// event, then child teardown. `cancel_turn` is `true` when a turn is in
    /// flight (cancel + disconnect) and `false` when the child already
    /// disconnected/errored (disconnect only). Does NOT touch the pending maps —
    /// the caller already migrated the task into `completed`.
    ///
    /// `duration_ms` is the elapsed captured by `drain_and_record_canceled` at
    /// drain time — reused here (not recomputed) so the parent-card meta matches
    /// the completed-cache duration the status/cancel cards report, even when
    /// this teardown is backgrounded.
    async fn teardown_canceled_child(
        &self,
        task: &RunningTask,
        duration_ms: u64,
        cancel_turn: bool,
    ) {
        self.write_meta_if_real(
            &task.parent_connection_id,
            &task.parent_tool_use_id,
            build_delegation_meta(
                "failed",
                Some(&task.child_connection_id),
                Some(task.child_conversation_id),
                Some("canceled"),
                None,
                Some(duration_ms),
                Some(&task.task_preview),
                Some(&task.task_id),
            ),
        )
        .await;
        // Requirement 2.8a: a continued turn (version > 1) must not re-emit a
        // completion event against the already-terminal tool call — the meta
        // snapshot above still updates the card state. Its terminal is instead
        // announced by the session-scoped update (T4.3).
        if task.turn_version <= 1 {
            self.emit_completed_if_real(
                &task.parent_connection_id,
                &task.parent_tool_use_id,
                &task.child_connection_id,
                task.child_conversation_id,
                task.agent_type,
                DelegationResultSummary::Err {
                    error_code: "canceled".to_string(),
                },
            )
            .await;
        } else {
            self.emit_session_update_for_settled_turn(task).await;
        }
        if cancel_turn {
            let _ = self.spawner.cancel(&task.child_connection_id).await;
        }
        let _ = self.spawner.disconnect(&task.child_connection_id).await;
    }

    /// Backs the `get_delegation_status` tool for a single task id — a thin
    /// wrapper over [`Self::get_tasks_status`] so the single- and batch-poll
    /// paths share one snapshot/wait implementation. A one-id batch's
    /// "any task settled" wake condition is exactly "this task settled", so the
    /// blocking semantics are identical to the historical single-task loop.
    pub async fn get_task_status(
        &self,
        parent_connection_id: &str,
        parent_conversation_id: Option<i32>,
        task_id: &str,
        wait: StatusWait,
    ) -> DelegationTaskReport {
        let ids = [task_id.to_string()];
        self.get_tasks_status(parent_connection_id, parent_conversation_id, &ids, wait)
            .await
            .pop()
            .unwrap_or_else(|| unknown_report(task_id))
    }

    /// Backs the batch `get_delegation_status` tool. Resolves the status of one
    /// or many task ids in a single pass — each from the completed-cache, then
    /// the running set, then the DB fallback — scoped to the calling parent (a
    /// task owned by another parent reports `Unknown`, never leaking it). Returns
    /// one report per requested id, in request order.
    ///
    /// Blocking obeys [`StatusWait`]: `Immediate` returns the first snapshot.
    /// `Bounded`/`Infinite` return as soon as ANY requested task is terminal —
    /// INCLUDING one already terminal at entry, so a completed result is never
    /// held hostage to a long-running sibling (the caller re-polls the
    /// still-running ids to collect the rest). Only an all-running batch parks:
    /// it wakes when a task settles (the running count drops below the total) or
    /// — for `Bounded` — when the deadline elapses. An all-settled batch returns
    /// immediately even under `Infinite`, so it never parks forever.
    pub async fn get_tasks_status(
        &self,
        parent_connection_id: &str,
        parent_conversation_id: Option<i32>,
        task_ids: &[String],
        wait: StatusWait,
    ) -> Vec<DelegationTaskReport> {
        if task_ids.is_empty() {
            return Vec::new();
        }
        // A bounded wait gets a single fixed deadline; Immediate and Infinite
        // carry none — Immediate returns on the first pass, Infinite parks on
        // `result_notify` until a task is terminal.
        let deadline = match wait {
            StatusWait::Bounded(ms) => Some(Instant::now() + Duration::from_millis(ms)),
            StatusWait::Immediate | StatusWait::Infinite => None,
        };
        loop {
            // Arm the notify BEFORE the snapshot so a completion landing between
            // the snapshot and the await isn't lost (enable() registers now).
            let notified = self.result_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            // One lock acquisition classifies every requested id. The async
            // resolution of running (live reply) / not-in-memory (DB) ids is
            // deferred to `assemble_reports`, OUTSIDE this lock.
            let classes: Vec<StatusClass> = {
                let inner = self.pending.inner.lock().await;
                task_ids
                    .iter()
                    .map(|id| classify_locked(&inner, parent_connection_id, id))
                    .collect()
            };
            let running_count = classes
                .iter()
                .filter(|c| matches!(c, StatusClass::Running { .. }))
                .count();

            // Return now when the poll is Immediate, OR when at least one
            // requested task is already (or now) terminal — i.e. not EVERY task
            // is still running. This honors the contract "returns as soon as ANY
            // requested task reaches a terminal state": a mixed [terminal,
            // running] batch surfaces the terminal report immediately instead of
            // holding it hostage to a long-running sibling, and the caller
            // re-polls (narrowing to the still-running ids) to collect the rest.
            // `running_count == 0` (all settled) is the special case that also
            // makes Infinite safe. The id set is fixed and a task can only LEAVE
            // the running map during a wait (never (re)enter), so once a parked
            // all-running batch is woken by a settle the count has dropped below
            // the total and this returns; a spurious wake (another parent's task)
            // re-snapshots all-running and re-parks.
            if matches!(wait, StatusWait::Immediate) || running_count < task_ids.len() {
                return self
                    .assemble_reports(parent_conversation_id, task_ids, classes)
                    .await;
            }
            // Every requested task is still running. A `Bounded` wait gives up at
            // its deadline and returns the running snapshot; `Infinite` parks on
            // the notify alone.
            let now = Instant::now();
            if deadline.is_some_and(|d| now >= d) {
                return self
                    .assemble_reports(parent_conversation_id, task_ids, classes)
                    .await;
            }
            // Park until the next completion signal, bounded by the deadline
            // when there is one (Infinite waits on the notify alone).
            match deadline {
                Some(d) => {
                    let remaining = d - now;
                    tokio::select! {
                        _ = &mut notified => {}
                        _ = tokio::time::sleep(remaining) => {}
                    }
                }
                None => {
                    notified.await;
                }
            }
            // Loop: re-snapshot (a task likely just completed, or the deadline
            // passed and the next pass returns the running snapshot).
        }
    }

    /// Finish a batch status pass: resolve each [`StatusClass`] into a final
    /// report AFTER the pending lock is released. `Running` ids get their latest
    /// live reply attached; `NotInMemory` ids fall back to the DB status lookup.
    /// Reports come back in `task_ids` order.
    async fn assemble_reports(
        &self,
        parent_conversation_id: Option<i32>,
        task_ids: &[String],
        classes: Vec<StatusClass>,
    ) -> Vec<DelegationTaskReport> {
        let mut out = Vec::with_capacity(classes.len());
        for (id, class) in task_ids.iter().zip(classes) {
            let report = match class {
                StatusClass::Settled(report) => report,
                StatusClass::Running {
                    mut report,
                    child_connection_id,
                } => {
                    self.attach_live_reply(&mut report, &child_connection_id)
                        .await;
                    report
                }
                StatusClass::NotInMemory => self.status_from_db(parent_conversation_id, id).await,
            };
            out.push(report);
        }
        out
    }

    /// Upgrade a running report's bare `"Running."` message with the child's
    /// latest one-line activity, so the parent LLM gets a concrete sign of
    /// progress it can report in one shot (instead of polling-and-narrating).
    /// Called only on the actual running-return paths, AFTER the pending lock is
    /// released. A no-op when the lookup has nothing (default Noop lookup, child
    /// gone, or no live output yet) — the report stays `"Running."`.
    ///
    /// The hint goes on its OWN line (`"Running.\nLatest sub-agent reply: …"`),
    /// not appended to the marker line. On hosts that persist only the
    /// `CallToolResult` content text (e.g. Claude Code), the frontend recognizes
    /// a still-running poll by the standalone first line `"Running."` — keeping
    /// the child-controlled reply text on a separate line means a *completed*
    /// result that merely starts with "Running. …" can never be misread as
    /// running. See `textRunningStatus` in `src/lib/delegation-status.ts`.
    async fn attach_live_reply(
        &self,
        report: &mut DelegationTaskReport,
        child_connection_id: &str,
    ) {
        if let Some(reply) = self
            .live_reply_lookup
            .latest_reply(child_connection_id)
            .await
        {
            report.message = Some(format!("Running.\nLatest sub-agent reply: {reply}"));
        }
    }

    /// Backs the `cancel_delegation` tool. Cancels a running task owned by the
    /// caller (recording it `Canceled` + tearing the child down) and returns the
    /// resulting report. A task that already finished returns its terminal
    /// report; one not in memory falls back to the DB status (a finished task
    /// can't be canceled). Parent-scoped like `get_task_status`.
    pub async fn cancel_task_by_id(
        &self,
        parent_connection_id: &str,
        parent_conversation_id: Option<i32>,
        task_id: &str,
    ) -> DelegationTaskReport {
        let drained = {
            let mut inner = self.pending.inner.lock().await;
            if let Some(c) = inner.completed.get(task_id) {
                if c.parent_connection_id == parent_connection_id {
                    return completed_report(task_id, c);
                }
                return unknown_report(task_id);
            }
            match inner.running.get(task_id) {
                Some(r) if r.parent_connection_id == parent_connection_id => {
                    drain_and_record_canceled(
                        &mut inner,
                        vec![task_id.to_string()],
                        "canceled by request",
                    )
                    .pop()
                }
                Some(_) => return unknown_report(task_id),
                None => None,
            }
        };
        match drained {
            Some((task, duration_ms)) => {
                // A turn is in flight → cancel + disconnect. Reuse the duration
                // captured at drain time for both the teardown meta and the
                // report, so all three (completed-cache, meta, report) agree.
                self.teardown_canceled_child(&task, duration_ms, true).await;
                self.result_notify.notify_waiters();
                report_from_outcome(
                    Some(task_id.to_string()),
                    Some(task.agent_type),
                    &canceled_outcome(task.child_conversation_id, "canceled by request"),
                    Some(duration_ms),
                )
            }
            None => self.status_from_db(parent_conversation_id, task_id).await,
        }
    }

    /// Backs `continue_with_session` (MCP) and the user-side continuation
    /// entry: send a follow-up into an existing child session under the SAME
    /// `task_id` (Property 4), adopting the existing child conversation row
    /// (Property 2 — never a second row).
    ///
    /// Five-stage orchestration with per-stage compensation (design §阶段化补偿矩阵):
    /// S1 validate + ledger + register cancellable (R3: BEFORE any I/O) →
    /// S2 pick live connection or resume-spawn → S3 pre-send cancel check →
    /// S4 send follow-up → S5 commit to `running` (with a final cancel check).
    ///
    /// Ownership is the parent CONVERSATION id (D5): a reconnected parent with
    /// a new connection id can still continue its tasks. `continuation_id` is
    /// the caller-generated idempotency key (required; Requirement 2.13).
    pub async fn continue_delegation(
        &self,
        parent_connection_id: &str,
        parent_conversation_id: Option<i32>,
        task_id: &str,
        message: String,
        continuation_id: &str,
        origin: TurnOrigin,
    ) -> DelegationTaskReport {
        let message = message.trim().to_string();
        if message.is_empty() {
            return continuation_err_report(
                task_id,
                DelegationError::NotContinuable("message is empty".into()),
                None,
                None,
            );
        }
        if continuation_id.trim().is_empty() {
            return continuation_err_report(
                task_id,
                DelegationError::NotContinuable("continuation_id is required".into()),
                None,
                None,
            );
        }

        // --- S1: dedupe + ownership + state gate, all under one lock --------
        struct DispatchPlan {
            inflight_id: u64,
            prior_status: TaskStatus,
            prior_conn: Option<String>,
            child_conversation_id: i32,
            agent_type: AgentType,
            parent_tool_use_id: String,
            folder_id: Option<i32>,
            external_id: Option<String>,
            working_dir: Option<String>,
        }
        let plan = {
            let mut inner = self.pending.inner.lock().await;
            // Operation ledger first: a replay must return the FIRST report
            // (never `session_still_running`), and a same-id different-payload
            // request is a conflict (R2-B5).
            if let Some(op) = inner.operations.get(continuation_id) {
                if op.task_id != task_id || op.message != message {
                    return continuation_err_report(
                        task_id,
                        DelegationError::ContinuationConflict,
                        None,
                        None,
                    );
                }
                if let Some(report) = &op.report {
                    return report.clone();
                }
                // Dispatch still in flight: an equivalent Running ack.
                let (conv, at) = inner
                    .sessions
                    .get(task_id)
                    .map(|s| (Some(s.child_conversation_id), Some(s.agent_type)))
                    .unwrap_or((None, None));
                return DelegationTaskReport {
                    task_id: Some(task_id.to_string()),
                    status: TaskStatus::Running,
                    child_conversation_id: conv,
                    agent_type: at,
                    text: None,
                    error_code: None,
                    message: Some("Continuation already dispatching.".to_string()),
                    duration_ms: None,
                };
            }
            let Some(s) = inner.sessions.get_mut(task_id) else {
                if inner.rebuilding {
                    return continuation_err_report(
                        task_id,
                        DelegationError::Rebuilding,
                        None,
                        None,
                    );
                }
                return unknown_report(task_id);
            };
            // Ownership (D5): prefer the persistent parent conversation id;
            // fall back to the connection id only when the caller has no
            // conversation context. A mismatch never leaks existence.
            let owned = match parent_conversation_id {
                Some(pc) => s.parent_conversation_id == pc,
                None => s.parent_connection_id == parent_connection_id,
            };
            if !owned {
                return unknown_report(task_id);
            }
            if s.released {
                return continuation_err_report(
                    task_id,
                    DelegationError::SessionReleased,
                    Some(s.child_conversation_id),
                    Some(s.agent_type),
                );
            }
            if s.status == TaskStatus::Running {
                return continuation_err_report(
                    task_id,
                    DelegationError::SessionStillRunning,
                    Some(s.child_conversation_id),
                    Some(s.agent_type),
                );
            }
            let prior_status = s.status;
            let prior_conn = s.child_connection_id.clone();
            let plan = DispatchPlan {
                inflight_id: 0, // filled below (needs &mut inner after s drops)
                prior_status,
                prior_conn,
                child_conversation_id: s.child_conversation_id,
                agent_type: s.agent_type,
                parent_tool_use_id: s.parent_tool_use_id.clone(),
                folder_id: s.folder_id,
                external_id: s.external_id.clone(),
                working_dir: s.working_dir.clone(),
            };
            // In-dispatch marker: a concurrent continuation (different
            // continuation_id) now sees Running and is rejected; the
            // compensation paths restore `prior_status` on failure.
            s.status = TaskStatus::Running;
            // Refresh the run lease to the dispatching parent connection.
            s.parent_connection_id = parent_connection_id.to_string();
            // The connection (if any) is about to be actively used — take it
            // out of the kept-alive FIFO so a racing cap eviction can't kill
            // it mid-dispatch.
            inner.kept_alive_order.retain(|t| t != task_id);
            // R3 fix: register as cancellable BEFORE any I/O, so a parent
            // cancel landing anywhere in the dispatch window reaches this turn.
            // Task-scoped so a racing close can flag exactly this dispatch.
            let inflight_id = inner.register_inflight_for_task(parent_connection_id, task_id);
            inner.operations.insert(
                continuation_id.to_string(),
                OperationRecord {
                    task_id: task_id.to_string(),
                    message: message.clone(),
                    report: None,
                },
            );
            DispatchPlan {
                inflight_id,
                ..plan
            }
        };

        // --- resolve resume/folder metadata (cache → DB fallback) -----------
        let (folder_id, external_id, working_dir) = if plan.folder_id.is_some() {
            (
                plan.folder_id,
                plan.external_id.clone(),
                plan.working_dir.clone(),
            )
        } else {
            match self.status_lookup.find_by_call_id(task_id).await {
                Some(rec) => (Some(rec.folder_id), rec.external_id, rec.working_dir),
                None => (None, plan.external_id.clone(), plan.working_dir.clone()),
            }
        };
        let Some(folder_id) = folder_id else {
            self.abort_continuation(
                task_id,
                continuation_id,
                plan.inflight_id,
                plan.prior_status,
                plan.prior_conn.clone(),
            )
            .await;
            return continuation_err_report(
                task_id,
                DelegationError::NotContinuable("missing folder for child conversation".into()),
                Some(plan.child_conversation_id),
                Some(plan.agent_type),
            );
        };

        // --- S2: pick a live kept connection, or resume the agent session ---
        let mut newly_spawned = false;
        let conn_id = match plan.prior_conn.clone() {
            Some(cid) if self.spawner.is_alive(&cid).await => cid,
            _ => {
                // Resume validation is grounded in the DB row (Requirement
                // 7.6): re-read it even when the session carries cached
                // metadata — the row is what the credential belongs to.
                let Some(rec) = self.status_lookup.find_by_call_id(task_id).await else {
                    self.abort_continuation(
                        task_id,
                        continuation_id,
                        plan.inflight_id,
                        plan.prior_status,
                        plan.prior_conn.clone(),
                    )
                    .await;
                    return continuation_err_report(
                        task_id,
                        DelegationError::NotContinuable(
                            "child conversation row not found; cannot validate the resume".into(),
                        ),
                        Some(plan.child_conversation_id),
                        Some(plan.agent_type),
                    );
                };
                // Triple binding check (7.6): agent + child conversation +
                // folder must all match the resume target. Refuse BEFORE
                // attempting the resume — never hand a foreign credential to
                // an agent.
                let folder_bound = plan.folder_id;
                let mismatch = rec.agent_type != plan.agent_type
                    || rec.child_conversation_id != plan.child_conversation_id
                    || folder_bound.is_some_and(|f| rec.folder_id != f);
                if mismatch {
                    self.abort_continuation(
                        task_id,
                        continuation_id,
                        plan.inflight_id,
                        plan.prior_status,
                        plan.prior_conn.clone(),
                    )
                    .await;
                    return continuation_err_report(
                        task_id,
                        DelegationError::NotContinuable(
                            "stored agent/folder binding does not match the resume target".into(),
                        ),
                        Some(plan.child_conversation_id),
                        Some(plan.agent_type),
                    );
                }
                // Requirement 3.2: no resume credential + dead process = not
                // continuable. Rejected BEFORE any prompt side effect. The DB
                // row's credential wins over the session cache (3.3a: the
                // cache is never OVERWRITTEN with a lesser value — see S5).
                let Some(session_id) = rec.external_id.clone().or_else(|| external_id.clone())
                else {
                    self.abort_continuation(
                        task_id,
                        continuation_id,
                        plan.inflight_id,
                        plan.prior_status,
                        plan.prior_conn.clone(),
                    )
                    .await;
                    return continuation_err_report(
                        task_id,
                        DelegationError::NotContinuable(
                            "child process is gone and no resume credential (external_id) exists"
                                .into(),
                        ),
                        Some(plan.child_conversation_id),
                        Some(plan.agent_type),
                    );
                };
                // Ambiguity check (7.7): a credential referenced by multiple
                // child rows cannot be resolved to ONE session — refuse rather
                // than guess.
                if self.status_lookup.external_id_ref_count(&session_id).await > 1 {
                    self.abort_continuation(
                        task_id,
                        continuation_id,
                        plan.inflight_id,
                        plan.prior_status,
                        plan.prior_conn.clone(),
                    )
                    .await;
                    return continuation_err_report(
                        task_id,
                        DelegationError::ResumeUnavailable(
                            "resume credential is ambiguous (external_id referenced by \
                             multiple child sessions); refusing to guess"
                                .into(),
                        ),
                        Some(plan.child_conversation_id),
                        Some(plan.agent_type),
                    );
                }
                let cfg = self.config_snapshot().await;
                let (preferred_mode_id, preferred_config_values) = cfg
                    .agent_defaults
                    .get(&plan.agent_type)
                    .map(|d: &AgentDelegationDefaults| (d.mode_id.clone(), d.config_values.clone()))
                    .unwrap_or((None, BTreeMap::new()));
                match self
                    .spawner
                    .spawn_for_resume(
                        parent_connection_id,
                        plan.agent_type,
                        working_dir.clone(),
                        Some(session_id),
                        preferred_mode_id,
                        preferred_config_values,
                    )
                    .await
                {
                    Ok(id) => {
                        newly_spawned = true;
                        id
                    }
                    Err(e) => {
                        self.abort_continuation(
                            task_id,
                            continuation_id,
                            plan.inflight_id,
                            plan.prior_status,
                            plan.prior_conn.clone(),
                        )
                        .await;
                        // A failed resume spawn = the resume chain cannot
                        // restore the context (3.3). No prompt side effect has
                        // occurred and the credential is untouched (3.3a).
                        return continuation_err_report(
                            task_id,
                            DelegationError::ResumeUnavailable(format!(
                                "resume failed before dispatch: {e}"
                            )),
                            Some(plan.child_conversation_id),
                            Some(plan.agent_type),
                        );
                    }
                }
            }
        };

        // --- S3: pre-send cancel checkpoint ---------------------------------
        // A cancel observed BEFORE the prompt went out: nothing was dispatched,
        // so restore the settled state. Only a NEWLY spawned connection is torn
        // down (design S3 compensation); a kept-alive one survives — settled
        // children are not victims of a parent turn cancel (R2-B6 / 3.10).
        if self.take_inflight_cancel(plan.inflight_id).await {
            let evicted = {
                let mut inner = self.pending.inner.lock().await;
                inner.operations.remove(continuation_id);
                inner.settle_session(task_id, plan.prior_status, plan.prior_conn.clone())
            };
            // Re-registering the restored conn can displace another kept
            // connection under a tight cap — release it out of lock.
            for cid in evicted {
                let _ = self.spawner.disconnect(&cid).await;
            }
            if newly_spawned {
                let _ = self.spawner.disconnect(&conn_id).await;
            }
            return report_from_outcome(
                Some(task_id.to_string()),
                Some(plan.agent_type),
                &canceled_outcome(plan.child_conversation_id, "parent canceled"),
                None,
            );
        }

        // --- S4: send the follow-up prompt (Branch A row adoption) ----------
        if let Err(e) = self
            .spawner
            .send_followup_prompt(
                &conn_id,
                message.clone(),
                plan.child_conversation_id,
                folder_id,
            )
            .await
        {
            self.abort_continuation(
                task_id,
                continuation_id,
                plan.inflight_id,
                plan.prior_status,
                plan.prior_conn.clone(),
            )
            .await;
            // S4 compensation: a newly spawned connection must not be leaked;
            // a kept-alive one is left in place (usable next time).
            if newly_spawned {
                let _ = self.spawner.disconnect(&conn_id).await;
            }
            let mut r = report_err(
                plan.agent_type,
                DelegationError::SubagentRuntimeError(e.to_string()),
                Some(plan.child_conversation_id),
            );
            r.task_id = Some(task_id.to_string());
            return r;
        }

        // --- S5: commit into `running` (final cancel check under the lock) --
        let started_at = Instant::now();
        let committed = {
            let mut inner = self.pending.inner.lock().await;
            if inner.inflight_canceled(plan.inflight_id) {
                // The prompt already reached the child: this turn is live and
                // must be cancelled + torn down (leaving it would recreate the
                // unmanaged-child gap R3 exists to close).
                inner.deregister_inflight(plan.inflight_id);
                inner.operations.remove(continuation_id);
                inner.remove_completed_entry(task_id);
                inner.insert_completed(
                    task_id,
                    build_completed(
                        parent_connection_id,
                        plan.child_conversation_id,
                        plan.agent_type,
                        started_at.elapsed().as_millis() as u64,
                        &canceled_outcome(plan.child_conversation_id, "parent canceled"),
                    ),
                );
                let evicted = inner.settle_session(task_id, TaskStatus::Canceled, None);
                debug_assert!(evicted.is_empty());
                None
            } else {
                inner.deregister_inflight(plan.inflight_id);
                let (turn_id, turn_version) = {
                    let s = inner
                        .sessions
                        .get_mut(task_id)
                        .expect("session registry entries are never removed in-process");
                    let (turn_id, version) = s.push_turn(origin);
                    s.status = TaskStatus::Running;
                    s.child_connection_id = Some(conn_id.clone());
                    // Cache resume metadata for later continuations. Never
                    // overwrite an existing credential with None (3.3a).
                    s.folder_id = Some(folder_id);
                    if s.external_id.is_none() {
                        s.external_id = external_id.clone();
                    }
                    if s.working_dir.is_none() {
                        s.working_dir = working_dir.clone();
                    }
                    (turn_id, version)
                };
                // The previous turn's cached text is no longer the current
                // answer — drop it so a status poll reports Running.
                inner.remove_completed_entry(task_id);
                inner.running.insert(
                    task_id.to_string(),
                    RunningTask {
                        child_connection_id: conn_id.clone(),
                        child_conversation_id: plan.child_conversation_id,
                        parent_connection_id: parent_connection_id.to_string(),
                        parent_tool_use_id: plan.parent_tool_use_id.clone(),
                        agent_type: plan.agent_type,
                        task_preview: truncate_on_char_boundary(&message, TASK_PREVIEW_CAP),
                        task_id: task_id.to_string(),
                        external_handle: None,
                        started_at,
                        turn_version,
                        turn_id: Some(turn_id),
                        origin,
                    },
                );
                let mut ack = running_ack(
                    task_id.to_string(),
                    plan.child_conversation_id,
                    plan.agent_type,
                );
                ack.message = Some(format!(
                    "Continue successful. task_id={task_id}. Call get_delegation_status \
                     with this id in the task_ids array (optionally wait_ms) to collect \
                     the new turn's result, or cancel_delegation / close_session when done."
                ));
                if let Some(op) = inner.operations.get_mut(continuation_id) {
                    op.report = Some(ack.clone());
                }
                Some(ack)
            }
        };
        let Some(ack) = committed else {
            // Post-send cancel: tear the dispatched turn down.
            let _ = self.spawner.cancel(&conn_id).await;
            let _ = self.spawner.disconnect(&conn_id).await;
            self.result_notify.notify_waiters();
            return report_from_outcome(
                Some(task_id.to_string()),
                Some(plan.agent_type),
                &canceled_outcome(plan.child_conversation_id, "parent canceled"),
                None,
            );
        };

        // Re-announce on the ORIGINAL tool_use_id — direct addressing, never
        // the claim path (44415f56). The frontend re-attaches / cancels its
        // detach timer on this started event.
        let task_preview = truncate_on_char_boundary(&message, TASK_PREVIEW_CAP);
        self.write_meta_if_real(
            parent_connection_id,
            &plan.parent_tool_use_id,
            build_delegation_meta(
                "running",
                Some(&conn_id),
                Some(plan.child_conversation_id),
                None,
                None,
                None,
                Some(&task_preview),
                Some(task_id),
            ),
        )
        .await;
        self.emit_started_if_real(
            parent_connection_id,
            &plan.parent_tool_use_id,
            &conn_id,
            plan.child_conversation_id,
            plan.agent_type,
            &task_preview,
            task_id,
        )
        .await;
        ack
    }

    /// Compensation for a continuation that failed BEFORE dispatch committed:
    /// drop the in-flight record and the operation-ledger entry, and restore
    /// the session to its pre-dispatch settled state (status + connection,
    /// re-registering the kept-alive slot). Any connections displaced by that
    /// re-registration are disconnected here. If a concurrent close RELEASED
    /// the session mid-dispatch, `settle_session` refuses the restore and
    /// returns `prior_conn` itself — the disconnect below then performs the
    /// teardown the close deferred to this compensation path (2.12).
    async fn abort_continuation(
        &self,
        task_id: &str,
        continuation_id: &str,
        inflight_id: u64,
        prior_status: TaskStatus,
        prior_conn: Option<String>,
    ) {
        let evicted = {
            let mut inner = self.pending.inner.lock().await;
            inner.deregister_inflight(inflight_id);
            inner.operations.remove(continuation_id);
            inner.settle_session(task_id, prior_status, prior_conn)
        };
        for cid in evicted {
            let _ = self.spawner.disconnect(&cid).await;
        }
    }

    /// Backs `close_session`: RELEASE a delegated child — free its process and
    /// refuse further continuation in this process (R2-B4: release semantics,
    /// not a permanent close; permanent disposal = deleting the conversation
    /// row). A running turn is canceled first (Requirement 2.9); repeat closes
    /// are side-effect-free and return the last known status (2.10); a task
    /// with no live connection still releases (2.11); a close racing an
    /// in-dispatch continuation serializes under the pending lock and cancels
    /// it (2.12).
    pub async fn close_delegation_session(
        &self,
        parent_connection_id: &str,
        parent_conversation_id: Option<i32>,
        task_id: &str,
        continuation_id: &str,
    ) -> DelegationTaskReport {
        let (report, conn_to_disconnect, teardown) = {
            let mut inner = self.pending.inner.lock().await;
            // Ledger replay: a re-sent close returns the first report.
            if let Some(op) = inner.operations.get(continuation_id) {
                if op.task_id != task_id || op.message != CLOSE_OP_PAYLOAD {
                    return continuation_err_report(
                        task_id,
                        DelegationError::ContinuationConflict,
                        None,
                        None,
                    );
                }
                if let Some(report) = &op.report {
                    return report.clone();
                }
                // Close ops are always recorded settled; defensively fall
                // through to normal handling if not.
            }
            let Some(snap) = inner.sessions.get(task_id).map(|s| {
                (
                    s.parent_conversation_id,
                    s.parent_connection_id.clone(),
                    s.released,
                    s.status,
                    s.child_conversation_id,
                    s.agent_type,
                )
            }) else {
                if inner.rebuilding {
                    return continuation_err_report(
                        task_id,
                        DelegationError::Rebuilding,
                        None,
                        None,
                    );
                }
                return unknown_report(task_id);
            };
            let (s_parent_conv, s_parent_conn, s_released, s_status, s_child_conv, s_agent) = snap;
            let owned = match parent_conversation_id {
                Some(pc) => s_parent_conv == pc,
                None => s_parent_conn == parent_connection_id,
            };
            if !owned {
                return unknown_report(task_id);
            }
            if s_released {
                // Idempotent repeat (2.10): last known status, zero side
                // effects — not even a ledger write.
                return last_known_report_locked(&inner, task_id, s_status, s_child_conv, s_agent);
            }
            // A running turn is canceled first (2.9). Two shapes: a committed
            // turn sits in `running` (drain it); an in-dispatch continuation
            // only has its in-flight record (flag it — the dispatch's S3/S5
            // checkpoints observe the flag and cancel themselves, 2.12).
            let teardown = if inner.running.contains_key(task_id) {
                drain_and_record_canceled(&mut inner, vec![task_id.to_string()], "session released")
                    .pop()
            } else {
                if s_status == TaskStatus::Running {
                    inner.mark_inflight_canceled_for_task(task_id);
                }
                None
            };
            let was_running = s_status == TaskStatus::Running;
            // Release: mark, take the kept connection, leave the FIFO. The
            // session entry itself survives (ownership facts stay queryable).
            let conn = {
                let s = inner
                    .sessions
                    .get_mut(task_id)
                    .expect("session registry entries are never removed in-process");
                s.released = true;
                if was_running {
                    s.status = TaskStatus::Canceled;
                }
                s.child_connection_id.take()
            };
            inner.kept_alive_order.retain(|t| t != task_id);
            // The operation ledger follows the session's lease (Requirement
            // 7.4 posture): release invalidates this task's continuations.
            inner.operations.retain(|_, op| op.task_id != task_id);
            let report = if was_running {
                report_from_outcome(
                    Some(task_id.to_string()),
                    Some(s_agent),
                    &canceled_outcome(s_child_conv, "session released"),
                    None,
                )
            } else {
                last_known_report_locked(&inner, task_id, s_status, s_child_conv, s_agent)
            };
            inner.operations.insert(
                continuation_id.to_string(),
                OperationRecord {
                    task_id: task_id.to_string(),
                    message: CLOSE_OP_PAYLOAD.to_string(),
                    report: Some(report.clone()),
                },
            );
            // An in-dispatch continuation owns its connection's fate (its
            // cancel checkpoints tear it down) — don't double-disconnect.
            let conn_to_disconnect = if was_running && teardown.is_none() {
                None
            } else {
                conn
            };
            (report, conn_to_disconnect, teardown)
        };
        // Slow I/O out of lock. The canceled-turn teardown is backgrounded
        // (same posture as `cancel_by_parent_turn`) so close stays responsive
        // even against a slow agent — the release is already committed.
        if let Some((task, duration_ms)) = teardown {
            let broker = self.clone();
            tokio::spawn(async move {
                broker
                    .teardown_canceled_child(&task, duration_ms, true)
                    .await;
            });
        }
        if let Some(cid) = conn_to_disconnect {
            if let Err(e) = self.spawner.disconnect(&cid).await {
                // The child process may still be alive: surface it (observable
                // `orphan_suspect`) and retry in the background. The
                // `--parent-pid` watchdog is the final backstop.
                tracing::warn!(
                    "[delegation] release disconnect failed for {cid}: {e}; \
                     marking orphan_suspect and scheduling a retry"
                );
                {
                    let mut inner = self.pending.inner.lock().await;
                    if let Some(s) = inner.sessions.get_mut(task_id) {
                        s.orphan_suspect = true;
                    }
                }
                let spawner = self.spawner.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let _ = spawner.disconnect(&cid).await;
                });
            }
        }
        self.result_notify.notify_waiters();
        report
    }

    /// Backs the user-side `get_continuation_availability` query (design §D4):
    /// project one child session onto the five-tier verdict the Sub Agent
    /// Session Dialog uses to enable/disable its input. Located by the child
    /// CONVERSATION id (D5 — the only stable handle the user side holds).
    ///
    /// The verdict is advisory: the actual `continue_delegation` re-validates
    /// everything under the pending lock, so a stale answer here can only cost
    /// the user one rejected attempt, never a wrong dispatch. An id with no
    /// session entry (never delegated, startup rebuild filtered it out, or
    /// rebuild still in flight) folds into `NotContinuable` — one verdict for
    /// "unknown" and "cannot continue", so existence is never disclosed.
    pub async fn get_continuation_availability(
        &self,
        child_conversation_id: i32,
    ) -> ContinuationAvailability {
        // Snapshot under the lock; the (slow) spawner probes run after release.
        let snap = {
            let inner = self.pending.inner.lock().await;
            inner
                .sessions
                .iter()
                .find(|(_, s)| s.child_conversation_id == child_conversation_id)
                .map(|(task_id, s)| {
                    (
                        task_id.clone(),
                        s.released,
                        s.status,
                        s.child_connection_id.clone(),
                        s.external_id.clone(),
                    )
                })
        };
        let Some((task_id, released, status, conn_id, cached_external)) = snap else {
            return ContinuationAvailability::NotContinuable;
        };
        if released {
            return ContinuationAvailability::Released;
        }
        if status == TaskStatus::Running {
            return ContinuationAvailability::Running;
        }
        if let Some(cid) = conn_id.as_deref() {
            if self.spawner.is_alive(cid).await {
                return ContinuationAvailability::ContinuableLive;
            }
        }
        // Connection dead/absent: a resume needs a credential. The session
        // cache is lazily filled, so fall back to the child conversation row
        // (the credential's true home) before concluding "none".
        let has_credential = cached_external.is_some()
            || self
                .status_lookup
                .find_by_call_id(&task_id)
                .await
                .and_then(|rec| rec.external_id)
                .is_some();
        if !has_credential {
            return ContinuationAvailability::NotContinuable;
        }
        // D3.1: the agent self-reported its continuation capability on
        // `initialize`. If the dead connection's state is still around and
        // says neither `session/load` nor resume is supported, the process
        // death lost the context for good (LiveOnly agent). Unknown state
        // (already removed) stays optimistic — the resume attempt itself
        // reports `resume_unavailable` when the capability was truly absent.
        if let Some(cid) = conn_id.as_deref() {
            if let Some(cap) = self.spawner.continuation_capability(cid).await {
                if !cap.supports_resume && !cap.supports_load_session {
                    return ContinuationAvailability::NotContinuable;
                }
            }
        }
        ContinuationAvailability::ContinuableResume
    }

    /// Requirement 1.4a: the parent CONVERSATION was deleted — the owner is
    /// gone for good, so every child session of that conversation is released:
    /// running turns are canceled + torn down, kept-alive connections are
    /// disconnected, and further continuation is refused. This is the ONLY
    /// parent-side event that frees kept-alive children; a mere parent
    /// CONNECTION teardown does not (Requirement 1.4 / R2-B6).
    pub async fn release_children_of_parent_conversation(&self, parent_conversation_id: i32) {
        let (kept_conns, drained) = {
            let mut inner = self.pending.inner.lock().await;
            let task_ids: Vec<String> = inner
                .sessions
                .iter()
                .filter(|(_, s)| s.parent_conversation_id == parent_conversation_id)
                .map(|(k, _)| k.clone())
                .collect();
            // Cancel running turns first (atomic running → completed).
            let running_keys: Vec<String> = task_ids
                .iter()
                .filter(|t| inner.running.contains_key(t.as_str()))
                .cloned()
                .collect();
            let drained =
                drain_and_record_canceled(&mut inner, running_keys, "parent conversation deleted");
            // Release every session: mark, take kept connections, clear the
            // FIFO slots and this conversation's operation-ledger entries.
            let mut kept_conns = Vec::new();
            for t in &task_ids {
                // In-dispatch continuations get their in-flight record flagged
                // so their own checkpoints cancel them (same seam as close).
                inner.mark_inflight_canceled_for_task(t);
                if let Some(s) = inner.sessions.get_mut(t.as_str()) {
                    s.released = true;
                    if s.status == TaskStatus::Running {
                        s.status = TaskStatus::Canceled;
                    }
                    if let Some(cid) = s.child_connection_id.take() {
                        kept_conns.push(cid);
                    }
                }
                inner.kept_alive_order.retain(|k| k != t);
                inner.operations.retain(|_, op| &op.task_id != t);
            }
            // Drained running tasks already had their connection taken by
            // `settle_session(.., None)` inside the drain — their teardown
            // below handles the disconnect; don't double-free.
            let drained_conns: Vec<String> = drained
                .iter()
                .map(|(t, _)| t.child_connection_id.clone())
                .collect();
            kept_conns.retain(|c| !drained_conns.contains(c));
            (kept_conns, drained)
        };
        for (task, duration_ms) in drained {
            self.teardown_canceled_child(&task, duration_ms, true).await;
        }
        for cid in kept_conns {
            let _ = self.spawner.disconnect(&cid).await;
        }
        self.result_notify.notify_waiters();
    }

    /// Startup rebuild protocol (Requirement 7.1-7.3): reconstruct the
    /// continuable session index from persisted child conversation rows.
    /// Rules:
    /// * a row whose parent conversation is gone / dangling never enters the
    ///   index (ownership validation; observable via the warn) — and never
    ///   aborts the rest of the scan (single-row isolation);
    /// * turn history and the operation ledger do NOT cross restarts — a
    ///   rebuilt session starts at `turn_version 0` with an empty ledger;
    /// * a LIVE in-memory entry always wins over the DB reconstruction (a
    ///   second call must not clobber kept-alive state);
    /// * while the scan runs, session-misses answer the retryable
    ///   `rebuilding` code (Requirement 7.2).
    pub async fn rebuild_sessions_from_db(&self) {
        {
            let mut inner = self.pending.inner.lock().await;
            inner.rebuilding = true;
        }
        let candidates = self.status_lookup.list_rebuildable().await;
        let mut inner = self.pending.inner.lock().await;
        for c in candidates {
            let Some(parent_conversation_id) = c.parent_conversation_id else {
                tracing::warn!(
                    "[delegation] rebuild: child conversation {} has no parent; not continuable",
                    c.child_conversation_id
                );
                continue;
            };
            if !c.parent_alive {
                tracing::warn!(
                    "[delegation] rebuild: parent conversation {} of child {} is gone; not continuable",
                    parent_conversation_id,
                    c.child_conversation_id
                );
                continue;
            }
            if c.task_id.trim().is_empty() {
                tracing::warn!(
                    "[delegation] rebuild: child conversation {} carries an empty delegation_call_id; skipped",
                    c.child_conversation_id
                );
                continue;
            }
            if inner.sessions.contains_key(&c.task_id) {
                continue;
            }
            inner.sessions.insert(
                c.task_id.clone(),
                SessionEntry {
                    parent_conversation_id,
                    // No live run lease after a restart; ownership checks fall
                    // back to the (persistent) parent conversation id.
                    parent_connection_id: String::new(),
                    // A missing tool_use_id degrades to the synthetic shape,
                    // which the meta/event writers already skip.
                    parent_tool_use_id: c
                        .parent_tool_use_id
                        .clone()
                        .unwrap_or_else(|| format!("delegation-{}", c.task_id)),
                    agent_type: c.agent_type,
                    child_conversation_id: c.child_conversation_id,
                    child_connection_id: None,
                    status: c.status,
                    released: false,
                    orphan_suspect: false,
                    turn_version: 0,
                    turns: VecDeque::new(),
                    folder_id: Some(c.folder_id),
                    external_id: c.external_id.clone(),
                    working_dir: c.working_dir.clone(),
                },
            );
        }
        inner.rebuilding = false;
    }

    /// DB status fallback for a task evicted from / never in the in-memory maps.
    /// Scopes to the caller's conversation: a child whose `parent_id` doesn't
    /// match (or when the caller has no active conversation) reports `Unknown`.
    async fn status_from_db(
        &self,
        parent_conversation_id: Option<i32>,
        task_id: &str,
    ) -> DelegationTaskReport {
        match self.status_lookup.find_by_call_id(task_id).await {
            Some(rec)
                if parent_conversation_id.is_some() && rec.parent_id == parent_conversation_id =>
            {
                db_report(task_id, &rec)
            }
            _ => unknown_report(task_id),
        }
    }

    /// Test-only shim preserving the old blocking `handle_request` contract over
    /// the async path: start the delegation, then block until it reaches a
    /// terminal state (driven by the test's `complete_call` / cancel), mapping
    /// the terminal report back to a `DelegationOutcome`. Keeps the broker's
    /// extensive setup-window race tests exercising the same lifecycle without
    /// each rewriting to the start/poll/collect shape.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn handle_request(&self, req: DelegationRequest) -> DelegationOutcome {
        let parent_connection_id = req.parent_connection_id.clone();
        let parent_conversation_id = Some(req.parent_conversation_id);
        let ack = self.start_delegation(req).await;
        let task_id = match ack.task_id.clone() {
            Some(id) => id,
            // Setup failed before a task existed — the ack itself is terminal.
            None => return report_to_outcome(&ack),
        };
        if ack.status != TaskStatus::Running {
            return report_to_outcome(&ack);
        }
        // Block until terminal via the long-poll path (re-issued so an
        // indefinitely-pending task in a test simply parks here, mirroring the
        // old unbounded `rx.await`).
        loop {
            let report = self
                .get_task_status(
                    &parent_connection_id,
                    parent_conversation_id,
                    &task_id,
                    StatusWait::Bounded(3_600_000),
                )
                .await;
            if report.status != TaskStatus::Running {
                return report_to_outcome(&report);
            }
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn peek_first_pending_call_id(&self) -> Option<String> {
        self.pending
            .inner
            .lock()
            .await
            .running
            .keys()
            .next()
            .cloned()
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn pending_count(&self) -> usize {
        self.pending.inner.lock().await.running.len()
    }

    /// Count of cached completed results across all parents.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn completed_count(&self) -> usize {
        self.pending.inner.lock().await.completed.len()
    }

    /// Count of in-flight (registered-at-entry, not-yet-parked / not-yet-exited)
    /// `handle_request` setups. Should return to 0 on every exit path.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn inflight_count(&self) -> usize {
        self.pending.inner.lock().await.inflight.len()
    }

    /// Count of in-setup (reserved, not-yet-parked) delegations. Each holds one
    /// child and one call_id, so this counts both.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn reserved_child_count(&self) -> usize {
        self.pending.inner.lock().await.setups.len()
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn reserved_call_count(&self) -> usize {
        self.pending.inner.lock().await.setups.len()
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn early_cancel_count(&self) -> usize {
        self.pending.inner.lock().await.early_cancels.len()
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn early_complete_count(&self) -> usize {
        self.pending.inner.lock().await.early_completes.len()
    }

    /// First reserved (mid-setup) `call_id`, if any — lets a test resolve a
    /// delegation via `complete_call` while it's pinned in the reserve→park
    /// window (its entry isn't parked yet, so `peek_first_pending_call_id`
    /// can't see it).
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn peek_reserved_call_id(&self) -> Option<String> {
        self.pending
            .inner
            .lock()
            .await
            .setups
            .keys()
            .next()
            .cloned()
    }
}

/// `ConversationDepthLookup` over the live `AppDatabase`. Used by the
/// production wiring; tests use the in-module `MockDepth`.
pub struct DbDepthLookup {
    pub db: Arc<crate::db::AppDatabase>,
}

#[async_trait]
impl ConversationDepthLookup for DbDepthLookup {
    async fn parent_of(&self, conversation_id: i32) -> Result<Option<i32>, DelegationError> {
        use sea_orm::EntityTrait;
        let row = crate::db::entities::conversation::Entity::find_by_id(conversation_id)
            .one(&self.db.conn)
            .await
            .map_err(|e| DelegationError::SubagentRuntimeError(format!("db: {e}")))?;
        Ok(row.and_then(|r| r.parent_id))
    }
}

/// `ChildStatusLookup` over the live `AppDatabase`. Recovers a delegation
/// task's terminal status (NOT its text — child output isn't in codeg's DB)
/// from the child conversation row once its in-memory result was evicted.
pub struct DbChildStatusLookup {
    pub db: Arc<crate::db::AppDatabase>,
}

#[async_trait]
impl ChildStatusLookup for DbChildStatusLookup {
    async fn find_by_call_id(&self, call_id: &str) -> Option<ChildStatusRecord> {
        let summary = crate::db::service::conversation_service::get_by_delegation_call_id(
            &self.db.conn,
            call_id,
        )
        .await
        .ok()
        .flatten()?;
        // `summary.status` is the serialized `ConversationStatus` string.
        let status = match summary.status.as_str() {
            "in_progress" => TaskStatus::Running,
            "pending_review" | "completed" => TaskStatus::Completed,
            "cancelled" => TaskStatus::Canceled,
            _ => TaskStatus::Unknown,
        };
        // Resolve the workspace path for a resume spawn (folder path). Missing
        // folder rows degrade to `None` — the continuation then falls back to
        // the parent connection's working_dir inside `spawn_for_resume`.
        let working_dir =
            crate::db::service::folder_service::get_folder_by_id(&self.db.conn, summary.folder_id)
                .await
                .ok()
                .flatten()
                .map(|f| f.path);
        Some(ChildStatusRecord {
            child_conversation_id: summary.id,
            status,
            agent_type: summary.agent_type,
            parent_id: summary.parent_id,
            folder_id: summary.folder_id,
            external_id: summary.external_id.clone(),
            working_dir,
        })
    }

    async fn external_id_ref_count(&self, external_id: &str) -> usize {
        use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
        crate::db::entities::conversation::Entity::find()
            .filter(crate::db::entities::conversation::Column::ExternalId.eq(external_id))
            .filter(crate::db::entities::conversation::Column::DeletedAt.is_null())
            .count(&self.db.conn)
            .await
            // A failed count must not silently authorize a resume: report the
            // ambiguous-side value so the caller refuses (fail closed).
            .map(|n| n as usize)
            .unwrap_or(usize::MAX)
    }

    async fn list_rebuildable(&self) -> Vec<RebuildCandidate> {
        use crate::db::entities::conversation;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let rows = match conversation::Entity::find()
            .filter(conversation::Column::Kind.eq(conversation::ConversationKind::Delegate))
            .filter(conversation::Column::DeletedAt.is_null())
            .filter(conversation::Column::ExternalId.is_not_null())
            .filter(conversation::Column::DelegationCallId.is_not_null())
            .all(&self.db.conn)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                // An unreadable table yields an empty rebuild — sessions fall
                // back to `Unknown` (queryable via the DB status path once the
                // DB recovers). Logged, not swallowed silently.
                tracing::warn!("[delegation] rebuild scan failed: {e}");
                return Vec::new();
            }
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            // Ownership validation: the parent row must exist and not be
            // soft-deleted. Resolved here so the broker stays storage-agnostic.
            let parent_alive = match row.parent_id {
                Some(pid) => conversation::Entity::find_by_id(pid)
                    .filter(conversation::Column::DeletedAt.is_null())
                    .one(&self.db.conn)
                    .await
                    .ok()
                    .flatten()
                    .is_some(),
                None => false,
            };
            let status = match row.status {
                conversation::ConversationStatus::InProgress => TaskStatus::Running,
                conversation::ConversationStatus::PendingReview
                | conversation::ConversationStatus::Completed => TaskStatus::Completed,
                conversation::ConversationStatus::Cancelled => TaskStatus::Canceled,
            };
            let working_dir =
                crate::db::service::folder_service::get_folder_by_id(&self.db.conn, row.folder_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|f| f.path);
            let agent_type = match serde_json::from_value::<AgentType>(serde_json::Value::String(
                row.agent_type.clone(),
            )) {
                Ok(at) => at,
                Err(_) => {
                    tracing::warn!(
                        "[delegation] rebuild: child conversation {} has unknown agent_type {:?}; skipped",
                        row.id,
                        row.agent_type
                    );
                    continue;
                }
            };
            out.push(RebuildCandidate {
                task_id: row.delegation_call_id.clone().unwrap_or_default(),
                child_conversation_id: row.id,
                parent_conversation_id: row.parent_id,
                parent_alive,
                parent_tool_use_id: row.parent_tool_use_id.clone(),
                agent_type,
                status,
                folder_id: row.folder_id,
                external_id: row.external_id.clone(),
                working_dir,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::delegation::spawner::{mock::MockSpawner, SpawnerError};
    use crate::acp::delegation::types::DelegationSuccess;
    use crate::models::AgentType;

    /// Test-only `ConversationDepthLookup` that resolves against a flat
    /// (id, parent_id) table. Unknown ids return `Ok(None)` to keep test
    /// setup small.
    struct MockDepth(Vec<(i32, Option<i32>)>);

    #[async_trait]
    impl ConversationDepthLookup for MockDepth {
        async fn parent_of(&self, id: i32) -> Result<Option<i32>, DelegationError> {
            Ok(self.0.iter().find(|(c, _)| *c == id).and_then(|(_, p)| *p))
        }
    }

    fn shallow_lookup() -> Arc<dyn ConversationDepthLookup> {
        // parent conversation is the root — depth = 0, no rejection.
        Arc::new(MockDepth(vec![(1, None)])) as Arc<dyn ConversationDepthLookup>
    }

    fn request(parent_conv: i32, tool_use: &str) -> DelegationRequest {
        DelegationRequest {
            parent_connection_id: "parent-conn".into(),
            parent_conversation_id: parent_conv,
            parent_tool_use_id: tool_use.into(),
            agent_type: AgentType::ClaudeCode,
            task: "do x".into(),
            working_dir: None,
            requested_working_dir: None,
            external_handle: None,
        }
    }

    fn request_with_handle(parent_conv: i32, tool_use: &str, handle: &str) -> DelegationRequest {
        let mut r = request(parent_conv, tool_use);
        r.external_handle = Some(handle.to_string());
        r
    }

    /// Bring the broker's `enabled` switch up before driving any test that
    /// hits `handle_request`. Production now defaults to `enabled: false`,
    /// so a bare `DelegationBroker::new(...)` would short-circuit before
    /// parking a pending entry. Tests that assert disabled behavior set
    /// their own config explicitly and skip this helper.
    async fn enable_delegation(broker: &DelegationBroker) {
        broker
            .set_config(DelegationConfig {
                enabled: true,
                ..DelegationConfig::default()
            })
            .await;
    }

    // -- Task 4.3 -----------------------------------------------------------

    #[tokio::test]
    async fn config_round_trip() {
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .set_config(DelegationConfig {
                enabled: false,
                depth_limit: 5,
                ..DelegationConfig::default()
            })
            .await;
        let got = broker.config_snapshot().await;
        assert!(!got.enabled);
        assert_eq!(got.depth_limit, 5);
    }

    #[tokio::test]
    async fn disabled_returns_canceled_without_touching_spawner() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        broker
            .set_config(DelegationConfig {
                enabled: false,
                depth_limit: 2,
                ..DelegationConfig::default()
            })
            .await;
        let outcome = broker.handle_request(request(1, "pt-1")).await;
        match outcome {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "canceled"),
            _ => panic!("expected Err"),
        }
        assert!(mock.disconnects.lock().await.is_empty());
    }

    // -- Task 4.4: happy path ----------------------------------------------

    #[tokio::test]
    async fn happy_path_returns_ok_after_complete_call() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-1".into())).await;
        mock.queue_send(Ok(42)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-1")).await })
        };

        // Spin until the broker has registered the pending call so the test
        // doesn't race the spawn/send awaits.
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "4".into(),
                    child_conversation_id: 42,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 50,
                    token_usage: None,
                }),
            )
            .await;

        let outcome = driver.await.unwrap();
        match outcome {
            DelegationOutcome::Ok(s) => {
                assert_eq!(s.text, "4");
                assert_eq!(s.child_conversation_id, 42);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
        assert_eq!(broker.pending_count().await, 0);
        // Completed keeps the child process for continue_with_session; only
        // cancel-coded outcomes (and close_session) tear it down.
        assert!(
            mock.disconnects.lock().await.is_empty(),
            "complete_call must not disconnect on success"
        );
    }

    /// Property 3 (terminal-outcome routing): a FAILED outcome — any wire code
    /// other than `"canceled"` — keeps the child connection alive exactly like
    /// `Completed`, so `continue_with_session` can retry / follow up with the
    /// child's context intact. Only cancel-coded outcomes tear the child down.
    #[tokio::test]
    async fn failed_outcome_keeps_child_connection_alive() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-fail-keep".into())).await;
        mock.queue_send(Ok(43)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let ack = broker.start_delegation(request(1, "pt-1")).await;
        assert_eq!(ack.status, TaskStatus::Running);
        let task_id = ack.task_id.expect("running task carries an id");

        broker
            .complete_call(
                &task_id,
                DelegationOutcome::from_err(DelegationError::ChildRefusal, Some(43)),
            )
            .await;

        let report = broker
            .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
            .await;
        assert_eq!(report.status, TaskStatus::Failed);
        assert!(
            mock.disconnects.lock().await.is_empty(),
            "a failed (non-cancel) outcome must keep the child alive"
        );
    }

    /// Property 3 counterpart: a cancel-coded outcome reaching the terminal
    /// path STILL disconnects the child — the parent abandoned the turn, so
    /// nothing may keep its process around.
    #[tokio::test]
    async fn canceled_outcome_still_disconnects_child() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-cancel-drop".into())).await;
        mock.queue_send(Ok(44)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let ack = broker.start_delegation(request(1, "pt-1")).await;
        let task_id = ack.task_id.expect("running task carries an id");

        broker
            .complete_call(
                &task_id,
                DelegationOutcome::from_err(
                    DelegationError::Canceled {
                        reason: "test cancel".into(),
                    },
                    Some(44),
                ),
            )
            .await;

        let report = broker
            .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
            .await;
        assert_eq!(report.status, TaskStatus::Canceled);
        assert_eq!(
            mock.disconnects.lock().await.as_slice(),
            &["c-cancel-drop"],
            "a cancel-coded outcome must still tear the child down"
        );
    }

    /// D2 three-layer split: the session registry (domain layer) records
    /// ownership + the kept-alive connection independently of the
    /// completed-cache (text layer). Dropping the parent's completed-cache
    /// entries must NOT lose the session's keep-alive facts.
    #[tokio::test]
    async fn settled_session_survives_completed_cache_drop() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-keep".into())).await;
        mock.queue_send(Ok(51)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let ack = broker.start_delegation(request(1, "pt-1")).await;
        let task_id = ack.task_id.expect("running task carries an id");
        broker
            .complete_call(
                &task_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "first".into(),
                    child_conversation_id: 51,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;

        // Simulate a full text-cache eviction for this parent.
        {
            let mut inner = broker.pending.inner.lock().await;
            inner.drop_completed_for_parent("parent-conn");
            let s = inner
                .sessions
                .get(&task_id)
                .expect("session must survive the cache drop");
            assert_eq!(s.parent_conversation_id, 1);
            assert_eq!(s.parent_connection_id, "parent-conn");
            assert_eq!(s.parent_tool_use_id, "pt-1");
            assert_eq!(s.agent_type, AgentType::ClaudeCode);
            assert_eq!(s.child_conversation_id, 51);
            assert_eq!(s.status, TaskStatus::Completed);
            assert_eq!(
                s.child_connection_id.as_deref(),
                Some("c-keep"),
                "keep-alive connection must live on the session, not the text cache"
            );
        }

        // A canceled turn instead clears the session's connection.
        mock.queue_spawn(Ok("c-drop".into())).await;
        mock.queue_send(Ok(52)).await;
        let ack2 = broker.start_delegation(request(1, "pt-2")).await;
        let task2 = ack2.task_id.expect("running task carries an id");
        broker
            .cancel_task_by_id("parent-conn", Some(1), &task2)
            .await;
        let inner = broker.pending.inner.lock().await;
        let s2 = inner.sessions.get(&task2).expect("session exists");
        assert_eq!(s2.status, TaskStatus::Canceled);
        assert!(
            s2.child_connection_id.is_none(),
            "cancel must clear the session's connection"
        );
    }

    /// Drive one full delegation to a Completed settle. Consumes one queued
    /// `(spawn, send)` pair from the mock and returns the task id.
    async fn settle_one(
        broker: &DelegationBroker,
        mock: &MockSpawner,
        child_conn: &str,
        child_conv: i32,
        tool_use: &str,
    ) -> String {
        mock.queue_spawn(Ok(child_conn.into())).await;
        mock.queue_send(Ok(child_conv)).await;
        let ack = broker.start_delegation(request(1, tool_use)).await;
        let task_id = ack.task_id.expect("running task carries an id");
        broker
            .complete_call(
                &task_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: format!("result of {child_conn}"),
                    child_conversation_id: child_conv,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        task_id
    }

    /// Property 1 (connection conservation), count-eviction path: when the
    /// number of kept-alive settled connections exceeds `kept_alive_cap`, the
    /// OLDEST settled connection is handed to `disconnect` (never silently
    /// dropped), its session's connection pointer is cleared, and the task's
    /// result text survives so a later continuation can go through the resume
    /// path (Requirements 5.4 / 5.6).
    #[tokio::test]
    async fn kept_alive_cap_fifo_evicts_oldest_settled_connection() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        broker
            .set_config(DelegationConfig {
                enabled: true,
                kept_alive_cap: 2,
                ..DelegationConfig::default()
            })
            .await;

        let t0 = settle_one(&broker, &mock, "c-cap-0", 60, "pt-cap-0").await;
        let t1 = settle_one(&broker, &mock, "c-cap-1", 61, "pt-cap-1").await;
        assert!(
            mock.disconnects.lock().await.is_empty(),
            "under the cap nothing is evicted"
        );

        // Third settle exceeds cap=2 → the FIRST kept connection is evicted.
        let _t2 = settle_one(&broker, &mock, "c-cap-2", 62, "pt-cap-2").await;
        assert_eq!(
            mock.disconnects.lock().await.as_slice(),
            &["c-cap-0"],
            "the oldest settled connection must be handed to disconnect"
        );

        let inner = broker.pending.inner.lock().await;
        let s0 = inner.sessions.get(&t0).expect("session survives eviction");
        assert!(
            s0.child_connection_id.is_none(),
            "evicted connection must be cleared from the session"
        );
        assert_eq!(
            s0.status,
            TaskStatus::Completed,
            "eviction is a resource action, not a domain-status change"
        );
        assert!(
            inner.completed.contains_key(&t0),
            "result text must survive connection eviction (Requirement 5.6)"
        );
        let s1 = inner.sessions.get(&t1).expect("session exists");
        assert!(
            s1.child_connection_id.is_some(),
            "younger kept connections stay alive"
        );
    }

    /// Property 1, per-parent scope (Requirement 5.5): the cap is enforced per
    /// parent conversation as well. Exercised at the `PendingInner` level with
    /// a per-parent cap tighter than the global one, because with equal values
    /// the global check always fires first (parent count <= global count).
    #[tokio::test]
    async fn kept_alive_per_parent_cap_evicts_within_that_parent() {
        let mut inner = PendingInner {
            kept_alive_cap_global: 10,
            kept_alive_cap_per_parent: 1,
            ..Default::default()
        };
        let entry = |parent: i32, conv: i32| SessionEntry {
            parent_conversation_id: parent,
            parent_connection_id: format!("pc-{parent}"),
            parent_tool_use_id: "pt".into(),
            agent_type: AgentType::ClaudeCode,
            child_conversation_id: conv,
            child_connection_id: Some(format!("conn-{conv}")),
            status: TaskStatus::Running,
            released: false,
            orphan_suspect: false,
            turn_version: 1,
            turns: VecDeque::new(),
            folder_id: None,
            external_id: None,
            working_dir: None,
        };
        inner.sessions.insert("a1".into(), entry(1, 11));
        inner.sessions.insert("a2".into(), entry(1, 12));
        inner.sessions.insert("b1".into(), entry(2, 21));

        let e1 = inner.settle_session("a1", TaskStatus::Completed, Some("conn-11".into()));
        assert!(e1.is_empty(), "first kept conn of parent 1 fits");
        let e2 = inner.settle_session("b1", TaskStatus::Completed, Some("conn-21".into()));
        assert!(e2.is_empty(), "parent 2 has its own budget");
        // Second kept conn for parent 1 breaches its per-parent cap of 1: the
        // OLDER one of THAT parent is evicted; parent 2 is untouched.
        let e3 = inner.settle_session("a2", TaskStatus::Completed, Some("conn-12".into()));
        assert_eq!(
            e3,
            vec!["conn-11".to_string()],
            "per-parent eviction must pick the oldest of the same parent"
        );
        assert!(inner
            .sessions
            .get("a1")
            .unwrap()
            .child_connection_id
            .is_none());
        assert!(inner
            .sessions
            .get("b1")
            .unwrap()
            .child_connection_id
            .is_some());
    }

    /// Byte-valve counterpart of Property 1: evicting a task's TEXT from the
    /// completed-cache must NOT touch its kept-alive connection — the session
    /// still owns it (D2 decoupling), so nothing leaks and nothing is killed
    /// by a text-budget decision.
    #[tokio::test]
    async fn byte_eviction_keeps_connection_on_session() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        broker
            .set_config(DelegationConfig {
                enabled: true,
                // Tiny text budget: the second result evicts the first's text.
                completed_cache_cap_bytes: 20,
                ..DelegationConfig::default()
            })
            .await;

        let t0 = settle_one(&broker, &mock, "c-byte-0", 70, "pt-byte-0").await;
        let _t1 = settle_one(&broker, &mock, "c-byte-1", 71, "pt-byte-1").await;

        let inner = broker.pending.inner.lock().await;
        assert!(
            !inner.completed.contains_key(&t0),
            "precondition: the first task's text was byte-evicted"
        );
        assert_eq!(
            inner
                .sessions
                .get(&t0)
                .and_then(|s| s.child_connection_id.as_deref()),
            Some("c-byte-0"),
            "text eviction must not clear the session's kept connection"
        );
        assert!(
            mock.disconnects.lock().await.is_empty(),
            "a text-budget decision must never kill a child process"
        );
    }

    // -- Task 5.2: get_continuation_availability (design §D4) ---------------

    /// An id the broker has never seen folds into `NotContinuable` — the same
    /// verdict as "cannot continue", so the query discloses nothing about
    /// whether a conversation exists.
    #[tokio::test]
    async fn availability_unknown_child_is_not_continuable() {
        let mock = Arc::new(MockSpawner::new());
        let broker = DelegationBroker::new(mock as Arc<dyn ConnectionSpawner>, shallow_lookup());
        assert_eq!(
            broker.get_continuation_availability(424_242).await,
            ContinuationAvailability::NotContinuable
        );
    }

    /// The Running / ContinuableLive / Released tiers, driven through the
    /// real lifecycle: an in-flight turn reports `Running`, a settled task
    /// with its kept-alive connection reports `ContinuableLive`, and a closed
    /// (released) session reports `Released`.
    #[tokio::test]
    async fn availability_running_live_and_released_tiers() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        broker
            .set_config(DelegationConfig {
                enabled: true,
                ..DelegationConfig::default()
            })
            .await;

        // Running: dispatched, not yet terminal.
        mock.queue_spawn(Ok("c-av-run".into())).await;
        mock.queue_send(Ok(80)).await;
        let ack = broker.start_delegation(request(1, "pt-av-run")).await;
        let running_task = ack.task_id.expect("running task id");
        assert_eq!(
            broker.get_continuation_availability(80).await,
            ContinuationAvailability::Running
        );

        // ContinuableLive: settled with the kept connection still alive.
        broker
            .complete_call(
                &running_task,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 80,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        assert_eq!(
            broker.get_continuation_availability(80).await,
            ContinuationAvailability::ContinuableLive
        );

        // Released: close_session retires it.
        broker
            .close_delegation_session("parent-conn", Some(1), &running_task, "cid-av-close")
            .await;
        assert_eq!(
            broker.get_continuation_availability(80).await,
            ContinuationAvailability::Released
        );
    }

    /// Dead connection + resume credential (from the DB row — the session
    /// cache is lazily filled) → `ContinuableResume`. The capability of the
    /// dead connection is unknown to the mock (no entry), which must stay
    /// optimistic rather than blocking the resume path.
    #[tokio::test]
    async fn availability_dead_connection_with_credential_is_resume() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup())
                .with_status_lookup(Arc::new(StaticStatusLookup(continue_status_record(81))));
        broker
            .set_config(DelegationConfig {
                enabled: true,
                ..DelegationConfig::default()
            })
            .await;
        let t = settle_one(&broker, &mock, "c-av-dead", 81, "pt-av-dead").await;
        let _ = t;
        mock.mark_dead("c-av-dead").await;
        assert_eq!(
            broker.get_continuation_availability(81).await,
            ContinuationAvailability::ContinuableResume
        );
    }

    /// Dead connection + NO resume credential anywhere (session cache empty,
    /// DB lookup answers nothing) → `NotContinuable` (Requirement 3.2's
    /// query-side mirror).
    #[tokio::test]
    async fn availability_dead_connection_without_credential_is_not_continuable() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        broker
            .set_config(DelegationConfig {
                enabled: true,
                ..DelegationConfig::default()
            })
            .await;
        settle_one(&broker, &mock, "c-av-nocred", 82, "pt-av-nocred").await;
        mock.mark_dead("c-av-nocred").await;
        assert_eq!(
            broker.get_continuation_availability(82).await,
            ContinuationAvailability::NotContinuable
        );
    }

    /// D3.1 capability consumption: the agent explicitly self-reported that it
    /// supports neither `session/load` nor resume (LiveOnly). Once its process
    /// is dead the context is gone for good — even with a stored credential
    /// the verdict must be `NotContinuable`, not a doomed resume invitation.
    #[tokio::test]
    async fn availability_live_only_agent_after_death_is_not_continuable() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup())
                .with_status_lookup(Arc::new(StaticStatusLookup(continue_status_record(83))));
        broker
            .set_config(DelegationConfig {
                enabled: true,
                ..DelegationConfig::default()
            })
            .await;
        settle_one(&broker, &mock, "c-av-liveonly", 83, "pt-av-liveonly").await;
        mock.set_capability(
            "c-av-liveonly",
            crate::acp::delegation::spawner::AgentContinuationCapability {
                supports_load_session: false,
                supports_resume: false,
            },
        )
        .await;
        mock.mark_dead("c-av-liveonly").await;
        assert_eq!(
            broker.get_continuation_availability(83).await,
            ContinuationAvailability::NotContinuable
        );

        // Control: the same shape with resume capability stays continuable.
        mock.set_capability(
            "c-av-liveonly",
            crate::acp::delegation::spawner::AgentContinuationCapability {
                supports_load_session: true,
                supports_resume: true,
            },
        )
        .await;
        assert_eq!(
            broker.get_continuation_availability(83).await,
            ContinuationAvailability::ContinuableResume
        );
    }

    // -- Task 3.5/3.6: continue_delegation ---------------------------------

    /// Static `ChildStatusLookup` serving the folder / resume metadata a
    /// continuation would otherwise read from the child conversation row.
    struct StaticStatusLookup(ChildStatusRecord);

    #[async_trait]
    impl ChildStatusLookup for StaticStatusLookup {
        async fn find_by_call_id(&self, _call_id: &str) -> Option<ChildStatusRecord> {
            Some(self.0.clone())
        }
    }

    /// Standard status record for continue tests: folder 7, matching child
    /// conversation, resumable credential.
    fn continue_status_record(child_conversation_id: i32) -> ChildStatusRecord {
        ChildStatusRecord {
            child_conversation_id,
            status: TaskStatus::Completed,
            agent_type: AgentType::ClaudeCode,
            parent_id: Some(1),
            folder_id: 7,
            external_id: Some("sess-resume".into()),
            working_dir: Some("/work".into()),
        }
    }

    /// Forwarding spawner that gates `send_followup_prompt` so a test can pin
    /// `continue_delegation` INSIDE its dispatch window (in-flight registered,
    /// `running` not yet re-inserted) and land a parent cancel there — the R3
    /// race. All other methods forward to the inner `MockSpawner`. Test-module
    /// local on purpose: the trait's mock lives in `spawner.rs`, which this
    /// task must not modify.
    struct GatedFollowupSpawner {
        inner: Arc<MockSpawner>,
        entered_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl ConnectionSpawner for GatedFollowupSpawner {
        async fn spawn(
            &self,
            parent_connection_id: &str,
            agent_type: AgentType,
            working_dir: Option<String>,
            preferred_mode_id: Option<String>,
            preferred_config_values: BTreeMap<String, String>,
        ) -> Result<String, SpawnerError> {
            self.inner
                .spawn(
                    parent_connection_id,
                    agent_type,
                    working_dir,
                    preferred_mode_id,
                    preferred_config_values,
                )
                .await
        }

        async fn send_prompt_linked_for_delegation(
            &self,
            conn_id: &str,
            task: String,
            link: crate::acp::delegation::spawner::DelegationLink,
        ) -> Result<i32, SpawnerError> {
            self.inner
                .send_prompt_linked_for_delegation(conn_id, task, link)
                .await
        }

        async fn cancel(&self, conn_id: &str) -> Result<(), SpawnerError> {
            self.inner.cancel(conn_id).await
        }

        async fn disconnect(&self, conn_id: &str) -> Result<(), SpawnerError> {
            self.inner.disconnect(conn_id).await
        }

        async fn spawn_for_resume(
            &self,
            parent_connection_id: &str,
            agent_type: AgentType,
            working_dir: Option<String>,
            session_id: Option<String>,
            preferred_mode_id: Option<String>,
            preferred_config_values: BTreeMap<String, String>,
        ) -> Result<String, SpawnerError> {
            self.inner
                .spawn_for_resume(
                    parent_connection_id,
                    agent_type,
                    working_dir,
                    session_id,
                    preferred_mode_id,
                    preferred_config_values,
                )
                .await
        }

        async fn send_followup_prompt(
            &self,
            conn_id: &str,
            message: String,
            conversation_id: i32,
            folder_id: i32,
        ) -> Result<(), SpawnerError> {
            if let Some(tx) = self.entered_tx.lock().await.take() {
                let _ = tx.send(());
            }
            let gate = self.gate.lock().await.take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            self.inner
                .send_followup_prompt(conn_id, message, conversation_id, folder_id)
                .await
        }

        async fn is_alive(&self, conn_id: &str) -> bool {
            self.inner.is_alive(conn_id).await
        }
    }

    /// Broker wired for continue tests: mock spawner + static status lookup.
    fn continue_broker(mock: Arc<MockSpawner>, child_conv: i32) -> DelegationBroker {
        DelegationBroker::new(mock as Arc<dyn ConnectionSpawner>, shallow_lookup())
            .with_status_lookup(Arc::new(StaticStatusLookup(continue_status_record(
                child_conv,
            ))))
    }

    /// Property 4 (task_id stability) + Requirement 2.3: a continuation on a
    /// settled task reuses the kept-alive connection, adopts the existing
    /// child row (Branch A shape: conversation_id + folder_id), returns a
    /// Running report under the ORIGINAL task id, and the next settle reports
    /// the new turn's result under that same id.
    #[tokio::test]
    async fn continue_on_live_connection_keeps_task_id_and_adopts_row() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;

        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        mock.queue_followup(Ok(())).await;
        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "next step please".into(),
                "op-1",
                TurnOrigin::ParentAgent,
            )
            .await;
        assert_eq!(cont.status, TaskStatus::Running);
        assert_eq!(cont.task_id.as_deref(), Some(task_id.as_str()));
        assert_eq!(cont.child_conversation_id, Some(42));

        let followups = mock.followups.lock().await;
        assert_eq!(followups.len(), 1);
        assert_eq!(followups[0].conn_id, "c-live");
        assert_eq!(followups[0].message, "next step please");
        assert_eq!(followups[0].conversation_id, 42);
        assert_eq!(followups[0].folder_id, 7);
        drop(followups);

        // The second settle reports under the SAME task id.
        broker
            .complete_call(
                &task_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "second turn".into(),
                    child_conversation_id: 42,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        let report = broker
            .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
            .await;
        assert_eq!(report.status, TaskStatus::Completed);
        assert_eq!(report.text.as_deref(), Some("second turn"));
        assert_eq!(report.task_id.as_deref(), Some(task_id.as_str()));
        assert!(
            mock.disconnects.lock().await.is_empty(),
            "both settles kept the child alive"
        );

        // Turn layer (Requirement 8.1): two dispatched turns, monotonic
        // version, correct origins, distinct broker-internal turn ids.
        let inner = broker.pending.inner.lock().await;
        let s = inner.sessions.get(&task_id).expect("session exists");
        assert_eq!(s.turn_version, 2);
        assert_eq!(s.turns.len(), 2);
        assert_eq!(s.turns[0].origin, TurnOrigin::ParentAgent);
        assert_eq!(s.turns[0].turn_version, 1);
        assert_eq!(s.turns[1].origin, TurnOrigin::ParentAgent);
        assert_eq!(s.turns[1].turn_version, 2);
        assert_ne!(s.turns[0].turn_id, s.turns[1].turn_id);
    }

    /// Property 2 (no new rows): a continuation must go through the follow-up
    /// channel ONLY. The row-creating first-prompt channel and the spawner's
    /// fresh-spawn path must not be touched (their queues are empty — any use
    /// fails loudly), and no second spawn call is recorded.
    #[tokio::test]
    async fn continue_never_touches_the_first_prompt_channel() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        mock.queue_followup(Ok(())).await;
        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "again".into(),
                "op-1",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.status, TaskStatus::Running);
        // Exactly the one spawn from the ORIGINAL delegation; a live-conn
        // continuation neither spawns nor resumes.
        assert_eq!(mock.spawn_args.lock().await.len(), 1);
        assert!(mock.resume_args.lock().await.is_empty());
        assert_eq!(mock.followups.lock().await.len(), 1);
    }

    /// Broker wired for continue-event tests: mock spawner + emitter + static
    /// status lookup (T4.3).
    fn continue_broker_with_emitter(
        mock: Arc<MockSpawner>,
        emitter: Arc<crate::acp::delegation::event_emitter::mock::MockEventEmitter>,
        child_conv: i32,
    ) -> DelegationBroker {
        DelegationBroker::with_writers(
            mock as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
            Arc::new(crate::acp::delegation::meta_writer::mock::MockMetaWriter::new())
                as Arc<dyn crate::acp::delegation::meta_writer::DelegationMetaWriter>,
            emitter as Arc<dyn crate::acp::delegation::event_emitter::DelegationEventEmitter>,
        )
        .with_status_lookup(Arc::new(StaticStatusLookup(continue_status_record(
            child_conv,
        ))))
    }

    /// Requirements 2.8 / 2.8a / 8.1 / 8.2 (T4.3): when a CONTINUED turn
    /// settles, the broker emits ONE session-scoped update carrying the
    /// four-tuple `(task_id, turn_id, turn_version, origin)` — and does NOT
    /// re-emit a completion event against the original `parent_tool_use_id`
    /// (its tool call already went terminal on turn 1).
    #[tokio::test]
    async fn continued_turn_settle_emits_session_update_not_second_completion() {
        let mock = Arc::new(MockSpawner::new());
        let emitter =
            Arc::new(crate::acp::delegation::event_emitter::mock::MockEventEmitter::new());
        let broker = continue_broker_with_emitter(mock.clone(), emitter.clone(), 42);
        enable_delegation(&broker).await;

        let task_id = settle_one(&broker, &mock, "c-evt", 42, "pt-evt").await;
        assert_eq!(emitter.count().await, 1, "first turn emits its completion");
        assert_eq!(emitter.session_update_count().await, 0);

        mock.queue_followup(Ok(())).await;
        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "next".into(),
                "op-evt-1",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.status, TaskStatus::Running);
        broker
            .complete_call(
                &task_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "second".into(),
                    child_conversation_id: 42,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;

        assert_eq!(
            emitter.count().await,
            1,
            "no second completion against the terminal tool call (2.8a)"
        );
        let updates = emitter.session_update_snapshot().await;
        assert_eq!(updates.len(), 1, "exactly one session-scoped update");
        let u = &updates[0];
        assert_eq!(u.task_id, task_id);
        assert_eq!(u.turn_version, 2);
        assert_eq!(u.origin, TurnOrigin::User);
        assert_eq!(u.child_conversation_id, 42);
        assert_eq!(u.parent_connection_id, "parent-conn");
        let inner = broker.pending.inner.lock().await;
        let s = inner.sessions.get(&task_id).expect("session exists");
        assert_eq!(
            u.turn_id, s.turns[1].turn_id,
            "the update carries the CONTINUED turn's broker-internal id"
        );
    }

    /// A canceled continued turn is terminal too: the teardown path emits the
    /// session-scoped update (with the turn's origin) instead of re-emitting a
    /// completion on the settled tool call.
    #[tokio::test]
    async fn canceled_continued_turn_emits_session_update_not_completion() {
        let mock = Arc::new(MockSpawner::new());
        let emitter =
            Arc::new(crate::acp::delegation::event_emitter::mock::MockEventEmitter::new());
        let broker = continue_broker_with_emitter(mock.clone(), emitter.clone(), 42);
        enable_delegation(&broker).await;

        let task_id = settle_one(&broker, &mock, "c-evt-2", 42, "pt-evt-2").await;
        mock.queue_followup(Ok(())).await;
        broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "next".into(),
                "op-evt-2",
                TurnOrigin::ParentAgent,
            )
            .await;
        broker
            .cancel_task_by_id("parent-conn", Some(1), &task_id)
            .await;

        assert_eq!(
            emitter.count().await,
            1,
            "cancel of a continued turn must not re-emit completion (2.8a)"
        );
        let updates = emitter.session_update_snapshot().await;
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].turn_version, 2);
        assert_eq!(updates[0].origin, TurnOrigin::ParentAgent);
    }

    /// Requirement 2.4: continuing a task whose turn is still running is
    /// rejected with the stable code `session_still_running`.
    #[tokio::test]
    async fn continue_while_running_returns_session_still_running() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = start_running(&broker, &mock, "c-run", 42, "pt-1").await;

        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "too early".into(),
                "op-1",
                TurnOrigin::ParentAgent,
            )
            .await;
        assert_eq!(cont.error_code.as_deref(), Some("session_still_running"));
        assert!(
            mock.followups.lock().await.is_empty(),
            "no prompt side effect on rejection"
        );
    }

    /// Requirement 2.13: replaying the SAME continuation_id executes the
    /// follow-up once and returns the first report to both callers.
    #[tokio::test]
    async fn continue_same_continuation_id_is_idempotent() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        mock.queue_followup(Ok(())).await;
        let first = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "do it".into(),
                "op-dup",
                TurnOrigin::User,
            )
            .await;
        let replay = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "do it".into(),
                "op-dup",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(first.status, TaskStatus::Running);
        assert_eq!(replay.status, TaskStatus::Running);
        assert_eq!(replay.task_id, first.task_id);
        assert_eq!(
            mock.followups.lock().await.len(),
            1,
            "the follow-up must be sent exactly once"
        );
        assert!(
            replay.error_code.is_none(),
            "a replay is NOT session_still_running — it returns the first report"
        );
    }

    /// Same continuation_id with a DIFFERENT payload is a caller bug — reject
    /// with `continuation_conflict`, never silently pick either message.
    #[tokio::test]
    async fn continue_same_continuation_id_conflicting_payload_rejected() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        mock.queue_followup(Ok(())).await;
        let _first = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "message A".into(),
                "op-x",
                TurnOrigin::User,
            )
            .await;
        let conflict = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "message B".into(),
                "op-x",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(
            conflict.error_code.as_deref(),
            Some("continuation_conflict")
        );
        assert_eq!(mock.followups.lock().await.len(), 1);
    }

    /// Ownership: another parent conversation cannot continue (or even probe)
    /// this task — `Unknown`, no existence leak (Requirement 2.6).
    #[tokio::test]
    async fn continue_from_other_parent_is_unknown() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        let cont = broker
            .continue_delegation(
                "other-conn",
                Some(99),
                &task_id,
                "mine now".into(),
                "op-1",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.status, TaskStatus::Unknown);
        assert!(mock.followups.lock().await.is_empty());
    }

    /// R3 race (Requirements 6.1 / 6.2): a parent cancel landing while the
    /// continuation is INSIDE its dispatch window (in-flight registered,
    /// follow-up prompt in flight, `running` not yet re-inserted) must cancel
    /// that continuation turn — the exact window PR #375 left uncovered.
    #[tokio::test]
    async fn parent_cancel_during_continue_dispatch_cancels_the_turn() {
        let mock = Arc::new(MockSpawner::new());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let gated = Arc::new(GatedFollowupSpawner {
            inner: mock.clone(),
            entered_tx: Mutex::new(Some(entered_tx)),
            gate: Mutex::new(Some(release_rx)),
        });
        let broker = DelegationBroker::new(gated as Arc<dyn ConnectionSpawner>, shallow_lookup())
            .with_status_lookup(Arc::new(StaticStatusLookup(continue_status_record(42))));
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        mock.queue_followup(Ok(())).await;
        let driver = {
            let broker = broker.clone();
            let task_id = task_id.clone();
            tokio::spawn(async move {
                broker
                    .continue_delegation(
                        "parent-conn",
                        Some(1),
                        &task_id,
                        "racing turn".into(),
                        "op-race",
                        TurnOrigin::ParentAgent,
                    )
                    .await
            })
        };
        // The continuation is now inside send_followup_prompt: in-flight is
        // registered, `running` is NOT. This is the window `cancel_by_parent*`
        // could historically miss.
        entered_rx.await.expect("continue reached the send");
        broker.cancel_by_parent_turn("parent-conn").await;
        let _ = release_tx.send(());

        let report = driver.await.unwrap();
        assert_eq!(
            report.error_code.as_deref(),
            Some("canceled"),
            "the racing continuation must resolve canceled, not keep running"
        );
        // The turn never stays behind in `running`, and the child was torn
        // down (cancel semantics), not left as an unmanaged process.
        assert_eq!(broker.pending_count().await, 0);
        assert!(mock.disconnects.lock().await.iter().any(|c| c == "c-live"));
        let inner = broker.pending.inner.lock().await;
        let s = inner.sessions.get(&task_id).expect("session exists");
        assert_eq!(s.status, TaskStatus::Canceled);
        assert!(s.child_connection_id.is_none());
    }

    // -- Task 3.7: close_delegation_session (release semantics) ------------

    /// Poll until `pred` holds or ~2s elapse — for assertions on backgrounded
    /// teardown I/O.
    async fn wait_until<F, Fut>(mut pred: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        for _ in 0..200 {
            if pred().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not reached within the polling budget");
    }

    /// Requirements 2.7 / 5.3 + release semantics: close disconnects the kept
    /// child, marks the session `released` (NOT a persistent "closed"), and a
    /// later continuation is refused with `session_released` and zero prompt
    /// side effects.
    #[tokio::test]
    async fn close_releases_kept_child_and_blocks_continue() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        let closed = broker
            .close_delegation_session("parent-conn", Some(1), &task_id, "close-1")
            .await;
        assert_eq!(closed.status, TaskStatus::Completed, "last known status");
        wait_until(|| async { !mock.disconnects.lock().await.is_empty() }).await;
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c-live"]);

        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "more please".into(),
                "op-after-close",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.error_code.as_deref(), Some("session_released"));
        assert!(
            mock.followups.lock().await.is_empty(),
            "a released session must take no prompt side effect"
        );

        let inner = broker.pending.inner.lock().await;
        let s = inner
            .sessions
            .get(&task_id)
            .expect("session survives release");
        assert!(s.released);
        assert!(s.child_connection_id.is_none());
        assert!(
            !inner.kept_alive_order.iter().any(|t| t == &task_id),
            "a released session must leave the kept-alive FIFO"
        );
    }

    /// Requirement 2.10: closing an already-released task returns the last
    /// known status WITHOUT side effects — for both a replayed continuation_id
    /// and a fresh one.
    #[tokio::test]
    async fn close_is_idempotent() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        let first = broker
            .close_delegation_session("parent-conn", Some(1), &task_id, "close-1")
            .await;
        wait_until(|| async { !mock.disconnects.lock().await.is_empty() }).await;

        // Replay with the SAME continuation_id.
        let replay = broker
            .close_delegation_session("parent-conn", Some(1), &task_id, "close-1")
            .await;
        assert_eq!(replay.status, first.status);
        // A FRESH continuation_id on an already-released task: same answer.
        let again = broker
            .close_delegation_session("parent-conn", Some(1), &task_id, "close-2")
            .await;
        assert_eq!(again.status, first.status);
        assert_eq!(
            mock.disconnects.lock().await.len(),
            1,
            "repeat closes must not re-run the teardown"
        );
    }

    /// Requirement 2.11: a task with no live child connection (already swept /
    /// evicted) still transitions to released.
    #[tokio::test]
    async fn close_without_live_connection_still_releases() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-gone", 42, "pt-1").await;
        // Simulate a cap/sweep having already taken the connection.
        {
            let mut inner = broker.pending.inner.lock().await;
            inner
                .sessions
                .get_mut(&task_id)
                .expect("session exists")
                .child_connection_id = None;
            inner.kept_alive_order.retain(|t| t != &task_id);
        }

        let closed = broker
            .close_delegation_session("parent-conn", Some(1), &task_id, "close-1")
            .await;
        assert_eq!(closed.status, TaskStatus::Completed);
        assert!(
            mock.disconnects.lock().await.is_empty(),
            "no connection to tear down"
        );
        let inner = broker.pending.inner.lock().await;
        assert!(inner.sessions.get(&task_id).expect("session").released);
    }

    /// Requirement 2.9: closing a RUNNING task cancels the in-flight turn
    /// first (cancel + disconnect), settles it Canceled, and releases.
    #[tokio::test]
    async fn close_cancels_running_turn_first() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = start_running(&broker, &mock, "c-run", 42, "pt-1").await;

        let closed = broker
            .close_delegation_session("parent-conn", Some(1), &task_id, "close-1")
            .await;
        assert_eq!(closed.status, TaskStatus::Canceled);
        wait_until(|| async { !mock.disconnects.lock().await.is_empty() }).await;
        assert_eq!(mock.cancels.lock().await.as_slice(), &["c-run"]);
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c-run"]);
        assert_eq!(broker.pending_count().await, 0);
        let inner = broker.pending.inner.lock().await;
        let s = inner.sessions.get(&task_id).expect("session");
        assert!(s.released);
        assert_eq!(s.status, TaskStatus::Canceled);
    }

    /// Requirement 2.12: close and continue on the same task serialize under
    /// the pending lock — a close arriving while a continuation is mid-dispatch
    /// cancels that continuation turn, and the later arrival observes it.
    #[tokio::test]
    async fn close_serializes_with_inflight_continue() {
        let mock = Arc::new(MockSpawner::new());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let gated = Arc::new(GatedFollowupSpawner {
            inner: mock.clone(),
            entered_tx: Mutex::new(Some(entered_tx)),
            gate: Mutex::new(Some(release_rx)),
        });
        let broker = DelegationBroker::new(gated as Arc<dyn ConnectionSpawner>, shallow_lookup())
            .with_status_lookup(Arc::new(StaticStatusLookup(continue_status_record(42))));
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        mock.queue_followup(Ok(())).await;
        let driver = {
            let broker = broker.clone();
            let task_id = task_id.clone();
            tokio::spawn(async move {
                broker
                    .continue_delegation(
                        "parent-conn",
                        Some(1),
                        &task_id,
                        "racing".into(),
                        "op-race",
                        TurnOrigin::User,
                    )
                    .await
            })
        };
        entered_rx.await.expect("continue reached the send");
        let closed = broker
            .close_delegation_session("parent-conn", Some(1), &task_id, "close-1")
            .await;
        let _ = release_tx.send(());

        let cont = driver.await.unwrap();
        assert_eq!(
            cont.error_code.as_deref(),
            Some("canceled"),
            "the in-dispatch continuation must observe the close as a cancel"
        );
        assert_eq!(closed.status, TaskStatus::Canceled);
        assert_eq!(broker.pending_count().await, 0);
        let inner = broker.pending.inner.lock().await;
        let s = inner.sessions.get(&task_id).expect("session");
        assert!(s.released);
    }

    /// Close racing an in-dispatch continuation whose follow-up SEND fails
    /// (S4 `Err`, not the S3/S5 cancel checkpoints): the close deferred the
    /// connection's teardown to the dispatch's compensation path
    /// (`conn_to_disconnect = None`), so `abort_continuation`'s settle must
    /// observe `released` and hand the prior connection to `disconnect`
    /// instead of restoring it onto the released session (which would leak
    /// the child process: `released = true` + a live pointer nobody owns).
    #[tokio::test]
    async fn close_racing_continue_send_failure_still_disconnects_released_connection() {
        let mock = Arc::new(MockSpawner::new());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let gated = Arc::new(GatedFollowupSpawner {
            inner: mock.clone(),
            entered_tx: Mutex::new(Some(entered_tx)),
            gate: Mutex::new(Some(release_rx)),
        });
        let broker = DelegationBroker::new(gated as Arc<dyn ConnectionSpawner>, shallow_lookup())
            .with_status_lookup(Arc::new(StaticStatusLookup(continue_status_record(42))));
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        // The gated follow-up FAILS once released: the S4 `Err` compensation
        // (not a cancel checkpoint) runs against the now-released session.
        mock.queue_followup(Err(SpawnerError::Send("agent hung up".into())))
            .await;
        let driver = {
            let broker = broker.clone();
            let task_id = task_id.clone();
            tokio::spawn(async move {
                broker
                    .continue_delegation(
                        "parent-conn",
                        Some(1),
                        &task_id,
                        "racing".into(),
                        "op-race-senderr",
                        TurnOrigin::User,
                    )
                    .await
            })
        };
        entered_rx.await.expect("continue reached the send");
        let closed = broker
            .close_delegation_session("parent-conn", Some(1), &task_id, "close-senderr")
            .await;
        let _ = release_tx.send(());

        let cont = driver.await.unwrap();
        assert_eq!(
            cont.error_code.as_deref(),
            Some("subagent_error"),
            "the failed send surfaces as a runtime error, not a silent success"
        );
        assert_eq!(closed.status, TaskStatus::Canceled);
        assert!(
            mock.disconnects.lock().await.iter().any(|c| c == "c-live"),
            "the released session's connection must be handed to disconnect, \
             not restored onto the released session"
        );
        let inner = broker.pending.inner.lock().await;
        let s = inner.sessions.get(&task_id).expect("session");
        assert!(s.released);
        assert!(
            s.child_connection_id.is_none(),
            "a released session must never point at a connection again"
        );
        assert_eq!(
            s.status,
            TaskStatus::Canceled,
            "the close's terminal status must not be overwritten by the restore"
        );
        assert!(
            !inner.kept_alive_order.iter().any(|t| t == &task_id),
            "a released session must not be resurrected into the kept-alive FIFO"
        );
    }

    /// R3-A5 partial failure: a failing disconnect marks the session
    /// `orphan_suspect` and schedules a background retry — never silently
    /// swallowed as success.
    #[tokio::test]
    async fn close_disconnect_failure_marks_orphan_suspect_and_retries() {
        /// Forwarding spawner whose FIRST disconnect fails (simulating a
        /// wedged agent process); later attempts succeed.
        struct FailingDisconnectSpawner {
            inner: Arc<MockSpawner>,
            attempts: std::sync::atomic::AtomicUsize,
        }
        #[async_trait]
        impl ConnectionSpawner for FailingDisconnectSpawner {
            async fn spawn(
                &self,
                p: &str,
                a: AgentType,
                w: Option<String>,
                m: Option<String>,
                c: BTreeMap<String, String>,
            ) -> Result<String, SpawnerError> {
                self.inner.spawn(p, a, w, m, c).await
            }
            async fn send_prompt_linked_for_delegation(
                &self,
                conn_id: &str,
                task: String,
                link: crate::acp::delegation::spawner::DelegationLink,
            ) -> Result<i32, SpawnerError> {
                self.inner
                    .send_prompt_linked_for_delegation(conn_id, task, link)
                    .await
            }
            async fn cancel(&self, conn_id: &str) -> Result<(), SpawnerError> {
                self.inner.cancel(conn_id).await
            }
            async fn disconnect(&self, conn_id: &str) -> Result<(), SpawnerError> {
                let n = self
                    .attempts
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    return Err(SpawnerError::Disconnect("wedged process".into()));
                }
                self.inner.disconnect(conn_id).await
            }
            async fn spawn_for_resume(
                &self,
                p: &str,
                a: AgentType,
                w: Option<String>,
                s: Option<String>,
                m: Option<String>,
                c: BTreeMap<String, String>,
            ) -> Result<String, SpawnerError> {
                self.inner.spawn_for_resume(p, a, w, s, m, c).await
            }
            async fn send_followup_prompt(
                &self,
                conn_id: &str,
                message: String,
                conversation_id: i32,
                folder_id: i32,
            ) -> Result<(), SpawnerError> {
                self.inner
                    .send_followup_prompt(conn_id, message, conversation_id, folder_id)
                    .await
            }
            async fn is_alive(&self, conn_id: &str) -> bool {
                self.inner.is_alive(conn_id).await
            }
        }

        let mock = Arc::new(MockSpawner::new());
        let failing = Arc::new(FailingDisconnectSpawner {
            inner: mock.clone(),
            attempts: std::sync::atomic::AtomicUsize::new(0),
        });
        let failing_probe = failing.clone();
        let broker = DelegationBroker::new(failing as Arc<dyn ConnectionSpawner>, shallow_lookup())
            .with_status_lookup(Arc::new(StaticStatusLookup(continue_status_record(42))));
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-wedge", 42, "pt-1").await;

        let closed = broker
            .close_delegation_session("parent-conn", Some(1), &task_id, "close-1")
            .await;
        assert_eq!(closed.status, TaskStatus::Completed);
        // Background retry lands eventually; the first attempt failed.
        wait_until(|| async {
            failing_probe
                .attempts
                .load(std::sync::atomic::Ordering::SeqCst)
                >= 2
        })
        .await;
        let inner = broker.pending.inner.lock().await;
        assert!(
            inner
                .sessions
                .get(&task_id)
                .expect("session")
                .orphan_suspect,
            "a failed disconnect must be observable, not swallowed"
        );
    }

    // -- Task 3.8: resume pre-rejection + external_id validation ------------

    /// Static lookup with a configurable duplicate count for the
    /// `external_id` ambiguity check (Requirement 7.7).
    struct CountingStatusLookup {
        record: Option<ChildStatusRecord>,
        external_id_refs: usize,
    }

    #[async_trait]
    impl ChildStatusLookup for CountingStatusLookup {
        async fn find_by_call_id(&self, _call_id: &str) -> Option<ChildStatusRecord> {
            self.record.clone()
        }
        async fn external_id_ref_count(&self, _external_id: &str) -> usize {
            self.external_id_refs
        }
    }

    /// Happy resume path (Requirement 3.1): a dead child is revived through
    /// `spawn_for_resume` carrying the persisted `external_id`, and the
    /// follow-up goes to the REPLACEMENT connection while the session adopts
    /// it.
    #[tokio::test]
    async fn continue_resumes_dead_child_with_external_id() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-dead", 42, "pt-1").await;
        mock.mark_dead("c-dead").await;

        mock.queue_spawn(Ok("c-revived".into())).await;
        mock.queue_followup(Ok(())).await;
        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "welcome back".into(),
                "op-res",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.status, TaskStatus::Running);
        let resumes = mock.resume_args.lock().await;
        assert_eq!(resumes.len(), 1);
        assert_eq!(resumes[0].session_id.as_deref(), Some("sess-resume"));
        drop(resumes);
        let followups = mock.followups.lock().await;
        assert_eq!(followups[0].conn_id, "c-revived");
        drop(followups);
        let inner = broker.pending.inner.lock().await;
        assert_eq!(
            inner
                .sessions
                .get(&task_id)
                .and_then(|s| s.child_connection_id.as_deref()),
            Some("c-revived")
        );
    }

    /// Requirement 3.3 (R2-B1): when the resume spawn itself fails, the
    /// continuation is rejected with `resume_unavailable` BEFORE any prompt
    /// side effect, and the session's resume credential is untouched.
    #[tokio::test]
    async fn resume_spawn_failure_is_resume_unavailable_before_prompt() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-dead", 42, "pt-1").await;
        mock.mark_dead("c-dead").await;

        mock.queue_spawn(Err(SpawnerError::Spawn("agent won't boot".into())))
            .await;
        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "try again".into(),
                "op-fail",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.error_code.as_deref(), Some("resume_unavailable"));
        assert!(
            mock.followups.lock().await.is_empty(),
            "no prompt side effect on a failed resume (3.3)"
        );
        // The session keeps its settled state and can be retried later.
        let inner = broker.pending.inner.lock().await;
        let s = inner.sessions.get(&task_id).expect("session");
        assert_eq!(s.status, TaskStatus::Completed);
        assert!(!s.released);
    }

    /// Requirement 7.6: the DB row's agent_type must match the resume target —
    /// a mismatch means the credential does not belong to this child; refuse
    /// WITHOUT attempting the resume.
    #[tokio::test]
    async fn resume_rejected_when_agent_type_mismatches() {
        let mock = Arc::new(MockSpawner::new());
        let mut rec = continue_status_record(42);
        rec.agent_type = AgentType::Codex; // session was dispatched as ClaudeCode
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup())
                .with_status_lookup(Arc::new(StaticStatusLookup(rec)));
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-dead", 42, "pt-1").await;
        mock.mark_dead("c-dead").await;

        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "hello".into(),
                "op-mismatch",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.error_code.as_deref(), Some("not_continuable"));
        assert!(
            mock.resume_args.lock().await.is_empty(),
            "must refuse BEFORE attempting the resume"
        );
        assert!(mock.followups.lock().await.is_empty());
    }

    /// Requirement 7.6, folder leg: a cached folder binding that disagrees
    /// with the DB row refuses the resume.
    #[tokio::test]
    async fn resume_rejected_when_folder_mismatches() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-dead", 42, "pt-1").await;
        // The session believes folder 9; the DB row says folder 7.
        {
            let mut inner = broker.pending.inner.lock().await;
            inner.sessions.get_mut(&task_id).expect("session").folder_id = Some(9);
        }
        mock.mark_dead("c-dead").await;

        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "hello".into(),
                "op-folder",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.error_code.as_deref(), Some("not_continuable"));
        assert!(mock.resume_args.lock().await.is_empty());
    }

    /// Requirement 7.7: an `external_id` referenced by more than one child row
    /// cannot be trusted as a resume credential — refuse rather than guess.
    #[tokio::test]
    async fn duplicate_external_id_refuses_resume() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup())
                .with_status_lookup(Arc::new(CountingStatusLookup {
                    record: Some(continue_status_record(42)),
                    external_id_refs: 2,
                }));
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-dead", 42, "pt-1").await;
        mock.mark_dead("c-dead").await;

        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "hello".into(),
                "op-dup-ext",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.error_code.as_deref(), Some("resume_unavailable"));
        assert!(
            mock.resume_args.lock().await.is_empty(),
            "an ambiguous credential must never be handed to the agent"
        );
        assert!(mock.followups.lock().await.is_empty());
    }

    // -- Task 3.9: startup rebuild protocol ---------------------------------

    /// Lookup backing rebuild tests: serves `list_rebuildable` from a mutable
    /// candidate list and answers `find_by_call_id` from the same rows (so a
    /// rebuilt session can pass resume validation).
    struct RebuildLookup {
        candidates: Mutex<Vec<RebuildCandidate>>,
    }

    #[async_trait]
    impl ChildStatusLookup for RebuildLookup {
        async fn find_by_call_id(&self, call_id: &str) -> Option<ChildStatusRecord> {
            self.candidates
                .lock()
                .await
                .iter()
                .find(|c| c.task_id == call_id)
                .map(|c| ChildStatusRecord {
                    child_conversation_id: c.child_conversation_id,
                    status: c.status,
                    agent_type: c.agent_type,
                    parent_id: c.parent_conversation_id,
                    folder_id: c.folder_id,
                    external_id: c.external_id.clone(),
                    working_dir: c.working_dir.clone(),
                })
        }
        async fn list_rebuildable(&self) -> Vec<RebuildCandidate> {
            self.candidates.lock().await.clone()
        }
    }

    fn rebuild_candidate(task_id: &str, parent: Option<i32>, alive: bool) -> RebuildCandidate {
        RebuildCandidate {
            task_id: task_id.to_string(),
            child_conversation_id: 42,
            parent_conversation_id: parent,
            parent_alive: alive,
            parent_tool_use_id: Some("pt-reb".into()),
            agent_type: AgentType::ClaudeCode,
            status: TaskStatus::Completed,
            folder_id: 7,
            external_id: Some("sess-reb".into()),
            working_dir: Some("/work".into()),
        }
    }

    /// Requirement 7.1: startup rebuilds the continuable index from child
    /// conversation rows — a rebuilt session carries its ownership + resume
    /// metadata, has NO turn history (not promised across restarts), and a
    /// continuation on it goes through the resume path with the persisted
    /// credential. Requirement 7.4: the operation ledger starts empty, so a
    /// pre-restart continuation_id executes as new.
    #[tokio::test]
    async fn rebuild_restores_continuable_sessions_from_db() {
        let mock = Arc::new(MockSpawner::new());
        let lookup = Arc::new(RebuildLookup {
            candidates: Mutex::new(vec![rebuild_candidate("t-reb", Some(1), true)]),
        });
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup())
                .with_status_lookup(lookup);
        enable_delegation(&broker).await;

        broker.rebuild_sessions_from_db().await;

        {
            let inner = broker.pending.inner.lock().await;
            let s = inner.sessions.get("t-reb").expect("session rebuilt");
            assert_eq!(s.parent_conversation_id, 1);
            assert_eq!(s.status, TaskStatus::Completed);
            assert!(
                s.child_connection_id.is_none(),
                "no live process after restart"
            );
            assert_eq!(s.external_id.as_deref(), Some("sess-reb"));
            assert_eq!(s.turn_version, 0, "turn history does not cross restarts");
            assert!(s.turns.is_empty());
            assert!(inner.operations.is_empty(), "ledger never crosses restarts");
            assert!(!inner.rebuilding, "rebuild must clear its in-progress flag");
        }

        // A pre-restart continuation_id is unseen — it executes for real.
        mock.queue_spawn(Ok("c-back".into())).await;
        mock.queue_followup(Ok(())).await;
        let cont = broker
            .continue_delegation(
                "parent-conn-2",
                Some(1),
                "t-reb",
                "pick it back up".into(),
                "op-from-before-restart",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.status, TaskStatus::Running);
        let resumes = mock.resume_args.lock().await;
        assert_eq!(resumes.len(), 1);
        assert_eq!(resumes[0].session_id.as_deref(), Some("sess-reb"));
    }

    /// Requirement 7.3 (single-row isolation) + ownership validation: rows
    /// whose parent conversation is gone are skipped — they never enter the
    /// index (observable as `Unknown`) and never abort the rest of the scan.
    #[tokio::test]
    async fn rebuild_skips_rows_with_dead_parent_but_keeps_the_rest() {
        let mock = Arc::new(MockSpawner::new());
        let lookup = Arc::new(RebuildLookup {
            candidates: Mutex::new(vec![
                rebuild_candidate("t-orphan", Some(9), false),
                rebuild_candidate("t-dangling", None, false),
                rebuild_candidate("t-ok", Some(1), true),
            ]),
        });
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup())
                .with_status_lookup(lookup);
        enable_delegation(&broker).await;

        broker.rebuild_sessions_from_db().await;

        let inner = broker.pending.inner.lock().await;
        assert!(inner.sessions.contains_key("t-ok"), "healthy row rebuilt");
        assert!(!inner.sessions.contains_key("t-orphan"));
        assert!(!inner.sessions.contains_key("t-dangling"));
        drop(inner);
        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(9),
                "t-orphan",
                "hello".into(),
                "op-x",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.status, TaskStatus::Unknown);
    }

    /// Requirement 7.2: while the rebuild is in progress, a continuation (or
    /// close) for a not-yet-indexed task answers the retryable `rebuilding`
    /// code — never `Unknown` (which would read as "your session is lost").
    #[tokio::test]
    async fn requests_during_rebuild_get_rebuilding_not_unknown() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        {
            let mut inner = broker.pending.inner.lock().await;
            inner.rebuilding = true;
        }
        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                "t-not-yet",
                "hello".into(),
                "op-1",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.error_code.as_deref(), Some("rebuilding"));
        let closed = broker
            .close_delegation_session("parent-conn", Some(1), "t-not-yet", "close-1")
            .await;
        assert_eq!(closed.error_code.as_deref(), Some("rebuilding"));
    }

    /// A rebuild must never clobber a LIVE session (e.g. a hot-restart path
    /// calling it twice): the in-memory entry — which may hold a kept-alive
    /// connection — wins over the DB reconstruction.
    #[tokio::test]
    async fn rebuild_does_not_clobber_live_sessions() {
        let mock = Arc::new(MockSpawner::new());
        let lookup = Arc::new(RebuildLookup {
            candidates: Mutex::new(Vec::new()),
        });
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup())
                .with_status_lookup(lookup.clone());
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        lookup
            .candidates
            .lock()
            .await
            .push(rebuild_candidate(&task_id, Some(1), true));
        broker.rebuild_sessions_from_db().await;

        let inner = broker.pending.inner.lock().await;
        assert_eq!(
            inner
                .sessions
                .get(&task_id)
                .and_then(|s| s.child_connection_id.as_deref()),
            Some("c-live"),
            "the live entry (with its kept connection) must win"
        );
    }

    // -- Task 3.10: parent teardown vs conversation deletion (R2-B6) --------

    /// Requirement 1.4: tearing down the parent CONNECTION releases only its
    /// run lease — settled kept-alive children survive (the session serves the
    /// user and a reconnected parent, not one connection). The text cache for
    /// that connection is dropped (slower, not wrong — D2).
    #[tokio::test]
    async fn parent_connection_teardown_keeps_settled_children_alive() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-live", 42, "pt-1").await;

        broker.cancel_by_parent("parent-conn").await;

        assert!(
            mock.disconnects.lock().await.is_empty(),
            "a parent-connection teardown must not kill settled kept children"
        );
        let inner = broker.pending.inner.lock().await;
        let s = inner.sessions.get(&task_id).expect("session survives");
        assert_eq!(s.child_connection_id.as_deref(), Some("c-live"));
        assert!(!s.released);
        drop(inner);

        // The session stays continuable for the reconnected parent (same
        // parent CONVERSATION, new connection id — D5).
        mock.queue_followup(Ok(())).await;
        let cont = broker
            .continue_delegation(
                "parent-conn-reborn",
                Some(1),
                &task_id,
                "still there?".into(),
                "op-reconnect",
                TurnOrigin::ParentAgent,
            )
            .await;
        assert_eq!(cont.status, TaskStatus::Running);
    }

    /// Requirement 1.4a: deleting the parent CONVERSATION is the real owner
    /// disappearing — every kept-alive child of that conversation is released
    /// (disconnected + refused further continuation), while other parents'
    /// children stay untouched.
    #[tokio::test]
    async fn deleting_parent_conversation_releases_its_children() {
        let mock = Arc::new(MockSpawner::new());
        let lookup = Arc::new(RebuildLookup {
            candidates: Mutex::new(Vec::new()),
        });
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup())
                .with_status_lookup(lookup.clone());
        enable_delegation(&broker).await;
        let task_id = settle_one(&broker, &mock, "c-mine", 42, "pt-1").await;
        // A second session under ANOTHER parent conversation, via rebuild.
        lookup
            .candidates
            .lock()
            .await
            .push(rebuild_candidate("t-other", Some(2), true));
        broker.rebuild_sessions_from_db().await;

        broker.release_children_of_parent_conversation(1).await;

        assert_eq!(
            mock.disconnects.lock().await.as_slice(),
            &["c-mine"],
            "conversation deletion frees exactly its own children"
        );
        {
            let inner = broker.pending.inner.lock().await;
            assert!(inner.sessions.get(&task_id).expect("session").released);
            assert!(
                !inner
                    .sessions
                    .get("t-other")
                    .expect("other session")
                    .released,
                "another parent's child must be untouched"
            );
        }
        let cont = broker
            .continue_delegation(
                "parent-conn",
                Some(1),
                &task_id,
                "hello?".into(),
                "op-after-del",
                TurnOrigin::User,
            )
            .await;
        assert_eq!(cont.error_code.as_deref(), Some("session_released"));
    }

    /// Requirement 1.4a running leg: a running turn under the deleted parent
    /// conversation is canceled and torn down, not left unmanaged.
    #[tokio::test]
    async fn deleting_parent_conversation_cancels_running_children() {
        let mock = Arc::new(MockSpawner::new());
        let broker = continue_broker(mock.clone(), 42);
        enable_delegation(&broker).await;
        let task_id = start_running(&broker, &mock, "c-run", 42, "pt-1").await;

        broker.release_children_of_parent_conversation(1).await;

        wait_until(|| async { !mock.disconnects.lock().await.is_empty() }).await;
        assert_eq!(mock.cancels.lock().await.as_slice(), &["c-run"]);
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c-run"]);
        assert_eq!(broker.pending_count().await, 0);
        let inner = broker.pending.inner.lock().await;
        let s = inner.sessions.get(&task_id).expect("session");
        assert!(s.released);
        assert_eq!(s.status, TaskStatus::Canceled);
    }

    /// `StatusWait::Infinite` (the explicit `wait_ms = 0` escape hatch) must
    /// park while the task is still running rather than returning the running
    /// snapshot, then resolve to the terminal report once the task completes —
    /// no matter how long the child takes.
    #[tokio::test]
    async fn infinite_wait_parks_until_terminal() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-1".into())).await;
        mock.queue_send(Ok(42)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let ack = broker.start_delegation(request(1, "pt-1")).await;
        assert_eq!(ack.status, TaskStatus::Running);
        let task_id = ack.task_id.clone().expect("running task carries an id");

        // Infinite wait: parks on the completion signal instead of returning
        // the still-running snapshot.
        let waiter = {
            let broker = broker.clone();
            let task_id = task_id.clone();
            tokio::spawn(async move {
                broker
                    .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Infinite)
                    .await
            })
        };

        // Give the waiter a beat — it must still be parked, not finished.
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !waiter.is_finished(),
            "infinite wait must park while the task is running"
        );

        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 42,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;

        let report = waiter.await.unwrap();
        assert_eq!(report.status, TaskStatus::Completed);
        assert_eq!(report.text.as_deref(), Some("done"));
    }

    /// A running snapshot upgrades its bare `"Running."` message with the child's
    /// latest one-line reply when the live-reply lookup has one.
    #[tokio::test]
    async fn running_status_appends_live_reply_when_available() {
        use crate::acp::delegation::live_reply::mock::MockChildLiveReplyLookup;

        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-1".into())).await;
        mock.queue_send(Ok(42)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup())
                .with_live_reply_lookup(Arc::new(MockChildLiveReplyLookup::new(Some(
                    "Reading config.rs".into(),
                ))));
        enable_delegation(&broker).await;

        let ack = broker.start_delegation(request(1, "pt-1")).await;
        assert_eq!(ack.status, TaskStatus::Running);
        let task_id = ack.task_id.clone().expect("running task carries an id");

        let report = broker
            .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
            .await;
        assert_eq!(report.status, TaskStatus::Running);
        // The live hint lands on its own line so a content-only host can anchor
        // "still running" to the standalone first line "Running.".
        assert_eq!(
            report.message.as_deref(),
            Some("Running.\nLatest sub-agent reply: Reading config.rs")
        );
    }

    /// With no live reply (default Noop lookup / child produced nothing yet) the
    /// running snapshot stays the bare `"Running."`.
    #[tokio::test]
    async fn running_status_stays_bare_without_live_reply() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-1".into())).await;
        mock.queue_send(Ok(42)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let ack = broker.start_delegation(request(1, "pt-1")).await;
        let task_id = ack.task_id.clone().expect("running task carries an id");

        let report = broker
            .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
            .await;
        assert_eq!(report.status, TaskStatus::Running);
        assert_eq!(report.message.as_deref(), Some("Running."));
    }

    // -- Batch get_tasks_status --------------------------------------------

    /// Queue one spawn+send pair and start a delegation, returning its task id.
    /// Each call consumes one queued `(spawn, send)` from the mock.
    async fn start_running(
        broker: &DelegationBroker,
        mock: &MockSpawner,
        child_conn: &str,
        child_conv: i32,
        tool_use: &str,
    ) -> String {
        mock.queue_spawn(Ok(child_conn.into())).await;
        mock.queue_send(Ok(child_conv)).await;
        broker
            .start_delegation(request(1, tool_use))
            .await
            .task_id
            .expect("running task carries an id")
    }

    /// The single-id batch agrees with `get_task_status` for a completed task —
    /// the refactor that routes the single path through `get_tasks_status` keeps
    /// the historical contract.
    #[tokio::test]
    async fn get_tasks_status_single_matches_get_task_status() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let t1 = start_running(&broker, &mock, "child-1", 42, "pt-1").await;
        broker
            .complete_call(
                &t1,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 42,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 7,
                    token_usage: None,
                }),
            )
            .await;

        let single = broker
            .get_task_status("parent-conn", Some(1), &t1, StatusWait::Immediate)
            .await;
        let batch = broker
            .get_tasks_status(
                "parent-conn",
                Some(1),
                std::slice::from_ref(&t1),
                StatusWait::Immediate,
            )
            .await;
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].status, single.status);
        assert_eq!(batch[0].text, single.text);
        assert_eq!(batch[0].task_id, single.task_id);
    }

    /// An immediate batch poll resolves a mix of completed / running / unknown
    /// tasks in ONE pass, preserving request order.
    #[tokio::test]
    async fn batch_status_immediate_mixed_preserves_order() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let t1 = start_running(&broker, &mock, "child-1", 1, "pt-1").await;
        let t2 = start_running(&broker, &mock, "child-2", 2, "pt-2").await;
        broker
            .complete_call(
                &t1,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "first".into(),
                    child_conversation_id: 1,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 3,
                    token_usage: None,
                }),
            )
            .await;

        let ids = vec![t1.clone(), t2.clone(), "no-such-id".to_string()];
        let reports = broker
            .get_tasks_status("parent-conn", Some(1), &ids, StatusWait::Immediate)
            .await;
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0].status, TaskStatus::Completed);
        assert_eq!(reports[0].text.as_deref(), Some("first"));
        assert_eq!(reports[0].task_id.as_deref(), Some(t1.as_str()));
        assert_eq!(reports[1].status, TaskStatus::Running);
        assert_eq!(reports[1].task_id.as_deref(), Some(t2.as_str()));
        assert_eq!(reports[2].status, TaskStatus::Unknown);
    }

    /// A batch `Infinite` wait returns as soon as ANY requested task settles,
    /// leaving the still-running siblings in the snapshot.
    #[tokio::test]
    async fn batch_infinite_returns_when_any_settles() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let t1 = start_running(&broker, &mock, "child-1", 1, "pt-1").await;
        let t2 = start_running(&broker, &mock, "child-2", 2, "pt-2").await;

        let waiter = {
            let broker = broker.clone();
            let ids = vec![t1.clone(), t2.clone()];
            tokio::spawn(async move {
                broker
                    .get_tasks_status("parent-conn", Some(1), &ids, StatusWait::Infinite)
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !waiter.is_finished(),
            "batch infinite wait must park while both tasks run"
        );

        broker
            .complete_call(
                &t1,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "first-done".into(),
                    child_conversation_id: 1,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 4,
                    token_usage: None,
                }),
            )
            .await;

        let reports = waiter.await.unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].status, TaskStatus::Completed);
        assert_eq!(reports[0].text.as_deref(), Some("first-done"));
        assert_eq!(reports[1].status, TaskStatus::Running);
    }

    /// A batch `Infinite` wait must NOT hold an already-terminal result hostage
    /// to a still-running sibling: when a task is terminal at call ENTRY (it
    /// completed before the poll), return immediately with the current snapshot
    /// rather than parking for the runner. This is the mixed-at-entry case the
    /// transition-only wake used to miss — distinct from
    /// [`batch_infinite_returns_when_any_settles`] (both running at entry).
    #[tokio::test]
    async fn batch_infinite_returns_immediately_when_one_already_terminal() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let t1 = start_running(&broker, &mock, "child-1", 1, "pt-1").await;
        let t2 = start_running(&broker, &mock, "child-2", 2, "pt-2").await;
        // t1 completes BEFORE the poll; t2 keeps running.
        broker
            .complete_call(
                &t1,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "first-done".into(),
                    child_conversation_id: 1,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 4,
                    token_usage: None,
                }),
            )
            .await;

        // Infinite wait, but one task is already terminal at entry → must return
        // at once (a bounded timeout guards against the regression: parking here
        // would block until t2 settles, which it never does in this test).
        let ids = vec![t1.clone(), t2.clone()];
        let reports = tokio::time::timeout(
            Duration::from_secs(2),
            broker.get_tasks_status("parent-conn", Some(1), &ids, StatusWait::Infinite),
        )
        .await
        .expect("a batch with an already-terminal task must not park under Infinite");
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].status, TaskStatus::Completed);
        assert_eq!(reports[0].text.as_deref(), Some("first-done"));
        assert_eq!(reports[1].status, TaskStatus::Running);
    }

    /// A batch `Infinite` wait where NOTHING is running (all ids unknown) must
    /// return immediately rather than parking forever.
    #[tokio::test]
    async fn batch_infinite_all_settled_returns_immediately() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let ids = vec!["nope-1".to_string(), "nope-2".to_string()];
        let reports = tokio::time::timeout(
            Duration::from_secs(2),
            broker.get_tasks_status("parent-conn", Some(1), &ids, StatusWait::Infinite),
        )
        .await
        .expect("all-settled infinite batch must not hang");
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|r| r.status == TaskStatus::Unknown));
    }

    /// A bounded batch wait with no completion returns the running snapshot once
    /// the deadline elapses (the child keeps running; the caller re-polls).
    #[tokio::test]
    async fn batch_bounded_deadline_returns_running_snapshot() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let t1 = start_running(&broker, &mock, "child-1", 1, "pt-1").await;
        let reports = broker
            .get_tasks_status("parent-conn", Some(1), &[t1], StatusWait::Bounded(40))
            .await;
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].status, TaskStatus::Running);
    }

    /// A task owned by a different parent reports `Unknown` in a batch — never
    /// leaking another parent's task, just like the single-task path.
    #[tokio::test]
    async fn batch_status_scopes_to_parent() {
        let mock = Arc::new(MockSpawner::new());
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let t1 = start_running(&broker, &mock, "child-1", 1, "pt-1").await;
        let reports = broker
            .get_tasks_status("other-parent", Some(2), &[t1], StatusWait::Immediate)
            .await;
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].status, TaskStatus::Unknown);
    }

    // -- Task 4.5: error paths ---------------------------------------------

    #[tokio::test]
    async fn spawn_failure_maps_to_spawn_failed() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Err(SpawnerError::Spawn("nope".into())))
            .await;
        let broker = DelegationBroker::new(mock as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let outcome = broker.handle_request(request(1, "pt-1")).await;
        match outcome {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "spawn_failed"),
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn agent_defaults_are_forwarded_to_spawner() {
        // Configure broker with per-agent defaults for ClaudeCode and verify
        // they reach the spawner. Other agent types should still get the
        // empty/None defaults.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-1".into())).await;
        mock.queue_send(Err(SpawnerError::Send("stop after spawn".into())))
            .await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());

        let mut claude_cfg = BTreeMap::new();
        claude_cfg.insert("model".into(), "claude-sonnet-4-5".into());
        let mut agent_defaults = BTreeMap::new();
        agent_defaults.insert(
            AgentType::ClaudeCode,
            AgentDelegationDefaults {
                mode_id: Some("auto".into()),
                config_values: claude_cfg.clone(),
            },
        );
        broker
            .set_config(DelegationConfig {
                enabled: true,
                depth_limit: 8,
                agent_defaults,
                ..DelegationConfig::default()
            })
            .await;

        let _ = broker.handle_request(request(1, "pt-1")).await;

        let args = mock.spawn_args.lock().await;
        assert_eq!(args.len(), 1);
        let call = &args[0];
        assert_eq!(call.agent_type, AgentType::ClaudeCode);
        assert_eq!(call.preferred_mode_id.as_deref(), Some("auto"));
        assert_eq!(call.preferred_config_values, claude_cfg);
    }

    #[tokio::test]
    async fn agent_with_no_defaults_gets_empty_preferred_args() {
        // ClaudeCode is configured in agent_defaults; a Codex request should
        // still receive (None, empty) — no cross-contamination.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-1".into())).await;
        mock.queue_send(Err(SpawnerError::Send("stop after spawn".into())))
            .await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());

        let mut agent_defaults = BTreeMap::new();
        agent_defaults.insert(
            AgentType::ClaudeCode,
            AgentDelegationDefaults {
                mode_id: Some("auto".into()),
                config_values: BTreeMap::new(),
            },
        );
        broker
            .set_config(DelegationConfig {
                enabled: true,
                depth_limit: 8,
                agent_defaults,
                ..DelegationConfig::default()
            })
            .await;

        let mut codex_req = request(1, "pt-1");
        codex_req.agent_type = AgentType::Codex;
        let _ = broker.handle_request(codex_req).await;

        let args = mock.spawn_args.lock().await;
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].agent_type, AgentType::Codex);
        assert!(args[0].preferred_mode_id.is_none());
        assert!(args[0].preferred_config_values.is_empty());
    }

    #[tokio::test]
    async fn send_failure_after_spawn_disconnects_child() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c1".into())).await;
        mock.queue_send(Err(SpawnerError::Send("agent rejected prompt".into())))
            .await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let outcome = broker.handle_request(request(1, "pt-1")).await;
        match outcome {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "spawn_failed"),
            other => panic!("expected Err, got {other:?}"),
        }
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c1"]);
    }

    #[tokio::test]
    async fn handle_request_waits_indefinitely_for_completion() {
        // No timeout race anymore: handle_request blocks on `rx.await` until
        // complete_call / cancel_* fires. This test asserts the pending entry
        // sticks around even after a generous idle window.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c1".into())).await;
        mock.queue_send(Ok(99)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-1")).await })
        };

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(broker.pending_count().await, 1);
        assert!(mock.cancels.lock().await.is_empty());

        let call_id = broker.peek_first_pending_call_id().await.unwrap();
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 99,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 50,
                    token_usage: None,
                }),
            )
            .await;

        let outcome = driver.await.unwrap();
        match outcome {
            DelegationOutcome::Ok(s) => assert_eq!(s.text, "done"),
            other => panic!("expected Ok, got {other:?}"),
        }
        // Success keeps the child process for continue_with_session; only
        // cancel / close_session tear it down.
        assert!(
            mock.disconnects.lock().await.is_empty(),
            "complete_call must not disconnect on success"
        );
    }

    // -- Task 4.6: parent-cancel cascade -----------------------------------

    #[tokio::test]
    async fn parent_cancel_cancels_all_pending_children() {
        let mock = Arc::new(MockSpawner::new());
        for i in 0..3 {
            mock.queue_spawn(Ok(format!("c{i}"))).await;
            mock.queue_send(Ok(100 + i)).await;
        }
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let mut handles = Vec::new();
        for i in 0..3 {
            let broker = broker.clone();
            handles.push(tokio::spawn(async move {
                broker.handle_request(request(1, &format!("pt-{i}"))).await
            }));
        }

        // Wait until all three are parked.
        while broker.pending_count().await < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        broker.cancel_by_parent("parent-conn").await;
        for h in handles {
            let outcome = h.await.unwrap();
            match outcome {
                DelegationOutcome::Err { code, .. } => assert_eq!(code, "canceled"),
                other => panic!("expected canceled, got {other:?}"),
            }
        }
        assert_eq!(mock.cancels.lock().await.len(), 3);
        // Each child disconnects exactly once via cancel_by_parent.
        assert_eq!(mock.disconnects.lock().await.len(), 3);
    }

    #[tokio::test]
    async fn cancel_by_parent_ignores_other_parents() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c1".into())).await;
        mock.queue_send(Ok(200)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-1")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        broker.cancel_by_parent("other-parent").await;
        // No effect — pending entry still there.
        assert_eq!(broker.pending_count().await, 1);

        let call_id = broker.peek_first_pending_call_id().await.unwrap();
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 200,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 10,
                    token_usage: None,
                }),
            )
            .await;
        let outcome = driver.await.unwrap();
        assert!(matches!(outcome, DelegationOutcome::Ok(_)));
    }

    // -- Task 4.7: depth limit ---------------------------------------------

    #[tokio::test]
    async fn depth_limit_rejects_before_spawn() {
        let mock = Arc::new(MockSpawner::new());
        // No queued spawn results — if the broker tries to spawn, it errors loudly.
        // chain: 1 (root, None) <- 2 (child of 1) <- 3 (grandchild of 2).
        // Parent = grandchild (id 3): parent_depth = 2. With limit = 2, child
        // would sit at depth 3 → reject.
        let lookup = Arc::new(MockDepth(vec![(1, None), (2, Some(1)), (3, Some(2))]))
            as Arc<dyn ConversationDepthLookup>;
        let broker = DelegationBroker::new(mock as Arc<dyn ConnectionSpawner>, lookup);
        broker
            .set_config(DelegationConfig {
                enabled: true,
                depth_limit: 2,
                ..DelegationConfig::default()
            })
            .await;
        let outcome = broker.handle_request(request(3, "pt-1")).await;
        match outcome {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "depth_limit"),
            other => panic!("expected depth_limit, got {other:?}"),
        }
    }

    // -- Pending tool_call_id queue (MCP `_meta.tool_use_id` fallback) ----

    #[tokio::test]
    async fn pending_tool_call_register_and_take_is_fifo() {
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-a".into()).await;
        broker.register_pending_tool_call("p1", "tc-b".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-a")
        );
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-b")
        );
        assert!(broker.take_pending_tool_call("p1").await.is_none());
    }

    #[tokio::test]
    async fn register_dedupes_repeated_tool_call_id() {
        // Regression: some hosts re-emit `sessionUpdate(tool_call)` (not
        // `tool_call_update`) for the same call as raw_input chunks arrive
        // or as the status flips. Without dedupe the second push leaves a
        // stale id in the queue that mis-binds the next delegation.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-a".into()).await;
        broker.register_pending_tool_call("p1", "tc-a".into()).await;
        broker.register_pending_tool_call("p1", "tc-a".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-a")
        );
        assert!(
            broker.take_pending_tool_call("p1").await.is_none(),
            "duplicate register must not leave a stale id in the queue"
        );
    }

    #[tokio::test]
    async fn register_after_claim_drops_stale_re_emit() {
        // Regression for the post-claim re-emit race: a host re-sends
        // `sessionUpdate(tool_call)` for the same id after the matching
        // MCP round-trip already consumed it (e.g. shipping the
        // `completed` status flip or a settled `raw_input`). The
        // in-queue dedupe alone leaves the queue empty at that moment,
        // so without the recently-consumed memory the re-emit would
        // sneak into the queue and mis-bind the next delegation.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-a".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-a")
        );
        // Re-emit of the same id after it was already claimed.
        broker.register_pending_tool_call("p1", "tc-a".into()).await;
        assert!(
            broker.take_pending_tool_call("p1").await.is_none(),
            "post-claim re-emit of the same id must not be re-queued"
        );
        // A genuinely new id on the same parent still flows through.
        broker.register_pending_tool_call("p1", "tc-b".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-b")
        );
    }

    #[tokio::test]
    async fn concurrent_take_and_re_register_never_leaks_stale_duplicate() {
        // TOCTOU regression: a host re-emit of the same tool_call_id
        // racing against the matching take must never inject a stale
        // duplicate. Co-locating `pending` and `consumed` under the
        // same mutex guarantees the claim → mark-consumed pair is
        // atomic, so the only two legal interleavings are:
        //
        //   * take wins → pending=[], consumed=[id]; re-register sees
        //     the id in consumed and drops it.
        //   * register wins → pending=[id] (still the original entry,
        //     in-queue dedupe drops the re-emit); take then pops it
        //     and records it in consumed.
        //
        // In neither case may the queue retain a duplicate id once
        // both futures settle. We drive many rounds with `tokio::spawn`
        // to stress the interleaving.
        let broker = std::sync::Arc::new(DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        ));
        for _ in 0..200 {
            broker.register_pending_tool_call("p1", "tc-a".into()).await;
            let b_take = broker.clone();
            let b_reg = broker.clone();
            let h_take = tokio::spawn(async move {
                b_take.take_pending_tool_call("p1").await;
            });
            let h_reg = tokio::spawn(async move {
                b_reg.register_pending_tool_call("p1", "tc-a".into()).await;
            });
            let _ = tokio::join!(h_take, h_reg);
            assert!(
                broker.take_pending_tool_call("p1").await.is_none(),
                "stale duplicate of tc-a leaked after concurrent take + re-register"
            );
        }
    }

    #[tokio::test]
    async fn consumed_memory_outlives_pending_ttl_for_long_running_delegation() {
        // Regression: a delegated child agent can run for
        // minutes-to-hours. When it finishes, the host may re-emit
        // the parent-side `tool_call` (e.g. as a `completed` status
        // flip via the non-update `ToolCall` variant). That re-emit
        // arrives well after PENDING_TOOL_CALL_TTL, so the consumed
        // memory MUST NOT age out under that TTL — otherwise the
        // stale id slips back into pending and mis-binds the next
        // delegation. Consumed entries are scoped to the parent
        // connection's lifetime instead.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-a".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-a")
        );
        // Simulate the host re-emitting the same tool_call_id 10×
        // the pending TTL later (i.e. a long-running delegation that
        // finishes after the pending eviction window).
        let long_after = Instant::now() + PENDING_TOOL_CALL_TTL * 10;
        broker
            .register_pending_tool_call_with_key_at("p1", "tc-a".into(), None, false, long_after)
            .await;
        assert!(
            broker
                .take_pending_tool_call_at("p1", long_after)
                .await
                .is_none(),
            "consumed memory must outlast the pending TTL so terminal status re-emits cannot leak through"
        );
    }

    #[tokio::test]
    async fn consumed_memory_unbounded_across_high_fan_out() {
        // Regression for the cap removal: a parent session with many
        // delegations (well past any prior per-bucket cap) must still
        // reject a late re-emit of the very first delegation's id,
        // because the consumed half has no cap. A bounded consumed
        // set with FIFO eviction would silently re-enable the
        // mis-binding bug at high fan-out.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        let first_id = "tc-first".to_string();
        broker
            .register_pending_tool_call("p1", first_id.clone())
            .await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some(first_id.as_str())
        );
        // Issue many more delegations than any prior per-bucket cap. With
        // no cap on consumed, the first id must remain remembered for the
        // lifetime of the parent connection.
        for i in 0..128 {
            let id = format!("tc-{i}");
            broker.register_pending_tool_call("p1", id.clone()).await;
            assert_eq!(
                broker.take_pending_tool_call("p1").await.as_deref(),
                Some(id.as_str())
            );
        }
        // Late re-emit of the very first id (would have been evicted
        // by the prior bounded consumed FIFO).
        broker
            .register_pending_tool_call("p1", first_id.clone())
            .await;
        assert!(
            broker.take_pending_tool_call("p1").await.is_none(),
            "consumed memory must retain the very first id even after high fan-out"
        );
    }

    #[tokio::test]
    async fn consumed_memory_cleared_on_parent_disconnect() {
        // The companion to the long-running invariant above: consumed
        // memory is scoped to the parent connection's lifetime, so
        // `drop_pending_tool_calls_for_parent` (called when the
        // parent disconnects) must clear it. Otherwise a brand-new
        // connection reusing the same id (UUID collision is unlikely
        // but UUIDs are not the only id scheme in play) would be
        // permanently blocked.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-a".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-a")
        );
        broker.drop_pending_tool_calls_for_parent("p1").await;
        broker.register_pending_tool_call("p1", "tc-a".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-a"),
            "parent disconnect must clear consumed memory so id reuse is acceptable"
        );
    }

    #[tokio::test]
    async fn take_skips_entries_older_than_ttl() {
        // Regression: an ACP `tool_call` whose matching MCP round-trip
        // never arrives (host changed its mind, transport dropped, etc.)
        // must not sit in the queue forever and mis-bind a subsequent
        // delegation. TTL eviction is exercised by advancing the
        // injected `as of` instant past PENDING_TOOL_CALL_TTL.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        let t0 = Instant::now();
        broker
            .register_pending_tool_call("p1", "stale".into())
            .await;
        // Fresh id registered "just before" the future `now`.
        broker
            .register_pending_tool_call("p1", "fresh".into())
            .await;
        let future_now = t0 + PENDING_TOOL_CALL_TTL + Duration::from_millis(50);
        // Forge "fresh" so it survives the TTL: rewrite its timestamp to
        // ~now-relative-to-future-now. Direct field access is OK — we're
        // a sibling test in the same module.
        {
            let mut map = broker.tool_calls.inner.lock().await;
            let bucket = map.get_mut("p1").expect("bucket present");
            // Re-stamp the second entry ("fresh") to `future_now`.
            if let Some(entry) = bucket
                .pending
                .iter_mut()
                .find(|p| p.tool_call_id == "fresh")
            {
                entry.registered_at = future_now;
            }
        }
        // First entry ("stale", stamped at ~t0) is past TTL relative to
        // future_now; the second ("fresh") was just re-stamped to
        // future_now and must survive.
        assert_eq!(
            broker
                .take_pending_tool_call_at("p1", future_now)
                .await
                .map(|(id, _)| id)
                .as_deref(),
            Some("fresh")
        );
        assert!(broker.take_pending_tool_call("p1").await.is_none());
    }

    #[tokio::test]
    async fn pending_tool_call_is_isolated_per_parent() {
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "p1-a".into()).await;
        broker.register_pending_tool_call("p2", "p2-a".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("p1-a")
        );
        assert_eq!(
            broker.take_pending_tool_call("p2").await.as_deref(),
            Some("p2-a")
        );
        assert!(broker.take_pending_tool_call("p1").await.is_none());
        assert!(broker.take_pending_tool_call("p2").await.is_none());
    }

    // -- (agent_type, task) correlation for parallel delegations ----------

    /// Build a match key with a fixed agent and no explicit working_dir for
    /// the common case where the test only varies the task. Use `key_for` to
    /// vary the agent, or `key_with_dir` to vary the directory.
    fn task_key(task: &str) -> DelegationMatchKey {
        key_for(AgentType::Codex, task)
    }

    fn key_for(agent_type: AgentType, task: &str) -> DelegationMatchKey {
        DelegationMatchKey {
            agent_type,
            task: task.to_string(),
            working_dir: None,
        }
    }

    fn key_with_dir(task: &str, working_dir: &str) -> DelegationMatchKey {
        DelegationMatchKey {
            agent_type: AgentType::Codex,
            task: task.to_string(),
            working_dir: Some(working_dir.to_string()),
        }
    }

    #[tokio::test]
    async fn parallel_delegations_bind_by_key_regardless_of_order() {
        // Two `delegate_to_agent` calls fire in parallel; both ACP tool_call
        // events register with their key. The MCP round-trips can claim in
        // EITHER order — each must bind to its own id by key match, never
        // swap. Pure FIFO would hand the first claimer "tc-A" regardless of
        // which call it represented.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-A".into(), Some(task_key("task A")))
            .await;
        broker
            .register_pending_tool_call_with_key("p1", "tc-B".into(), Some(task_key("task B")))
            .await;
        // Claim "task B" first (reverse of registration order).
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task B"))
                .await
                .as_deref(),
            Some("tc-B")
        );
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .as_deref(),
            Some("tc-A")
        );
        // A re-claim of an already-consumed key finds nothing.
        assert!(broker
            .take_matching_tool_call("p1", &task_key("task A"))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn parallel_same_task_different_agent_do_not_swap() {
        // Regression for Codex review: two parallel calls with the SAME task
        // text but DIFFERENT agents must bind by the full key, not by task
        // alone — otherwise the codex card could show the claude_code child
        // and vice versa.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key(
                "p1",
                "tc-codex".into(),
                Some(key_for(AgentType::Codex, "review this")),
            )
            .await;
        broker
            .register_pending_tool_call_with_key(
                "p1",
                "tc-claude".into(),
                Some(key_for(AgentType::ClaudeCode, "review this")),
            )
            .await;
        // The claude_code round-trip must claim the claude_code id even though
        // the codex entry shares the identical task and registered first.
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &key_for(AgentType::ClaudeCode, "review this"))
                .await
                .as_deref(),
            Some("tc-claude")
        );
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &key_for(AgentType::Codex, "review this"))
                .await
                .as_deref(),
            Some("tc-codex")
        );
    }

    #[tokio::test]
    async fn parallel_same_task_same_agent_different_dir_do_not_swap() {
        // Regression for Codex review round 2: two parallel calls with the
        // SAME agent and SAME task text but DIFFERENT explicit working_dir
        // (e.g. "run tests" against /repo-a vs /repo-b) must bind by the full
        // key including working_dir. Claimed in reverse registration order to
        // prove it's not arrival-order FIFO.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key(
                "p1",
                "tc-a".into(),
                Some(key_with_dir("run tests", "/repo-a")),
            )
            .await;
        broker
            .register_pending_tool_call_with_key(
                "p1",
                "tc-b".into(),
                Some(key_with_dir("run tests", "/repo-b")),
            )
            .await;
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &key_with_dir("run tests", "/repo-b"))
                .await
                .as_deref(),
            Some("tc-b")
        );
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &key_with_dir("run tests", "/repo-a"))
                .await
                .as_deref(),
            Some("tc-a")
        );
    }

    #[tokio::test]
    async fn claim_does_not_steal_sibling_and_waits_for_own_registration() {
        // Regression for the reported bug: with only the SIBLING's keyed id
        // registered, a delegation must NOT grab it (which would swap the two
        // cards) — it waits for its own id. The brief-wait loop picks it up
        // once it registers shortly after.
        let broker = std::sync::Arc::new(DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        ));
        broker
            .register_pending_tool_call_with_key("p1", "tc-A".into(), Some(task_key("task A")))
            .await;
        // Immediate claim for "task B" while only tc-A (task A) is pending
        // must refuse to steal tc-A.
        assert!(
            broker
                .take_matching_tool_call("p1", &task_key("task B"))
                .await
                .is_none(),
            "must not steal a sibling's keyed id"
        );
        // tc-A is still claimable by its own key.
        let broker_bg = broker.clone();
        let register_late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            broker_bg
                .register_pending_tool_call_with_key("p1", "tc-B".into(), Some(task_key("task B")))
                .await;
        });
        // The brief-wait claim polls until tc-B (task B) registers.
        let claimed = broker
            .claim_pending_tool_call_with_brief_wait("p1", &task_key("task B"))
            .await;
        register_late.await.unwrap();
        assert_eq!(claimed.map(|(id, _)| id).as_deref(), Some("tc-B"));
        // tc-A remains for its own key.
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .as_deref(),
            Some("tc-A")
        );
    }

    #[tokio::test]
    async fn lone_unkeyed_entry_is_not_claimed_in_loop_only_post_budget() {
        // A host that ships no parseable `raw_input` registers match_key=None.
        // The in-loop path NEVER claims it — not even when it's the only entry,
        // and regardless of how old it gets (10s here). Entry age is no proof a
        // key isn't still coming: a serialized round-trip can register/backfill
        // arbitrarily late, and the entry could belong to a parallel sibling
        // whose owner hasn't registered yet (the staggered-singleton race —
        // Codex review). Arrival-order FIFO is reserved for the post-budget last
        // resort, which only runs once the CALLER has waited its full budget.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-A".into()).await;
        // Even aged 10s (well past any heuristic grace, still < TTL so not
        // evicted), the in-loop claim refuses to hand out the unkeyed id.
        let way_aged = Instant::now() + Duration::from_secs(10);
        assert!(
            broker
                .take_matching_tool_call_at("p1", &task_key("whatever"), way_aged)
                .await
                .is_none(),
            "an unkeyed entry must never be claimed in-loop, regardless of age"
        );
        // The post-budget last resort is where a genuinely keyless entry binds.
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-A")
        );
    }

    #[tokio::test]
    async fn parallel_unkeyed_entries_are_not_claimed_in_loop() {
        // THE Finding 1 regression. Two delegations whose initial ToolCalls
        // registered UNKEYED (args arrive later on a ToolCallUpdate). Before
        // either is keyed, a round-trip arrives. The old `all unkeyed →
        // pop_front` handed it the OLDEST entry (tc-A), mis-binding it to the
        // wrong delegation. The in-loop claim now withholds (None) because no
        // key matches — arrival-order FIFO is left to the post-budget last
        // resort. Age never unlocks an in-loop claim.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-A".into()).await;
        broker.register_pending_tool_call("p1", "tc-B".into()).await;
        // Aged (but < TTL, so not evicted): still withheld in-loop.
        let aged = Instant::now() + Duration::from_secs(5);
        assert!(
            broker
                .take_matching_tool_call_at("p1", &task_key("task B"), aged)
                .await
                .is_none(),
            "unkeyed siblings must not be FIFO-claimed in-loop"
        );
        // Neither entry was consumed.
        let map = broker.tool_calls.inner.lock().await;
        assert_eq!(map.get("p1").expect("bucket present").pending.len(), 2);
    }

    #[tokio::test]
    async fn parallel_unkeyed_resolves_by_backfilled_key_not_fifo() {
        // The pay-off: while the claim is withheld, the args arrive and
        // backfill a key onto the sibling. The round-trip then binds by EXACT
        // MATCH to its own id — never the FIFO-oldest. This is the would-be
        // mis-bind turned into a correct correlation.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-A".into()).await;
        broker.register_pending_tool_call("p1", "tc-B".into()).await;
        // tc-B's args land → backfills its key.
        broker
            .register_pending_tool_call_with_key("p1", "tc-B".into(), Some(task_key("task B")))
            .await;
        // The "task B" round-trip binds to tc-B by key, not to the older tc-A.
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task B"))
                .await
                .as_deref(),
            Some("tc-B")
        );
        // tc-A is untouched, still pending for its own key/round-trip.
        let map = broker.tool_calls.inner.lock().await;
        let pending = &map.get("p1").expect("bucket present").pending;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].tool_call_id, "tc-A");
    }

    #[tokio::test]
    async fn post_budget_fallback_still_fifos_parallel_unkeyed() {
        // A genuinely keyless host (no key ever lands) must still bind both
        // parallel delegations end-to-end. The in-loop claim withholds them,
        // but the post-budget last resort `take_pending_tool_call` claims them
        // oldest-first — the best a keyless host allows, and unchanged from
        // before. Only the premature in-loop FIFO is gone.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-A".into()).await;
        broker.register_pending_tool_call("p1", "tc-B".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-A")
        );
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-B")
        );
    }

    #[tokio::test]
    async fn brief_wait_binds_own_late_registration_not_unkeyed_sibling() {
        // The staggered-singleton timeline Codex flagged, end-to-end: only an
        // UNKEYED sibling (tc-A) is visible when a DIFFERENT delegation's
        // round-trip (task B) starts claiming; B's own keyed `tool_call`
        // registers a little later, still inside the wait budget. The brief-wait
        // loop must bind B to its OWN id (tc-B) by exact match, never FIFO-steal
        // the older unkeyed tc-A. The old in-loop FIFO popped tc-A on the very
        // first poll (all-unkeyed); a grace gate would still steal it once tc-A
        // aged past the grace before tc-B arrived. Deferring all FIFO to the
        // post-budget — i.e. binding by exact match in-loop only — is what makes
        // this correct.
        let broker = std::sync::Arc::new(DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        ));
        broker.register_pending_tool_call("p1", "tc-A".into()).await;
        // B's own ACP registration lands ~200ms in — well after any age-based
        // heuristic would have fired, but far inside the ~2s claim budget.
        let broker_bg = broker.clone();
        let register_late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            broker_bg
                .register_pending_tool_call_with_key("p1", "tc-B".into(), Some(task_key("task B")))
                .await;
        });
        let claimed = broker
            .claim_pending_tool_call_with_brief_wait("p1", &task_key("task B"))
            .await;
        register_late.await.unwrap();
        assert_eq!(
            claimed.map(|(id, _)| id).as_deref(),
            Some("tc-B"),
            "must wait for its own registration, not FIFO-steal the unkeyed sibling"
        );
        // tc-A is untouched, still pending for its own correlation.
        let map = broker.tool_calls.inner.lock().await;
        let pending = &map.get("p1").expect("bucket present").pending;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].tool_call_id, "tc-A");
    }

    #[tokio::test]
    async fn reemit_backfills_key_onto_unkeyed_entry() {
        // A host that re-emits the `session/update(tool_call)` variant: the
        // first ToolCall has no parseable args (registers match_key=None), a
        // later re-emit carries the full args. The re-emit must backfill the
        // key onto the existing entry (not push a duplicate, not be dropped)
        // so key matching works.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-A".into()).await;
        broker
            .register_pending_tool_call_with_key("p1", "tc-A".into(), Some(task_key("task A")))
            .await;
        // Now claimable by the backfilled key.
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .as_deref(),
            Some("tc-A")
        );
        assert!(broker.take_pending_tool_call("p1").await.is_none());
    }

    #[tokio::test]
    async fn fallback_never_steals_a_keyed_sibling() {
        // A keyed sibling is pending but the requesting round-trip's key never
        // matches (its own tool_call was genuinely lost). The post-budget last
        // resort must NOT hand out the keyed sibling — stealing it would just
        // move the dead card from this delegation to the sibling. It returns
        // None (→ caller mints a synthetic id), and the sibling stays claimable
        // by its own round-trip. (Regression: the old behavior FIFO-popped the
        // keyed entry here, swapping which delegation broke.)
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-A".into(), Some(task_key("task A")))
            .await;
        // No entry matches "task Z", and a keyed entry is present, so the
        // match step refuses to claim.
        assert!(broker
            .take_matching_tool_call("p1", &task_key("task Z"))
            .await
            .is_none());
        // The post-budget last resort steps over the keyed entry → None.
        assert!(
            broker.take_pending_tool_call("p1").await.is_none(),
            "must not steal a keyed sibling via the anonymous fallback"
        );
        // The keyed sibling is untouched — still claimable by its own key.
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .as_deref(),
            Some("tc-A")
        );
    }

    #[tokio::test]
    async fn keyed_entry_survives_past_ttl_for_serialized_round_trip() {
        // THE headline regression for the reported bug. A 2nd parallel
        // delegation's tool_call registers (keyed), then its MCP round-trip is
        // serialized far behind the 1st delegation — arriving well past
        // PENDING_TOOL_CALL_TTL. The keyed entry must NOT be aged out: an exact
        // key match claims it at any age, so the parent card binds instead of
        // falling to a synthetic id. (Observed live: round-trip landed 77s
        // after registration, past the 60s TTL → evicted → synthetic → dead
        // card stuck on "sub-agent running…".)
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key(
                "p1",
                "tc-late".into(),
                Some(task_key("slow task")),
            )
            .await;
        // Claim "as of" long past the TTL — simulates the round-trip arriving
        // after a many-times-TTL wait behind a serialized sibling.
        let way_past_ttl = Instant::now() + PENDING_TOOL_CALL_TTL * 10;
        assert_eq!(
            broker
                .take_matching_tool_call_at("p1", &task_key("slow task"), way_past_ttl)
                .await
                .as_deref(),
            Some("tc-late"),
            "a keyed entry must remain claimable by exact key match regardless of age"
        );
    }

    #[tokio::test]
    async fn unkeyed_entry_is_still_aged_out() {
        // The flip side: UNKEYED entries (host shipped no parseable raw_input)
        // remain anonymous and arrival-order-correlated, so a stale one MUST
        // still be GC'd by age — otherwise it could mis-bind a much later
        // unkeyed delegation via the FIFO path.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call("p1", "tc-stale".into())
            .await;
        let way_past_ttl = Instant::now() + PENDING_TOOL_CALL_TTL * 10;
        // Unkeyed + stale → evicted by the match path's GC → nothing to claim.
        assert!(broker
            .take_matching_tool_call_at("p1", &task_key("whatever"), way_past_ttl)
            .await
            .is_none());
        // And the anonymous path agrees it's gone.
        assert!(broker
            .take_pending_tool_call_at("p1", way_past_ttl)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn explicit_tool_use_id_consumes_pending_entry_acp_first() {
        // Codex review fix: client supplies the real id via `_meta.tool_use_id`
        // AFTER the dispatcher already registered it (ACP-before-MCP). The
        // explicit-id path must consume the keyed pending entry so it can't
        // linger (keyed entries are retained indefinitely) and be mis-claimed
        // by a later same-key delegation.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-x".into(), Some(task_key("task A")))
            .await;
        broker.consume_explicit_tool_call("p1", "tc-x").await;
        // No longer claimable by its key.
        assert!(broker
            .take_matching_tool_call("p1", &task_key("task A"))
            .await
            .is_none());
        // A late ACP re-registration of the same id is dropped (consumed).
        broker
            .register_pending_tool_call_with_key("p1", "tc-x".into(), Some(task_key("task A")))
            .await;
        assert!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .is_none(),
            "a re-registration after explicit consume must stay dropped"
        );
    }

    #[tokio::test]
    async fn explicit_tool_use_id_consumes_pending_entry_mcp_first() {
        // The MCP-before-ACP order: the explicit-id request is handled before
        // the ACP tool_call event registers. consume_explicit_tool_call records
        // the id as consumed up front, so the later registration is dropped.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.consume_explicit_tool_call("p1", "tc-y").await;
        broker
            .register_pending_tool_call_with_key("p1", "tc-y".into(), Some(task_key("task B")))
            .await;
        assert!(broker
            .take_matching_tool_call("p1", &task_key("task B"))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn tombstone_removes_stale_keyed_entry() {
        // A `delegate_to_agent` tool call registered a keyed entry but its MCP
        // round-trip never reached the broker (the call failed / the turn was
        // interrupted). A terminal `ToolCallUpdate` tombstones the entry so it
        // can't linger indefinitely (keyed entries are never aged out).
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-stale".into(), Some(task_key("task A")))
            .await;
        assert!(broker.tombstone_pending_tool_call("p1", "tc-stale").await);
        assert!(broker
            .take_matching_tool_call("p1", &task_key("task A"))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn tombstone_prevents_same_key_misbind() {
        // The High regression: without the tombstone, a stale keyed entry is
        // retained forever and a LATER identical-key delegation claims its dead
        // id (the exact-key scan returns the oldest match). After tombstoning the
        // stale entry, a fresh registration for the same key binds to the FRESH
        // id instead.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-stale".into(), Some(task_key("task A")))
            .await;
        broker.tombstone_pending_tool_call("p1", "tc-stale").await;
        broker
            .register_pending_tool_call_with_key("p1", "tc-fresh".into(), Some(task_key("task A")))
            .await;
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .as_deref(),
            Some("tc-fresh"),
            "a later same-key delegation must claim the fresh id, not the tombstoned one"
        );
    }

    #[tokio::test]
    async fn tombstone_leaves_other_entries_intact() {
        // A terminal update for an unrelated (non-delegation) id no-ops and must
        // leave a registered delegation untouched.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-keep".into(), Some(task_key("task A")))
            .await;
        assert!(
            !broker
                .tombstone_pending_tool_call("p1", "tc-bash-123")
                .await
        );
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .as_deref(),
            Some("tc-keep")
        );
    }

    #[tokio::test]
    async fn tombstone_then_reregister_same_id_stays_dropped() {
        // After tombstoning a real entry, an out-of-order re-registration of the
        // same id is dropped by the Tier-1 consumed check — mirrors
        // `explicit_tool_use_id_consumes_pending_entry_acp_first`.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-stale".into(), Some(task_key("task A")))
            .await;
        assert!(broker.tombstone_pending_tool_call("p1", "tc-stale").await);
        broker
            .register_pending_tool_call_with_key("p1", "tc-stale".into(), Some(task_key("task A")))
            .await;
        assert!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .is_none(),
            "a re-registration after tombstone must stay dropped"
        );
    }

    #[tokio::test]
    async fn tombstone_noop_does_not_record_consumed() {
        // The tombstone runs for EVERY terminal tool-call update, most of them
        // non-delegations. A no-op tombstone (id not pending) must NOT record
        // `consumed` — otherwise `consumed` (no TTL/cap) would grow with every
        // completed tool call, and a later legitimate registration of that id
        // would be wrongly dropped.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        assert!(!broker.tombstone_pending_tool_call("p1", "tc-x").await);
        broker
            .register_pending_tool_call_with_key("p1", "tc-x".into(), Some(task_key("task A")))
            .await;
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .as_deref(),
            Some("tc-x"),
            "a no-op tombstone must not record consumed and drop a later registration"
        );
    }

    #[tokio::test]
    async fn tombstone_removes_only_the_matching_entry_from_a_multi_entry_bucket() {
        // Tombstoning a MIDDLE entry removes only that id (retain is by exact
        // tool_call_id, position-independent) and leaves the siblings claimable
        // by their own keys.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-a".into(), Some(task_key("task A")))
            .await;
        broker
            .register_pending_tool_call_with_key("p1", "tc-b".into(), Some(task_key("task B")))
            .await;
        broker
            .register_pending_tool_call_with_key("p1", "tc-c".into(), Some(task_key("task C")))
            .await;
        assert!(broker.tombstone_pending_tool_call("p1", "tc-b").await);
        assert!(broker
            .take_matching_tool_call("p1", &task_key("task B"))
            .await
            .is_none());
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .as_deref(),
            Some("tc-a")
        );
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task C"))
                .await
                .as_deref(),
            Some("tc-c")
        );
    }

    #[tokio::test]
    async fn keyed_pending_entries_have_no_count_cap() {
        // Regression for the PENDING_QUEUE_CAP removal: a high-fan-out parent
        // can register hundreds of keyed pending tool_calls — each awaiting its
        // own serialized MCP round-trip — and EVERY one is retained. The old
        // hard cap evicted the oldest keyed entry past 32, orphaning its card to
        // a synthetic id. Keyed entries are now bounded only by claim, terminal
        // tombstoning, and per-parent teardown.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        const N: usize = 256;
        for i in 0..N {
            broker
                .register_pending_tool_call_with_key(
                    "p1",
                    format!("tc-{i}"),
                    Some(task_key(&format!("task {i}"))),
                )
                .await;
        }
        {
            let map = broker.tool_calls.inner.lock().await;
            let bucket = map.get("p1").expect("bucket present");
            assert_eq!(
                bucket.pending.len(),
                N,
                "all keyed pending entries must be retained — no count cap"
            );
        }
        // Each entry stays individually claimable by its exact key, in any
        // order — proving none were dropped or mis-bound by fan-out.
        for i in [0usize, N / 2, N - 1] {
            let claimed = broker
                .take_matching_tool_call("p1", &task_key(&format!("task {i}")))
                .await;
            assert_eq!(claimed.as_deref(), Some(format!("tc-{i}").as_str()));
        }
    }

    #[tokio::test]
    async fn keyed_pending_entry_drains_via_tombstone() {
        // The drain path the no-cap design relies on: when the parent-side ACP
        // tool_call goes terminal before its MCP round-trip ever claims it,
        // `tombstone_pending_tool_call` removes the keyed entry (so it can't
        // linger) AND records it consumed (so a late re-emit can't mis-bind a
        // later delegation sharing the same key).
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-x".into(), Some(task_key("task x")))
            .await;
        assert!(
            broker.tombstone_pending_tool_call("p1", "tc-x").await,
            "tombstone must report it removed the pending entry"
        );
        assert!(
            broker
                .take_matching_tool_call("p1", &task_key("task x"))
                .await
                .is_none(),
            "a tombstoned entry must be drained from pending"
        );
        // Re-register of the same id after tombstoning is dropped by the
        // Tier-1 consumed check, so it can never be claimed.
        broker
            .register_pending_tool_call_with_key("p1", "tc-x".into(), Some(task_key("task x")))
            .await;
        assert!(
            broker
                .take_matching_tool_call("p1", &task_key("task x"))
                .await
                .is_none(),
            "consumed memory must reject a re-emit of a tombstoned id"
        );
    }

    #[tokio::test]
    async fn reregistration_refines_key_with_late_working_dir() {
        // Codex re-review fix: the same tool_call_id first registers with a key
        // LACKING working_dir (an early parseable raw_input), then a later
        // ToolCallUpdate completes it with the explicit working_dir. The stored
        // key must be REPLACED with the fuller one — otherwise the MCP claim
        // keying on Some(dir) can't match the stale None and orphans to a
        // synthetic id (dead card for explicit-working-dir delegations).
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key(
                "p1",
                "tc-d".into(),
                Some(key_for(AgentType::Codex, "build")),
            )
            .await;
        // Later update adds the explicit working_dir → key is refined in place.
        broker
            .register_pending_tool_call_with_key(
                "p1",
                "tc-d".into(),
                Some(key_with_dir("build", "/repo")),
            )
            .await;
        // The stale `working_dir: None` key no longer matches (it was replaced)…
        assert!(broker
            .take_matching_tool_call("p1", &key_for(AgentType::Codex, "build"))
            .await
            .is_none());
        // …and the refined `Some("/repo")` key claims the real id.
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &key_with_dir("build", "/repo"))
                .await
                .as_deref(),
            Some("tc-d"),
            "the MCP claim with the explicit working_dir must match the refined key"
        );
    }

    #[tokio::test]
    async fn empty_parent_tool_use_id_claims_pending_then_completes() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c1".into())).await;
        mock.queue_send(Ok(7)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        broker
            .register_pending_tool_call("parent-conn", "tu-from-acp".into())
            .await;
        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // The captured ACP id was consumed.
        assert!(broker.take_pending_tool_call("parent-conn").await.is_none());
        let call_id = broker.peek_first_pending_call_id().await.unwrap();
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "ok".into(),
                    child_conversation_id: 7,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        let outcome = driver.await.unwrap();
        assert!(matches!(outcome, DelegationOutcome::Ok(_)));
    }

    #[tokio::test]
    async fn empty_parent_tool_use_id_claims_pending_arriving_late() {
        // Regression: when the parent's ACP `session/update(tool_call)`
        // lands at the lifecycle dispatcher AFTER `broker.handle_request`
        // already entered the claim phase, the brief poll loop must still
        // pick it up rather than falling back to the synthetic UUID.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-late".into())).await;
        mock.queue_send(Ok(13)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "")).await })
        };

        // Give the driver time to enter the claim wait loop on an empty
        // queue, then register the ACP id (simulates the dispatcher's
        // ToolCall handling landing late).
        tokio::time::sleep(Duration::from_millis(30)).await;
        broker
            .register_pending_tool_call("parent-conn", "tu-late".into())
            .await;

        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // The late-arriving ACP id was consumed by the broker — no leftover
        // entry.
        assert!(broker.take_pending_tool_call("parent-conn").await.is_none());
        let call_id = broker.peek_first_pending_call_id().await.unwrap();
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "late ok".into(),
                    child_conversation_id: 13,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        let outcome = driver.await.unwrap();
        assert!(matches!(outcome, DelegationOutcome::Ok(_)));
    }

    #[tokio::test]
    async fn empty_parent_tool_use_id_with_no_pending_falls_back_to_uuid() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c1".into())).await;
        mock.queue_send(Ok(11)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let call_id = broker.peek_first_pending_call_id().await.unwrap();
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "fallback ok".into(),
                    child_conversation_id: 11,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        let outcome = driver.await.unwrap();
        assert!(matches!(outcome, DelegationOutcome::Ok(_)));
    }

    #[tokio::test]
    async fn cancel_by_parent_also_drops_pending_tool_calls() {
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call("parent-conn", "tu-1".into())
            .await;
        broker.cancel_by_parent("parent-conn").await;
        assert!(broker.take_pending_tool_call("parent-conn").await.is_none());
    }

    #[tokio::test]
    async fn turn_cancel_keeps_consumed_rejects_reemit() {
        // A turn/prompt cancel (parent connection STAYS ALIVE) must NOT drop the
        // `consumed` tool_call memory. Otherwise a host re-emit of an
        // already-claimed id (e.g. a terminal status-flip) re-registers as fresh
        // `pending` and the next same-key delegation mis-binds to it — the
        // dead-card/wrong-child class this correlation machinery exists to
        // prevent. `cancel_by_parent_turn` retains `consumed`, so the re-emit
        // stays rejected by the Tier-1 consumed check.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        // Register + claim a keyed id (the delegation that just ran).
        broker
            .register_pending_tool_call_with_key("p1", "tc-A".into(), Some(task_key("task A")))
            .await;
        assert_eq!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .as_deref(),
            Some("tc-A"),
        );
        // Turn cancel — parent still alive.
        broker.cancel_by_parent_turn("p1").await;
        // Host re-emits the now-consumed id with the same key.
        broker
            .register_pending_tool_call_with_key("p1", "tc-A".into(), Some(task_key("task A")))
            .await;
        assert!(
            broker
                .take_matching_tool_call("p1", &task_key("task A"))
                .await
                .is_none(),
            "re-emit of a consumed id must stay rejected across a turn cancel"
        );
    }

    #[tokio::test]
    async fn turn_cancel_drops_unclaimed_pending() {
        // The unclaimed `pending` half is cleared by a turn cancel (tombstoned
        // into `consumed`): the cancelled turn's serial round-trip won't arrive,
        // so the stale keyed entry must not remain claimable by a later same-key
        // delegation. `take_matching` scans only `pending`, so it returns None.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-B".into(), Some(task_key("task B")))
            .await;
        broker.cancel_by_parent_turn("p1").await;
        assert!(
            broker
                .take_matching_tool_call("p1", &task_key("task B"))
                .await
                .is_none(),
            "unclaimed pending must not stay claimable after a turn cancel"
        );
    }

    #[tokio::test]
    async fn turn_cancel_tombstones_pending_rejects_late_reemit() {
        // Stronger than the clear test: after a turn cancel clears an UNCLAIMED
        // keyed pending id, a late host re-emit of that SAME id must not
        // resurrect it as a claimable entry — otherwise the next same-key
        // delegation would mis-bind to the stale id. The cancel tombstones the
        // cleared id into `consumed`, so the re-emit is dropped by the Tier-1
        // consumed check and never re-enters `pending`.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .register_pending_tool_call_with_key("p1", "tc-X".into(), Some(task_key("task X")))
            .await;
        broker.cancel_by_parent_turn("p1").await;
        // Late re-emit of the cancelled turn's unclaimed id (same key).
        broker
            .register_pending_tool_call_with_key("p1", "tc-X".into(), Some(task_key("task X")))
            .await;
        assert!(
            broker
                .take_matching_tool_call("p1", &task_key("task X"))
                .await
                .is_none(),
            "a re-emit of a tombstoned (cleared-on-cancel) pending id must not be claimable"
        );
    }

    #[tokio::test]
    async fn teardown_cancel_clears_consumed() {
        // The teardown variant (`cancel_by_parent`) DOES drop consumed — the
        // connection is going away, so a reused connection_id must start clean.
        // Contrast with `turn_cancel_keeps_consumed_rejects_reemit`.
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker.register_pending_tool_call("p1", "tc-A".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-A"),
        );
        broker.cancel_by_parent("p1").await;
        // consumed cleared → the same id re-registers and is claimable again.
        broker.register_pending_tool_call("p1", "tc-A".into()).await;
        assert_eq!(
            broker.take_pending_tool_call("p1").await.as_deref(),
            Some("tc-A"),
            "teardown cancel must clear consumed so id reuse is acceptable"
        );
    }

    #[tokio::test]
    async fn cancel_by_parent_turn_drains_synchronously_then_tears_down_child() {
        // The turn cancel must (a) drop the tracker + remove parked calls
        // SYNCHRONOUSLY — before the connection loop could accept the next
        // prompt — so a delayed cancel can't tombstone/cancel a NEXT turn's
        // entries (the invariant `drop_tool_calls_for_parent` relies on); and
        // (b) still fully tear the child down (backgrounded), resolving the
        // awaiting `handle_request` as canceled exactly once.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-1".into())).await;
        mock.queue_send(Ok(7)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        // Park a delegation for "parent-conn"...
        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-1")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // ...plus a separate unclaimed keyed tracker entry on the same parent.
        broker
            .register_pending_tool_call_with_key(
                "parent-conn",
                "tc-Z".into(),
                Some(task_key("task Z")),
            )
            .await;

        broker.cancel_by_parent_turn("parent-conn").await;

        // (a) Synchronously — no sleep: the parked call is removed and the
        // tracker entry is dropped (tombstoned), so neither can leak into a
        // next-turn registration that the backgrounded teardown might clobber.
        assert_eq!(
            broker.pending_count().await,
            0,
            "parked call must be drained synchronously by the turn cancel"
        );
        assert!(
            broker
                .take_matching_tool_call("parent-conn", &task_key("task Z"))
                .await
                .is_none(),
            "tracker pending must be dropped synchronously by the turn cancel"
        );

        // (b) The backgrounded child teardown still resolves the driver as
        // canceled and tears the child down exactly once.
        match driver.await.unwrap() {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "canceled"),
            other => panic!("expected canceled, got {other:?}"),
        }
        assert_eq!(mock.cancels.lock().await.as_slice(), &["child-1"]);
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["child-1"]);
    }

    #[tokio::test]
    async fn depth_limit_allows_root() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c1".into())).await;
        mock.queue_send(Ok(7)).await;
        let lookup = Arc::new(MockDepth(vec![(1, None)])) as Arc<dyn ConversationDepthLookup>;
        let broker = DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, lookup);
        broker
            .set_config(DelegationConfig {
                enabled: true,
                depth_limit: 2,
                ..DelegationConfig::default()
            })
            .await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-1")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let call_id = broker.peek_first_pending_call_id().await.unwrap();
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "ok".into(),
                    child_conversation_id: 7,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        let outcome = driver.await.unwrap();
        assert!(matches!(outcome, DelegationOutcome::Ok(_)));
    }

    // -- Meta writer lifecycle --------------------------------------------

    use crate::acp::delegation::meta_writer::mock::MockMetaWriter;
    use crate::acp::delegation::meta_writer::DelegationMetaWriter;

    async fn broker_with_meta(
        mock: Arc<MockSpawner>,
        writer: Arc<MockMetaWriter>,
    ) -> DelegationBroker {
        let broker = DelegationBroker::with_meta_writer(
            mock as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
            writer as Arc<dyn DelegationMetaWriter>,
        );
        enable_delegation(&broker).await;
        broker
    }

    #[tokio::test]
    async fn meta_writes_carry_task_preview_and_task_id_on_every_write() {
        // Meta is replace-wholesale on the ToolCallState, so BOTH the running
        // and the terminal writes must carry the task label + broker task id —
        // they are the persisted card's only label source on hosts whose
        // raw_input never carries the arguments (Cursor).
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-t".into())).await;
        mock.queue_send(Ok(42)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-task")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 42,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        let calls = writer.snapshot().await;
        assert_eq!(calls.len(), 2);
        for call in &calls {
            let inner = call
                .meta
                .get("codeg.delegation")
                .unwrap()
                .as_object()
                .unwrap();
            assert_eq!(
                inner.get("task_preview").unwrap().as_str().unwrap(),
                "do x",
                "every write must keep the task label (status={})",
                inner.get("status").unwrap()
            );
            assert_eq!(
                inner.get("task_id").unwrap().as_str().unwrap(),
                &call_id,
                "task_id must be the broker call id on every write"
            );
        }
    }

    #[tokio::test]
    async fn identityless_fifo_claim_restores_delegate_identity() {
        // Cursor path: the ACP announcement registered an identity-less
        // candidate ("MCP: tool" + "{}"), the MCP round-trip arrives with no
        // explicit tool_use id. The post-budget FIFO claim must land on the
        // candidate AND restore the call's identity (canonical delegate title
        // + reconstructed arguments) on the live tool call.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-i".into())).await;
        mock.queue_send(Ok(43)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;
        broker
            .register_identityless_tool_call("parent-conn", "call-cursor-7\nfc_x_0".into())
            .await;

        let driver = {
            let broker = broker.clone();
            // Empty tool_use id → claim path (2s budget, then FIFO).
            tokio::spawn(async move { broker.handle_request(request(1, "")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 43,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        let identities = writer.identity_snapshot().await;
        assert_eq!(identities.len(), 1, "exactly one identity restoration");
        let id_write = &identities[0];
        assert_eq!(id_write.parent_connection_id, "parent-conn");
        assert_eq!(id_write.tool_call_id, "call-cursor-7\nfc_x_0");
        assert_eq!(
            id_write.title,
            crate::acp::delegation::DELEGATE_TOOL_REWRITE_TITLE
        );
        assert_eq!(
            id_write.raw_input.get("task").unwrap().as_str().unwrap(),
            "do x"
        );
        assert_eq!(
            id_write
                .raw_input
                .get("agent_type")
                .unwrap()
                .as_str()
                .unwrap(),
            "claude_code"
        );
        // The meta writes bound to the REAL claimed id, not a synthetic one.
        let metas = writer.snapshot().await;
        assert!(metas
            .iter()
            .all(|m| m.parent_tool_use_id == "call-cursor-7\nfc_x_0"));
    }

    #[tokio::test]
    async fn keyed_claim_does_not_rewrite_host_identity() {
        // A host that shipped real raw_input (keyed registration) keeps its
        // own wire identity — reconstructing raw_input would clobber
        // host-specific fields.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-k".into())).await;
        mock.queue_send(Ok(44)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;
        broker
            .register_pending_tool_call_with_key(
                "parent-conn",
                "tc-keyed".into(),
                Some(DelegationMatchKey {
                    agent_type: AgentType::ClaudeCode,
                    task: "do x".into(),
                    working_dir: None,
                }),
            )
            .await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 44,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        assert!(
            writer.identity_snapshot().await.is_empty(),
            "keyed (host-identified) claims must not be rewritten"
        );
        let metas = writer.snapshot().await;
        assert!(metas.iter().all(|m| m.parent_tool_use_id == "tc-keyed"));
    }

    #[tokio::test]
    async fn rewrite_identityless_tool_call_claims_and_writes_status_identity() {
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(Arc::new(MockSpawner::new()), writer.clone()).await;
        broker
            .register_identityless_tool_call("parent-conn", "call-status-1".into())
            .await;

        let claimed = broker
            .rewrite_identityless_tool_call(
                "parent-conn",
                crate::acp::delegation::STATUS_TOOL_REWRITE_TITLE,
                serde_json::json!({"task_ids": ["t-1"], "wait_ms": 0}),
            )
            .await;
        assert_eq!(claimed.as_deref(), Some("call-status-1"));
        let identities = writer.identity_snapshot().await;
        assert_eq!(identities.len(), 1);
        assert_eq!(
            identities[0].title,
            crate::acp::delegation::STATUS_TOOL_REWRITE_TITLE
        );
        assert_eq!(
            identities[0].raw_input.get("task_ids").unwrap()[0]
                .as_str()
                .unwrap(),
            "t-1"
        );
        // Consumed: the candidate can't be claimed again by the delegate FIFO.
        assert!(broker.take_pending_tool_call("parent-conn").await.is_none());
    }

    #[tokio::test]
    async fn rewrite_skips_connections_that_never_announced_identityless() {
        // Plain unkeyed registration (a keyless delegation invocation) must
        // NOT be renamed — and the sticky gate must return instantly (no
        // 300 ms poll) for such connections. Also: the entry stays claimable
        // by the delegate FIFO afterwards.
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(Arc::new(MockSpawner::new()), writer.clone()).await;
        broker
            .register_pending_tool_call("parent-conn", "tc-plain".into())
            .await;

        let started = std::time::Instant::now();
        let claimed = broker
            .rewrite_identityless_tool_call(
                "parent-conn",
                crate::acp::delegation::STATUS_TOOL_REWRITE_TITLE,
                serde_json::json!({"task_ids": ["t-1"]}),
            )
            .await;
        assert!(claimed.is_none());
        // The poll path sleeps ≥300 ms (30 × 10 ms lower bounds); the gate
        // path sleeps zero. 280 ms discriminates the two while leaving slack
        // for full-suite parallel-load scheduling jitter.
        assert!(
            started.elapsed() < Duration::from_millis(280),
            "sticky gate must skip the poll for hosts without identity-less announcements"
        );
        assert!(writer.identity_snapshot().await.is_empty());
        assert_eq!(
            broker
                .take_pending_tool_call("parent-conn")
                .await
                .as_deref(),
            Some("tc-plain"),
            "the plain unkeyed entry must remain for the delegate FIFO"
        );
    }

    #[tokio::test]
    async fn meta_writer_records_running_then_completed_on_happy_path() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-1".into())).await;
        mock.queue_send(Ok(42)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-real")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 42,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        let calls = writer.snapshot().await;
        assert_eq!(calls.len(), 2);
        // First write: running, with child connection + conversation ids.
        let first = &calls[0];
        assert_eq!(first.parent_tool_use_id, "pt-real");
        let inner_first = first
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(
            inner_first.get("status").unwrap().as_str().unwrap(),
            "running"
        );
        assert_eq!(
            inner_first
                .get("child_connection_id")
                .unwrap()
                .as_str()
                .unwrap(),
            "child-conn-1"
        );
        assert_eq!(
            inner_first
                .get("child_conversation_id")
                .unwrap()
                .as_i64()
                .unwrap(),
            42
        );
        // Second write: completed.
        let second = &calls[1];
        let inner_second = second
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(
            inner_second.get("status").unwrap().as_str().unwrap(),
            "completed"
        );
    }

    #[tokio::test]
    async fn meta_writer_records_failed_on_err_outcome() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-2".into())).await;
        mock.queue_send(Ok(7)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-err")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::from_err(
                    DelegationError::SubagentRuntimeError("agent died".into()),
                    Some(7),
                ),
            )
            .await;
        driver.await.unwrap();

        let calls = writer.snapshot().await;
        assert_eq!(calls.len(), 2);
        let inner = calls[1]
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(inner.get("status").unwrap().as_str().unwrap(), "failed");
        assert_eq!(
            inner.get("error_code").unwrap().as_str().unwrap(),
            "subagent_error"
        );
    }

    // -- Registration-race: child terminal failure before the entry is parked --

    /// Headline regression: a child terminal failure (auth error / immediate
    /// process death) that fires AFTER the broker reserved the child but BEFORE
    /// it parked the pending entry must still resolve the parked request — not
    /// no-op and strand it on `rx.await` forever. The `send_gate` pins
    /// `handle_request` in exactly that window; we fire the failure, release the
    /// gate, and assert the request resolves as canceled (carrying the
    /// terminal-error detail) with a single child disconnect and a clean
    /// running→failed meta trail.
    #[tokio::test]
    async fn child_failure_before_park_resolves_instead_of_hanging() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-fast-fail".into())).await;
        mock.queue_send(Ok(55)).await;
        let release = mock.install_send_gate().await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-fast")).await })
        };

        // Wait until handle_request has spawned + reserved the child and is
        // held inside send_prompt by the gate — entry NOT yet parked.
        loop {
            if broker.reserved_child_count().await == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(broker.pending_count().await, 0, "entry not parked yet");

        // Child dies before the entry is parked. With the reservation in place
        // this buffers (rather than no-oping on a not-yet-existent entry).
        broker
            .cancel_by_child_connection("c-fast-fail", Some("Authentication required"))
            .await;
        assert_eq!(broker.early_cancel_count().await, 1, "failure buffered");

        // Release send_prompt → handle_request parks, drains the buffered
        // failure, and resolves inline instead of hanging.
        let _ = release.send(());
        let outcome = driver.await.unwrap();
        match outcome {
            DelegationOutcome::Err {
                code,
                message,
                child_conversation_id,
            } => {
                assert_eq!(code, "canceled");
                assert!(
                    message.contains("Authentication required"),
                    "reason should carry the terminal-error detail, got: {message}"
                );
                assert_eq!(child_conversation_id, Some(55));
            }
            other => panic!("expected canceled Err, got {other:?}"),
        }

        // Reservation + buffer drained; child torn down exactly once.
        assert_eq!(broker.pending_count().await, 0);
        assert_eq!(broker.reserved_child_count().await, 0);
        assert_eq!(broker.early_cancel_count().await, 0);
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c-fast-fail"]);

        // Meta trail: running (written pre-park) then failed/canceled (pickup).
        let calls = writer.snapshot().await;
        assert_eq!(calls.len(), 2);
        let running = calls[0]
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(running.get("status").unwrap().as_str().unwrap(), "running");
        let failed = calls[1]
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(failed.get("status").unwrap().as_str().unwrap(), "failed");
        assert_eq!(
            failed.get("error_code").unwrap().as_str().unwrap(),
            "canceled"
        );
    }

    /// The SAME race on the SUCCESS path: a `TurnComplete` whose `complete_call`
    /// fires AFTER the delegation reserved but BEFORE `handle_request` parked (a
    /// fast/empty turn whose completion propagates while the broker is still
    /// awaiting the parent `write_meta`) must still resolve the request. The
    /// prompt is only *enqueued* by `send_prompt`, so the child loop can emit
    /// `TurnComplete` before the park. The `send_gate` pins `handle_request` in
    /// the reserve→park window; we resolve via the reserved `call_id` (the entry
    /// isn't parked yet) and assert the request returns Ok instead of hanging.
    #[tokio::test]
    async fn completion_before_park_resolves_instead_of_hanging() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-fast-ok".into())).await;
        mock.queue_send(Ok(70)).await;
        let release = mock.install_send_gate().await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-ok")).await })
        };

        // Wait until reserved (spawned + id minted, held in send_prompt by the
        // gate); the entry is NOT parked yet, so grab the call_id from the
        // reservation rather than the parked-calls map.
        let call_id = loop {
            if let Some(id) = broker.peek_reserved_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(broker.pending_count().await, 0, "entry not parked yet");

        // TurnComplete beats the park. With the reservation in place this
        // buffers (rather than no-oping on a not-yet-existent entry).
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "fast done".into(),
                    child_conversation_id: 70,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        assert_eq!(
            broker.early_complete_count().await,
            1,
            "completion buffered"
        );

        // Release send_prompt → handle_request parks, drains the buffered
        // completion, and resolves inline instead of hanging.
        let _ = release.send(());
        let outcome = driver.await.unwrap();
        match outcome {
            DelegationOutcome::Ok(s) => {
                assert_eq!(s.text, "fast done");
                assert_eq!(s.child_conversation_id, 70);
            }
            other => panic!("expected Ok, got {other:?}"),
        }

        assert_eq!(broker.pending_count().await, 0);
        assert_eq!(broker.reserved_call_count().await, 0);
        assert_eq!(broker.reserved_child_count().await, 0);
        assert_eq!(broker.early_complete_count().await, 0);
        // An Ok early-complete settles as Completed — keep the child for
        // continue_with_session (only cancel-coded outcomes disconnect).
        assert!(
            mock.disconnects.lock().await.is_empty(),
            "early Ok complete must not disconnect"
        );

        // Meta trail: running (written pre-park) then completed (pickup).
        let calls = writer.snapshot().await;
        assert_eq!(calls.len(), 2);
        let running = calls[0]
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(running.get("status").unwrap().as_str().unwrap(), "running");
        let completed = calls[1]
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(
            completed.get("status").unwrap().as_str().unwrap(),
            "completed"
        );
    }

    /// The reservation is released at park, and a SUCCESSFUL completion buffers
    /// nothing. The child's post-completion disconnect (normal v1 one-shot
    /// teardown) finds the child un-reserved and must NOT buffer a spurious
    /// cancel — otherwise every completed delegation would leak a buffer entry.
    #[tokio::test]
    async fn normal_completion_leaves_no_reservation_or_buffer() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-clean".into())).await;
        mock.queue_send(Ok(60)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-clean")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        // Parked → reservation already released.
        assert_eq!(
            broker.reserved_child_count().await,
            0,
            "park releases the reservation"
        );

        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "ok".into(),
                    child_conversation_id: 60,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        assert!(matches!(driver.await.unwrap(), DelegationOutcome::Ok(_)));

        // The child's post-completion disconnect arrives. Child is no longer
        // reserved → must NOT buffer a spurious cancel.
        broker.cancel_by_child_connection("c-clean", None).await;
        assert_eq!(
            broker.early_cancel_count().await,
            0,
            "a post-resolution teardown must not buffer a spurious cancel"
        );
        assert_eq!(broker.pending_count().await, 0);
    }

    // -- Item 1: parent-cancel coverage of the `handle_request` setup window --

    /// A parent cancel that lands while `handle_request` is INSIDE `spawn` (the
    /// child exists but no prompt has been sent) must disconnect the child and
    /// bail — never send it a prompt — instead of no-oping and letting it run
    /// orphaned. Pinned with the spawn gate.
    #[tokio::test]
    async fn parent_cancel_in_spawn_window_disconnects_child_without_sending() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c2".into())).await;
        mock.queue_send(Ok(99)).await; // staged but must NOT be consumed
        let release = mock.install_spawn_gate().await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-2")).await })
        };
        // Inside spawn (call recorded, held by the gate): registered in-flight,
        // not yet reserved.
        loop {
            if !mock.spawn_args.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(broker.inflight_count().await, 1);
        assert_eq!(broker.reserved_child_count().await, 0, "not reserved yet");

        broker.cancel_by_parent_turn("parent-conn").await;
        let _ = release.send(());

        match driver.await.unwrap() {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "canceled"),
            other => panic!("expected canceled, got {other:?}"),
        }
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c2"]);
        assert!(
            mock.cancels.lock().await.is_empty(),
            "no prompt was sent, so no cancel — disconnect only"
        );
        assert_eq!(
            mock.send_results.lock().await.len(),
            1,
            "send must not be consumed — no prompt sent to an abandoned child"
        );
        assert_eq!(broker.inflight_count().await, 0);
        assert_eq!(broker.reserved_child_count().await, 0);
    }

    /// A parent cancel that lands in the reserve→park window (prompt already
    /// sent, entry not yet parked) must cancel AND disconnect the child and
    /// resolve the request as canceled. Pinned with the send gate; also asserts
    /// the running→failed/canceled meta trail.
    #[tokio::test]
    async fn parent_cancel_in_reserve_park_window_tears_down_child() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c3".into())).await;
        mock.queue_send(Ok(33)).await;
        let release = mock.install_send_gate().await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-3")).await })
        };
        // Spawned + reserved, held inside send_prompt.
        loop {
            if broker.reserved_child_count().await == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(broker.inflight_count().await, 1);
        assert_eq!(broker.pending_count().await, 0, "not parked yet");

        broker.cancel_by_parent_turn("parent-conn").await;
        let _ = release.send(());

        match driver.await.unwrap() {
            DelegationOutcome::Err {
                code,
                child_conversation_id,
                ..
            } => {
                assert_eq!(code, "canceled");
                assert_eq!(child_conversation_id, Some(33));
            }
            other => panic!("expected canceled, got {other:?}"),
        }
        // Prompt was sent → child cancel()'d AND disconnected.
        assert_eq!(mock.cancels.lock().await.as_slice(), &["c3"]);
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c3"]);
        assert_eq!(broker.inflight_count().await, 0);
        assert_eq!(broker.reserved_child_count().await, 0);
        assert_eq!(broker.early_cancel_count().await, 0);
        assert_eq!(broker.pending_count().await, 0);

        // Meta trail: running (pre-park) then failed/canceled (ParentCanceled).
        let calls = writer.snapshot().await;
        assert_eq!(calls.len(), 2);
        let running = calls[0]
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(running.get("status").unwrap().as_str().unwrap(), "running");
        let failed = calls[1]
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(failed.get("status").unwrap().as_str().unwrap(), "failed");
        assert_eq!(
            failed.get("error_code").unwrap().as_str().unwrap(),
            "canceled"
        );
    }

    /// Strict first-terminal-wins: when a child completion buffers FIRST and a
    /// parent cancel lands afterward, the child's earlier arrival stamp wins and
    /// its real result is preserved (the cancel is moot — the child already
    /// finished before it).
    #[tokio::test]
    async fn child_terminal_wins_over_later_parent_cancel() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c4".into())).await;
        mock.queue_send(Ok(44)).await;
        let release = mock.install_send_gate().await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-4")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_reserved_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(broker.inflight_count().await, 1);

        // Child completes FIRST, then the parent cancels — child result wins.
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 44,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        broker.cancel_by_parent_turn("parent-conn").await;
        let _ = release.send(());

        assert!(matches!(driver.await.unwrap(), DelegationOutcome::Ok(_)));
        assert!(
            mock.cancels.lock().await.is_empty(),
            "child completed — the moot parent cancel must not cancel it"
        );
        assert_eq!(broker.inflight_count().await, 0);
        assert_eq!(broker.early_complete_count().await, 0);
    }

    /// Strict first-terminal-wins (Item 3): when the parent cancel is recorded
    /// BEFORE the child completion buffers, the cancel wins — the late
    /// completion is discarded and the child is torn down, because the parent
    /// had already abandoned the turn by the time the completion landed.
    #[tokio::test]
    async fn parent_cancel_wins_when_it_arrives_before_child_terminal() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c5".into())).await;
        mock.queue_send(Ok(55)).await;
        let release = mock.install_send_gate().await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-5")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_reserved_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        // Parent cancels FIRST (earlier arrival stamp); the child completes
        // afterward (later stamp) — first-terminal-wins judges the cancel the
        // winner and discards the late completion.
        broker.cancel_by_parent_turn("parent-conn").await;
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "late".into(),
                    child_conversation_id: 55,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        let _ = release.send(());

        match driver.await.unwrap() {
            DelegationOutcome::Err {
                code,
                child_conversation_id,
                ..
            } => {
                assert_eq!(code, "canceled");
                assert_eq!(child_conversation_id, Some(55));
            }
            other => panic!(
                "first-terminal-wins: an earlier parent cancel must beat a later completion, got {other:?}"
            ),
        }
        // The abandoned child is torn down (prompt was sent → cancel + disconnect).
        assert_eq!(mock.cancels.lock().await.as_slice(), &["c5"]);
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c5"]);
        assert_eq!(broker.inflight_count().await, 0);
        // The buffered completion was drained (and discarded), leaving no leak.
        assert_eq!(broker.early_complete_count().await, 0);
    }

    /// Strict first-terminal-wins through the child-FAILURE buffer: a child
    /// failure that buffers BEFORE a parent cancel keeps its (earlier) arrival
    /// stamp and wins, so the request resolves with the child's failure detail
    /// and the child is torn down once (disconnect only — the child already
    /// failed, so there's no in-flight prompt to cancel). Exercises the
    /// `early_cancels` stamp path that mirrors the completion case above.
    #[tokio::test]
    async fn child_failure_wins_over_later_parent_cancel() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("cF".into())).await;
        mock.queue_send(Ok(66)).await;
        let release = mock.install_send_gate().await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-f")).await })
        };
        // Spawned + reserved, held inside send_prompt by the gate.
        loop {
            if broker.reserved_child_count().await == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Child fails FIRST (earlier stamp), then the parent cancels (later
        // stamp) — the child terminal wins and carries its failure detail.
        broker
            .cancel_by_child_connection("cF", Some("boom detail"))
            .await;
        broker.cancel_by_parent_turn("parent-conn").await;
        let _ = release.send(());

        match driver.await.unwrap() {
            DelegationOutcome::Err {
                code,
                message,
                child_conversation_id,
            } => {
                assert_eq!(code, "canceled");
                assert!(
                    message.contains("boom detail"),
                    "child failure detail must survive, got: {message}"
                );
                assert_eq!(child_conversation_id, Some(66));
            }
            other => panic!("expected child failure Err, got {other:?}"),
        }
        // Child-terminal path tears down via disconnect only (no cancel).
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["cF"]);
        assert!(
            mock.cancels.lock().await.is_empty(),
            "child already failed — the moot parent cancel must not cancel it"
        );
        assert_eq!(broker.inflight_count().await, 0);
        assert_eq!(broker.early_cancel_count().await, 0);
    }

    /// The teardown variant `cancel_by_parent` covers the same reserve→park
    /// window as the turn variant — both funnel through `drain_for_parent_cancel`
    /// where the in-flight mark is applied.
    #[tokio::test]
    async fn parent_teardown_in_reserve_park_window_tears_down_child() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c7".into())).await;
        mock.queue_send(Ok(77)).await;
        let release = mock.install_send_gate().await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-7")).await })
        };
        loop {
            if broker.reserved_child_count().await == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        broker.cancel_by_parent("parent-conn").await;
        let _ = release.send(());

        match driver.await.unwrap() {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "canceled"),
            other => panic!("expected canceled, got {other:?}"),
        }
        assert_eq!(mock.cancels.lock().await.as_slice(), &["c7"]);
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c7"]);
        assert_eq!(broker.inflight_count().await, 0);
    }

    /// A cancel targeting a DIFFERENT parent must not flag this setup: it parks
    /// normally and resolves via its own child terminal.
    #[tokio::test]
    async fn parent_cancel_for_other_parent_leaves_setup_intact() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c8".into())).await;
        mock.queue_send(Ok(88)).await;
        let release = mock.install_send_gate().await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-8")).await })
        };
        loop {
            if broker.reserved_child_count().await == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Wrong-parent cancel — a no-op for this setup.
        broker.cancel_by_parent_turn("some-other-parent").await;
        let _ = release.send(());

        // It must park normally; resolve it via its child completion.
        let parked = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &parked,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "fine".into(),
                    child_conversation_id: 88,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        assert!(matches!(driver.await.unwrap(), DelegationOutcome::Ok(_)));
        assert!(
            mock.cancels.lock().await.is_empty(),
            "a wrong-parent cancel must not tear this child down"
        );
        assert_eq!(broker.inflight_count().await, 0);
    }

    /// The in-flight record is deregistered on every exit path: the normal park
    /// hand-off, and each early-return (disabled / spawn-fail / send-fail).
    #[tokio::test]
    async fn inflight_drained_on_normal_park() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-ok".into())).await;
        mock.queue_send(Ok(70)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-ok")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        // Parked → the in-flight record was handed off (deregistered) at park.
        assert_eq!(
            broker.inflight_count().await,
            0,
            "park deregisters in-flight"
        );
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "ok".into(),
                    child_conversation_id: 70,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        assert!(matches!(driver.await.unwrap(), DelegationOutcome::Ok(_)));
        assert_eq!(broker.inflight_count().await, 0);
    }

    #[tokio::test]
    async fn inflight_drained_on_disabled() {
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        // `enabled` defaults to false → short-circuits at the disabled check.
        let outcome = broker.handle_request(request(1, "pt-d")).await;
        assert!(matches!(outcome, DelegationOutcome::Err { .. }));
        assert_eq!(broker.inflight_count().await, 0);
    }

    #[tokio::test]
    async fn inflight_drained_on_spawn_failure() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Err(SpawnerError::Spawn("nope".into())))
            .await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        match broker.handle_request(request(1, "pt-sf")).await {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "spawn_failed"),
            other => panic!("expected spawn_failed, got {other:?}"),
        }
        assert_eq!(broker.inflight_count().await, 0);
        assert!(mock.disconnects.lock().await.is_empty());
    }

    #[tokio::test]
    async fn inflight_drained_on_send_failure() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c6".into())).await;
        mock.queue_send(Err(SpawnerError::Send("boom".into())))
            .await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;
        match broker.handle_request(request(1, "pt-sendf")).await {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "spawn_failed"),
            other => panic!("expected spawn_failed, got {other:?}"),
        }
        assert_eq!(broker.inflight_count().await, 0);
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c6"]);
        assert!(mock.cancels.lock().await.is_empty());
    }

    /// A terminal failure for a child the broker never reserved (unknown id, or
    /// one whose delegation already fully resolved) is a clean no-op — it must
    /// not buffer, so the buffer can only ever hold genuine pre-registration
    /// races.
    #[tokio::test]
    async fn cancel_for_unreserved_child_never_buffers() {
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        broker
            .cancel_by_child_connection("never-reserved", Some("boom"))
            .await;
        assert_eq!(broker.early_cancel_count().await, 0);
        assert_eq!(broker.pending_count().await, 0);
    }

    #[tokio::test]
    async fn meta_writer_records_failed_on_parent_cancel() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-cancel".into())).await;
        mock.queue_send(Ok(33)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-pcancel")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        broker.cancel_by_parent("parent-conn").await;
        let outcome = driver.await.unwrap();
        assert!(matches!(outcome, DelegationOutcome::Err { .. }));

        let calls = writer.snapshot().await;
        // running + canceled
        assert_eq!(calls.len(), 2);
        let inner = calls[1]
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(inner.get("status").unwrap().as_str().unwrap(), "failed");
        assert_eq!(
            inner.get("error_code").unwrap().as_str().unwrap(),
            "canceled"
        );
    }

    #[tokio::test]
    async fn meta_writer_skipped_for_synthetic_parent_tool_use_id() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-synth".into())).await;
        mock.queue_send(Ok(8)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let broker = broker_with_meta(mock.clone(), writer.clone()).await;

        // Empty `parent_tool_use_id` triggers the broker's UUID fallback —
        // `"delegation-<uuid>"` — which the writer must skip because no
        // matching ACP tool_call_id exists.
        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "ok".into(),
                    child_conversation_id: 8,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        let calls = writer.snapshot().await;
        assert!(
            calls.is_empty(),
            "writer should be skipped for synthetic parent_tool_use_id, got {:?}",
            calls
        );
    }

    // -- Event emitter lifecycle ------------------------------------------
    //
    // Issue: `.docs/issues/2026-05-24-delegation-termination-cascade.md`.
    // The broker must emit `AcpEvent::DelegationCompleted` once per drained
    // pending entry, regardless of which terminal path drained it (happy
    // `complete_call`, MCP `cancel_by_external_handle`, child-disconnect
    // cleanup, or parent-cancel cascade). Without these emits the frontend's live
    // delegation binding stays at "running" forever — see the issue doc
    // for the full path matrix.

    use crate::acp::delegation::event_emitter::mock::MockEventEmitter;
    use crate::acp::delegation::event_emitter::DelegationEventEmitter;
    use crate::acp::types::DelegationResultSummary;

    async fn broker_with_emitter(
        mock: Arc<MockSpawner>,
        writer: Arc<MockMetaWriter>,
        emitter: Arc<MockEventEmitter>,
    ) -> DelegationBroker {
        let broker = DelegationBroker::with_writers(
            mock as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
            writer as Arc<dyn DelegationMetaWriter>,
            emitter as Arc<dyn DelegationEventEmitter>,
        );
        enable_delegation(&broker).await;
        broker
    }

    #[tokio::test]
    async fn emitter_records_ok_on_complete_call_happy_path() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-1".into())).await;
        mock.queue_send(Ok(42)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-ok")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 42,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 73,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        let calls = emitter.snapshot().await;
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.parent_tool_use_id, "pt-ok");
        assert_eq!(call.child_connection_id, "child-conn-1");
        assert_eq!(call.child_conversation_id, 42);
        // The completed event carries the child agent_type (from the running
        // task), so a frontend that missed `DelegationStarted` still binds the
        // correct agent. `request()` delegates to ClaudeCode.
        assert_eq!(call.agent_type, AgentType::ClaudeCode);
        // duration_ms is now broker-measured (not the outcome's value); assert
        // the Ok variant + the enriched text_preview instead.
        assert!(
            matches!(
                &call.result,
                DelegationResultSummary::Ok { text_preview, .. }
                    if text_preview.as_deref() == Some("done")
            ),
            "expected Ok with preview, got {:?}",
            call.result
        );
    }

    #[tokio::test]
    async fn emitter_records_started_on_start_delegation_happy_path() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-start".into())).await;
        mock.queue_send(Ok(55)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-start")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };

        // `started` fires during setup, BEFORE the task parks — so it is already
        // recorded by the time a pending entry is visible, and it must not be
        // conflated with the (not-yet-emitted) terminal.
        let started = emitter.started_snapshot().await;
        assert_eq!(started.len(), 1, "exactly one DelegationStarted per task");
        let s = &started[0];
        assert_eq!(s.parent_connection_id, "parent-conn");
        assert_eq!(s.parent_tool_use_id, "pt-start");
        assert_eq!(s.child_connection_id, "child-conn-start");
        assert_eq!(s.child_conversation_id, 55);
        assert_eq!(s.agent_type, AgentType::ClaudeCode);
        assert_eq!(
            emitter.count().await,
            0,
            "no terminal emit before completion"
        );

        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "done".into(),
                    child_conversation_id: 55,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 10,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        // Completion adds exactly one terminal emit; started stays at 1.
        assert_eq!(emitter.started_count().await, 1);
        assert_eq!(emitter.count().await, 1);
    }

    #[tokio::test]
    async fn emitter_skips_started_for_synthetic_parent_tool_use_id() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-synth-start".into())).await;
        mock.queue_send(Ok(9)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        // Empty parent_tool_use_id → broker falls back to a synthetic
        // `delegation-<uuid>` id (no ACP tool_call to claim in a mock harness).
        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "ok".into(),
                    child_conversation_id: 9,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        assert_eq!(
            emitter.started_count().await,
            0,
            "started emit must skip synthetic parent_tool_use_id (same rule as the meta writer / completed emit)"
        );
    }

    #[tokio::test]
    async fn emitter_records_err_on_complete_call_err_outcome() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-err".into())).await;
        mock.queue_send(Ok(11)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-err")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::from_err(
                    DelegationError::SubagentRuntimeError("agent died".into()),
                    Some(11),
                ),
            )
            .await;
        driver.await.unwrap();

        let calls = emitter.snapshot().await;
        assert_eq!(calls.len(), 1);
        match &calls[0].result {
            DelegationResultSummary::Err { error_code } => {
                assert_eq!(error_code, "subagent_error")
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn emitter_records_canceled_on_cancel_by_external_handle() {
        // MCP-driven cancel path: companion received notifications/cancelled
        // and the listener forwarded it to broker.cancel_by_external_handle.
        // The broker must drain the pending entry, cancel + disconnect the
        // child, and emit DelegationCompleted with error_code = "canceled".
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-h".into())).await;
        mock.queue_send(Ok(91)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move {
                broker
                    .handle_request(request_with_handle(1, "pt-mcp-cancel", "h-1"))
                    .await
            })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        broker
            .cancel_by_external_handle("h-1", "user requested".into())
            .await;
        let outcome = driver.await.unwrap();
        assert!(matches!(
            outcome,
            DelegationOutcome::Err { ref code, .. } if code == "canceled"
        ));

        assert_eq!(mock.cancels.lock().await.as_slice(), &["child-conn-h"]);
        let calls = emitter.snapshot().await;
        assert_eq!(calls.len(), 1, "expected exactly one emit, got {calls:?}");
        let call = &calls[0];
        assert_eq!(call.parent_tool_use_id, "pt-mcp-cancel");
        assert_eq!(call.child_connection_id, "child-conn-h");
        assert_eq!(call.child_conversation_id, 91);
        match &call.result {
            DelegationResultSummary::Err { error_code } => {
                assert_eq!(error_code, "canceled")
            }
            other => panic!("expected Err{{canceled}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_by_external_handle_no_match_buffers_pre_cancel() {
        // Cancel arrives before handle_request reaches pending registration.
        // The broker must buffer the handle in pre_canceled_handles so the
        // in-flight call drains itself on its post-registration checkpoint.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn-pre".into())).await;
        mock.queue_send(Ok(13)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        // Pre-cancel before spawning the driver — handle is unknown to the
        // broker right now, but a buffered entry should make the next
        // handle_request with the same handle bail out canceled.
        broker
            .cancel_by_external_handle("h-pre", "early cancel".into())
            .await;
        // Pre-cancel set is single-shot: a second call with the same handle
        // and no pending entry just buffers it again (idempotent in practice).
        let outcome = broker
            .handle_request(request_with_handle(1, "pt-pre", "h-pre"))
            .await;
        match outcome {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "canceled"),
            other => panic!("expected canceled, got {other:?}"),
        }
        // Since the cancel won pre-spawn, no child connection should have
        // been opened.
        assert!(mock.cancels.lock().await.is_empty());
        assert!(mock.disconnects.lock().await.is_empty());
        // The pre-cancel early-return must also drop the in-flight record
        // (registered as handle_request's first statement, before this check).
        assert_eq!(broker.inflight_count().await, 0);
    }

    /// The real MCP-shaped path carries an `external_handle`. Registration now
    /// happens as `handle_request`'s FIRST statement — before the pre-cancel
    /// `.await` — so a parent cancel in the setup window reaches these requests
    /// too, not just the synthetic-id path. Guards the regression Codex flagged
    /// (registration ordered after the pre-cancel await left a miss window).
    #[tokio::test]
    async fn parent_cancel_covers_external_handle_setup_window() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-eh".into())).await;
        mock.queue_send(Ok(21)).await;
        let release = mock.install_send_gate().await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move {
                broker
                    .handle_request(request_with_handle(1, "pt-eh", "h-eh"))
                    .await
            })
        };
        loop {
            if broker.reserved_child_count().await == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(broker.inflight_count().await, 1);

        broker.cancel_by_parent_turn("parent-conn").await;
        let _ = release.send(());

        match driver.await.unwrap() {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "canceled"),
            other => panic!("expected canceled, got {other:?}"),
        }
        assert_eq!(mock.cancels.lock().await.as_slice(), &["c-eh"]);
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c-eh"]);
        assert_eq!(broker.inflight_count().await, 0);
    }

    #[tokio::test]
    async fn emitter_records_canceled_on_cancel_by_child_connection() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-dropped".into())).await;
        mock.queue_send(Ok(55)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-cbc")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        broker.cancel_by_child_connection("c-dropped", None).await;
        let outcome = driver.await.unwrap();
        match &outcome {
            DelegationOutcome::Err { code, message, .. } => {
                assert_eq!(code, "canceled");
                // No terminal_error supplied → falls back to default reason.
                assert_eq!(
                    message,
                    "canceled: child session ended without TurnComplete"
                );
            }
            other => panic!("expected Err{{canceled}}, got {other:?}"),
        }

        let calls = emitter.snapshot().await;
        assert_eq!(calls.len(), 1);
        match &calls[0].result {
            DelegationResultSummary::Err { error_code } => {
                assert_eq!(error_code, "canceled")
            }
            other => panic!("expected Err{{canceled}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_by_child_connection_threads_terminal_error_into_reason() {
        // The lifecycle worker forwards the child's last AcpEvent::Error
        // detail through `cancel_by_child_connection`. The broker stitches it
        // into the `Canceled { reason }` message so the parent's
        // `delegate_to_agent` tool-call result surfaces the real failure
        // cause (e.g. Gemini OAuth expired) instead of the opaque default.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-auth".into())).await;
        mock.queue_send(Ok(77)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-auth")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        broker
            .cancel_by_child_connection("c-auth", Some("[auth_required] Authentication required"))
            .await;
        let outcome = driver.await.unwrap();
        match &outcome {
            DelegationOutcome::Err { code, message, .. } => {
                assert_eq!(code, "canceled");
                assert_eq!(
                    message,
                    "canceled: child session ended without TurnComplete: \
                     [auth_required] Authentication required"
                );
            }
            other => panic!("expected Err{{canceled}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_by_child_connection_ignores_empty_terminal_error() {
        // Whitespace-only or empty detail strings shouldn't produce a
        // dangling "...:" suffix on the reason — fall back to the default.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-empty".into())).await;
        mock.queue_send(Ok(78)).await;
        let broker =
            DelegationBroker::new(mock.clone() as Arc<dyn ConnectionSpawner>, shallow_lookup());
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-empty")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        broker
            .cancel_by_child_connection("c-empty", Some("   "))
            .await;
        let outcome = driver.await.unwrap();
        match &outcome {
            DelegationOutcome::Err { message, .. } => {
                assert_eq!(
                    message,
                    "canceled: child session ended without TurnComplete"
                );
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn emitter_records_one_event_per_drained_entry_on_cancel_by_parent() {
        let mock = Arc::new(MockSpawner::new());
        for i in 0..3 {
            mock.queue_spawn(Ok(format!("c{i}"))).await;
            mock.queue_send(Ok(100 + i)).await;
        }
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let mut handles = Vec::new();
        for i in 0..3 {
            let broker = broker.clone();
            handles.push(tokio::spawn(async move {
                broker.handle_request(request(1, &format!("pt-{i}"))).await
            }));
        }
        while broker.pending_count().await < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        broker.cancel_by_parent("parent-conn").await;
        for h in handles {
            let _ = h.await.unwrap();
        }

        let calls = emitter.snapshot().await;
        assert_eq!(calls.len(), 3, "expected 3 emits, got {calls:?}");
        let mut parent_tool_use_ids: Vec<String> =
            calls.iter().map(|c| c.parent_tool_use_id.clone()).collect();
        parent_tool_use_ids.sort();
        assert_eq!(
            parent_tool_use_ids,
            vec!["pt-0".to_string(), "pt-1".to_string(), "pt-2".to_string()]
        );
        for call in &calls {
            match &call.result {
                DelegationResultSummary::Err { error_code } => {
                    assert_eq!(error_code, "canceled")
                }
                other => panic!("expected Err{{canceled}}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn emitter_does_not_double_emit_on_repeat_cancel_by_parent() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-once".into())).await;
        mock.queue_send(Ok(42)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-idem")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // First call drains the entry + emits one.
        broker.cancel_by_parent("parent-conn").await;
        // Second call finds the pending map empty — no extra emit.
        broker.cancel_by_parent("parent-conn").await;
        // Cleanup-guard-style triple call also stays bounded.
        broker.cancel_by_parent("parent-conn").await;
        let _ = driver.await.unwrap();

        assert_eq!(emitter.count().await, 1);
    }

    #[tokio::test]
    async fn emitter_skipped_for_synthetic_parent_tool_use_id() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-synth".into())).await;
        mock.queue_send(Ok(8)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "ok".into(),
                    child_conversation_id: 8,
                    child_agent_type: AgentType::Codex,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        let calls = emitter.snapshot().await;
        assert!(
            calls.is_empty(),
            "emitter must skip synthetic parent_tool_use_id (same rule as meta writer); got {calls:?}"
        );
    }

    #[tokio::test]
    async fn emitter_records_after_meta_write_on_complete_call() {
        // Frontend's snapshot-recovery path reads `meta["codeg.delegation"]`
        // first and the live event second; if the emit lands before the
        // meta write, a snapshot taken between them would see "running"
        // meta paired with a "completed" event. Enforce meta-before-emit
        // by checking the MockMetaWriter has at least one call before the
        // emitter records.
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-order".into())).await;
        mock.queue_send(Ok(7)).await;
        let writer = Arc::new(MockMetaWriter::new());
        let emitter = Arc::new(MockEventEmitter::new());
        let broker = broker_with_emitter(mock.clone(), writer.clone(), emitter.clone()).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-order")).await })
        };
        let call_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(
                &call_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "ok".into(),
                    child_conversation_id: 7,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 5,
                    token_usage: None,
                }),
            )
            .await;
        driver.await.unwrap();

        let meta_calls = writer.snapshot().await;
        let event_calls = emitter.snapshot().await;
        // running (from handle_request) + completed (from complete_call) =
        // 2 meta writes. The single event must be the "completed" one,
        // and it must land AFTER the running meta — guaranteed structurally
        // by complete_call's order (write_meta_if_real then emit).
        assert_eq!(meta_calls.len(), 2);
        assert_eq!(event_calls.len(), 1);
        let inner_second = meta_calls[1]
            .meta
            .get("codeg.delegation")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(
            inner_second.get("status").unwrap().as_str().unwrap(),
            "completed"
        );
    }

    // -- Production-path fanout coverage ----------------------------------
    //
    // Every other emitter test in this module uses `MockEventEmitter`. The
    // production wiring goes through `ConnectionManagerEventEmitter`, which
    // resolves `(state, emitter)` against the live `ConnectionManager` and
    // hands the event to `emit_with_state` so it fans out to (1) the parent
    // connection's `ConnectionEventStream` (the WS attach path) and (2) the
    // `InternalEventBus` (the lifecycle/pet/chat-channel subscriber path).
    // These tests exercise that real fanout end-to-end so a regression in
    // `get_state_and_emitter` lookup, `emit_with_state` routing, or the
    // `EventEmitter::WebOnly { bus, .. }` wiring is caught here even when
    // every mock-backed test stays green.

    #[tokio::test]
    async fn real_emitter_fans_out_delegation_completed_to_parent_stream_and_bus() {
        use crate::acp::delegation::event_emitter::ConnectionManagerEventEmitter;
        use crate::acp::manager::ConnectionManager;
        use crate::acp::types::AcpEvent;
        use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};

        // Real ConnectionManager + fake parent wired to a WebOnly emitter so
        // the InternalEventBus gets typed envelopes and we can subscribe to
        // verify the lifecycle-path delivery alongside the per-connection
        // stream delivery.
        let manager = ConnectionManager::new();
        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let parent_emitter = EventEmitter::test_web_only(broadcaster);
        let bus = parent_emitter
            .acp_event_bus()
            .expect("WebOnly emitter must expose an InternalEventBus");
        manager
            .insert_test_connection("parent-conn", AgentType::ClaudeCode, None, parent_emitter)
            .await;

        // Subscribe BEFORE triggering events — broadcast channels drop
        // sends that happen with no receivers registered.
        let mut bus_rx = bus.subscribe();
        let (parent_state, _) = manager
            .get_state_and_emitter("parent-conn")
            .await
            .expect("parent just inserted");
        let mut stream_rx = parent_state.read().await.event_stream().subscribe();

        // Build the broker with the PRODUCTION emitter; meta writer can stay
        // noop because this test is asserting the event-fanout invariant.
        let mock_spawner = Arc::new(MockSpawner::new());
        mock_spawner.queue_spawn(Ok("child-conn-real".into())).await;
        mock_spawner.queue_send(Ok(77)).await;
        let real_emitter = Arc::new(ConnectionManagerEventEmitter {
            manager: Arc::new(manager.clone_ref()),
        });
        let broker = DelegationBroker::with_writers(
            mock_spawner.clone() as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
            Arc::new(crate::acp::delegation::meta_writer::NoopMetaWriter)
                as Arc<dyn crate::acp::delegation::meta_writer::DelegationMetaWriter>,
            real_emitter as Arc<dyn crate::acp::delegation::event_emitter::DelegationEventEmitter>,
        );
        enable_delegation(&broker).await;

        // Park a pending entry then trigger cancel_by_parent to drive the
        // production emit path. `request()` hard-codes parent_connection_id
        // = "parent-conn" which matches the insert above.
        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-fanout")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        broker.cancel_by_parent("parent-conn").await;
        let _ = driver.await.unwrap();

        // Per-connection stream (WS attach delivery path) must receive the
        // envelope tagged with the right connection + payload shape.
        // The parent stream now also carries the setup-time DelegationStarted;
        // skip past it to the terminal DelegationCompleted.
        let envelope = loop {
            let env = tokio::time::timeout(Duration::from_millis(500), stream_rx.recv())
                .await
                .expect("per-connection stream should receive DelegationCompleted within 500ms")
                .expect("envelope recv must not error");
            if matches!(env.payload, AcpEvent::DelegationStarted { .. }) {
                continue;
            }
            break env;
        };
        assert_eq!(envelope.connection_id, "parent-conn");
        match &envelope.payload {
            AcpEvent::DelegationCompleted {
                parent_tool_use_id,
                child_connection_id,
                child_conversation_id,
                result,
                ..
            } => {
                assert_eq!(parent_tool_use_id, "pt-fanout");
                assert_eq!(child_connection_id, "child-conn-real");
                assert_eq!(*child_conversation_id, 77);
                match result {
                    DelegationResultSummary::Err { error_code } => {
                        assert_eq!(error_code, "canceled");
                    }
                    other => panic!("expected Err{{canceled}}, got {other:?}"),
                }
            }
            other => panic!("expected DelegationCompleted, got {other:?}"),
        }

        // InternalEventBus (lifecycle/pet/chat-channel subscriber path) must
        // also receive the same envelope — proves the WebOnly emitter's bus
        // arm in `emit_with_state` is reached.
        let bus_envelope = loop {
            let env = tokio::time::timeout(Duration::from_millis(500), bus_rx.recv())
                .await
                .expect("InternalEventBus should receive DelegationCompleted within 500ms")
                .expect("bus recv must not error");
            if matches!(env.payload, AcpEvent::DelegationStarted { .. }) {
                continue;
            }
            break env;
        };
        assert_eq!(bus_envelope.connection_id, "parent-conn");
        assert!(matches!(
            bus_envelope.payload,
            AcpEvent::DelegationCompleted { .. }
        ));
    }

    #[tokio::test]
    async fn real_emitter_fans_out_delegation_started_to_parent_stream_and_bus() {
        use crate::acp::delegation::event_emitter::ConnectionManagerEventEmitter;
        use crate::acp::manager::ConnectionManager;
        use crate::acp::types::AcpEvent;
        use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};

        // Web/server delivery shape: a real ConnectionManager + a fake parent on
        // a WebOnly emitter. `DelegationStarted` must land on the PARENT's
        // per-connection stream (the WS attach path the frontend subscribes to
        // in web/server mode) AND the InternalEventBus — mirroring the
        // completed-path invariant. This is the regression lock for the
        // web-mode live-delegation gap: before moving the emit to the parent
        // stream, started rode the (un-attached) child stream and was lost here.
        let manager = ConnectionManager::new();
        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let parent_emitter = EventEmitter::test_web_only(broadcaster);
        let bus = parent_emitter
            .acp_event_bus()
            .expect("WebOnly emitter must expose an InternalEventBus");
        manager
            .insert_test_connection("parent-conn", AgentType::ClaudeCode, None, parent_emitter)
            .await;

        // Subscribe BEFORE triggering events — broadcast channels drop sends
        // that happen with no receivers registered.
        let mut bus_rx = bus.subscribe();
        let (parent_state, _) = manager
            .get_state_and_emitter("parent-conn")
            .await
            .expect("parent just inserted");
        let mut stream_rx = parent_state.read().await.event_stream().subscribe();

        let mock_spawner = Arc::new(MockSpawner::new());
        mock_spawner
            .queue_spawn(Ok("child-conn-started".into()))
            .await;
        mock_spawner.queue_send(Ok(88)).await;
        let real_emitter = Arc::new(ConnectionManagerEventEmitter {
            manager: Arc::new(manager.clone_ref()),
        });
        let broker = DelegationBroker::with_writers(
            mock_spawner.clone() as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
            Arc::new(crate::acp::delegation::meta_writer::NoopMetaWriter)
                as Arc<dyn crate::acp::delegation::meta_writer::DelegationMetaWriter>,
            real_emitter as Arc<dyn crate::acp::delegation::event_emitter::DelegationEventEmitter>,
        );
        enable_delegation(&broker).await;

        // `started` fires during setup, before park — drive the request, wait
        // for it to park, then assert the envelope already arrived.
        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-started")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let envelope = tokio::time::timeout(Duration::from_millis(500), stream_rx.recv())
            .await
            .expect("per-connection stream should receive DelegationStarted within 500ms")
            .expect("envelope recv must not error");
        assert_eq!(envelope.connection_id, "parent-conn");
        match &envelope.payload {
            AcpEvent::DelegationStarted {
                parent_connection_id,
                parent_tool_use_id,
                child_connection_id,
                child_conversation_id,
                agent_type,
                task_preview,
                task_id,
            } => {
                assert_eq!(parent_connection_id, "parent-conn");
                assert_eq!(parent_tool_use_id, "pt-started");
                assert_eq!(child_connection_id, "child-conn-started");
                assert_eq!(*child_conversation_id, 88);
                assert_eq!(*agent_type, AgentType::ClaudeCode);
                // The event labels the card even when the parent tool call's
                // raw_input never carried the arguments (identity-less hosts).
                assert_eq!(task_preview, "do x");
                assert!(
                    !task_id.is_empty(),
                    "broker-minted task id must ride the event"
                );
            }
            other => panic!("expected DelegationStarted, got {other:?}"),
        }

        let bus_envelope = tokio::time::timeout(Duration::from_millis(500), bus_rx.recv())
            .await
            .expect("InternalEventBus should receive DelegationStarted within 500ms")
            .expect("bus recv must not error");
        assert_eq!(bus_envelope.connection_id, "parent-conn");
        assert!(matches!(
            bus_envelope.payload,
            AcpEvent::DelegationStarted { .. }
        ));

        // Drain the parked driver so the test doesn't leak the spawned task.
        broker.cancel_by_parent("parent-conn").await;
        let _ = driver.await.unwrap();
    }

    /// T4.3 bridge proof: the session-scoped update rides the SAME
    /// `emit_with_state` fanout as every other ACP event — per-connection
    /// stream (the WS attach path that becomes the frontend `EventEnvelope`)
    /// AND the `InternalEventBus`. Direct emitter call: the broker-side
    /// emission is covered by the mock-backed tests above; this pins the
    /// production fanout so the new variant can't silently become desktop-only.
    #[tokio::test]
    async fn real_emitter_fans_out_session_update_to_parent_stream_and_bus() {
        use crate::acp::delegation::event_emitter::{
            ConnectionManagerEventEmitter, DelegationEventEmitter,
        };
        use crate::acp::manager::ConnectionManager;
        use crate::acp::types::AcpEvent;
        use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};

        let manager = ConnectionManager::new();
        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let parent_emitter = EventEmitter::test_web_only(broadcaster);
        let bus = parent_emitter
            .acp_event_bus()
            .expect("WebOnly emitter must expose an InternalEventBus");
        manager
            .insert_test_connection("parent-conn", AgentType::ClaudeCode, None, parent_emitter)
            .await;
        let mut bus_rx = bus.subscribe();
        let (parent_state, _) = manager
            .get_state_and_emitter("parent-conn")
            .await
            .expect("parent just inserted");
        let mut stream_rx = parent_state.read().await.event_stream().subscribe();

        let real_emitter = ConnectionManagerEventEmitter {
            manager: Arc::new(manager.clone_ref()),
        };
        real_emitter
            .emit_session_update(
                "parent-conn",
                "child-conn-su",
                42,
                "task-su",
                "turn-su",
                2,
                TurnOrigin::User,
            )
            .await;

        let envelope = tokio::time::timeout(Duration::from_millis(500), stream_rx.recv())
            .await
            .expect("per-connection stream should receive DelegationSessionUpdate within 500ms")
            .expect("envelope recv must not error");
        assert_eq!(envelope.connection_id, "parent-conn");
        match &envelope.payload {
            AcpEvent::DelegationSessionUpdate {
                parent_connection_id,
                child_conversation_id,
                task_id,
                turn_id,
                turn_version,
                origin,
            } => {
                assert_eq!(parent_connection_id, "parent-conn");
                assert_eq!(*child_conversation_id, 42);
                assert_eq!(task_id, "task-su");
                assert_eq!(turn_id, "turn-su");
                assert_eq!(*turn_version, 2);
                assert_eq!(*origin, TurnOrigin::User);
            }
            other => panic!("expected DelegationSessionUpdate, got {other:?}"),
        }

        let bus_envelope = tokio::time::timeout(Duration::from_millis(500), bus_rx.recv())
            .await
            .expect("InternalEventBus should receive DelegationSessionUpdate within 500ms")
            .expect("bus recv must not error");
        assert_eq!(bus_envelope.connection_id, "parent-conn");
        assert!(matches!(
            bus_envelope.payload,
            AcpEvent::DelegationSessionUpdate { .. }
        ));

        // Wire-shape pin: snake_case tag + origin as `user` (the TS mirror in
        // src/lib/types.ts binds to exactly this JSON).
        let json = serde_json::to_value(&bus_envelope.payload).unwrap();
        assert_eq!(json["type"], "delegation_session_update");
        assert_eq!(json["origin"], "user");
        assert_eq!(json["turn_version"], 2);
    }

    #[tokio::test]
    async fn real_emitter_session_update_falls_back_to_child_stream_for_user_origin() {
        // A USER-originated continuation settles with the synthetic
        // `USER_ENTRY_CONNECTION_ID` as parent — that id never resolves, so
        // before the fallback the settle push was silently dropped and the
        // sub-agent dialog only caught up on remount. The emitter must now
        // fan the event out on the CHILD's stream instead.
        use crate::acp::delegation::event_emitter::{
            ConnectionManagerEventEmitter, DelegationEventEmitter,
        };
        use crate::acp::manager::ConnectionManager;
        use crate::acp::types::AcpEvent;
        use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};

        let manager = ConnectionManager::new();
        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let child_emitter = EventEmitter::test_web_only(broadcaster);
        manager
            .insert_test_connection("child-conn-fb", AgentType::ClaudeCode, None, child_emitter)
            .await;
        let (child_state, _) = manager
            .get_state_and_emitter("child-conn-fb")
            .await
            .expect("child just inserted");
        let mut stream_rx = child_state.read().await.event_stream().subscribe();

        let real_emitter = ConnectionManagerEventEmitter {
            manager: Arc::new(manager.clone_ref()),
        };
        // Parent id is the synthetic user-entry marker: resolves to nothing.
        real_emitter
            .emit_session_update(
                crate::commands::delegation::USER_ENTRY_CONNECTION_ID,
                "child-conn-fb",
                77,
                "task-fb",
                "turn-fb",
                3,
                TurnOrigin::User,
            )
            .await;

        let envelope = tokio::time::timeout(Duration::from_millis(500), stream_rx.recv())
            .await
            .expect("child stream should receive the fallback DelegationSessionUpdate")
            .expect("envelope recv must not error");
        assert_eq!(envelope.connection_id, "child-conn-fb");
        match &envelope.payload {
            AcpEvent::DelegationSessionUpdate {
                child_conversation_id,
                task_id,
                turn_version,
                ..
            } => {
                assert_eq!(*child_conversation_id, 77);
                assert_eq!(task_id, "task-fb");
                assert_eq!(*turn_version, 3);
            }
            other => panic!("expected DelegationSessionUpdate, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn real_emitter_is_silent_no_op_when_parent_already_detached() {
        // Parent torn down mid-delegation: `get_state_and_emitter` returns
        // None, the emit silently drops, BUT the broker still drains its
        // pending table and surfaces the outcome to the awaiting caller.
        // This is the "parent disappeared before terminal" path that the
        // mock-backed tests can't observe.
        use crate::acp::delegation::event_emitter::ConnectionManagerEventEmitter;
        use crate::acp::manager::ConnectionManager;

        let manager = ConnectionManager::new();
        // Intentionally no insert_test_connection — parent is absent.
        let real_emitter = Arc::new(ConnectionManagerEventEmitter {
            manager: Arc::new(manager.clone_ref()),
        });
        let mock_spawner = Arc::new(MockSpawner::new());
        mock_spawner.queue_spawn(Ok("c-orphan".into())).await;
        mock_spawner.queue_send(Ok(1)).await;
        let broker = DelegationBroker::with_writers(
            mock_spawner.clone() as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
            Arc::new(crate::acp::delegation::meta_writer::NoopMetaWriter)
                as Arc<dyn crate::acp::delegation::meta_writer::DelegationMetaWriter>,
            real_emitter as Arc<dyn crate::acp::delegation::event_emitter::DelegationEventEmitter>,
        );
        enable_delegation(&broker).await;

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.handle_request(request(1, "pt-orphan")).await })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        broker.cancel_by_parent("parent-conn").await;
        let outcome = driver.await.unwrap();

        assert!(matches!(
            outcome,
            DelegationOutcome::Err { ref code, .. } if code == "canceled"
        ));
        assert_eq!(
            broker.pending_count().await,
            0,
            "broker must drain pending even when no parent exists to receive the emit"
        );
    }

    // -- Async-cutover review regressions -----------------------------------

    /// A pre-cancel that bails `start_delegation` before the claim path must
    /// still drain the keyed ACP tool_call, so a later same-key delegation
    /// can't claim the canceled call's id and mis-bind to the wrong card.
    #[tokio::test]
    async fn pre_cancel_drains_keyed_tool_call_to_avoid_misbinding() {
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        enable_delegation(&broker).await;
        let key = DelegationMatchKey {
            agent_type: AgentType::ClaudeCode,
            task: "do x".into(),
            working_dir: None,
        };
        // The lifecycle registered the keyed tool_call for this delegation.
        broker
            .register_pending_tool_call_with_key("parent-conn", "tc-1".into(), Some(key.clone()))
            .await;
        // notifications/cancelled lands before the round-trip → buffered (no
        // running task yet).
        broker.cancel_by_external_handle("h-1", "user".into()).await;
        // The MCP round-trip arrives (empty parent_tool_use_id, same key) and
        // bails at the first pre-cancel check.
        let report = broker
            .start_delegation(request_with_handle(1, "", "h-1"))
            .await;
        assert_eq!(report.status, TaskStatus::Canceled);
        // The keyed entry must have been drained — not claimable afterward.
        assert_eq!(
            broker.take_matching_tool_call("parent-conn", &key).await,
            None
        );
    }

    /// The running ack must carry the literal task_id in its message, so a
    /// client that only surfaces MCP `content` text (not `structuredContent`)
    /// can still call get_delegation_status / cancel_delegation.
    #[test]
    fn running_ack_message_embeds_task_id() {
        let report = running_ack("task-xyz".into(), 42, AgentType::Codex);
        assert_eq!(report.task_id.as_deref(), Some("task-xyz"));
        assert!(
            report.message.as_deref().unwrap().contains("task-xyz"),
            "ack message must embed the literal task_id, got {:?}",
            report.message
        );
    }

    /// Previews and cached text must stay within their advertised BYTE caps
    /// (including the appended ellipsis), and truncate on UTF-8 boundaries.
    #[test]
    fn previews_and_cached_text_respect_byte_caps() {
        let preview = build_text_preview(&"x".repeat(STATUS_PREVIEW_CAP * 2)).unwrap();
        assert!(
            preview.len() <= STATUS_PREVIEW_CAP,
            "preview {} > cap {STATUS_PREVIEW_CAP}",
            preview.len()
        );
        let cached = cap_completed_text(&"y".repeat(COMPLETED_TEXT_CAP * 2));
        assert!(cached.len() <= COMPLETED_TEXT_CAP);
        // Multibyte safety: 3-byte chars must not be split, and the cap holds.
        let multibyte = build_text_preview(&"€".repeat(STATUS_PREVIEW_CAP)).unwrap();
        assert!(multibyte.len() <= STATUS_PREVIEW_CAP);
        assert!(std::str::from_utf8(multibyte.as_bytes()).is_ok());
    }

    // -- completed-cache byte valve ----------------------------------------

    fn completed_with_text(parent: &str, text_len: usize) -> CompletedTask {
        CompletedTask {
            parent_connection_id: parent.to_string(),
            child_conversation_id: 1,
            agent_type: AgentType::ClaudeCode,
            status: TaskStatus::Completed,
            text: Some("x".repeat(text_len)),
            error_code: None,
            message: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn completed_cache_valve_evicts_oldest_over_byte_budget() {
        let mut inner = PendingInner {
            completed_cap_bytes: 1000,
            ..Default::default()
        };
        // Three 400-byte results = 1200 bytes > 1000 cap. Oldest must evict.
        inner.insert_completed("a", completed_with_text("p1", 400));
        inner.insert_completed("b", completed_with_text("p1", 400));
        inner.insert_completed("c", completed_with_text("p1", 400));
        assert!(!inner.completed.contains_key("a"), "oldest must be evicted");
        assert!(inner.completed.contains_key("b"));
        assert!(inner.completed.contains_key("c"), "newest must be retained");
        // Counter + order reflect only the two retained entries.
        assert_eq!(inner.completed_bytes.get("p1").copied(), Some(800));
        assert_eq!(inner.completed_order.get("p1").map(|o| o.len()), Some(2));
        // Survivors keep their FULL text — the valve drops whole entries, it
        // never truncates a survivor.
        assert_eq!(
            inner
                .completed
                .get("c")
                .unwrap()
                .text
                .as_deref()
                .map(str::len),
            Some(400)
        );
    }

    #[test]
    fn completed_cache_valve_keeps_newest_even_if_alone_over_budget() {
        // A single result larger than the whole budget is still retained — the
        // valve never evicts the entry just inserted (the LLM's immediate
        // get_delegation_status must hit). Per-result text is independently
        // bounded by COMPLETED_TEXT_CAP.
        let mut inner = PendingInner {
            completed_cap_bytes: 100,
            ..Default::default()
        };
        inner.insert_completed("solo", completed_with_text("p1", 500));
        assert!(inner.completed.contains_key("solo"));
        assert_eq!(inner.completed_bytes.get("p1").copied(), Some(500));
    }

    #[test]
    fn completed_cache_unlimited_when_cap_zero() {
        let mut inner = PendingInner::default(); // completed_cap_bytes == 0
        for i in 0..50 {
            inner.insert_completed(&format!("t{i}"), completed_with_text("p1", 10_000));
        }
        assert_eq!(inner.completed.len(), 50, "cap 0 disables eviction");
        assert_eq!(inner.completed_bytes.get("p1").copied(), Some(500_000));
    }

    #[test]
    fn completed_cache_valve_is_per_parent() {
        let mut inner = PendingInner {
            completed_cap_bytes: 1000,
            ..Default::default()
        };
        // p1 overflows; p2 stays under its own independent budget.
        inner.insert_completed("a1", completed_with_text("p1", 600));
        inner.insert_completed("a2", completed_with_text("p1", 600)); // evicts a1
        inner.insert_completed("b1", completed_with_text("p2", 600));
        assert!(!inner.completed.contains_key("a1"));
        assert!(inner.completed.contains_key("a2"));
        assert!(
            inner.completed.contains_key("b1"),
            "p2 must be untouched by p1 overflow"
        );
        assert_eq!(inner.completed_bytes.get("p1").copied(), Some(600));
        assert_eq!(inner.completed_bytes.get("p2").copied(), Some(600));
    }

    #[test]
    fn drop_completed_for_parent_clears_byte_counter() {
        let mut inner = PendingInner::default(); // unlimited; teardown still clears
        inner.insert_completed("a", completed_with_text("p1", 100));
        inner.insert_completed("b", completed_with_text("p2", 100));
        inner.drop_completed_for_parent("p1");
        assert!(!inner.completed.contains_key("a"));
        assert!(inner.completed.contains_key("b"));
        assert_eq!(
            inner.completed_bytes.get("p1"),
            None,
            "byte counter must be cleared on teardown"
        );
        assert_eq!(inner.completed_bytes.get("p2").copied(), Some(100));
    }

    #[tokio::test]
    async fn lowering_cap_prunes_existing_completed_results() {
        let broker = DelegationBroker::new(
            Arc::new(MockSpawner::new()) as Arc<dyn ConnectionSpawner>,
            shallow_lookup(),
        );
        // Start unlimited and retain several results for one parent.
        broker
            .set_config(DelegationConfig {
                completed_cache_cap_bytes: 0,
                ..DelegationConfig::default()
            })
            .await;
        {
            let mut inner = broker.pending.inner.lock().await;
            for i in 0..5 {
                inner.insert_completed(&format!("t{i}"), completed_with_text("p1", 400));
            }
            assert_eq!(inner.completed.len(), 5);
            assert_eq!(inner.completed_bytes.get("p1").copied(), Some(2000));
        }
        // Lower the cap to 1000 bytes — existing results must be pruned NOW,
        // not only on the next completion (which may never arrive).
        broker
            .set_config(DelegationConfig {
                completed_cache_cap_bytes: 1000,
                ..DelegationConfig::default()
            })
            .await;
        let inner = broker.pending.inner.lock().await;
        assert!(
            inner.completed_bytes.get("p1").copied().unwrap_or(0) <= 1000,
            "retained bytes must fit the lowered cap"
        );
        assert!(
            inner.completed.contains_key("t4"),
            "newest result must survive pruning"
        );
        assert!(
            !inner.completed.contains_key("t0"),
            "oldest result must be pruned"
        );
    }
}
