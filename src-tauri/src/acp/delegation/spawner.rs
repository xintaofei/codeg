//! `ConnectionSpawner` trait — the subset of `ConnectionManager` capabilities
//! that the delegation broker needs. Defined as a trait so:
//!
//! 1. The broker can be unit-tested with a `MockSpawner` (no real ACP
//!    processes, no DB writes).
//! 2. Future cross-host / remote-agent work (v3+) can plug in a different
//!    backend without touching the broker.
//!
//! The concrete impl on `Arc<ConnectionManager>` lives in
//! `acp::manager` next to the existing `ConnectionManager` methods to keep
//! the manager's surface area contiguous.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::models::agent::AgentType;

/// Identifies a delegation call across the broker, the ACP layer, and the DB.
///
/// `parent_conversation_id` is the **DB** id (i32) of the parent's conversation
/// row, not the ACP-side external session id. The child's new conversation
/// row will carry this as `parent_id` plus `parent_tool_use_id` (the MCP
/// tool_use_id from the parent's LLM-issued ToolUse) and `delegation_call_id`
/// (broker-internal UUID).
#[derive(Debug, Clone)]
pub struct DelegationLink {
    pub parent_conversation_id: i32,
    pub parent_tool_use_id: String,
    pub delegation_call_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SpawnerError {
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("send prompt failed: {0}")]
    Send(String),
    #[error("disconnect failed: {0}")]
    Disconnect(String),
    #[error("cancel failed: {0}")]
    Cancel(String),
}

/// Capabilities the delegation broker needs from whatever owns the ACP
/// connections. v1 production impl is `Arc<ConnectionManager>` (see
/// `acp/manager.rs`); tests use `mock::MockSpawner`.
///
/// All methods are `async` because the production impl drives a Tokio runtime
/// and DB; the mock returns immediately.
#[async_trait]
pub trait ConnectionSpawner: Send + Sync {
    /// Spawn a fresh child ACP connection of `agent_type` in `working_dir`.
    /// Delegation children are always brand-new sessions (no resume), but the
    /// broker may inject per-agent defaults configured in
    /// `DelegationConfig::agent_defaults`:
    ///   * `preferred_mode_id` — applied via `session/set_mode`
    ///   * `preferred_config_values` — applied via `session/set_config_option`
    ///
    /// Both are passed through to `ConnectionManager::spawn_agent` verbatim
    /// and are applied right after `SessionStarted`, before the child's first
    /// prompt is sent.
    ///
    /// `parent_connection_id` identifies the parent ACP connection so the
    /// production impl can inherit the parent's `EventEmitter` and
    /// `owner_window_label` (both required by `ConnectionManager::spawn_agent`)
    /// without leaking those types into the broker. If `working_dir` is
    /// `None`, the impl may fall back to the parent connection's `working_dir`.
    ///
    /// Returns the new connection id (codeg-internal UUID, not the ACP
    /// session id assigned by the agent).
    async fn spawn(
        &self,
        parent_connection_id: &str,
        agent_type: AgentType,
        working_dir: Option<String>,
        preferred_mode_id: Option<String>,
        preferred_config_values: BTreeMap<String, String>,
    ) -> Result<String, SpawnerError>;

    /// Send the delegation task as the child's first prompt. The
    /// `DelegationLink` is persisted onto the new conversation row so the
    /// lifecycle subscriber can later notify the broker on `TurnComplete`.
    ///
    /// Returns the new child conversation row id (i32).
    async fn send_prompt_linked_for_delegation(
        &self,
        conn_id: &str,
        task: String,
        link: DelegationLink,
    ) -> Result<i32, SpawnerError>;

    /// Cancel any in-flight prompt on the child connection. Idempotent:
    /// calling on a connection with nothing in flight is a no-op success.
    async fn cancel(&self, conn_id: &str) -> Result<(), SpawnerError>;

    /// Tear down the child connection. Always called after the broker has
    /// resolved (or failed) the pending call, to enforce v1's one-shot
    /// semantics.
    async fn disconnect(&self, conn_id: &str) -> Result<(), SpawnerError>;
}

#[cfg(any(test, feature = "test-utils"))]
pub mod mock {
    use super::*;
    use std::collections::VecDeque;
    use tokio::sync::Mutex;

    /// In-memory spawner that returns pre-queued results and records every
    /// `cancel` / `disconnect` it sees. Use `queue_spawn` / `queue_send` to
    /// stage the next return value; calls without queued results fail loudly.
    ///
    /// `spawn_args` records every `spawn` invocation so broker tests can
    /// assert that the right per-agent defaults were forwarded. Entry order
    /// matches call order.
    #[derive(Default)]
    pub struct MockSpawner {
        pub spawn_results: Mutex<VecDeque<Result<String, SpawnerError>>>,
        pub send_results: Mutex<VecDeque<Result<i32, SpawnerError>>>,
        pub cancels: Mutex<Vec<String>>,
        pub disconnects: Mutex<Vec<String>>,
        pub spawn_args: Mutex<Vec<SpawnCallArgs>>,
        /// When set, `send_prompt_linked_for_delegation` awaits this receiver
        /// before returning — lets a test hold `handle_request` in the window
        /// AFTER it has reserved the child (post-spawn) but BEFORE it parks the
        /// pending entry, so a racing terminal event can be exercised
        /// deterministically. `None` (default) = no gate, return immediately.
        pub send_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        /// When set, `spawn` awaits this receiver before returning the child id
        /// (but AFTER recording `spawn_args`) — lets a test pin `handle_request`
        /// INSIDE `spawn`, before it reserves the child or sends a prompt, to
        /// exercise a parent cancel landing in the spawn window. `None`
        /// (default) = no gate, return immediately.
        pub spawn_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SpawnCallArgs {
        pub parent_connection_id: String,
        pub agent_type: AgentType,
        pub working_dir: Option<String>,
        pub preferred_mode_id: Option<String>,
        pub preferred_config_values: BTreeMap<String, String>,
    }

