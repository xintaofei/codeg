use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::acp::connection::{
    in_memory_agent_pair, spawn_agent_connection_with_transport, InMemoryAgentEnds,
    InMemoryAgentTransport,
};
use crate::acp::delegation::continuation::SessionRecovery;
use crate::acp::internal_bus::{EventBusMetrics, InternalEventBus};
use crate::acp::types::{AcpEvent, PromptInputBlock};
use crate::db::service::collaboration_service;
use crate::db::test_helpers;
use crate::models::AgentType;
use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};

use super::super::ConnectionManager;

struct CancelProbe {
    prompt_seen: tokio::sync::oneshot::Receiver<()>,
    cancel_seen: tokio::sync::oneshot::Receiver<()>,
}

async fn seed_running_owner(
    db: &crate::db::AppDatabase,
    coordinator: &crate::acp::delegation::continuation::ContinuationCoordinator,
    child_connection_id: &str,
    parent_connection_id: &str,
    child_conversation_id: i32,
) -> (String, String) {
    let session = collaboration_service::upsert_session_once(
        &db.conn,
        collaboration_service::NewSession {
            id: uuid::Uuid::new_v4().to_string(),
            source_task_id: uuid::Uuid::new_v4().to_string(),
            parent_conversation_id: 1,
            child_conversation_id,
            resume_binding_json: "{}".to_string(),
        },
    )
    .await
    .expect("seed session");
    let turn = collaboration_service::admit_turn(
        &db.conn,
        collaboration_service::NewTurn {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            ordinal: 0,
            request_id: uuid::Uuid::new_v4().to_string(),
            message: "active child".to_string(),
            initiator_parent_conversation_id: 1,
            initiator_tool_use_id: None,
        },
    )
    .await
    .expect("seed turn");
    for (from, to, sent) in [
        ("accepted", "preparing", false),
        ("preparing", "dispatching", true),
        ("dispatching", "running", false),
    ] {
        assert!(collaboration_service::cas_turn_state(
            &db.conn,
            &turn.id,
            Some(&turn.execution_id),
            &[from],
            to,
            sent,
        )
        .await
        .expect("advance seeded turn"));
    }
    collaboration_service::set_turn_connection(
        &db.conn,
        &turn.id,
        &turn.execution_id,
        child_connection_id,
    )
    .await
    .expect("persist child connection");
    coordinator
        .register_execution_for_test(
            child_connection_id,
            &turn.id,
            &turn.execution_id,
            &session.id,
            parent_connection_id,
        )
        .await;
    (turn.id, session.id)
}

async fn run_cancel_reply_agent(ends: InMemoryAgentEnds, stop_reason: &'static str) -> CancelProbe {
    let (prompt_tx, prompt_rx) = tokio::sync::oneshot::channel();
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut prompt_tx = Some(prompt_tx);
        let mut cancel_tx = Some(cancel_tx);
        let mut pending_prompt_id = None;
        let mut reader = tokio::io::BufReader::new(ends.client_to_agent).lines();
        let mut writer = ends.agent_to_client;
        while let Ok(Some(line)) = reader.next_line().await {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                continue;
            };
            let id = message.get("id").cloned();
            let response = match method {
                "initialize" => Some(json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {"protocolVersion": 1, "agentCapabilities": {}, "authMethods": []}
                })),
                "session/new" => Some(json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {"sessionId": "terminal-probe-session"}
                })),
                "session/prompt" => {
                    pending_prompt_id = id;
                    if stop_reason == "end_turn" {
                        let update = json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": "terminal-probe-session",
                                "update": {
                                    "sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": "completed work"}
                                }
                            }
                        });
                        writer
                            .write_all(format!("{update}\n").as_bytes())
                            .await
                            .expect("write completion update");
                    }
                    if let Some(tx) = prompt_tx.take() {
                        let _ = tx.send(());
                    }
                    None
                }
                "session/cancel" => {
                    if let Some(tx) = cancel_tx.take() {
                        let _ = tx.send(());
                    }
                    pending_prompt_id.take().map(|prompt_id| {
                        json!({
                            "jsonrpc": "2.0", "id": prompt_id,
                            "result": {"stopReason": stop_reason}
                        })
                    })
                }
                _ if id.is_some() => Some(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
                _ => None,
            };
            if let Some(response) = response {
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .expect("write terminal response");
            }
        }
    });
    CancelProbe {
        prompt_seen: prompt_rx,
        cancel_seen: cancel_rx,
    }
}

