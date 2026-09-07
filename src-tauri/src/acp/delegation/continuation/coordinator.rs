//! The turn coordinator: the ONE writer for every collaboration session.
//!
//! All admission (continue_turn), progress (drive), settlement (settle),
//! cancellation, closing, and startup recovery funnel through here. The
//! database's unique constraints and CAS updates are the concurrency
//! backstop; the coordinator's own in-process state (execution registry,
//! per-source admission locks) only prevents local races from even reaching
//! the DB.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify, RwLock};

use crate::acp::delegation::broker::DelegationOutcomeStore;
use crate::acp::delegation::continuation::runtime::{
    AttachTarget, ContinuationRuntime, NoopRuntime,
};
use crate::acp::delegation::continuation::types::{
    cap_turn_result_text, validate_turn_request, CollaborationSessionState, ContinuationError,
    ContinuationErrorCode, SchemaVersion1, SessionSummary, TurnAck, TurnReport, TurnState,
    TurnTerminal,
};
use crate::db::entities::collaboration_session;
use crate::db::service::collaboration_service::{self, SettlePayload};
use crate::db::AppDatabase;

/// How long a `cancel_requested` round may wait for the agent's stop
/// confirmation before the host declares the outcome UNKNOWABLE (settles
/// `outcome_unknown` and blocks the session). Without this deadline a lost
/// confirmation would wedge the session forever: a `cancel_requested` turn
/// is active, and close refuses active sessions (acceptance F7).
pub const CANCEL_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(60);

/// A parent identity resolved from a TRUSTED context (the MCP listener's
/// token registry), never from LLM-supplied parameters. Every coordinator
/// entry is scoped by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedParent {
    pub conversation_id: i32,
}

/// Which live connection currently owns which turn execution. Keyed by
/// connection id so lifecycle events can find their settle target without
/// trusting any LLM-supplied identifier.
/// A round registered against its parent before attach: (turn, session).
type PendingRound = (String, String);

#[derive(Debug, Clone)]
struct ExecutionOwner {
    turn_id: String,
    execution_id: String,
    /// The collaboration session this round belongs to — lets close_session
    /// find (and refuse to lie about) connections it still holds.
    session_id: String,
    /// The parent ACP connection that initiated the round — used to cascade
    /// cancel when the parent goes away (never trusted from the wire).
    parent_connection_id: String,
}

#[derive(Clone)]
pub struct ContinuationCoordinator {
    db: Arc<AppDatabase>,
    runtime: Arc<dyn ContinuationRuntime>,
    outcome_store: Arc<dyn DelegationOutcomeStore>,
    /// `continuable_delegation_enabled` — default false. Off: no NEW turns;
    /// dedup'd re-reads of existing requests, status, cancel, and close keep
    /// working (they are how a parent winds things down).
    enabled: Arc<RwLock<bool>>,
    /// connection_id → current execution owner. Registered on attach,
    /// removed on settle/close.
    executions: Arc<Mutex<HashMap<String, ExecutionOwner>>>,
    /// parent_connection_id → rounds accepted but NOT yet attached
    /// (accepted/preparing). The parent cleanup path cancels these so a
    /// parent disconnecting mid-prepare can never let the drive attach +
    /// send afterwards (acceptance F4).
    parent_pending: Arc<Mutex<HashMap<String, Vec<PendingRound>>>>,
    /// Per-source-task admission locks: two concurrent continue_turn calls
    /// for the same source serialize here before touching the DB. `close`
    /// takes the SAME lock so admission and closing cannot interleave
    /// (acceptance F6).
    admission_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// Woken on every settle so bounded `get_turn` waits re-check promptly.
    settle_notify: Arc<Notify>,
}

impl ContinuationCoordinator {
    pub fn new(
        db: Arc<AppDatabase>,
        runtime: Arc<dyn ContinuationRuntime>,
        outcome_store: Arc<dyn DelegationOutcomeStore>,
    ) -> Self {
        Self {
            db,
            runtime,
            outcome_store,
            enabled: Arc::new(RwLock::new(false)),
            executions: Arc::new(Mutex::new(HashMap::new())),
            parent_pending: Arc::new(Mutex::new(HashMap::new())),
            admission_locks: Arc::new(Mutex::new(HashMap::new())),
            settle_notify: Arc::new(Notify::new()),
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_runtime(mut self, runtime: Arc<dyn ContinuationRuntime>) -> Self {
        self.runtime = runtime;
        self
    }

    /// Test-only: register an execution owner directly (production does this
    /// inside `drive` after a successful strict attach). Lets lifecycle-layer
    /// tests route terminals at a collaboration round without a live runtime.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn register_execution_for_test(
        &self,
        connection_id: &str,
        turn_id: &str,
        execution_id: &str,
        session_id: &str,
        parent_connection_id: &str,
    ) {
        self.executions.lock().await.insert(
            connection_id.to_string(),
            ExecutionOwner {
                turn_id: turn_id.to_string(),
                execution_id: execution_id.to_string(),
                session_id: session_id.to_string(),
                parent_connection_id: parent_connection_id.to_string(),
            },
        );
    }

    pub async fn set_enabled(&self, enabled: bool) {
        *self.enabled.write().await = enabled;
    }

    pub async fn is_enabled(&self) -> bool {
        *self.enabled.read().await
    }

    /// The execution owning a connection, for lifecycle routing (Task 4):
    /// `(turn_id, execution_id)` when the connection is currently driving a
    /// collaboration turn.
    pub async fn execution_owner(
        &self,
        connection_id: &str,
    ) -> Option<(String, String)> {
        self.executions
            .lock()
            .await
            .get(connection_id)
            .map(|o| (o.turn_id.clone(), o.execution_id.clone()))
    }

    /// The `(turn_id, execution_id)` pair currently registered for a turn —
    /// the settle handle tests and the cancel-timeout path use.
    pub async fn execution_owner_by_turn(&self, turn_id: &str) -> Option<(String, String)> {
        self.executions
            .lock()
            .await
            .values()
            .find(|o| o.turn_id == turn_id)
            .map(|o| (o.turn_id.clone(), o.execution_id.clone()))
    }

    /// Which connection is currently driving a turn, for cancel/disconnect.
    pub async fn connection_of_turn(&self, turn_id: &str) -> Option<String> {
        self.executions
            .lock()
            .await
            .iter()
            .find(|(_, o)| o.turn_id == turn_id)
            .map(|(conn, _)| conn.clone())
    }

    // -----------------------------------------------------------------------
    // continue_with_session
    // -----------------------------------------------------------------------

    /// Accept a rework turn for a frozen completed source. Returns the ack as
    /// soon as the accepted turn is DURABLE — the actual attach/send runs in
    /// the background drive.
    pub async fn continue_turn(
        &self,
        parent: VerifiedParent,
        parent_connection_id: &str,
        source_task_id: &str,
        request_id: &str,
        message: &str,
        initiator_tool_use_id: Option<&str>,
    ) -> Result<TurnAck, ContinuationError> {
        // Input validation precedes everything: malformed input fails before
        // any write or launch.
        validate_turn_request(request_id, message).map_err(|e| e.with_ids(None, None))?;

        // --- Resolve the source ------------------------------------------------
        // Only THIS parent's frozen completed results are continuable. A
        // foreign or unknown source is indistinguishable from a nonexistent
        // one (no existence leak).
        let outcome = match self.outcome_store.find_owned(parent.conversation_id, source_task_id).await {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!("[continuation] outcome lookup failed: {e}");
                return Err(ContinuationError::new(
                    ContinuationErrorCode::StorageUnavailable,
                    "the outcome store could not be read",
                ));
            }
        };
        let Some(outcome) = outcome else {
            return Err(ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no continuable source under this session",
            ));
        };
        // A frozen result without a verifiable binding is never continuable —
        // PR1 stores null exactly for that case, and guessing defaults is
        // forbidden by design.
        let binding = outcome
            .resume_binding_json
            .as_deref()
            .and_then(AttachTarget::from_binding_json)
            .ok_or_else(|| {
                ContinuationError::new(
                    ContinuationErrorCode::SourceNotResumable,
                    "the recorded result carries no verifiable resume binding",
                )
            })?;