    impl MockSpawner {
        pub fn new() -> Self {
            Self::default()
        }

        pub async fn queue_spawn(&self, r: Result<String, SpawnerError>) {
            self.spawn_results.lock().await.push_back(r);
        }

        pub async fn queue_send(&self, r: Result<i32, SpawnerError>) {
            self.send_results.lock().await.push_back(r);
        }

        /// Install a one-shot gate that holds the next
        /// `send_prompt_linked_for_delegation` until the returned sender fires.
        /// Used to deterministically pin `handle_request` in the
        /// reserve→park window. See [`MockSpawner::send_gate`].
        pub async fn install_send_gate(&self) -> tokio::sync::oneshot::Sender<()> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            *self.send_gate.lock().await = Some(rx);
            tx
        }

        /// Install a one-shot gate that holds the next `spawn` (after it records
        /// `spawn_args`, before it returns the child id) until the returned
        /// sender fires. Used to deterministically pin `handle_request` in the
        /// spawn window. See [`MockSpawner::spawn_gate`].
        pub async fn install_spawn_gate(&self) -> tokio::sync::oneshot::Sender<()> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            *self.spawn_gate.lock().await = Some(rx);
            tx
        }
    }

    #[async_trait]
    impl ConnectionSpawner for MockSpawner {
        async fn spawn(
            &self,
            parent_connection_id: &str,
            agent_type: AgentType,
            working_dir: Option<String>,
            preferred_mode_id: Option<String>,
            preferred_config_values: BTreeMap<String, String>,
        ) -> Result<String, SpawnerError> {
            self.spawn_args.lock().await.push(SpawnCallArgs {
                parent_connection_id: parent_connection_id.to_string(),
                agent_type,
                working_dir,
                preferred_mode_id,
                preferred_config_values,
            });
            // Honor a test-installed gate: block here (after recording the call,
            // before returning the child id) so a test can pin `handle_request`
            // in the spawn window — before it reserves the child or sends.
            let gate = self.spawn_gate.lock().await.take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            self.spawn_results
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| Err(SpawnerError::Spawn("no queued spawn result".into())))
        }

        async fn send_prompt_linked_for_delegation(
            &self,
            _conn_id: &str,
            _task: String,
            _link: DelegationLink,
        ) -> Result<i32, SpawnerError> {
            // Honor a test-installed gate: block here (after the broker has
            // reserved the child, before it parks the pending entry) until the
            // test releases it.
            let gate = self.send_gate.lock().await.take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            self.send_results
                .lock()
                .await
                .pop_front()
                .unwrap_or_else(|| Err(SpawnerError::Send("no queued send result".into())))
        }

        async fn cancel(&self, conn_id: &str) -> Result<(), SpawnerError> {
            self.cancels.lock().await.push(conn_id.to_string());
            Ok(())
        }

        async fn disconnect(&self, conn_id: &str) -> Result<(), SpawnerError> {
            self.disconnects.lock().await.push(conn_id.to_string());
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn mock_records_cancel_and_disconnect() {
            let m = MockSpawner::new();
            m.cancel("c1").await.unwrap();
            m.disconnect("c2").await.unwrap();
            assert_eq!(m.cancels.lock().await.as_slice(), &["c1".to_string()]);
            assert_eq!(m.disconnects.lock().await.as_slice(), &["c2".to_string()]);
        }

        #[tokio::test]
        async fn mock_consumes_queued_spawn_results() {
            let m = MockSpawner::new();
            m.queue_spawn(Ok("child-1".into())).await;
            m.queue_spawn(Err(SpawnerError::Spawn("oh no".into())))
                .await;
            let r1 = m
                .spawn(
                    "parent-1",
                    AgentType::ClaudeCode,
                    Some("/tmp".into()),
                    None,
                    BTreeMap::new(),
                )
                .await
                .unwrap();
            assert_eq!(r1, "child-1");
            let r2 = m
                .spawn("parent-1", AgentType::Codex, None, None, BTreeMap::new())
                .await
                .unwrap_err();
            assert!(matches!(r2, SpawnerError::Spawn(_)));
        }

        #[tokio::test]
        async fn mock_unqueued_spawn_fails_loudly() {
            let m = MockSpawner::new();
            let r = m
                .spawn(
                    "parent-1",
                    AgentType::ClaudeCode,
                    None,
                    None,
                    BTreeMap::new(),
                )
                .await
                .unwrap_err();
            match r {
                SpawnerError::Spawn(msg) => assert!(msg.contains("no queued")),
                other => panic!("expected SpawnerError::Spawn, got {other:?}"),
            }
        }

        #[tokio::test]
        async fn mock_records_spawn_args_for_assertion() {
            let m = MockSpawner::new();
            m.queue_spawn(Ok("c1".into())).await;
            let mut cfg = BTreeMap::new();
            cfg.insert("model".into(), "claude-sonnet-4-5".into());
            m.spawn(
                "p1",
                AgentType::ClaudeCode,
                Some("/work".into()),
                Some("auto".into()),
                cfg.clone(),
            )
            .await
            .unwrap();
            let args = m.spawn_args.lock().await;
            assert_eq!(args.len(), 1);
            assert_eq!(args[0].agent_type, AgentType::ClaudeCode);
            assert_eq!(args[0].preferred_mode_id.as_deref(), Some("auto"));
            assert_eq!(args[0].preferred_config_values, cfg);
        }
    }
}