async fn assert_real_owned_terminal(stop_reason: &'static str, expected_state: &str) {
    let db = test_helpers::fresh_in_memory_db().await;
    let manager = ConnectionManager::new();
    let data_dir = tempfile::tempdir().expect("data dir");
    let (broker, _, _, _, _, _, _, coordinator) = crate::app_state::build_delegation_stack(
        &manager,
        db.conn.clone(),
        data_dir.path().to_path_buf(),
    );
    let bus = Arc::new(InternalEventBus::new(Arc::new(EventBusMetrics::default())));
    let mut events = bus.subscribe();
    tokio::spawn(
        crate::acp::lifecycle::lifecycle_subscriber_task_with_continuation(
            db.conn.clone(),
            manager.clone_ref(),
            Arc::clone(&bus),
            Some(broker),
            Some(Arc::clone(&coordinator)),
        ),
    );
    let emitter = EventEmitter::web_only(Arc::new(WebEventBroadcaster::new()), Arc::clone(&bus));
    let connection_id = format!("terminal-{stop_reason}");
    let (transport, ends) = in_memory_agent_pair(64 * 1024);
    let probe = run_cancel_reply_agent(ends, stop_reason).await;
    spawn_test_connection(
        &manager,
        emitter,
        data_dir.path(),
        &connection_id,
        transport,
    )
    .await;
    let folder_id = test_helpers::seed_folder(&db, data_dir.path().to_str().unwrap()).await;
    let child_conversation_id =
        test_helpers::seed_conversation(&db, folder_id, AgentType::ClaudeCode).await;
    manager
        .get_state(&connection_id)
        .await
        .expect("connection state")
        .write()
        .await
        .conversation_id = Some(child_conversation_id);
    let (turn_id, _) = seed_running_owner(
        &db,
        &coordinator,
        &connection_id,
        "terminal-parent",
        child_conversation_id,
    )
    .await;
    manager
        .send_prompt_for_continuation(
            &db.conn,
            &connection_id,
            vec![PromptInputBlock::Text {
                text: "race terminal".to_string(),
            }],
        )
        .await
        .unwrap();
    probe.prompt_seen.await.expect("prompt seen");
    if stop_reason == "end_turn" {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = events.recv().await.expect("event bus remains live");
                if matches!(event.payload, AcpEvent::ContentDelta { .. }) {
                    break;
                }
            }
        })
        .await
        .expect("completion text reached the driver before cancellation");
    }
    manager.cancel(&db.conn, &connection_id).await.unwrap();
    probe.cancel_seen.await.expect("cancel seen");
    let mut observed_state = String::new();
    for _ in 0..200 {
        let turn = collaboration_service::find_turn(&db.conn, &turn_id)
            .await
            .unwrap()
            .unwrap();
        observed_state = turn.state.clone();
        if turn.state == expected_state {
            if expected_state == "completed" {
                assert_eq!(turn.result_text.as_deref(), Some("completed work"));
            }
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("real terminal did not settle as {expected_state}; observed {observed_state}");
}

async fn run_parent_probe_agent(
    ends: InMemoryAgentEnds,
    hold_first_prompt: bool,
) -> tokio::sync::mpsc::UnboundedReceiver<&'static str> {
    let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut prompt_count = 0;
        let mut reader = tokio::io::BufReader::new(ends.client_to_agent).lines();
        let mut writer = ends.agent_to_client;
        while let Ok(Some(line)) = reader.next_line().await {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                continue;
            };
            let id = message.get("id").cloned();
            let response = match method {
                "initialize" => Some(json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {"protocolVersion": 1, "agentCapabilities": {}, "authMethods": []}
                })),
                "session/new" => Some(json!({
                    "jsonrpc": "2.0", "id": id, "result": {"sessionId": "parent-session"}
                })),
                "session/prompt" => {
                    prompt_count += 1;
                    let _ = events_tx.send("prompt");
                    if hold_first_prompt && prompt_count == 1 {
                        None
                    } else {
                        Some(json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"stopReason": "end_turn"}
                        }))
                    }
                }
                "session/cancel" => {
                    let _ = events_tx.send("cancel");
                    None
                }
                _ if id.is_some() => Some(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
                _ => None,
            };
            if let Some(response) = response {
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .expect("write parent response");
            }
        }
    });
    events_rx
}

async fn spawn_test_connection(
    manager: &ConnectionManager,
    emitter: EventEmitter,
    data_dir: &std::path::Path,
    connection_id: &str,
    transport: InMemoryAgentTransport,
) {
    spawn_agent_connection_with_transport(
        transport,
        connection_id.to_string(),
        AgentType::ClaudeCode,
        Some(data_dir.to_string_lossy().into_owned()),
        None,
        BTreeMap::new(),
        "test-window".to_string(),
        emitter,
        Arc::clone(&manager.connections),
        None,
        BTreeMap::new(),
        manager.delegation_snapshot(),
        manager.terminal_shell_config(),
        format!("{connection_id}-fingerprint"),
        Arc::new(crate::acp::stderr_tail::StderrTail::new()),
        SessionRecovery::AllowNewFallback,
    )
    .await
    .expect("spawn in-memory connection");
}

