use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::acp::error::AcpError;
use crate::acp::types::{AcpEvent, PromptInputBlock};
use crate::db::entities::conversation::ConversationStatus;
use crate::db::service::conversation_service;
use crate::web::event_bridge::emit_with_state;

use super::ConnectionManager;

impl ConnectionManager {
    /// Enqueue the coordinator-controlled cancel command without re-entering
    /// the public Stop path. The coordinator has already performed the durable
    /// state transition and armed its confirmation deadline before calling the
    /// runtime boundary.
    pub(crate) async fn enqueue_continuation_cancel(
        &self,
        connection_id: &str,
    ) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            connections
                .get(connection_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(connection_id.to_string()))?
                .cmd_tx
                .clone()
        };
        cmd_tx
            .send(crate::acp::connection::ConnectionCommand::CancelContinuation)
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    /// The continuation coordinator's privileged prompt send. Identical to
    /// [`Self::send_prompt`] except it does NOT consult the collaboration
    /// write reservation: the coordinator IS the reservation's owner,
    /// driving the reserved child through its controlled rounds. Every
    /// ordinary entry (`send_prompt`, `send_prompt_linked*`) keeps the check,
    /// so the coordinator↔ordinary boundary stays explicit (reacceptance
    /// R8).
    pub async fn send_prompt_for_continuation(
        &self,
        db: &DatabaseConnection,
        conn_id: &str,
        blocks: Vec<PromptInputBlock>,
    ) -> Result<(), AcpError> {
        let prompt_lock = self.clone_prompt_lock(conn_id).await?;
        let _guard = prompt_lock.lock_owned().await;
        // Mirror `send_prompt_linked`'s status transition: every prompt flips
        // the bound conversation row to InProgress (DB write before the emit
        // so subscribers observe a consistent row).
        let (state_arc, emitter) = self
            .get_state_and_emitter(conn_id)
            .await
            .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
        let conversation_id_for_status = state_arc.read().await.conversation_id;
        if let Some(cid) = conversation_id_for_status {
            conversation_service::update_status(db, cid, ConversationStatus::InProgress)
                .await
                .map_err(|e| AcpError::protocol(e.to_string()))?;
            emit_with_state(
                &state_arc,
                &emitter,
                AcpEvent::ConversationStatusChanged {
                    conversation_id: cid,
                    status: ConversationStatus::InProgress,
                },
            )
            .await;
        }
        self.send_prompt_inner(conn_id, blocks, None).await
    }

    /// Whether the conversation is currently reserved by an open/blocked
    /// collaboration session (the continuation coordinator's exclusive write
    /// scope). Ordinary prompt entries refuse reserved conversations with a
    /// backend error — a disabled button is not the contract.
    pub(super) async fn collaboration_reserved(&self, conversation_id: i32) -> bool {
        match self.delegation_snapshot().and_then(|d| d.collaboration) {
            Some(collab) => collab.child_is_reserved(conversation_id).await,
            None => false,
        }
    }
}

/// Production [`ContinuationRuntime`]: bridges the collaboration coordinator
/// to the REAL connection world — the manager's strict-attach entry, the
/// child connection's prompt channel, and the manager's cancel/disconnect.
/// This is the piece that turns the coordinator from a tested state machine
/// into the actual rework loop (acceptance F3).
pub struct ConnectionManagerContinuationRuntime {
    pub manager: Arc<ConnectionManager>,
    pub db: Arc<crate::db::AppDatabase>,
    pub data_dir: Arc<std::path::PathBuf>,
}

/// Bounded wait for the strict attach verdict. Long enough for a cold agent
/// process + session/resume handshake; short enough that a stuck attach is
/// reported instead of hanging the turn forever (the drive settles it as a
/// typed failure and the turn stays queryable).
pub const STRICT_ATTACH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