        // --- Serialize admission per source ------------------------------------
        let admission = self
            .admission_locks
            .lock()
            .await
            .entry(source_task_id.to_string())
            .or_default()
            .clone();
        let _guard = admission.lock().await;

        let existing_session =
            collaboration_service::find_session_by_source(&self.db.conn, source_task_id)
                .await
                .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?;

        if let Some(session) = existing_session.as_ref() {
            if session.parent_conversation_id != parent.conversation_id {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::NotFoundOrForbidden,
                    "no continuable source under this session",
                ));
            }
            if session.state == "closed" {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::SessionClosed,
                    "the collaboration session for this source is closed; its \
                     history stays readable but no new rounds can start",
                )
                .with_ids(Some(&session.id), None));
            }
            // Request dedup precedes the busy/blocked checks: a retry of an
            // EXISTING request must return its original handle even when the
            // feature is off or the session is blocked.
            if let Some(turn) = collaboration_service::find_turn_by_request(
                &self.db.conn,
                &session.id,
                request_id,
            )
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
            {
                if turn.message == message {
                    return Ok(turn_ack(source_task_id, &turn));
                }
                return Err(ContinuationError::new(
                    ContinuationErrorCode::RequestConflict,
                    "this request_id was already used with a different message",
                )
                .with_ids(Some(&session.id), Some(&turn.id)));
            }
            if session.state == "blocked" {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::SessionBlocked,
                    "the session has an unknown outcome; inspect the actual \
                     artifacts, then close it — no new rounds",
                )
                .with_ids(Some(&session.id), None));
            }
            // A NEW round on an existing relationship needs the feature too
            // (dedup'd replays above already passed through).
            if !self.is_enabled().await {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::FeatureDisabled,
                    "continuable delegation is disabled",
                ));
            }
        } else {
            // A brand-new relationship requires the feature to be on.
            if !self.is_enabled().await {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::FeatureDisabled,
                    "continuable delegation is disabled",
                ));
            }
            // The child conversation must still exist and be soft-alive.
            if !collaboration_service::child_conversation_alive(
                &self.db.conn,
                outcome.child_conversation_id.unwrap_or(0),
            )
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
            {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::SourceMissing,
                    "the source child session no longer exists",
                ));
            }
        }

        // Reject admission while a turn is already active (the DB's partial
        // unique index is the hard backstop; this pre-check gives the clean
        // domain error).
        if let Some(session) = existing_session.as_ref() {
            if collaboration_service::active_turn(&self.db.conn, &session.id)
                .await
                .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
                .is_some()
            {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::SessionBusy,
                    "another turn is already active for this session; query or \
                     cancel it first",
                )
                .with_ids(Some(&session.id), None));
            }
        }

        // --- Durable admission --------------------------------------------------
        let session = match existing_session {
            Some(session) => session,
            None => collaboration_service::upsert_session_once(
                &self.db.conn,
                collaboration_service::NewSession {
                    id: uuid::Uuid::new_v4().to_string(),
                    source_task_id: source_task_id.to_string(),
                    parent_conversation_id: parent.conversation_id,
                    child_conversation_id: outcome.child_conversation_id.unwrap_or(0),
                    resume_binding_json: outcome.resume_binding_json.clone().unwrap_or_default(),
                },
            )
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?,
        };

        // Transactional admission (reacceptance R5): session state and the
        // single-active invariant are re-validated in the SAME transaction
        // that inserts the turn, so an interleaving unknown-settle (turn
        // terminal + session blocked) can never slip between the checks and
        // the insert. The pre-checks above keep producing the clean domain
        // errors for the common paths; this is the authoritative one.
        let turn = match collaboration_service::admit_turn(
            &self.db.conn,
            collaboration_service::NewTurn {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session.id.clone(),
                ordinal: 0, // assigned inside the admission transaction
                request_id: request_id.to_string(),
                message: message.to_string(),
                initiator_parent_conversation_id: parent.conversation_id,
                initiator_tool_use_id: initiator_tool_use_id
                    .filter(|id| !id.trim().is_empty())
                    .map(str::to_string),
            },
        )
        .await
        {
            Ok(turn) => turn,
            Err(collaboration_service::AdmitTurnError::SessionBlocked) => {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::SessionBlocked,
                    "the session has an unknown outcome; inspect the actual \
                     artifacts, then close it — no new rounds",
                )
                .with_ids(Some(&session.id), None));
            }
            Err(collaboration_service::AdmitTurnError::SessionClosed) => {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::SessionClosed,
                    "the collaboration session for this source is closed; its \
                     history stays readable but no new rounds can start",
                )
                .with_ids(Some(&session.id), None));
            }
            Err(collaboration_service::AdmitTurnError::SessionBusy) => {
                return Err(ContinuationError::new(
                    ContinuationErrorCode::SessionBusy,
                    "another turn is already active for this session",
                )
                .with_ids(Some(&session.id), None));
            }
            Err(collaboration_service::AdmitTurnError::Storage(e)) => {
                tracing::warn!("[continuation] admission storage failure: {e}");
                return Err(storage_unavailable());
            }
        };

        let ack = turn_ack(source_task_id, &turn);

        // --- Background drive ---------------------------------------------------
        let coordinator = self.clone();
        let turn_id = turn.id.clone();
        let execution_id = turn.execution_id.clone();
        let message_owned = message.to_string();
        {
            // Register the parent ownership BEFORE the drive can attach: a
            // parent disconnect landing anywhere from here on finds this
            // round either here (not yet attached → CAS-canceled, no send)
            // or in `executions` (attached → cancel + settle).
            let mut pending = self.parent_pending.lock().await;
            pending
                .entry(parent_connection_id.to_string())
                .or_default()
                .push((turn.id.clone(), session.id.clone()));
        }

        let session_id_owned = session.id.clone();
        let child_conversation_id = session.child_conversation_id;
        let parent_conn_owned = parent_connection_id.to_string();
        tokio::spawn(async move {
            coordinator
                .drive(
                    turn_id,
                    execution_id,
                    session_id_owned,
                    child_conversation_id,
                    parent_conn_owned,
                    binding,
                    message_owned,
                )
                .await;
        });

        Ok(ack)
    }

    // -----------------------------------------------------------------------
    // Drive: accepted → preparing → attach → dispatching → send → running
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn drive(
        &self,
        turn_id: String,
        execution_id: String,
        session_id: String,
        child_conversation_id: i32,
        parent_connection_id: String,
        binding: AttachTarget,
        message: String,
    ) {
        let conn = &self.db.conn;
        // accepted → preparing.
        match collaboration_service::cas_turn_state(
            conn,
            &turn_id,
            Some(&execution_id),
            &["accepted"],
            "preparing",
            false,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                // Canceled/lost while parked — drop the parent registration.
                let mut pending = self.parent_pending.lock().await;
                if let Some(list) = pending.get_mut(&parent_connection_id) {
                    list.retain(|(tid, _)| tid != &turn_id);
                }
                return;
            }
            Err(e) => {
                // A CAS that ERRORS (not just loses) means the store is
                // untrustworthy: no send may happen and the session must be
                // isolated.
                tracing::warn!("[continuation] preparing CAS failed for {turn_id}: {e}");
                let mut pending = self.parent_pending.lock().await;
                if let Some(list) = pending.get_mut(&parent_connection_id) {
                    list.retain(|(tid, _)| tid != &turn_id);
                }
                self.isolate_on_storage_failure(&turn_id, &session_id, None)
                    .await;
                return;
            }
        }

        // Strict attach — the ONLY path into the child session. The runtime
        // binds the attached connection to the reserved child conversation
        // row (R1) so lifecycle terminals can settle this round.
        let parent_conn_id = parent_connection_id.clone();
        let connection_id = match self
            .runtime
            .attach_strict(
                &binding,
                &parent_connection_id,
                &turn_id,
                &execution_id,
                child_conversation_id,
            )
            .await
        {
            Ok(conn_id) => conn_id,
            Err(err) => {
                let mapped = ContinuationError::from(err);
                self.settle_failure(&turn_id, &execution_id, &mapped).await;
                let mut pending = self.parent_pending.lock().await;
                if let Some(list) = pending.get_mut(&parent_connection_id) {
                    list.retain(|(tid, _)| tid != &turn_id);
                }
                return;
            }
        };

        // Register the execution owner BEFORE dispatching, so lifecycle
        // events racing the send already route here.
        self.executions.lock().await.insert(
            connection_id.clone(),
            ExecutionOwner {
                turn_id: turn_id.clone(),
                execution_id: execution_id.clone(),
                session_id: session_id.clone(),
                parent_connection_id,
            },
        );
        // The round has left the parent-pending registry: from here the
        // execution registry owns it.
        {
            let mut pending = self.parent_pending.lock().await;
            if let Some(list) = pending.get_mut(&parent_conn_id) {
                list.retain(|(tid, _)| tid != &turn_id);
            }
        }
        // Persist the diagnostic connection id (also what cancel uses to
        // reach the agent).
        if let Err(e) = collaboration_service::set_turn_connection(
            conn,
            &turn_id,
            &execution_id,
            &connection_id,
        )
        .await
        {
            tracing::warn!("[continuation] could not persist connection id for {turn_id}: {e}");
        }

        // preparing → dispatching (durably persisted BEFORE any send).
        let dispatched = match collaboration_service::cas_turn_state(
            conn,
            &turn_id,
            Some(&execution_id),
            &["preparing"],
            "dispatching",
            true,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[continuation] dispatch CAS failed for {turn_id}: {e}");
                // Storage failed after attach: no send may happen, and the
                // session must be isolated.
                self.isolate_on_storage_failure(&turn_id, &session_id, Some(&connection_id))
                    .await;
                return;
            }
        };
        if !dispatched {
            // Canceled between attach and dispatch — release and stop.
            self.release_connection(&turn_id, &connection_id).await;
            return;
        }

        // Persist-failure guard is behind us; send EXACTLY once.
        let sent = self.runtime.send_prompt(&connection_id, &message).await;
        if let Err(send_err) = sent {
            tracing::warn!("[continuation] send failed for {turn_id}: {send_err}");
            let terminal = SettlePayload {
                to_state: "failed",
                result_text: None,
                text_truncated: false,
                error_code: Some("resume_failed".to_string()),
                error_message: Some(format!("rework prompt could not be delivered: {send_err}")),
            };
            match collaboration_service::settle_turn(conn, &turn_id, &execution_id, terminal).await {
                Ok(_) => {}
                Err(e) => {
                    // The failure could not be durably recorded — the
                    // session's state is untrustworthy; isolate it.
                    tracing::warn!("[continuation] settle-after-send-failure failed: {e}");
                    self.isolate_on_storage_failure(&turn_id, &session_id, Some(&connection_id))
                        .await;
                    return;
                }
            }
            self.release_connection(&turn_id, &connection_id).await;
            self.settle_notify.notify_waiters();
            return;
        }

        // dispatching → running. A cancel that raced in keeps
        // `cancel_requested`; the drive no longer owns the terminal.
        let running = matches!(
            collaboration_service::cas_turn_state(
                conn,
                &turn_id,
                Some(&execution_id),
                &["dispatching"],
                "running",
                false,
            )
            .await,
            Ok(true)
        );
        if !running {
            // A cancel (or settle) won between dispatch and running: forward
            // the cancel to the agent, best-effort. The confirmation path
            // owns the terminal from here.
            let _ = self.runtime.cancel(&connection_id).await;
        }
    }

    // -----------------------------------------------------------------------
    // settle
    // -----------------------------------------------------------------------

    /// Deliver a terminal for a turn from its EXECUTION OWNER (lifecycle
    /// routing or a test). Returns `applied` when this caller won the
    /// terminal; `stale` when the execution is superseded or the turn is
    /// already terminal — the stored result is never overwritten.
    pub async fn settle(
        &self,
        turn_id: &str,
        execution_id: &str,
        terminal: TurnTerminal,
    ) -> Result<bool, ContinuationError> {
        let payload = match &terminal {
            TurnTerminal::Completed { text } => {
                let capped = cap_turn_result_text(text);
                let truncated = capped.len() != text.len();
                SettlePayload {
                    to_state: "completed",
                    result_text: Some(capped),
                    text_truncated: truncated,
                    error_code: None,
                    error_message: None,
                }
            }
            TurnTerminal::Failed { code, message } => SettlePayload {
                to_state: "failed",
                result_text: None,
                text_truncated: false,
                error_code: Some(code.clone()),
                error_message: Some(message.clone()),
            },
            TurnTerminal::Canceled => SettlePayload {
                to_state: "canceled",
                result_text: None,
                text_truncated: false,
                error_code: Some("canceled".to_string()),
                error_message: None,
            },
        };
        let applied = collaboration_service::settle_turn(&self.db.conn, turn_id, execution_id, payload)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?;

        if applied {
            // outcome_unknown keeps its meaning only if the session is
            // blocked accordingly; every other terminal leaves the session
            // usable (state open). Handle disconnect/release + session state.
            if let Some(connection_id) = self.connection_of_turn(turn_id).await {
                self.release_connection(turn_id, &connection_id).await;
            }
            if matches!(terminal, TurnTerminal::Completed { .. } | TurnTerminal::Failed { .. } | TurnTerminal::Canceled) {
                // nothing extra — the session stays open for the next round
            }
            self.settle_notify.notify_waiters();
        }
        Ok(applied)
    }

    /// A cancel-without-confirmation timeout / hard disconnect: the outcome
    /// is UNKNOWABLE — settle `outcome_unknown` and block the session IN ONE
    /// TRANSACTION (acceptance F5), so no admission can slip between and a
    /// crash cannot leave unknown+open.
    ///
    /// The settle also takes the source's per-source admission lock
    /// (reacceptance R5): unknown settlement and `continue_turn` admission
    /// share one serialization boundary, so an admission that already read
    /// the session as `open` cannot insert a fresh round after this write
    /// commits (and the transactional re-validation inside `admit_turn`
    /// covers any residual interleaving).
    pub async fn settle_unknown(
        &self,
        turn_id: &str,
        execution_id: &str,
    ) -> Result<bool, ContinuationError> {
        // Resolve the source for the admission lock; a vanished turn/session
        // settles nothing (same outcome the CAS below would produce).
        let source_task_id = match collaboration_service::find_turn(&self.db.conn, turn_id).await {
            Ok(Some(turn)) => {
                match collaboration_service::find_session_by_id(&self.db.conn, &turn.session_id)
                    .await
                {
                    Ok(Some(session)) => Some(session.source_task_id),
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!("[continuation] unknown-settle session lookup failed: {e}");
                        return Err(storage_unavailable());
                    }
                }
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("[continuation] unknown-settle turn lookup failed: {e}");
                return Err(storage_unavailable());
            }
        };
        let _guard = match source_task_id.as_deref() {
            Some(source) => {
                let admission = self
                    .admission_locks
                    .lock()
                    .await
                    .entry(source.to_string())
                    .or_default()
                    .clone();
                // lock_owned: the guard must not borrow the local Arc (which
                // leaves this match arm otherwise).
                Some(admission.lock_owned().await)
            }
            None => None,
        };
        let applied = collaboration_service::settle_unknown_and_block(
            &self.db.conn,
            turn_id,
            execution_id,
            "outcome_unknown",
            "the host cannot prove whether the agent received or acted on the \
             rework prompt; inspect the actual artifacts",
        )
        .await
        .map_err(|e| {
            tracing::warn!("[continuation] unknown-settle failed: {e}");
            storage_unavailable()
        })?;
        if applied {
            if let Some(connection_id) = self.connection_of_turn(turn_id).await {
                self.release_connection(turn_id, &connection_id).await;
            }
            self.settle_notify.notify_waiters();
        }
        Ok(applied)
    }

    async fn settle_failure(
        &self,
        turn_id: &str,
        execution_id: &str,
        err: &ContinuationError,
    ) {
        let payload = SettlePayload {
            to_state: "failed",
            result_text: None,
            text_truncated: false,
            error_code: Some(err.error_code.as_str().to_string()),
            error_message: Some(err.message.clone()),
        };
        if let Err(e) =
            collaboration_service::settle_turn(&self.db.conn, turn_id, execution_id, payload).await
        {
            tracing::warn!("[continuation] failed to persist attach failure for {turn_id}: {e}");
        }
        self.settle_notify.notify_waiters();
    }

    /// The session's persistence is untrustworthy (a required state write
    /// failed): do NOT publish anything authoritative, do NOT send, isolate.
    async fn isolate_on_storage_failure(
        &self,
        turn_id: &str,
        session_id: &str,
        connection_id: Option<&str>,
    ) {
        tracing::warn!(
            "[continuation] storage failure around turn {turn_id}; isolating the session \
             (no authoritative result will be published, nothing re-sent)"
        );
        if let Some(connection_id) = connection_id {
            self.release_connection(turn_id, connection_id).await;
        }
        // The turn row may itself be unreadable (that is often WHY we are
        // here) — block the session via its id, which the caller owns.
        if let Ok(Some(session)) =
            collaboration_service::find_session_by_id(&self.db.conn, session_id).await
        {
            if session.state != "closed" {
                let _ = collaboration_service::set_session_state(
                    &self.db.conn,
                    session_id,
                    "blocked",
                )
                .await;
            }
        }
        self.settle_notify.notify_waiters();
    }

    /// Release a round's connection at terminal/close. The ownership record
    /// is removed ONLY after the disconnect actually succeeded (acceptance
    /// F8): a failed disconnect leaves the owner entry in place with a
    /// warning, so `close_session` can still see — and report — the resource
    /// instead of claiming a release that never happened.
    async fn release_connection(&self, turn_id: &str, connection_id: &str) {
        match self.runtime.disconnect(connection_id).await {
            Ok(()) => {
                self.executions.lock().await.remove(connection_id);
            }
            Err(e) => {
                tracing::warn!(
                    "[continuation] disconnect FAILED for turn {turn_id} (connection                      {connection_id}): {e}; the ownership record is KEPT so close can                      retry and must not report this resource as released"
                );
            }
        }
        let _ = turn_id;
    }

    // -----------------------------------------------------------------------
    // get_session_turn_status
    // -----------------------------------------------------------------------

    pub async fn get_turn(
        &self,
        parent: VerifiedParent,
        turn_id: &str,
        wait_ms: u64,
    ) -> Result<TurnReport, ContinuationError> {
        let deadline = (wait_ms > 0).then(|| std::time::Instant::now() + Duration::from_millis(wait_ms));
        loop {
            let report = self.load_report(parent, turn_id).await?;
            if !report.state.is_active() || deadline.is_none() {
                return Ok(report);
            }
            let Some(deadline) = deadline else {
                return Ok(report);
            };
            if std::time::Instant::now() >= deadline {
                return Ok(report);
            }
            let notified = self.settle_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            tokio::select! {
                _ = &mut notified => {}
                _ = tokio::time::sleep_until(deadline.into()) => {}
            }
            // Loop: re-load (a settle likely landed).
        }
    }

    async fn load_report(
        &self,
        parent: VerifiedParent,
        turn_id: &str,
    ) -> Result<TurnReport, ContinuationError> {
        let turn = collaboration_service::find_turn(&self.db.conn, turn_id)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
            .ok_or_else(|| ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no such turn",
            ))?;
        let session = collaboration_service::find_session_by_id(&self.db.conn, &turn.session_id)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
            .ok_or_else(|| ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no such turn",
            ))?;
        if session.parent_conversation_id != parent.conversation_id {
            // Cross-parent: opaque, no existence leak.
            return Err(ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no such turn",
            ));
        }
        let state = TurnState::parse(&turn.state).unwrap_or(TurnState::OutcomeUnknown);
        let blocked_on = if state.is_active() {
            self.blocked_probe(&turn).await
        } else {
            None
        };
        Ok(TurnReport {
            schema_version: SchemaVersion1,
            session_id: session.id,
            turn_id: turn.id,
            source_task_id: session.source_task_id,
            ordinal: turn.ordinal,
            state,
            message: turn.message,
            initiator_kind: turn.initiator_kind,
            initiator_parent_conversation_id: turn.initiator_parent_conversation_id,
            result_text: turn.result_text,
            text_truncated: turn.text_truncated,
            error_code: turn.error_code,
            error_message: turn.error_message,
            blocked_on,
            created_at: turn.created_at,
            started_at: turn.started_at,
            finished_at: turn.finished_at,
            version: turn.version,
        })
    }

    async fn blocked_probe(
        &self,
        turn: &crate::db::entities::collaboration_turn::Model,
    ) -> Option<String> {
        let connection_id = turn.connection_id.as_deref()?;
        self.runtime.blocked_on(connection_id).await
    }

    // -----------------------------------------------------------------------
    // cancel_session_turn
    // -----------------------------------------------------------------------

    pub async fn cancel_turn(
        &self,
        parent: VerifiedParent,
        turn_id: &str,
    ) -> Result<TurnReport, ContinuationError> {
        // Ownership first: the same opaque error for unknown and foreign.
        let _report = self.load_report(parent, turn_id).await?;
        let mut turn = collaboration_service::find_turn(&self.db.conn, turn_id)
            .await
            .map_err(|e| {
                tracing::warn!("[continuation] cancel lookup failed: {e}");
                storage_unavailable()
            })?
            .ok_or_else(|| {
                ContinuationError::new(
                    ContinuationErrorCode::NotFoundOrForbidden,
                    "no such turn",
                )
            })?;

        // The drive advances states concurrently; a CAS can lose or even read
        // a stale row. Re-read and re-decide (bounded) instead of swallowing
        // the mismatch (acceptance F7). Every iteration decides on the state
        // of the FRESHLY READ row (reacceptance R3): a snapshot taken while
        // the round was still `preparing` must not pin the decision to the
        // pre-send branch after the drive has moved it to running.
        for _ in 0..3 {
            let state = TurnState::parse(&turn.state).unwrap_or(TurnState::OutcomeUnknown);
            match state {
                TurnState::Accepted | TurnState::Preparing => {
                    // Not sent yet — CAS straight to canceled; the drive
                    // observes the loss of its own CAS and never sends.
                    let applied = collaboration_service::cas_turn_state(
                        &self.db.conn,
                        turn_id,
                        Some(&turn.execution_id),
                        &["accepted", "preparing"],
                        "canceled",
                        false,
                    )
                    .await
                    .map_err(|e| {
                        tracing::warn!("[continuation] cancel CAS failed: {e}");
                        storage_unavailable()
                    })?;
                    if applied {
                        break;
                    }
                    // Lost: re-read and re-decide — the drive may have moved
                    // the turn to dispatching/running in the meantime.
                    turn = self.reread_for_cancel(turn_id).await?;
                }
                TurnState::Dispatching | TurnState::Running | TurnState::CancelRequested => {
                    // Sent (or about to be): request cancellation and wait
                    // for the confirmation (or its deadline) to settle.
                    let applied = collaboration_service::cas_turn_state(
                        &self.db.conn,
                        turn_id,
                        Some(&turn.execution_id),
                        &["dispatching", "running"],
                        "cancel_requested",
                        false,
                    )
                    .await
                    .map_err(|e| {
                        tracing::warn!("[continuation] cancel CAS failed: {e}");
                        storage_unavailable()
                    })?;
                    if applied || turn.state == "cancel_requested" {
                        if let Some(connection_id) = turn.connection_id.clone() {
                            let _ = self.runtime.cancel(&connection_id).await;
                        }
                        // Arm the confirmation deadline ONCE per transition:
                        // a lost confirmation must not wedge the session.
                        self.arm_cancel_deadline(&turn.id, &turn.execution_id);
                        break;
                    }
                    // Lost to a racing terminal (or a state we no longer
                    // recognize) — re-read once and decide again.
                    turn = self.reread_for_cancel(turn_id).await?;
                }
                _ => {
                    // Terminal already — nothing to cancel.
                    break;
                }
            }
        }
        self.load_report(parent, turn_id).await
    }

    /// The bounded cancel-retry loop's re-read: a fresh turn row or the
    /// honest storage error (never a silently fabricated "lost" CAS).
    async fn reread_for_cancel(
        &self,
        turn_id: &str,
    ) -> Result<crate::db::entities::collaboration_turn::Model, ContinuationError> {
        collaboration_service::find_turn(&self.db.conn, turn_id)
            .await
            .map_err(|e| {
                tracing::warn!("[continuation] cancel re-read failed: {e}");
                storage_unavailable()
            })?
            .ok_or_else(|| {
                ContinuationError::new(ContinuationErrorCode::NotFoundOrForbidden, "no such turn")
            })
    }

    /// Arm the one-shot deadline that settles an unconfirmed
    /// `cancel_requested` round as `outcome_unknown` + blocked. Production
    /// wires this where the agent's stop confirmation (or disconnect)
    /// settles the turn; without the deadline a lost confirmation would
    /// wedge the session in an active state forever.
    fn arm_cancel_deadline(&self, turn_id: &str, execution_id: &str) {
        let coordinator = self.clone();
        let turn_id = turn_id.to_string();
        let execution_id = execution_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(CANCEL_CONFIRMATION_TIMEOUT).await;
            // settle_unknown is CAS-guarded by execution id AND active state:
            // if the confirmation already settled the turn, this is a no-op.
            match coordinator.settle_unknown(&turn_id, &execution_id).await {
                Ok(applied) => {
                    if applied {
                        tracing::warn!(
                            "[continuation] cancel confirmation for {turn_id} timed out; \
                             settled outcome_unknown and blocked the session"
                        );
                    }
                }
                Err(e) => tracing::warn!(
                    "[continuation] cancel-deadline settle failed for {turn_id}: {e}"
                ),
            }
        });
    }

    // -----------------------------------------------------------------------
    // close_session
    // -----------------------------------------------------------------------

    pub async fn close_session(
        &self,
        parent: VerifiedParent,
        session_id: &str,
    ) -> Result<SessionSummary, ContinuationError> {
        let session = collaboration_service::find_session_by_id(&self.db.conn, session_id)
            .await
            .map_err(|e| {
                tracing::warn!("[continuation] close lookup failed: {e}");
                storage_unavailable()
            })?
            .ok_or_else(|| {
                ContinuationError::new(
                    ContinuationErrorCode::NotFoundOrForbidden,
                    "no such session",
                )
            })?;
        if session.parent_conversation_id != parent.conversation_id {
            return Err(ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no such session",
            ));
        }
        if session.state != "closed" {
            // Take the SAME per-source admission lock `continue_turn` uses
            // (acceptance F6): a continue admitted between close's active
            // check and its write would otherwise produce a CLOSED session
            // with a LIVE turn — the one interleaving the write reservation
            // must never allow.
            let admission = self
                .admission_locks
                .lock()
                .await
                .entry(session.source_task_id.clone())
                .or_default()
                .clone();
            let _guard = admission.lock().await;

            // Re-read BOTH facts under the lock: the session may have been
            // closed concurrently, and a continue may have inserted a turn.
            let session = collaboration_service::find_session_by_id(&self.db.conn, session_id)
                .await
                .map_err(|e| {
                    tracing::warn!("[continuation] close re-read failed: {e}");
                    storage_unavailable()
                })?
                .ok_or_else(|| {
                    ContinuationError::new(
                        ContinuationErrorCode::NotFoundOrForbidden,
                        "no such session",
                    )
                })?;
            if session.state != "closed" {
                if collaboration_service::active_turn(&self.db.conn, session_id)
                    .await
                    .map_err(|e| {
                        tracing::warn!("[continuation] close active-check failed: {e}");
                        storage_unavailable()
                    })?
                    .is_some()
                {
                    return Err(ContinuationError::new(
                        ContinuationErrorCode::SessionBusy,
                        "a turn is still active; cancel it and query its terminal \
                         before closing"
                            .to_string(),
                    )
                    .with_ids(Some(session_id), None));
                }
                // Release BEFORE writing closed (reacceptance R6): the write
                // reservation is what keeps ordinary writers out of the child
                // while the platform still holds its connection. Writing
                // `closed` first and failing the release afterwards would
                // hand the child back to ordinary prompts with a live
                // coordinator-owned connection still attached. A failed
                // release therefore leaves the session open/blocked (the
                // reservation stands) and reports the failure honestly.
                let held: Vec<String> = {
                    let executions = self.executions.lock().await;
                    executions
                        .values()
                        .filter(|o| o.session_id == session_id)
                        .map(|o| o.turn_id.clone())
                        .collect()
                };
                for turn_id in held {
                    if let Some(connection_id) = self.connection_of_turn(&turn_id).await {
                        self.release_connection(&turn_id, &connection_id)
                            .await;
                        if self.connection_of_turn(&turn_id).await.is_some() {
                            // Still registered → the disconnect failed again.
                            // The session is NOT closed and the write
                            // reservation is intact; retry close later.
                            return Err(ContinuationError::new(
                                ContinuationErrorCode::SessionBusy,
                                "a connection from a finished round could not be \
                                 released; the session stays reserved — retry close"
                                    .to_string(),
                            )
                            .with_ids(Some(session_id), Some(&turn_id)));
                        }
                    }
                }
                collaboration_service::set_session_state(&self.db.conn, session_id, "closed")
                    .await
                    .map_err(|e| {
                        tracing::warn!("[continuation] close write failed: {e}");
                        storage_unavailable()
                    })?;
            }
        }
        let session = collaboration_service::find_session_by_id(&self.db.conn, session_id)
            .await
            .map_err(|e| {
                tracing::warn!("[continuation] close final-read failed: {e}");
                storage_unavailable()
            })?
            .ok_or_else(storage_unavailable)?;
        Ok(SessionSummary {
            schema_version: SchemaVersion1,
            session_id: session.id,
            source_task_id: session.source_task_id,
            child_conversation_id: session.child_conversation_id,
            state: CollaborationSessionState::parse(&session.state)
                .unwrap_or(CollaborationSessionState::Open),
        })
    }

    // -----------------------------------------------------------------------
    // startup recovery
    // -----------------------------------------------------------------------

    /// Scan the collaboration tables AFTER a host restart and bring every
    /// turn to its recovery-table state. MUST run before new requests are
    /// accepted. Never re-sends anything.
    pub async fn recover_on_startup(&self) -> Result<collaboration_service::RecoverySummary, ContinuationError> {
        collaboration_service::recover_on_startup(&self.db.conn)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })
    }

    /// The session summary for the read-only UI snapshot (null when the
    /// source has no session — the UI treats that as "no collaboration yet").
    pub async fn session_summary_for_source(
        &self,
        parent: VerifiedParent,
        source_task_id: &str,
    ) -> Result<Option<SessionSummary>, ContinuationError> {
        let session = collaboration_service::find_session_by_source(&self.db.conn, source_task_id)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?;
        match session {
            None => Ok(None),
            Some(session) if session.parent_conversation_id != parent.conversation_id => Ok(None),
            Some(session) => Ok(Some(SessionSummary {
                schema_version: SchemaVersion1,
                session_id: session.id,
                source_task_id: session.source_task_id,
                child_conversation_id: session.child_conversation_id,
                state: CollaborationSessionState::parse(&session.state)
                    .unwrap_or(CollaborationSessionState::Open),
            })),
        }
    }

    /// Read-only turn listing for the UI projection (Task 5): turns ordered
    /// by ordinal after `after_ordinal`, scoped to the owning parent.
    pub async fn list_turns_for_session(
        &self,
        parent: VerifiedParent,
        session_id: &str,
        after_ordinal: i32,
        limit: u64,
    ) -> Result<Option<Vec<TurnReport>>, ContinuationError> {
        let session = collaboration_service::find_session_by_id(&self.db.conn, session_id)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?;
        let Some(session) = session else {
            return Ok(None);
        };
        if session.parent_conversation_id != parent.conversation_id {
            return Ok(None);
        }
        let turns = collaboration_service::list_turns(&self.db.conn, session_id, after_ordinal, limit)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?;
        let mut reports = Vec::with_capacity(turns.len());
        for turn in turns {
            let state = TurnState::parse(&turn.state).unwrap_or(TurnState::OutcomeUnknown);
            reports.push(TurnReport {
                schema_version: SchemaVersion1,
                session_id: session.id.clone(),
                turn_id: turn.id,
                source_task_id: session.source_task_id.clone(),
                ordinal: turn.ordinal,
                state,
                message: turn.message,
                initiator_kind: turn.initiator_kind,
                initiator_parent_conversation_id: turn.initiator_parent_conversation_id,
                result_text: turn.result_text,
                text_truncated: turn.text_truncated,
                error_code: turn.error_code,
                error_message: turn.error_message,
                blocked_on: None,
                created_at: turn.created_at,
                started_at: turn.started_at,
                finished_at: turn.finished_at,
                version: turn.version,
            });
        }
        Ok(Some(reports))
    }

    /// Parent connection went away (disconnect / teardown): cancel every
    /// round it owns and settle each to a safe terminal. Covers BOTH phases:
    /// * accepted/preparing (registered in `parent_pending`, nothing sent —
    ///   CAS straight to `canceled`; the drive observes the lost CAS and
    ///   never attaches/sends), and
    /// * attached rounds (`executions`): the agent gets a best-effort cancel
    ///   and the round moves to `cancel_requested` under the SAME
    ///   confirmation deadline `cancel_turn` arms — a clean `canceled`
    ///   terminal requires the agent's own stop event (via the lifecycle);
    ///   an undeliverable stop or an expired deadline settles
    ///   `outcome_unknown` and blocks the session. Enqueuing the cancel
    ///   command is never treated as the agent's stop confirmation
    ///   (reacceptance R4).
    pub async fn cancel_by_parent_connection(&self, parent_connection_id: &str) {
        // Phase 1: accepted/preparing rounds — nothing sent, plain cancel.
        let pending_rounds: Vec<(String, String)> = {
            let mut pending = self.parent_pending.lock().await;
            pending.remove(parent_connection_id).unwrap_or_default()
        };
        for (turn_id, session_id) in pending_rounds {
            let applied = collaboration_service::cas_turn_state(
                &self.db.conn,
                &turn_id,
                None,
                &collaboration_service::ACTIVE_TURN_STATES,
                "canceled",
                false,
            )
            .await
            .unwrap_or(false);
            if !applied {
                // The drive won a race (already dispatched or attached): it is
                // in `executions` now — nothing to do here, the phase-2 sweep
                // below (running after phase 1) can't see it this call. Mark
                // it by leaving a tombstone? Simplest honest path: re-add so
                // the sweep below re-examines it.
                let mut pending = self.parent_pending.lock().await;
                pending
                    .entry(parent_connection_id.to_string())
                    .or_default()
                    .push((turn_id.clone(), session_id.clone()));
            }
        }

        // Phase 2: attached rounds.
        let owned: Vec<(String, String)> = {
            let executions = self.executions.lock().await;
            executions
                .values()
                .filter(|o| o.parent_connection_id == parent_connection_id)
                .map(|o| (o.turn_id.clone(), o.execution_id.clone()))
                .collect()
        };
        for (turn_id, execution_id) in owned {
            let cancel_enqueued = match self.connection_of_turn(&turn_id).await {
                Some(connection_id) => self.runtime.cancel(&connection_id).await.is_ok(),
                None => false,
            };
            if !cancel_enqueued {
                // The stop could not be delivered: the outcome is unknowable.
                let _ = self.settle_unknown(&turn_id, &execution_id).await;
            } else {
                // A successful enqueue is NOT the agent's stop confirmation
                // (reacceptance R4): the manager's cancel only hands the
                // command to the connection's queue. Move the round to
                // `cancel_requested` and arm the SAME confirmation deadline
                // `cancel_turn` uses — the agent's terminal settles the
                // outcome (canceled via lifecycle, or outcome_unknown +
                // blocked when the deadline expires). Never fabricate a
                // clean stop from a transport-level Ok.
                let requested = collaboration_service::cas_turn_state(
                    &self.db.conn,
                    &turn_id,
                    Some(&execution_id),
                    &["dispatching", "running"],
                    "cancel_requested",
                    false,
                )
                .await
                .unwrap_or(false);
                if requested {
                    self.arm_cancel_deadline(&turn_id, &execution_id);
                }
                // If the CAS lost, a racing terminal already settled the
                // round (and its own path released the connection) — or the
                // round was already `cancel_requested` and a deadline is
                // armed; re-arming is unnecessary.
            }
            self.settle_notify.notify_waiters();
        }

        // Phase 3: re-sweep the phase-1 re-adds now that the executions sweep
        // has run (they either appear in `executions` — handled above — or
        // were genuinely lost races that are now terminal via the drive's own
        // CAS-failure release; a final CAS attempt is idempotent).
        let leftover: Vec<(String, String)> = {
            let mut pending = self.parent_pending.lock().await;
            pending.remove(parent_connection_id).unwrap_or_default()
        };
        for (turn_id, session_id) in leftover {
            let _ = collaboration_service::cas_turn_state(
                &self.db.conn,
                &turn_id,
                None,
                &collaboration_service::ACTIVE_TURN_STATES,
                "canceled",
                false,
            )
            .await;
            let _ = session_id;
        }
    }

    /// The session's active turn as a full report (`None` when idle) — the
    /// read-only UI uses it to keep the live round visible across pagination.
    pub async fn active_turn_of_session(
        &self,
        session_id: &str,
    ) -> Result<Option<TurnReport>, ContinuationError> {
        let Some(active) = collaboration_service::active_turn(&self.db.conn, session_id)
            .await
            .map_err(|e| {
                tracing::warn!("[continuation] active-turn lookup failed: {e}");
                storage_unavailable()
            })?
        else {
            return Ok(None);
        };
        let state = TurnState::parse(&active.state).unwrap_or(TurnState::OutcomeUnknown);
        Ok(Some(TurnReport {
            schema_version: SchemaVersion1,
            session_id: active.session_id.clone(),
            turn_id: active.id.clone(),
            source_task_id: String::new(), // caller joins via the session summary
            ordinal: active.ordinal,
            state,
            message: active.message.clone(),
            initiator_kind: active.initiator_kind.clone(),
            initiator_parent_conversation_id: active.initiator_parent_conversation_id,
            result_text: active.result_text.clone(),
            text_truncated: active.text_truncated,
            error_code: active.error_code.clone(),
            error_message: active.error_message.clone(),
            blocked_on: None,
            created_at: active.created_at,
            started_at: active.started_at,
            finished_at: active.finished_at,
            version: active.version,
        }))
    }

    /// The collaboration session holding this child conversation, if any
    /// (lifecycle routing consults it to quarantine stale events for
    /// reserved children).
    pub async fn session_summary_for_child(
        &self,
        child_conversation_id: i32,
    ) -> Option<SessionSummary> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let row = collaboration_session::Entity::find()
            .filter(collaboration_session::Column::ChildConversationId.eq(child_conversation_id))
            .one(&self.db.conn)
            .await
            .ok()??;
        Some(SessionSummary {
            schema_version: SchemaVersion1,
            session_id: row.id,
            source_task_id: row.source_task_id,
            child_conversation_id: row.child_conversation_id,
            state: CollaborationSessionState::parse(&row.state)
                .unwrap_or(CollaborationSessionState::Open),
        })
    }

    /// The reservation check ordinary write paths consult (Task 4 wiring):
    /// is this child conversation currently reserved by an open/blocked
    /// collaboration session?
    pub async fn child_is_reserved(&self, child_conversation_id: i32) -> bool {
        collaboration_service::child_is_reserved(&self.db.conn, child_conversation_id)
            .await
            .unwrap_or(false)
    }
}

fn turn_ack(source_task_id: &str, turn: &crate::db::entities::collaboration_turn::Model) -> TurnAck {
    TurnAck {
        schema_version: SchemaVersion1,
        session_id: turn.session_id.clone(),
        turn_id: turn.id.clone(),
        source_task_id: source_task_id.to_string(),
        ordinal: turn.ordinal,
        state: TurnState::parse(&turn.state).unwrap_or(TurnState::Accepted),
    }
}

fn storage_unavailable() -> ContinuationError {
    ContinuationError::new(
        ContinuationErrorCode::StorageUnavailable,
        "the collaboration store is unavailable; nothing was accepted",
    )
}

/// Keep imports honest when compiled without test wiring.
#[allow(dead_code)]
fn _shape_asserts(coordinator: &ContinuationCoordinator, session: &collaboration_session::Model) {
    let _ = (coordinator, session.state.clone());
    let _ = NoopRuntime;
}
