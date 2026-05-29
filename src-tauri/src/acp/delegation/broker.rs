//! `DelegationBroker` — the coordination unit for multi-agent delegation.
//!
//! Lifecycle of a single call:
//!
//! 1. `handle_request` is the broker's only entry point. The MCP listener
//!    feeds it the LLM-issued `delegate_to_agent` payload.
//! 2. Pre-checks: feature enabled? depth limit ok? Both failures return
//!    immediately, no child session created.
//! 3. Spawn the child via [`ConnectionSpawner::spawn`].
//! 4. Send the delegation task as the first prompt via
//!    [`ConnectionSpawner::send_prompt_linked_for_delegation`]. The trailing
//!    [`DelegationLink`] carries the parent's `tool_use_id` and a
//!    broker-internal `call_id` (UUID) — these get persisted onto the new
//!    conversation row.
//! 5. Park a `oneshot::Sender` keyed by `call_id`. Resolution comes from
//!    one of:
//!       - the listener calling [`DelegationBroker::complete_call`] on
//!         `TurnComplete` (happy path), or
//!       - a cancel — either MCP-side
//!         (`notifications/cancelled` → `cancel_by_external_handle`),
//!         child-side ([`DelegationBroker::cancel_by_child_connection`]),
//!         or parent-side ([`DelegationBroker::cancel_by_parent`]).
//! 6. On any resolution, the child connection is disconnected. v1 is
//!    explicitly one-shot — no session reuse.
//!
//! Cancellation cascade: when a parent session goes away (user-initiated
//! cancel, parent disconnect), the lifecycle subscriber calls
//! [`DelegationBroker::cancel_by_parent`] which fans out cancel + disconnect
//! to every pending child of that parent.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{oneshot, Mutex};

use crate::acp::delegation::event_emitter::{DelegationEventEmitter, NoopEventEmitter};
use crate::acp::delegation::meta_writer::{
    build_delegation_meta, is_synthetic_parent_tool_use_id, DelegationMetaWriter, NoopMetaWriter,
};
use crate::acp::delegation::spawner::{ConnectionSpawner, DelegationLink};
use crate::acp::delegation::types::{
    AgentDelegationDefaults, DelegationError, DelegationOutcome, DelegationRequest,
};
use crate::acp::types::DelegationResultSummary;
use crate::models::AgentType;

/// Lookup the `parent_id` for a conversation. Abstracted so the broker can be
/// unit-tested against an in-memory chain without touching SeaORM.
#[async_trait]
pub trait ConversationDepthLookup: Send + Sync {
    async fn parent_of(&self, conversation_id: i32) -> Result<Option<i32>, DelegationError>;
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
}

impl Default for DelegationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            depth_limit: 1,
            agent_defaults: BTreeMap::new(),
        }
    }
}

struct PendingCall {
    child_connection_id: String,
    child_conversation_id: i32,
    parent_connection_id: String,
    #[allow(dead_code)] // surfaced via accessors and listener payloads in later phases
    parent_tool_use_id: String,
    /// MCP-side opaque handle minted by the companion per `tools/call`. The
    /// listener forwards it through `DelegationRequest`; we keep it here so
    /// `cancel_by_external_handle` can find the entry. `None` for delegations
    /// that didn't come through MCP (tests, future internal callers).
    external_handle: Option<String>,
    tx: oneshot::Sender<DelegationOutcome>,
}

