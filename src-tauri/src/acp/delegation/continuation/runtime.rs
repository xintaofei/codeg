//! Runtime boundary between the continuation coordinator and the ACP world.
//!
//! The coordinator owns state, admission, and durability; the runtime owns
//! processes. Everything the coordinator needs from a live connection —
//! strict attach, one prompt send, cancel, release, blocking-prompt probe —
//! goes through this trait, so tests can drive the full turn state machine
//! against a controllable double with counters and fault injection, while
//! production wires the real `ConnectionManager` strict entry (Task 4).

use async_trait::async_trait;

use crate::acp::delegation::continuation::StrictAttachError;

/// The facts a strict attach needs, extracted from the session's verified
/// resume binding. Identities only — never tokens or environment variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachTarget {
    pub agent_type: crate::models::AgentType,
    pub external_session_id: String,
    pub cwd: String,
    pub config_fingerprint: String,
}

impl AttachTarget {
    /// Parse the target out of a session row's verified binding JSON. Returns
    /// `None` when the binding is absent or cannot be trusted (never guess a
    /// default configuration from it).
    pub fn from_binding_json(binding_json: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(binding_json).ok()?;
        if value.get("schema_version")?.as_i64()? != 1 {
            return None;
        }
        let agent_type = crate::models::AgentType::from_wire(value.get("agent_type")?.as_str()?)?;
        Some(Self {
            agent_type,
            external_session_id: value
                .get("external_session_id")?
                .as_str()?
                .to_string(),
            cwd: value.get("cwd")?.as_str()?.to_string(),
            config_fingerprint: value
                .get("config_fingerprint")?
                .as_str()?
                .to_string(),
        })
    }
}

/// What the coordinator needs from the live world for one turn.
#[async_trait]
pub trait ContinuationRuntime: Send + Sync {
    /// Strictly attach the recorded external session for a new turn.
    /// `parent_connection_id` is the initiating parent ACP connection — the
    /// child inherits its emitter / owner window like any delegation spawn.
    /// `child_conversation_id` is the persistent child conversation the
    /// session row reserves: the runtime MUST bind the attached connection
    /// to that row before any prompt flows (reacceptance R1) — without the
    /// binding, a completing turn carries no conversation id and the
    /// lifecycle's settlement routing never fires.
    /// The returned connection id is bound to `(turn_id, execution_id)`; any
    /// failure is a typed strict error and NOTHING has been sent.
    async fn attach_strict(
        &self,
        target: &AttachTarget,
        parent_connection_id: &str,
        turn_id: &str,
        execution_id: &str,
        child_conversation_id: i32,
    ) -> Result<String, StrictAttachError>;

    /// Send exactly ONE prompt (the rework message) on the attached
    /// connection. Called only after `dispatching` was durably persisted.
    async fn send_prompt(&self, connection_id: &str, message: &str) -> Result<(), String>;

    /// Best-effort cancel of the in-flight turn on this connection.
    async fn cancel(&self, connection_id: &str) -> Result<(), String>;

    /// Release the connection at the end of the round (or on failure).
    async fn disconnect(&self, connection_id: &str) -> Result<(), String>;

    /// The blocking prompt currently parked on this connection, if any:
    /// `"permission"` or `"question"`. `None` = working normally.
    async fn blocked_on(&self, connection_id: &str) -> Option<String>;
}

/// No-op runtime: every call fails loudly. Used when the coordinator is
/// constructed without production wiring (tests inject their own double).
#[cfg(any(test, feature = "test-utils"))]
#[derive(Default, Clone)]
pub struct NoopRuntime;

#[cfg(any(test, feature = "test-utils"))]
#[async_trait]
impl ContinuationRuntime for NoopRuntime {
    async fn attach_strict(
        &self,
        _target: &AttachTarget,
        _parent_connection_id: &str,
        _turn_id: &str,
        _execution_id: &str,
        _child_conversation_id: i32,
    ) -> Result<String, StrictAttachError> {
        Err(StrictAttachError::new(
            crate::acp::delegation::continuation::StrictAttachErrorCode::ResumeFailed,
            "no runtime wired",
        ))
    }

    async fn send_prompt(&self, _connection_id: &str, _message: &str) -> Result<(), String> {
        Err("no runtime wired".to_string())
    }

    async fn cancel(&self, _connection_id: &str) -> Result<(), String> {
        Ok(())
    }

    async fn disconnect(&self, _connection_id: &str) -> Result<(), String> {
        Ok(())
    }

    async fn blocked_on(&self, _connection_id: &str) -> Option<String> {
        None
    }
}
