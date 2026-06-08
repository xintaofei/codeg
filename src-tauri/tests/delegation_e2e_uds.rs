//! End-to-end Phase 6 integration: drive real UDS round-trips from a
//! companion-style client through the listener → broker → mock spawner →
//! `complete_call`. Under the async protocol `delegate_to_agent` returns a
//! Running ack and the terminal result is collected by a follow-up
//! `get_delegation_status` round-trip — both asserted over the wire.
//!
//! Skipped on non-unix targets (named-pipe windows path tested separately).

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use codeg_lib::acp::delegation::broker::{
    ConversationDepthLookup, DelegationBroker, DelegationConfig,
};
use codeg_lib::acp::delegation::listener::{
    DelegationListener, ParentSessionLookup, TokenEntry, TokenRegistry,
};
use codeg_lib::acp::delegation::spawner::{mock::MockSpawner, ConnectionSpawner};
use codeg_lib::acp::delegation::transport::{
    client_round_trip, client_status_round_trip, BrokerRequest, BrokerStatusRequest,
};
use codeg_lib::acp::delegation::types::{DelegationError, DelegationOutcome, DelegationSuccess};
use codeg_lib::models::AgentType;
use serde_json::json;

struct AlwaysRoot;
#[async_trait]
impl ConversationDepthLookup for AlwaysRoot {
    async fn parent_of(&self, _id: i32) -> Result<Option<i32>, DelegationError> {
        Ok(None)
    }
}

struct FixedParent(i32);
#[async_trait]
impl ParentSessionLookup for FixedParent {
    async fn current_conversation_id(&self, _: &str) -> Option<i32> {
        Some(self.0)
    }
}

/// No-op feedback access — this e2e suite exercises delegation, not feedback.
struct NoFeedback;
#[async_trait]
impl codeg_lib::acp::feedback::SessionFeedbackAccess for NoFeedback {
    async fn read_pending_feedback(
        &self,
        _parent_connection_id: &str,
    ) -> Vec<codeg_lib::acp::feedback::PendingFeedback> {
        Vec::new()
    }
    async fn commit_feedback_delivered(&self, _parent_connection_id: &str, _ids: Vec<String>) {}
}

#[tokio::test]
async fn end_to_end_uds_happy_path() {
    let mock = Arc::new(MockSpawner::new());
    mock.queue_spawn(Ok("child-conn-1".into())).await;
    mock.queue_send(Ok(77)).await;

    let broker = Arc::new(DelegationBroker::new(
        mock.clone() as Arc<dyn ConnectionSpawner>,
        Arc::new(AlwaysRoot) as Arc<dyn ConversationDepthLookup>,
    ));
    broker
        .set_config(DelegationConfig {
            enabled: true,
            depth_limit: 8,
            ..DelegationConfig::default()
        })
        .await;

    let tokens = Arc::new(TokenRegistry::default());
    tokens
        .register(
            "tok".into(),
            TokenEntry {
                parent_connection_id: "p1".into(),
                working_dir: PathBuf::from("/tmp"),
            },
        )
        .await;

    let listener = DelegationListener::new(
        broker.clone(),
        tokens,
        Arc::new(FixedParent(1)) as Arc<dyn ParentSessionLookup>,
        Arc::new(NoFeedback) as Arc<dyn codeg_lib::acp::feedback::SessionFeedbackAccess>,
    );

    // PID-scoped socket inside the OS temp dir — no clashes across test bins.
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("codeg-e2e.sock");
    let socket_for_listener = socket.clone();
    let listener_task = tokio::spawn(async move {
        let _ = listener.run(socket_for_listener).await;
    });

    // Spin until the socket is bound and ready to accept.
    for _ in 0..50 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket.exists(), "listener never bound the socket");

    // 1. delegate_to_agent → Running ack carrying the child conversation id and
    //    a task_id to follow up on. Under the async protocol the call returns
    //    immediately instead of blocking for the result.
    let req = BrokerRequest {
        token: "tok".into(),
        parent_connection_id: "p1".into(),
        parent_tool_use_id: "pt-1".into(),
        external_handle: None,
        input: json!({"agent_type": "codex", "task": "do x"}),
    };
    let ack = client_round_trip(&socket.to_string_lossy(), &req)
        .await
        .expect("client round-trip");
    assert_eq!(ack.outcome["status"], "running");
    assert_eq!(ack.outcome["child_conversation_id"], 77);
    let task_id = ack.outcome["task_id"]
        .as_str()
        .expect("running ack carries a task_id")
        .to_string();

    // 2. The lifecycle resolves the child on TurnComplete. The ack already
    //    returned, so the task is registered and `complete_call` migrates it to
    //    completed deterministically — no race against registration.
    broker
        .complete_call(
            &task_id,
            DelegationOutcome::Ok(DelegationSuccess {
                text: "uds-result".into(),
                child_conversation_id: 77,
                child_agent_type: AgentType::Codex,
                turn_count: 1,
                duration_ms: 12,
                token_usage: None,
            }),
        )
        .await;

    // 3. get_delegation_status → Completed with the result text, over the wire.
    //    The Status arm returns a `{ tasks: [..] }` envelope; one id → one entry.
    let status_req = BrokerStatusRequest {
        token: "tok".into(),
        task_ids: vec![task_id],
        wait_ms: Some(1_000),
    };
    let resp = client_status_round_trip(&socket.to_string_lossy(), &status_req)
        .await
        .expect("status round-trip");
    listener_task.abort();

    assert_eq!(resp.outcome["tasks"][0]["status"], "completed");
    assert_eq!(resp.outcome["tasks"][0]["text"], "uds-result");
    assert_eq!(resp.outcome["tasks"][0]["child_conversation_id"], 77);
}

