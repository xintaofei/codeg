//! End-to-end Phase 6 integration: drive a real UDS round-trip from a
//! companion-style client through the listener → broker → mock spawner →
//! `complete_call`, and assert the outcome is delivered to the wire.
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
use codeg_lib::acp::delegation::transport::{client_round_trip, BrokerRequest};
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

    // Drive completion from a parallel task so the client send→recv races
    // against the broker registration.
    let broker_for_completion = broker.clone();
    let completer = tokio::spawn(async move {
        loop {
            if let Some(call_id) = broker_for_completion.peek_first_pending_call_id().await {
                broker_for_completion
                    .complete_call(
                        &call_id,
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
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    let req = BrokerRequest {
        token: "tok".into(),
        parent_connection_id: "p1".into(),
        parent_tool_use_id: "pt-1".into(),
        external_handle: None,
        input: json!({"agent_type": "codex", "task": "do x"}),
    };
    let resp = client_round_trip(&socket.to_string_lossy(), &req)
        .await
        .expect("client round-trip");

    completer.await.unwrap();
    listener_task.abort();

    assert_eq!(resp.outcome["kind"], "ok");
    assert_eq!(resp.outcome["text"], "uds-result");
    assert_eq!(resp.outcome["child_conversation_id"], 77);
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

    assert_eq!(resp.outcome["kind"], "err");
    assert_eq!(resp.outcome["code"], "canceled");
}