async fn run_cancel_probe_agent(ends: InMemoryAgentEnds) -> CancelProbe {
    let (prompt_tx, prompt_rx) = tokio::sync::oneshot::channel();
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut prompt_tx = Some(prompt_tx);
        let mut cancel_tx = Some(cancel_tx);
        let mut reader = tokio::io::BufReader::new(ends.client_to_agent).lines();
        let mut writer = ends.agent_to_client;
        while let Ok(Some(line)) = reader.next_line().await {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                continue;
            };
            let id = message.get("id").cloned();
            let response = match method {
                "initialize" => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": 1,
                        "agentCapabilities": {},
                        "authMethods": []
                    }
                })),
                "session/new" => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"sessionId": "cancel-probe-session"}
                })),
                "session/prompt" => {
                    if let Some(tx) = prompt_tx.take() {
                        let _ = tx.send(());
                    }
                    None // Deliberately keep the real prompt reply in flight.
                }
                "session/cancel" => {
                    if let Some(tx) = cancel_tx.take() {
                        let _ = tx.send(());
                    }
                    None // ACP cancel is a notification.
                }
                _ if id.is_some() => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {}
                })),
                _ => None,
            };
            if let Some(response) = response {
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .expect("write fake ACP response");
            }
        }
    });
    CancelProbe {
        prompt_seen: prompt_rx,
        cancel_seen: cancel_rx,
    }
}

#[tokio::test]
async fn owned_cancel_does_not_emit_local_terminal_while_prompt_reply_is_withheld() {
    let db = test_helpers::fresh_in_memory_db().await;
    let manager = ConnectionManager::new();
    let data_dir = tempfile::tempdir().expect("data dir");
    let (_, _, _, _, _, _, _, coordinator) = crate::app_state::build_delegation_stack(
        &manager,
        db.conn.clone(),
        data_dir.path().to_path_buf(),
    );

    let metrics = Arc::new(EventBusMetrics::default());
    let bus = Arc::new(InternalEventBus::new(metrics));
    let emitter = EventEmitter::web_only(Arc::new(WebEventBroadcaster::new()), Arc::clone(&bus));
    let mut events = bus.subscribe();
    let (transport, ends) = in_memory_agent_pair(64 * 1024);
    let probe = run_cancel_probe_agent(ends).await;
    let connection_id = "owned-cancel-probe".to_string();
    spawn_agent_connection_with_transport(
        transport,
        connection_id.clone(),
        AgentType::ClaudeCode,
        Some(data_dir.path().to_string_lossy().into_owned()),
        None,
        BTreeMap::new(),
        "test-window".to_string(),
        emitter,
        Arc::clone(&manager.connections),
        None,
        BTreeMap::new(),
        manager.delegation_snapshot(),
        manager.terminal_shell_config(),
        "cancel-probe-fingerprint".to_string(),
        Arc::new(crate::acp::stderr_tail::StderrTail::new()),
        SessionRecovery::AllowNewFallback,
    )
    .await
    .expect("spawn in-memory connection");

    coordinator
        .register_execution_for_test(
            &connection_id,
            "turn-owned-cancel",
            "execution-owned-cancel",
            "session-owned-cancel",
            "parent-owned-cancel",
        )
        .await;
    manager
        .send_prompt(
            &connection_id,
            vec![PromptInputBlock::Text {
                text: "hold this prompt".to_string(),
            }],
        )
        .await
        .expect("enqueue prompt");
    tokio::time::timeout(Duration::from_secs(2), probe.prompt_seen)
        .await
        .expect("agent observed prompt")
        .expect("prompt signal");

    manager
        .cancel(&db.conn, &connection_id)
        .await
        .expect("enqueue owned cancellation");
    tokio::time::timeout(Duration::from_secs(2), probe.cancel_seen)
        .await
        .expect("agent observed cancel")
        .expect("cancel signal");

    let local_terminal = tokio::time::timeout(Duration::from_millis(150), async {
        loop {
            let event = events.recv().await.expect("event bus remains live");
            if matches!(event.payload, AcpEvent::TurnComplete { .. }) {
                return event;
            }
        }
    })
    .await;
    assert!(
        local_terminal.is_err(),
        "an owned continuation cancel must keep observing the real prompt reply"
    );

    let _ = manager.disconnect(&connection_id).await;
}

#[tokio::test]
async fn owned_cancel_settles_from_real_cancel_terminal() {
    assert_real_owned_terminal("cancelled", "canceled").await;
}

#[tokio::test]
async fn real_completion_wins_a_racing_owned_cancel() {
    assert_real_owned_terminal("end_turn", "completed").await;
}