impl ConnectionManagerContinuationRuntime {
    /// Bind a strict-attached connection to the child conversation row its
    /// collaboration session reserves (reacceptance R1). Emits the same
    /// `ConversationLinked` event `send_prompt_linked`'s adopt-a-row branch
    /// does, which latches `state.conversation_id`/`folder_id` (what the
    /// lifecycle's TurnComplete settlement routing reads) and registers the
    /// connection in the lifecycle's terminal-event cache. The row must still
    /// hold the external session that was strictly resumed — a drifted
    /// binding fails loudly instead of silently rebinding to another row.
    pub(super) async fn bind_child_conversation(
        &self,
        connection_id: &str,
        child_conversation_id: i32,
        target: &crate::acp::delegation::continuation::runtime::AttachTarget,
    ) -> Result<(), String> {
        let row = crate::db::service::conversation_service::get_by_id(
            &self.db.conn,
            child_conversation_id,
        )
        .await
        .map_err(|e| e.to_string())?;
        if row.external_id.as_deref() != Some(target.external_session_id.as_str()) {
            return Err(format!(
                "child conversation {child_conversation_id} no longer holds the recorded \
                 external session {:?} (now {:?})",
                target.external_session_id, row.external_id
            ));
        }
        let (state_arc, emitter) = self
            .manager
            .get_state_and_emitter(connection_id)
            .await
            .ok_or_else(|| "the connection vanished after the strict attach".to_string())?;
        emit_with_state(
            &state_arc,
            &emitter,
            AcpEvent::ConversationLinked {
                conversation_id: child_conversation_id,
                folder_id: row.folder_id,
                parent_conversation_id: row.parent_id,
                parent_tool_use_id: row.parent_tool_use_id.clone(),
            },
        )
        .await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl crate::acp::delegation::continuation::ContinuationRuntime
    for ConnectionManagerContinuationRuntime
{
    async fn attach_strict(
        &self,
        target: &crate::acp::delegation::continuation::runtime::AttachTarget,
        parent_connection_id: &str,
        turn_id: &str,
        execution_id: &str,
        child_conversation_id: i32,
    ) -> Result<String, crate::acp::delegation::continuation::StrictAttachError> {
        use crate::acp::delegation::continuation::{StrictAttachError, StrictAttachErrorCode};
        // Same parent inheritance as `spawn_for_resume`: the child emits on
        // the parent's stream so the browser keeps seeing the sub-thread.
        let (emitter, owner_window) = {
            let conns = self.manager.connections.lock().await;
            let parent = conns.get(parent_connection_id).ok_or_else(|| {
                StrictAttachError::new(
                    StrictAttachErrorCode::ResumeFailed,
                    format!("parent connection {parent_connection_id} not found"),
                )
            })?;
            (parent.emitter.clone(), parent.owner_window_label.clone())
        };
        let runtime_env = crate::commands::acp::build_session_runtime_env(
            &self.db,
            target.agent_type,
            None,
            self.data_dir.as_path(),
        )
        .await
        .map_err(|e| {
            StrictAttachError::new(
                StrictAttachErrorCode::ResumeFailed,
                format!("runtime env: {e}"),
            )
        })?;
        let cwd = std::path::PathBuf::from(&target.cwd);
        if !cwd.is_dir() {
            return Err(StrictAttachError::new(
                StrictAttachErrorCode::BindingMismatch,
                format!(
                    "the recorded working directory {} no longer exists",
                    cwd.display()
                ),
            ));
        }
        let ready = self
            .manager
            .attach_existing_session_strict(
                target.agent_type,
                target.cwd.clone(),
                target.external_session_id.clone(),
                runtime_env,
                owner_window,
                emitter,
                None,
                Default::default(),
                cwd,
                target.config_fingerprint.clone(),
                STRICT_ATTACH_TIMEOUT,
                turn_id,
                execution_id,
            )
            .await?;
        // Bind the attached connection to the reserved child conversation
        // row BEFORE any prompt flows (reacceptance R1): a strict-attached
        // connection starts with no conversation identity, and the
        // lifecycle's TurnComplete routing bails on that — the round would
        // stay `running` forever even though the agent finished. A failed
        // binding fails the attach honestly (nothing was sent).
        if let Err(detail) = self
            .bind_child_conversation(&ready.connection_id, child_conversation_id, target)
            .await
        {
            let _ = self.manager.disconnect(&ready.connection_id).await;
            return Err(StrictAttachError::new(
                StrictAttachErrorCode::ResumeFailed,
                format!(
                    "the resumed connection could not be bound to child \
                     conversation {child_conversation_id}: {detail}"
                ),
            ));
        }
        Ok(ready.connection_id)
    }

    async fn send_prompt(&self, connection_id: &str, message: &str) -> Result<(), String> {
        // The coordinator is the ONLY writer for a reserved child session:
        // this internal path deliberately bypasses the session write
        // reservation via the manager's explicit continuation entry (the
        // reservation exists to stop everyone ELSE — reacceptance R8).
        self.manager
            .send_prompt_for_continuation(
                &self.db.conn,
                connection_id,
                vec![crate::acp::types::PromptInputBlock::Text {
                    text: message.to_string(),
                }],
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn cancel(&self, connection_id: &str) -> Result<(), String> {
        self.manager
            .enqueue_continuation_cancel(connection_id)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn disconnect(&self, connection_id: &str) -> Result<(), String> {
        self.manager
            .disconnect(connection_id)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn blocked_on(&self, connection_id: &str) -> Option<String> {
        let state = self.manager.get_state(connection_id).await?;
        let guard = state.read().await;
        guard.blocking_prompt(1024).map(|b| {
            serde_json::json!(b.kind)
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
    }
}