/// Batch `get_delegation_status` over the wire: two delegations are started,
/// one completes, and a single status round-trip with `task_ids: [t1, t2]`
/// returns both reports in request order — `t1` completed, `t2` still running.
#[tokio::test]
async fn end_to_end_uds_batch_status() {
    let mock = Arc::new(MockSpawner::new());
    // Two children: first resolves to conv 77, second to conv 88.
    mock.queue_spawn(Ok("child-conn-1".into())).await;
    mock.queue_send(Ok(77)).await;
    mock.queue_spawn(Ok("child-conn-2".into())).await;
    mock.queue_send(Ok(88)).await;

    let broker = Arc::new(DelegationBroker::new(
        mock.clone() as Arc<dyn ConnectionSpawner>,
        Arc::new(AlwaysRoot) as Arc<dyn ConversationDepthLookup>,
    ));
    broker
        .set_config(DelegationConfig {
            enabled: true,
            depth_limit: 8,
            ..DelegationConfig::default()
        })
        .await;

    let tokens = Arc::new(TokenRegistry::default());
    tokens
        .register(
            "tok".into(),
            TokenEntry {
                parent_connection_id: "p1".into(),
                working_dir: PathBuf::from("/tmp"),
            },
        )
        .await;

    let listener = DelegationListener::new(
        broker.clone(),
        tokens,
        Arc::new(FixedParent(1)) as Arc<dyn ParentSessionLookup>,
        Arc::new(NoFeedback) as Arc<dyn codeg_lib::acp::feedback::SessionFeedbackAccess>,
    );

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("codeg-e2e-batch.sock");
    let socket_for_listener = socket.clone();
    let listener_task = tokio::spawn(async move {
        let _ = listener.run(socket_for_listener).await;
    });
    for _ in 0..50 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket.exists(), "listener never bound the socket");

    // Start two delegations; capture each task_id from its Running ack.
    let mut task_ids = Vec::new();
    for task in ["do x", "do y"] {
        let req = BrokerRequest {
            token: "tok".into(),
            parent_connection_id: "p1".into(),
            parent_tool_use_id: format!("pt-{task}"),
            external_handle: None,
            input: json!({ "agent_type": "codex", "task": task }),
        };
        let ack = client_round_trip(&socket.to_string_lossy(), &req)
            .await
            .expect("client round-trip");
        assert_eq!(ack.outcome["status"], "running");
        task_ids.push(ack.outcome["task_id"].as_str().unwrap().to_string());
    }

    // Resolve only the FIRST task.
    broker
        .complete_call(
            &task_ids[0],
            DelegationOutcome::Ok(DelegationSuccess {
                text: "first-result".into(),
                child_conversation_id: 77,
                child_agent_type: AgentType::Codex,
                turn_count: 1,
                duration_ms: 9,
                token_usage: None,
            }),
        )
        .await;

    // Immediate batch poll → both reports, in request order.
    let status_req = BrokerStatusRequest {
        token: "tok".into(),
        task_ids: task_ids.clone(),
        wait_ms: None,
    };
    let resp = client_status_round_trip(&socket.to_string_lossy(), &status_req)
        .await
        .expect("batch status round-trip");
    listener_task.abort();

    let tasks = resp.outcome["tasks"]
        .as_array()
        .expect("batch status returns a tasks array");
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0]["status"], "completed");
    assert_eq!(tasks[0]["text"], "first-result");
    assert_eq!(tasks[0]["task_id"], task_ids[0].as_str());
    assert_eq!(tasks[1]["status"], "running");
    assert_eq!(tasks[1]["task_id"], task_ids[1].as_str());
}

#[tokio::test]
async fn end_to_end_uds_invalid_token_rejected() {
    let mock = Arc::new(MockSpawner::new());
    // No queued spawn — listener should reject before reaching broker.
    let broker = Arc::new(DelegationBroker::new(
        mock as Arc<dyn ConnectionSpawner>,
        Arc::new(AlwaysRoot) as Arc<dyn ConversationDepthLookup>,
    ));
    let tokens = Arc::new(TokenRegistry::default());
    let listener = DelegationListener::new(
        broker,
        tokens,
        Arc::new(FixedParent(1)) as Arc<dyn ParentSessionLookup>,
        Arc::new(NoFeedback) as Arc<dyn codeg_lib::acp::feedback::SessionFeedbackAccess>,
    );

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("codeg-e2e-reject.sock");
    let socket_for_listener = socket.clone();
    let listener_task = tokio::spawn(async move {
        let _ = listener.run(socket_for_listener).await;
    });

    for _ in 0..50 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let req = BrokerRequest {
        token: "wrong-token".into(),
        parent_connection_id: "p1".into(),
        parent_tool_use_id: "pt-1".into(),
        external_handle: None,
        input: json!({"agent_type": "codex", "task": "x"}),
    };
    let resp = client_round_trip(&socket.to_string_lossy(), &req)
        .await
        .expect("client round-trip");
    listener_task.abort();

    assert_eq!(resp.outcome["status"], "canceled");
    assert_eq!(resp.outcome["error_code"], "canceled");
}