#[tokio::test]
async fn idle_parent_cancel_stops_owned_continuation_and_accepts_later_prompt() {
    let db = test_helpers::fresh_in_memory_db().await;
    let manager = ConnectionManager::new();
    let data_dir = tempfile::tempdir().expect("data dir");
    let (_, _, _, _, _, _, _, coordinator) = crate::app_state::build_delegation_stack(
        &manager,
        db.conn.clone(),
        data_dir.path().to_path_buf(),
    );
    let bus = Arc::new(InternalEventBus::new(Arc::new(EventBusMetrics::default())));
    let emitter = EventEmitter::web_only(Arc::new(WebEventBroadcaster::new()), Arc::clone(&bus));

    let child_id = "idle-parent-child";
    let (child_transport, child_ends) = in_memory_agent_pair(64 * 1024);
    let child_probe = run_cancel_probe_agent(child_ends).await;
    spawn_test_connection(
        &manager,
        emitter.clone(),
        data_dir.path(),
        child_id,
        child_transport,
    )
    .await;

    let parent_id = "idle-parent";
    let (parent_transport, parent_ends) = in_memory_agent_pair(64 * 1024);
    let mut parent_events = run_parent_probe_agent(parent_ends, false).await;
    spawn_test_connection(
        &manager,
        emitter,
        data_dir.path(),
        parent_id,
        parent_transport,
    )
    .await;
    seed_running_owner(&db, &coordinator, child_id, parent_id, 2).await;

    manager
        .cancel(&db.conn, parent_id)
        .await
        .expect("cancel parent");
    tokio::time::timeout(Duration::from_secs(2), child_probe.cancel_seen)
        .await
        .expect("child cancellation delivered")
        .expect("child cancel signal");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), parent_events.recv())
            .await
            .expect("idle parent cancel observed"),
        Some("cancel")
    );

    manager
        .send_prompt(
            parent_id,
            vec![PromptInputBlock::Text {
                text: "later prompt".to_string(),
            }],
        )
        .await
        .expect("parent remains usable");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), parent_events.recv())
            .await
            .expect("later prompt observed"),
        Some("prompt")
    );
    let _ = manager.disconnect(parent_id).await;
    let _ = manager.disconnect(child_id).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
}

#[tokio::test]
async fn mid_prompt_parent_cancel_stops_owned_continuation_and_accepts_next_prompt() {
    let db = test_helpers::fresh_in_memory_db().await;
    let manager = ConnectionManager::new();
    let data_dir = tempfile::tempdir().expect("data dir");
    let (_, _, _, _, _, _, _, coordinator) = crate::app_state::build_delegation_stack(
        &manager,
        db.conn.clone(),
        data_dir.path().to_path_buf(),
    );
    let bus = Arc::new(InternalEventBus::new(Arc::new(EventBusMetrics::default())));
    let emitter = EventEmitter::web_only(Arc::new(WebEventBroadcaster::new()), Arc::clone(&bus));

    let child_id = "mid-parent-child";
    let (child_transport, child_ends) = in_memory_agent_pair(64 * 1024);
    let child_probe = run_cancel_probe_agent(child_ends).await;
    spawn_test_connection(
        &manager,
        emitter.clone(),
        data_dir.path(),
        child_id,
        child_transport,
    )
    .await;

    let parent_id = "mid-parent";
    let (parent_transport, parent_ends) = in_memory_agent_pair(64 * 1024);
    let mut parent_events = run_parent_probe_agent(parent_ends, true).await;
    spawn_test_connection(
        &manager,
        emitter,
        data_dir.path(),
        parent_id,
        parent_transport,
    )
    .await;
    seed_running_owner(&db, &coordinator, child_id, parent_id, 2).await;

    manager
        .send_prompt(
            parent_id,
            vec![PromptInputBlock::Text {
                text: "active parent prompt".to_string(),
            }],
        )
        .await
        .unwrap();
    assert_eq!(parent_events.recv().await, Some("prompt"));
    manager
        .cancel(&db.conn, parent_id)
        .await
        .expect("cancel parent");

    tokio::time::timeout(Duration::from_secs(2), child_probe.cancel_seen)
        .await
        .expect("child cancellation delivered")
        .expect("child cancel signal");
    loop {
        if tokio::time::timeout(Duration::from_secs(2), parent_events.recv())
            .await
            .expect("parent cancel observed")
            == Some("cancel")
        {
            break;
        }
    }

    manager
        .send_prompt(
            parent_id,
            vec![PromptInputBlock::Text {
                text: "next parent prompt".to_string(),
            }],
        )
        .await
        .expect("parent remains usable");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), parent_events.recv())
            .await
            .expect("next prompt observed"),
        Some("prompt")
    );
    let _ = manager.disconnect(parent_id).await;
    let _ = manager.disconnect(child_id).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
}
