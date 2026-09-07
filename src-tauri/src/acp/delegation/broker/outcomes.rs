use std::sync::Arc;

use async_trait::async_trait;

use crate::acp::delegation::meta_writer::is_synthetic_parent_tool_use_id;
use crate::acp::delegation::spawner::ResumeBindingFacts;
use crate::acp::delegation::types::{
    DelegationOutcome, DelegationTaskReport, TaskStatus,
};
use crate::db::service::delegation_outcome_service::{
    DelegationOutcomeInsert, DelegationOutcomeRow, OutcomeWriteResult,
};
use crate::models::AgentType;

use super::{cap_completed_text, DelegationBroker};

/// Immutable store of COMPLETED delegation results (v2 design §4.1). One row
/// per `task_id`, written when the task's current execution wins the terminal
/// race with a SUCCESSFUL outcome. Canceled/failed terminals are deliberately
/// never written: a canceled task keeps the upstream `resume_delegation` path
/// under the same id, and freezing one would block that resume forever.
///
/// Abstracted like [`super::ChildStatusLookup`] so broker tests run without
/// SeaORM; production wires [`DbDelegationOutcomeStore`] via
/// [`DelegationBroker::with_outcome_store`]. The default [`NoopOutcomeStore`]
/// keeps tests that don't exercise durability unchanged.
#[async_trait]
pub trait DelegationOutcomeStore: Send + Sync {
    /// First-writer-wins insert. `Conflict` means a different success result
    /// was already frozen for this task; the original always stands.
    async fn insert_once(
        &self,
        record: DelegationOutcomeInsert,
    ) -> Result<OutcomeWriteResult, String>;

    /// The frozen result for `task_id` IF it belongs to `parent_conversation_id`.
    /// Query errors and "no row" are distinct: an error must never be read as
    /// "the task has no frozen result".
    async fn find_owned(
        &self,
        parent_conversation_id: i32,
        task_id: &str,
    ) -> Result<Option<DelegationOutcomeRow>, String>;
}

/// Default store — writes report success (so tests don't need to care), reads
/// find nothing. Production replaces it via [`DelegationBroker::with_outcome_store`].
#[derive(Default, Clone)]
pub struct NoopOutcomeStore;

#[async_trait]
impl DelegationOutcomeStore for NoopOutcomeStore {
    async fn insert_once(
        &self,
        _record: DelegationOutcomeInsert,
    ) -> Result<OutcomeWriteResult, String> {
        Ok(OutcomeWriteResult::Inserted)
    }

    async fn find_owned(
        &self,
        _parent_conversation_id: i32,
        _task_id: &str,
    ) -> Result<Option<DelegationOutcomeRow>, String> {
        Ok(None)
    }
}

/// Production store backed by the SeaORM `delegation_outcome` table.
pub struct DbDelegationOutcomeStore {
    pub db: Arc<crate::db::AppDatabase>,
}

#[async_trait]
impl DelegationOutcomeStore for DbDelegationOutcomeStore {
    async fn insert_once(
        &self,
        record: DelegationOutcomeInsert,
    ) -> Result<OutcomeWriteResult, String> {
        crate::db::service::delegation_outcome_service::insert_once(&self.db.conn, record)
            .await
            .map_err(|e| e.to_string())
    }

    async fn find_owned(
        &self,
        parent_conversation_id: i32,
        task_id: &str,
    ) -> Result<Option<DelegationOutcomeRow>, String> {
        crate::db::service::delegation_outcome_service::find_owned(
            &self.db.conn,
            parent_conversation_id,
            task_id,
        )
        .await
        .map_err(|e| e.to_string())
    }
}

/// Serialize the non-sensitive resume-binding facts for the outcome row:
/// external session id, canonical cwd, agent type, execution-config
/// fingerprint. Deliberately `None` (store null, don't guess) unless BOTH the
/// external session id and the execution-config identity were captured — a
/// binding that can't be verified later must not pose as one that can. Never
/// contains tokens, API keys, or environment variables.
pub(super) fn build_resume_binding_json(
    agent_type: AgentType,
    facts: &ResumeBindingFacts,
) -> Option<String> {
    let external_session_id = facts.external_session_id.as_ref()?;
    let config_fingerprint = facts.config_fingerprint.as_ref()?;
    Some(
        serde_json::json!({
            "schema_version": 1,
            "agent_type": agent_type,
            "external_session_id": external_session_id,
            "cwd": facts.cwd,
            "config_fingerprint": config_fingerprint,
        })
        .to_string(),
    )
}

