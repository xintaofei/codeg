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
#[derive(Debug, Clone)]
struct ExecutionOwner {
    turn_id: String,
    execution_id: String,
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
    /// Per-source-task admission locks: two concurrent continue_turn calls
    /// for the same source serialize here before touching the DB.
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
            admission_locks: Arc::new(Mutex::new(HashMap::new())),
            settle_notify: Arc::new(Notify::new()),
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_runtime(mut self, runtime: Arc<dyn ContinuationRuntime>) -> Self {
        self.runtime = runtime;
        self
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

        let ordinal = collaboration_service::next_ordinal(&self.db.conn, &session.id)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?;
        let turn = collaboration_service::insert_turn(
            &self.db.conn,
            collaboration_service::NewTurn {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session.id.clone(),
                ordinal,
                request_id: request_id.to_string(),
                message: message.to_string(),
                initiator_parent_conversation_id: parent.conversation_id,
                initiator_tool_use_id: initiator_tool_use_id
                    .filter(|id| !id.trim().is_empty())
                    .map(str::to_string),
            },
        )
        .await
        .map_err(|e| {
            // The partial unique index / request uniqueness fired: another
            // admission won the race.
            tracing::info!("[continuation] admission lost a DB race: {e}");
            ContinuationError::new(
                ContinuationErrorCode::SessionBusy,
                "another turn is already active for this session",
            )
            .with_ids(Some(&session.id), None)
        })?;

        let ack = turn_ack(source_task_id, &turn);

        // --- Background drive ---------------------------------------------------
        let coordinator = self.clone();
        let turn_id = turn.id.clone();
        let execution_id = turn.execution_id.clone();
        let message_owned = message.to_string();
        let session_id_owned = session.id.clone();
        let parent_conn_owned = parent_connection_id.to_string();
        tokio::spawn(async move {
            coordinator
                .drive(
                    turn_id,
                    execution_id,
                    session_id_owned,
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

    async fn drive(
        &self,
        turn_id: String,
        execution_id: String,
        session_id: String,
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
            Ok(false) => return, // canceled/lost — nothing to do
            Err(e) => {
                // A CAS that ERRORS (not just loses) means the store is
                // untrustworthy: no send may happen and the session must be
                // isolated.
                tracing::warn!("[continuation] preparing CAS failed for {turn_id}: {e}");
                self.isolate_on_storage_failure(&turn_id, &session_id, None)
                    .await;
                return;
            }
        }

        // Strict attach — the ONLY path into the child session.
        let connection_id = match self
            .runtime
            .attach_strict(&binding, &turn_id, &execution_id)
            .await
        {
            Ok(conn_id) => conn_id,
            Err(err) => {
                let mapped = ContinuationError::from(err);
                self.settle_failure(&turn_id, &execution_id, &mapped).await;
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
                parent_connection_id,
            },
        );
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
    /// is UNKNOWABLE — settle `outcome_unknown` and block the session.
    pub async fn settle_unknown(&self, turn_id: &str, execution_id: &str) -> Result<bool, ContinuationError> {
        let applied = collaboration_service::settle_turn(
            &self.db.conn,
            turn_id,
            execution_id,
            SettlePayload {
                to_state: "outcome_unknown",
                result_text: None,
                text_truncated: false,
                error_code: Some("outcome_unknown".to_string()),
                error_message: Some(
                    "the host cannot prove whether the agent received or acted on the \
                     rework prompt; inspect the actual artifacts"
                        .to_string(),
                ),
            },
        )
        .await
        .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?;
        if applied {
            // Block the session: no new rounds until a human closes it.
            if let Some(turn) = collaboration_service::find_turn(&self.db.conn, turn_id)
                .await
                .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
            {
                if let Some(session) =
                    collaboration_service::find_session_by_id(&self.db.conn, &turn.session_id)
                        .await
                        .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
                {
                    if session.state != "closed" {
                        let _ = collaboration_service::set_session_state(
                            &self.db.conn,
                            &session.id,
                            "blocked",
                        )
                        .await;
                    }
                }
            }
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

    async fn release_connection(&self, turn_id: &str, connection_id: &str) {
        self.executions.lock().await.remove(connection_id);
        let _ = self.runtime.disconnect(connection_id).await;
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
        let report = self.load_report(parent, turn_id).await?;
        let turn = collaboration_service::find_turn(&self.db.conn, turn_id)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
            .ok_or_else(|| ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no such turn",
            ))?;

        match report.state {
            TurnState::Accepted | TurnState::Preparing => {
                // Not sent yet — CAS straight to canceled; the drive observes
                // the loss of its CAS and never attaches/sends.
                let _ = collaboration_service::cas_turn_state(
                    &self.db.conn,
                    turn_id,
                    Some(&turn.execution_id),
                    &["accepted", "preparing"],
                    "canceled",
                    false,
                )
                .await;
                self.settle_notify.notify_waiters();
            }
            TurnState::Dispatching | TurnState::Running | TurnState::CancelRequested => {
                // Sent (or about to be): request cancellation and wait for
                // the confirmation (or its timeout) to settle.
                let applied = collaboration_service::cas_turn_state(
                    &self.db.conn,
                    turn_id,
                    Some(&turn.execution_id),
                    &["dispatching", "running"],
                    "cancel_requested",
                    false,
                )
                .await
                .unwrap_or(false);
                if applied || report.state == TurnState::CancelRequested {
                    if let Some(connection_id) = turn.connection_id.clone() {
                        let _ = self.runtime.cancel(&connection_id).await;
                    }
                }
            }
            _ => {
                // Terminal already — nothing to cancel; return the report.
            }
        }
        self.load_report(parent, turn_id).await
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
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
            .ok_or_else(|| ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no such session",
            ))?;
        if session.parent_conversation_id != parent.conversation_id {
            return Err(ContinuationError::new(
                ContinuationErrorCode::NotFoundOrForbidden,
                "no such session",
            ));
        }
        if session.state != "closed" {
            // A session with an active turn cannot close — cancel and settle
            // first.
            if collaboration_service::active_turn(&self.db.conn, session_id)
                .await
                .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
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
            // Release any connection the platform still holds for this
            // session (e.g. left over from a canceled round). A release
            // failure must NOT be reported as a successful release.
            // (Connections are registered per execution; by the time no turn
            // is active, the drive has released its own — nothing to do here
            // beyond marking the session closed.)
            collaboration_service::set_session_state(&self.db.conn, session_id, "closed")
                .await
                .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?;
        }
        let session = collaboration_service::find_session_by_id(&self.db.conn, session_id)
            .await
            .map_err(|e| { tracing::warn!("[continuation] storage op failed: {e}"); storage_unavailable() })?
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
    /// turn it owns and settle them canceled. The relationship survives in a
    /// safe terminal state; nothing escapes the parent cleanup boundary.
    pub async fn cancel_by_parent_connection(&self, parent_connection_id: &str) {
        let owned: Vec<(String, String, String)> = {
            let executions = self.executions.lock().await;
            executions
                .values()
                .filter(|o| o.parent_connection_id == parent_connection_id)
                .map(|o| (o.turn_id.clone(), o.execution_id.clone(), o.turn_id.clone()))
                .collect()
        };
        for (turn_id, execution_id, _) in owned {
            // Best-effort cancel of the in-flight agent turn, then settle:
            // the parent is gone, so the round is over from the platform's
            // point of view.
            if let Some(connection_id) = self.connection_of_turn(&turn_id).await {
                let _ = self.runtime.cancel(&connection_id).await;
            }
            let _ = collaboration_service::cas_turn_state(
                &self.db.conn,
                &turn_id,
                Some(&execution_id),
                &collaboration_service::ACTIVE_TURN_STATES,
                "canceled",
                false,
            )
            .await;
            if let Some(connection_id) = self.connection_of_turn(&turn_id).await {
                self.release_connection(&turn_id, &connection_id).await;
            }
            self.settle_notify.notify_waiters();
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
            session_id: active.session_id,
            turn_id: active.id,
            source_task_id: String::new(), // caller joins via the session summary
            ordinal: active.ordinal,
            state,
            message: active.message,
            initiator_kind: active.initiator_kind,
            initiator_parent_conversation_id: active.initiator_parent_conversation_id,
            result_text: active.result_text,
            text_truncated: active.text_truncated,
            error_code: active.error_code,
            error_message: active.error_message,
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