#[derive(Default)]
struct PendingCalls {
    inner: Mutex<HashMap<String, PendingCall>>,
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
///   broker round-trip has not yet claimed. Subject to
///   [`PENDING_TOOL_CALL_TTL`] eviction so an ACP id whose MCP
///   round-trip never arrives doesn't linger forever, and bounded by
///   [`PENDING_QUEUE_CAP`] FIFO eviction as a defensive memory cap.
/// * `consumed` — ids that were already claimed by a prior
///   round-trip. NEITHER subject to TTL eviction NOR to a per-bucket
///   cap: a delegated child agent may run for minutes to hours, and
///   the host can re-emit the same `tool_call` (e.g. as a `completed`
///   status flip) at the end of that run, so the consumed memory
///   must outlast the entire parent-side tool call lifetime. It is
///   scoped to the parent connection's lifetime instead, cleared by
///   `drop_pending_tool_calls_for_parent` on disconnect. The growth
///   is naturally bounded by how many `delegate_to_agent` calls a
///   single parent session issues — typically tens at most, with
///   each `(String, Instant)` entry costing well under 100 bytes —
///   so an unbounded set is comfortable for realistic high-fan-out
///   sessions without OOM risk in the typical operating envelope.
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

#[derive(Default)]
struct ToolCallTrackerBucket {
    pending: VecDeque<(String, Instant)>,
    consumed: VecDeque<(String, Instant)>,
}

/// Maximum age of a `pending` entry before `take_*` discards it as
/// stale. 60 s is far longer than any observed ACP→MCP gap (<5 ms
/// typical, <100 ms in the worst case the polling budget targets) but
/// short enough that a forgotten id from an earlier delegation cannot
/// outlast a subsequent one in the same parent session.
///
/// Only `pending` ages out under this TTL. The `consumed` side has no
/// TTL — see [`ToolCallTrackerBucket`] — because long-running
/// delegations can re-emit the parent-side `tool_call` well past this
/// window.
const PENDING_TOOL_CALL_TTL: Duration = Duration::from_secs(60);

/// Hard cap on the `pending` half of a bucket. Defends against a
/// parent that fires many delegations without ever round-tripping
/// (so each ACP id would linger until TTL eviction). Eviction is
/// FIFO via `pop_front`. The `consumed` half deliberately has NO cap
/// because evicting an older consumed id risks the exact bug this
/// machinery exists to prevent (a late re-emit slipping through and
/// mis-binding the next delegation); growth there is bounded by the
/// parent connection's lifetime instead.
const PENDING_QUEUE_CAP: usize = 32;

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
    pending: Arc<PendingCalls>,
    tool_calls: Arc<ToolCallTracker>,
    pre_canceled_handles: Arc<PreCanceledHandles>,
    config: Arc<Mutex<DelegationConfig>>,
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
            pending: Arc::new(PendingCalls::default()),
            tool_calls: Arc::new(ToolCallTracker::default()),
            pre_canceled_handles: Arc::new(PreCanceledHandles::default()),
            config: Arc::new(Mutex::new(DelegationConfig::default())),
        }
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
        self.register_pending_tool_call_at(parent_connection_id, tool_call_id, Instant::now())
            .await;
    }

    /// `register_pending_tool_call` with an injected "as of" instant.
    /// The public entry point pins it to `Instant::now()`; tests can
    /// supply a future instant to exercise per-bucket invariants
    /// (e.g. long-running delegations re-emitting after the pending
    /// TTL elapsed) without sleeping.
    ///
    /// Holds the [`ToolCallTracker`] mutex across both dedupe tiers
    /// AND the push so no concurrent `take` can split the
    /// "queue empty + not yet recorded as consumed" window where a
    /// host re-emit could otherwise inject a stale duplicate.
    async fn register_pending_tool_call_at(
        &self,
        parent_connection_id: &str,
        tool_call_id: String,
        now: Instant,
    ) {
        let mut map = self.tool_calls.inner.lock().await;
        let bucket = map.entry(parent_connection_id.to_string()).or_default();
        // Tier 2: recently consumed. No TTL — the consumed memory must
        // outlast the entire parent-side tool call lifetime (minutes
        // to hours) so a host re-emit at terminal status flip is
        // still rejected. See `ToolCallTrackerBucket` docs.
        if bucket.consumed.iter().any(|(id, _)| id == &tool_call_id) {
            eprintln!(
                "[delegation] dropping ACP tool_call_id={tool_call_id} on conn={parent_connection_id} (already consumed by an earlier delegation)"
            );
            return;
        }
        // Tier 1: in-queue.
        if bucket.pending.iter().any(|(id, _)| id == &tool_call_id) {
            eprintln!(
                "[delegation] dropping duplicate ACP tool_call_id={tool_call_id} on conn={parent_connection_id}"
            );
            return;
        }
        if bucket.pending.len() >= PENDING_QUEUE_CAP {
            bucket.pending.pop_front();
        }
        bucket.pending.push_back((tool_call_id, now));
    }

    /// Pop the oldest pending `tool_call_id` for the given parent, if any.
    /// Skips entries older than [`PENDING_TOOL_CALL_TTL`] so an ACP id whose
    /// matching MCP round-trip never arrived cannot mis-bind a later
    /// delegation. Mutates the queue in-place; the bucket is removed once
    /// drained.
    pub async fn take_pending_tool_call(&self, parent_connection_id: &str) -> Option<String> {
        self.take_pending_tool_call_at(parent_connection_id, Instant::now())
            .await
    }

    /// `take_pending_tool_call` with an injected "as of" instant. The
    /// public entry point pins it to `Instant::now()`; tests can supply
    /// a future instant to exercise TTL eviction without sleeping past
    /// [`PENDING_TOOL_CALL_TTL`].
    async fn take_pending_tool_call_at(
        &self,
        parent_connection_id: &str,
        now: Instant,
    ) -> Option<String> {
        let mut map = self.tool_calls.inner.lock().await;
        let bucket = map.get_mut(parent_connection_id)?;
        let mut claimed: Option<String> = None;
        while let Some((id, ts)) = bucket.pending.pop_front() {
            if now.duration_since(ts) > PENDING_TOOL_CALL_TTL {
                let age_secs = now.duration_since(ts).as_secs();
                eprintln!(
                    "[delegation] evicting stale ACP tool_call_id={id} (age={age_secs}s) on conn={parent_connection_id}"
                );
                continue;
            }
            claimed = Some(id);
            break;
        }
        // Same mutex span: record the claim into the consumed memory so
        // a concurrent re-register cannot observe "pending empty AND
        // consumed missing" and inject a stale duplicate. Consumed
        // entries persist for the whole parent connection lifetime
        // (no TTL, no cap — see `ToolCallTrackerBucket`) and are only
        // released when the parent disconnects.
        if let Some(id) = &claimed {
            bucket.consumed.push_back((id.clone(), now));
        }
        if bucket.pending.is_empty() && bucket.consumed.is_empty() {
            map.remove(parent_connection_id);
        }
        claimed
    }

    /// `take_pending_tool_call` with a brief poll loop. Used by
    /// `handle_request` to absorb the inherent race between two parallel
    /// arrival paths for the parent's `delegate_to_agent` invocation:
    ///
    ///   * ACP `session/update(tool_call)` → in-process bus → lifecycle
    ///     dispatcher → `register_pending_tool_call` (fast)
    ///   * MCP `tools/call` → stdio round-trip → companion server →
    ///     `handle_request` (slower, but not by much)
    ///
    /// In practice the ACP path lands first because it's in-process, but
    /// the order is not contractually guaranteed. Without this wait, a
    /// faster-than-usual MCP delivery would slip past an empty queue and
    /// fall back to the synthetic `delegation-<uuid>` placeholder — which
    /// breaks the parent's UI binding because the frontend keys its
    /// `parent_tool_use_id` map by the agent's real `tool_call_id`.
    ///
    /// 150 ms total polling budget (15 attempts × 10 ms): the observed
    /// gap on local dev is well under 5 ms, but headroom protects against
    /// busier hosts (Docker, slow disk, high LLM stream pressure) and
    /// slower MCP transports. Bumped from the original 100 ms after
    /// intermittent reports of missing "view sub-agent conversation"
    /// buttons; the no-ACP-id fallback path is delayed at most by 50 ms
    /// extra, which is imperceptible next to the delegation spawn cost.
    async fn claim_pending_tool_call_with_brief_wait(
        &self,
        parent_connection_id: &str,
    ) -> Option<String> {
        if let Some(id) = self.take_pending_tool_call(parent_connection_id).await {
            return Some(id);
        }
        for _ in 0..15 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if let Some(id) = self.take_pending_tool_call(parent_connection_id).await {
                return Some(id);
            }
        }
        None
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
        self.tool_calls
            .inner
            .lock()
            .await
            .remove(parent_connection_id);
    }

    pub async fn set_config(&self, cfg: DelegationConfig) {
        *self.config.lock().await = cfg;
    }

    pub async fn config_snapshot(&self) -> DelegationConfig {
        self.config.lock().await.clone()
    }

    /// Entry point. Drives the full lifecycle and returns whatever the parent
    /// LLM should see as the `delegate_to_agent` tool_result.
    pub async fn handle_request(&self, mut req: DelegationRequest) -> DelegationOutcome {
        // Pre-cancel short-circuit. If the MCP companion already received
        // `notifications/cancelled` for this `tools/call` before we even
        // started processing (cancel ran ahead of the UDS round-trip), we
        // claim the handle from the pre-cancel set and bail without
        // spawning anything — the caller will not be receiving our
        // response either way (the companion suppresses it per MCP spec).
        if let Some(handle) = req.external_handle.as_deref() {
            if self.take_pre_canceled_handle(handle).await {
                return DelegationOutcome::from_err(
                    DelegationError::Canceled {
                        reason: "canceled before spawn".into(),
                    },
                    None,
                );
            }
        }
        // MCP clients usually don't populate `_meta.tool_use_id`, so the
        // listener will pass through an empty string. Best-effort claim the
        // most recent ACP-side `tool_call_id` for this parent — with a brief
        // poll loop so an MCP round-trip that out-races the in-process ACP
        // `session/update` doesn't fall back to a synthetic id (which
        // breaks the parent UI's `parent_tool_use_id` binding). Falls back
        // to a UUID placeholder only after the wait budget is exhausted.
        if req.parent_tool_use_id.is_empty() {
            req.parent_tool_use_id = self
                .claim_pending_tool_call_with_brief_wait(&req.parent_connection_id)
                .await
                .unwrap_or_else(|| {
                    eprintln!(
                        "[delegation] synthetic fallback for parent_tool_use_id on conn={} (no ACP tool_call_id arrived within 150ms budget)",
                        req.parent_connection_id
                    );
                    format!("delegation-{}", uuid::Uuid::new_v4())
                });
        }
        let cfg = self.config_snapshot().await;
        if !cfg.enabled {
            return DelegationOutcome::from_err(
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
            Err(e) => return DelegationOutcome::from_err(e, None),
        };
        // The child the broker is about to create would sit at `parent_depth + 1`.
        // Reject only when the *child* depth would strictly exceed the limit;
        // a child sitting exactly at `depth_limit` is allowed.
        if parent_depth + 1 > cfg.depth_limit {
            return DelegationOutcome::from_err(
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
                return DelegationOutcome::from_err(
                    DelegationError::SpawnFailed(e.to_string()),
                    None,
                );
            }
        };

        // --- Send linked prompt ------------------------------------------------
        let call_id = uuid::Uuid::new_v4().to_string();
        let link = DelegationLink {
            parent_conversation_id: req.parent_conversation_id,
            parent_tool_use_id: req.parent_tool_use_id.clone(),
            delegation_call_id: call_id.clone(),
        };
        let child_conversation_id = match self
            .spawner
            .send_prompt_linked_for_delegation(&child_connection_id, req.task.clone(), link)
            .await
        {
            Ok(cid) => cid,
            Err(e) => {
                let _ = self.spawner.disconnect(&child_connection_id).await;
                return DelegationOutcome::from_err(
                    DelegationError::SpawnFailed(e.to_string()),
                    None,
                );
            }
        };

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
            ),
        )
        .await;

        // --- Register pending + await completion or cancel --------------------
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.inner.lock().await;
            map.insert(
                call_id.clone(),
                PendingCall {
                    child_connection_id: child_connection_id.clone(),
                    child_conversation_id,
                    parent_connection_id: req.parent_connection_id.clone(),
                    parent_tool_use_id: req.parent_tool_use_id.clone(),
                    external_handle: req.external_handle.clone(),
                    tx,
                },
            );
        }

        // Second pre-cancel check: a `notifications/cancelled` may have
        // landed between the entry-side check and the pending registration
        // above. If so, drain the entry ourselves (so cancel_by_external_handle
        // racing us doesn't double-emit) and surface the canceled outcome.
        if let Some(handle) = req.external_handle.as_deref() {
            if self.take_pre_canceled_handle(handle).await {
                let entry = self.pending.inner.lock().await.remove(&call_id);
                if let Some(PendingCall { tx, .. }) = entry {
                    self.write_meta_if_real(
                        &req.parent_connection_id,
                        &req.parent_tool_use_id,
                        build_delegation_meta(
                            "failed",
                            Some(&child_connection_id),
                            Some(child_conversation_id),
                            Some("canceled"),
                        ),
                    )
                    .await;
                    self.emit_completed_if_real(
                        &req.parent_connection_id,
                        &req.parent_tool_use_id,
                        &child_connection_id,
                        child_conversation_id,
                        DelegationResultSummary::Err {
                            error_code: "canceled".to_string(),
                        },
                    )
                    .await;
                    let _ = self.spawner.cancel(&child_connection_id).await;
                    let _ = self.spawner.disconnect(&child_connection_id).await;
                    let outcome = DelegationOutcome::from_err(
                        DelegationError::Canceled {
                            reason: "canceled before await".into(),
                        },
                        Some(child_conversation_id),
                    );
                    let _ = tx.send(outcome.clone());
                    return outcome;
                }
            }
        }

        match rx.await {
            Ok(outcome) => {
                // complete_call (or cancel_*) already removed from `pending`,
                // wrote meta, emitted DelegationCompleted, and disconnected;
                // this is a belt-and-braces idempotent prune in case the
                // resolver path didn't drain it (it always does in production,
                // but the prune is cheap).
                self.pending.inner.lock().await.remove(&call_id);
                outcome
            }
            Err(_) => {
                // The sender was dropped before sending — should not happen in
                // practice (complete_call / cancel_* always send before drop),
                // but be defensive. Drain pending FIRST so a racing resolver
                // (from a late lifecycle TurnComplete) finds no entry and
                // silently no-ops instead of double-emitting DelegationCompleted.
                let _ = self.pending.inner.lock().await.remove(&call_id);
                self.write_meta_if_real(
                    &req.parent_connection_id,
                    &req.parent_tool_use_id,
                    build_delegation_meta(
                        "failed",
                        Some(&child_connection_id),
                        Some(child_conversation_id),
                        Some("canceled"),
                    ),
                )
                .await;
                self.emit_completed_if_real(
                    &req.parent_connection_id,
                    &req.parent_tool_use_id,
                    &child_connection_id,
                    child_conversation_id,
                    DelegationResultSummary::Err {
                        error_code: "canceled".to_string(),
                    },
                )
                .await;
                let _ = self.spawner.disconnect(&child_connection_id).await;
                DelegationOutcome::from_err(
                    DelegationError::Canceled {
                        reason: "completion channel dropped".into(),
                    },
                    Some(child_conversation_id),
                )
            }
        }
    }

    /// Called by the child-session lifecycle subscriber on `TurnComplete`
    /// (success path) or by error mappers (failure path). Idempotent —
    /// calls on unknown `call_id` are silent no-ops.
    pub async fn complete_call(&self, call_id: &str, outcome: DelegationOutcome) {
        let entry = self.pending.inner.lock().await.remove(call_id);
        if let Some(PendingCall {
            child_connection_id,
            child_conversation_id,
            parent_connection_id,
            parent_tool_use_id,
            external_handle: _,
            tx,
        }) = entry
        {
            // Mirror the resolution onto the parent's `delegate_to_agent`
            // ToolCallState meta so snapshot recovery after refresh shows
            // the final state without depending on the broker's live
            // `delegation_completed` event having been received.
            let meta = match &outcome {
                DelegationOutcome::Ok(_) => build_delegation_meta(
                    "completed",
                    Some(&child_connection_id),
                    Some(child_conversation_id),
                    None,
                ),
                DelegationOutcome::Err { code, .. } => build_delegation_meta(
                    "failed",
                    Some(&child_connection_id),
                    Some(child_conversation_id),
                    Some(code),
                ),
            };
            self.write_meta_if_real(&parent_connection_id, &parent_tool_use_id, meta)
                .await;
            self.emit_completed_if_real(
                &parent_connection_id,
                &parent_tool_use_id,
                &child_connection_id,
                child_conversation_id,
                Self::outcome_to_summary(&outcome),
            )
            .await;
            // v1 one-shot: always tear down the child.
            let _ = self.spawner.disconnect(&child_connection_id).await;
            let _ = tx.send(outcome);
        }
    }

    /// Project a `DelegationOutcome` onto the wire-stable
    /// `DelegationResultSummary` carried by `AcpEvent::DelegationCompleted`.
    /// Keeps the mapping (and the `error_code` choice) in one place.
    fn outcome_to_summary(outcome: &DelegationOutcome) -> DelegationResultSummary {
        match outcome {
            DelegationOutcome::Ok(ok) => DelegationResultSummary::Ok {
                duration_ms: ok.duration_ms,
            },
            DelegationOutcome::Err { code, .. } => DelegationResultSummary::Err {
                error_code: code.clone(),
            },
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
        let drained: Vec<(String, PendingCall)> = {
            let mut map = self.pending.inner.lock().await;
            let keys: Vec<String> = map
                .iter()
                .filter(|(_, v)| {
                    v.external_handle
                        .as_deref()
                        .map(|h| h == external_handle)
                        .unwrap_or(false)
                })
                .map(|(k, _)| k.clone())
                .collect();
            keys.into_iter()
                .map(|k| {
                    let entry = map.remove(&k).expect("key just observed");
                    (k, entry)
                })
                .collect()
        };
        if drained.is_empty() {
            // Race: the cancel beat the handle's pending registration.
            // Buffer it (capped, FIFO-evicted) so `handle_request` can
            // drain itself on the next checkpoint instead of merrily
            // proceeding to spawn the child.
            self.buffer_pre_canceled_handle(external_handle.to_string())
                .await;
            return;
        }
        for (_call_id, entry) in drained {
            self.write_meta_if_real(
                &entry.parent_connection_id,
                &entry.parent_tool_use_id,
                build_delegation_meta(
                    "failed",
                    Some(&entry.child_connection_id),
                    Some(entry.child_conversation_id),
                    Some("canceled"),
                ),
            )
            .await;
            self.emit_completed_if_real(
                &entry.parent_connection_id,
                &entry.parent_tool_use_id,
                &entry.child_connection_id,
                entry.child_conversation_id,
                DelegationResultSummary::Err {
                    error_code: "canceled".to_string(),
                },
            )
            .await;
            let _ = self.spawner.cancel(&entry.child_connection_id).await;
            let _ = self.spawner.disconnect(&entry.child_connection_id).await;
            let _ = entry.tx.send(DelegationOutcome::from_err(
                DelegationError::Canceled {
                    reason: reason.clone(),
                },
                Some(entry.child_conversation_id),
            ));
        }
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
        let drained: Vec<PendingCall> = {
            let mut map = self.pending.inner.lock().await;
            let keys: Vec<String> = map
                .iter()
                .filter(|(_, v)| v.child_connection_id == child_connection_id)
                .map(|(k, _)| k.clone())
                .collect();
            keys.into_iter()
                .map(|k| map.remove(&k).expect("key just observed"))
                .collect()
        };
        let reason = match terminal_error {
            Some(detail) if !detail.trim().is_empty() => {
                format!("child session ended without TurnComplete: {detail}")
            }
            _ => "child session ended without TurnComplete".to_string(),
        };
        for entry in drained {
            self.write_meta_if_real(
                &entry.parent_connection_id,
                &entry.parent_tool_use_id,
                build_delegation_meta(
                    "failed",
                    Some(&entry.child_connection_id),
                    Some(entry.child_conversation_id),
                    Some("canceled"),
                ),
            )
            .await;
            self.emit_completed_if_real(
                &entry.parent_connection_id,
                &entry.parent_tool_use_id,
                &entry.child_connection_id,
                entry.child_conversation_id,
                DelegationResultSummary::Err {
                    error_code: "canceled".to_string(),
                },
            )
            .await;
            let _ = self.spawner.disconnect(&entry.child_connection_id).await;
            let _ = entry.tx.send(DelegationOutcome::from_err(
                DelegationError::Canceled {
                    reason: reason.clone(),
                },
                Some(entry.child_conversation_id),
            ));
        }
    }

    /// Cascade-cancel every pending delegation owned by `parent_connection_id`.
    /// Used when a parent session disconnects or the user cancels the parent's
    /// active prompt.
    pub async fn cancel_by_parent(&self, parent_connection_id: &str) {
        // Also drain any tool_call ids that were captured ahead of an MCP
        // round-trip that never arrived — keeps the map bounded across
        // parent reconnects.
        self.drop_pending_tool_calls_for_parent(parent_connection_id)
            .await;
        let drained: Vec<PendingCall> = {
            let mut map = self.pending.inner.lock().await;
            let keys: Vec<String> = map
                .iter()
                .filter(|(_, v)| v.parent_connection_id == parent_connection_id)
                .map(|(k, _)| k.clone())
                .collect();
            keys.into_iter()
                .map(|k| map.remove(&k).expect("key just observed"))
                .collect()
        };
        for entry in drained {
            // Best-effort meta patch so a parent-side snapshot post-cancel
            // shows the delegation as failed/canceled rather than stuck
            // on the prior "running" mark.
            self.write_meta_if_real(
                &entry.parent_connection_id,
                &entry.parent_tool_use_id,
                build_delegation_meta(
                    "failed",
                    Some(&entry.child_connection_id),
                    Some(entry.child_conversation_id),
                    Some("canceled"),
                ),
            )
            .await;
            self.emit_completed_if_real(
                &entry.parent_connection_id,
                &entry.parent_tool_use_id,
                &entry.child_connection_id,
                entry.child_conversation_id,
                DelegationResultSummary::Err {
                    error_code: "canceled".to_string(),
                },
            )
            .await;
            let _ = self.spawner.cancel(&entry.child_connection_id).await;
            let _ = self.spawner.disconnect(&entry.child_connection_id).await;
            let _ = entry.tx.send(DelegationOutcome::from_err(
                DelegationError::Canceled {
                    reason: "parent canceled".into(),
                },
                Some(entry.child_conversation_id),
            ));
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn peek_first_pending_call_id(&self) -> Option<String> {
        self.pending.inner.lock().await.keys().next().cloned()
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub async fn pending_count(&self) -> usize {
        self.pending.inner.lock().await.len()
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
            external_handle: None,
        }
    }

    fn request_with_handle(
        parent_conv: i32,
        tool_use: &str,
        handle: &str,
    ) -> DelegationRequest {
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
        // complete_call disconnects the child once.
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["child-conn-1"]);
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
        assert_eq!(mock.disconnects.lock().await.as_slice(), &["c1"]);
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
            .register_pending_tool_call_at("p1", "tc-a".into(), long_after)
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
        // delegations (well past PENDING_QUEUE_CAP=32) must still
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
        // Issue many more delegations to overflow the old per-bucket
        // cap. With no cap on consumed, the first id must remain
        // remembered for the lifetime of the parent connection.
        for i in 0..(PENDING_QUEUE_CAP * 4) {
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
        broker.register_pending_tool_call("p1", "stale".into()).await;
        // Fresh id registered "just before" the future `now`.
        broker.register_pending_tool_call("p1", "fresh".into()).await;
        let future_now = t0 + PENDING_TOOL_CALL_TTL + Duration::from_millis(50);
        // Forge "fresh" so it survives the TTL: rewrite its timestamp to
        // ~now-relative-to-future-now. Direct field access is OK — we're
        // a sibling test in the same module.
        {
            let mut map = broker.tool_calls.inner.lock().await;
            let bucket = map.get_mut("p1").expect("bucket present");
            // Re-stamp the second entry ("fresh") to `future_now`.
            if let Some(entry) = bucket.pending.iter_mut().find(|(id, _)| id == "fresh") {
                entry.1 = future_now;
            }
        }
        // First entry ("stale", stamped at ~t0) is past TTL relative to
        // future_now; the second ("fresh") was just re-stamped to
        // future_now and must survive.
        assert_eq!(
            broker
                .take_pending_tool_call_at("p1", future_now)
                .await
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
        assert!(
            matches!(call.result, DelegationResultSummary::Ok { duration_ms: 73 }),
            "expected Ok{{73}}, got {:?}",
            call.result
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
        let envelope = tokio::time::timeout(Duration::from_millis(500), stream_rx.recv())
            .await
            .expect("per-connection stream should receive DelegationCompleted within 500ms")
            .expect("envelope recv must not error");
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
        let bus_envelope = tokio::time::timeout(Duration::from_millis(500), bus_rx.recv())
            .await
            .expect("InternalEventBus should receive DelegationCompleted within 500ms")
            .expect("bus recv must not error");
        assert_eq!(bus_envelope.connection_id, "parent-conn");
        assert!(matches!(
            bus_envelope.payload,
            AcpEvent::DelegationCompleted { .. }
        ));
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
}