/// Status report projected from the frozen completed outcome. Unlike the DB
/// fallback ([`super::db_report`]), this restores the ORIGINAL bounded result
/// text — even after the in-memory cache was evicted, the broker rebuilt, or
/// the child row's mutable status moved on.
pub(super) fn frozen_report(
    task_id: &str,
    row: &DelegationOutcomeRow,
) -> DelegationTaskReport {
    DelegationTaskReport {
        task_id: Some(task_id.to_string()),
        status: TaskStatus::Completed,
        child_conversation_id: row.child_conversation_id,
        agent_type: serde_json::from_value(serde_json::Value::String(row.agent_type.clone()))
            .ok(),
        text: Some(row.text.clone()),
        error_code: None,
        message: Some(
            "Completed result restored from the durable delegation outcome store.".to_string(),
        ),
        duration_ms: Some(row.duration_ms.max(0) as u64),
        blocked_on: None,
    }
}

impl DelegationBroker {
    /// Replace the immutable completed-outcome store used to freeze success
    /// results and answer frozen-first status queries. Builder-style, layered
    /// onto `with_writers` by the production wiring; tests opt in with a
    /// recording mock.
    pub fn with_outcome_store(mut self, outcome_store: Arc<dyn DelegationOutcomeStore>) -> Self {
        self.outcome_store = outcome_store;
        self
    }

    /// Freeze a SUCCESSFUL outcome into the immutable store. Called ONLY for
    /// the outcome the current execution won the terminal race with, AFTER the
    /// pending lock was released and BEFORE teardown disconnects the child
    /// (so the resume binding can still be captured from the live connection).
    ///
    /// Canceled / failed outcomes return without writing: a canceled task must
    /// keep the upstream `resume_delegation` path under the same id, and no
    /// failed attempt may masquerade as a permanent frozen result. Best-effort
    /// by design — a storage failure logs a diagnostic and leaves the task
    /// without a frozen result (which also disqualifies it from later
    /// continuation); the one-shot teardown proceeds either way.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn persist_completed_outcome(
        &self,
        task_id: &str,
        parent_conversation_id: i32,
        parent_tool_use_id: Option<&str>,
        child_connection_id: Option<&str>,
        child_conversation_id: Option<i32>,
        agent_type: AgentType,
        duration_ms: u64,
        outcome: &DelegationOutcome,
    ) {
        let DelegationOutcome::Ok(ok) = outcome else {
            return;
        };
        // Persist under the SAME bounded-text policy as the in-memory cache —
        // the frozen row is an honest copy, never an unlimited transcript.
        let text = cap_completed_text(&ok.text);
        let text_truncated = text.len() != ok.text.len();
        let resume_binding_json = match child_connection_id {
            Some(conn_id) => match self.spawner.capture_resume_binding(conn_id).await {
                Some(facts) => build_resume_binding_json(agent_type, &facts),
                None => {
                    tracing::debug!(
                        "[delegation-outcome] no resume binding captured for task {task_id}; \
                         storing a null binding"
                    );
                    None
                }
            },
            None => None,
        };
        // A synthetic parent tool_use id is not a real identity — store null
        // rather than the fabricated value.
        let parent_tool_use_id = parent_tool_use_id
            .filter(|id| !is_synthetic_parent_tool_use_id(id))
            .map(|id| id.to_string());
        let record = DelegationOutcomeInsert {
            task_id: task_id.to_string(),
            parent_conversation_id,
            parent_tool_use_id,
            child_conversation_id,
            agent_type: serde_json::to_value(agent_type)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            text,
            duration_ms: duration_ms.min(i64::MAX as u64) as i64,
            text_truncated,
            completed_at: chrono::Utc::now(),
            resume_binding_json,
        };
        match self.outcome_store.insert_once(record).await {
            Ok(OutcomeWriteResult::Inserted) => {}
            Ok(OutcomeWriteResult::AlreadyIdentical) => {}
            Ok(OutcomeWriteResult::Conflict) => {
                tracing::warn!(
                    "[delegation-outcome] conflicting second success result for task {task_id}; \
                     first-writer-wins kept the original frozen result"
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[delegation-outcome] FAILED to persist completed result for task \
                     {task_id}: {e}; this source is not continuable and falls back to the \
                     legacy status path"
                );
            }
        }
    }

    /// Resolve a not-in-memory task id: the frozen completed outcome (scoped to
    /// the caller's parent conversation) FIRST, then the legacy DB status
    /// fallback. A frozen hit restores the original result text even after the
    /// in-memory cache was evicted, the broker rebuilt, or the child row's
    /// mutable status drifted (a frozen result must never be shadowed by
    /// `conversation.status`). The fallback answers canceled/failed tasks and
    /// never-frozen ids exactly as before; a frozen lookup that ERRORS also
    /// falls through (logged) rather than fabricating a report.
    pub(super) async fn frozen_then_db_report(
        &self,
        parent_conversation_id: Option<i32>,
        task_id: &str,
    ) -> DelegationTaskReport {
        if let Some(parent_conversation_id) = parent_conversation_id {
            match self
                .outcome_store
                .find_owned(parent_conversation_id, task_id)
                .await
            {
                Ok(Some(row)) => return frozen_report(task_id, &row),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        "[delegation-outcome] frozen lookup failed for task {task_id}: {e}"
                    );
                }
            }
        }
        self.status_from_db(parent_conversation_id, task_id).await
    }
}
