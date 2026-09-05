//! PR2 Task 2: strict session recovery over a FAKE ACP transport.
//!
//! These tests drive the REAL establishment chain (`run_connection`:
//! initialize → session/resume → session/load → session/new) against an
//! in-memory fake agent that records every method call in order. The strict
//! recovery guarantees under test:
//!
//! * A06 — a successful `session/resume` (or resume-fail → load-success)
//!   attaches to the EXISTING external session id and delivers a typed
//!   `Ready`; `session/new` is never sent.
//! * A07 — resume and load both failing (or unsupported, or hanging) yields a
//!   typed failure, ZERO prompts, and still no `session/new`.
//! * A08 — an early `Connected`/`SessionStarted` is not readiness: only the
//!   gate verdict (fired after config application succeeds) counts, and a
//!   failed config application means no Ready and no prompt.
//! * AllowNewFallback — the ordinary (non-strict) chain still legally falls
//!   through to `session/new` when recovery fails.
//!
//! The transport is process-free byte streams, so the suite runs identically
//! on every platform.

// The generic transport parameter makes run_connection's future monomorphize
// into a large type; the default recursion limit is not enough to lay it out
// in the test crate.
#![recursion_limit = "1024"]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use codeg_lib::acp::connection::{
    in_memory_agent_pair, spawn_agent_connection_with_transport, ConnectionCommand,
    InMemoryAgentEnds,
};
use codeg_lib::acp::delegation::continuation::{
    StrictAttachError, StrictAttachErrorCode, StrictOutcome,
};
use codeg_lib::models::agent::AgentType;
use codeg_lib::web::event_bridge::EventEmitter;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Fake agent
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct FakeAgentConfig {
    /// `agentCapabilities.loadSession`.
    load_session: bool,
    /// `agentCapabilities.sessionCapabilities.resume` advertised?
    resume: bool,
    /// `session/resume` outcome: `Some(code)` = JSON-RPC error, `true` = never
    /// respond (handshake hangs).
    resume_fail_code: Option<i64>,
    resume_no_answer: bool,
    /// `session/load` outcome — same encoding as above.
    load_fail_code: Option<i64>,
    load_no_answer: bool,
    /// `session/update` notifications replayed after a successful
    /// `session/load` (exercises the replay drain).
    replay_notifications: usize,
    /// `session/set_config_option` failure code, if injected.
    set_config_fail_code: Option<i64>,
}

struct FakeAgent {
    methods: Mutex<Vec<String>>,
    prompt_count: Mutex<usize>,
    config: FakeAgentConfig,
}

fn fake_agent(config: FakeAgentConfig) -> Arc<FakeAgent> {
    Arc::new(FakeAgent {
        methods: Mutex::new(Vec::new()),
        prompt_count: Mutex::new(0),
        config,
    })
}

impl FakeAgent {
    async fn recorded(&self) -> Vec<String> {
        self.methods.lock().await.clone()
    }

    async fn prompts(&self) -> usize {
        *self.prompt_count.lock().await
    }
}

/// Run the fake agent: read newline-delimited JSON-RPC from the client, answer
/// per the scenario config, and record every agent-directed method in order.
async fn run_fake_agent(agent: Arc<FakeAgent>, ends: InMemoryAgentEnds) {
    let mut reader = tokio::io::BufReader::new(ends.client_to_agent).lines();
    let mut writer = ends.agent_to_client;

    while let Ok(Some(line)) = reader.next_line().await {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(method) = msg.get("method").and_then(Value::as_str).map(String::from) else {
            continue;
        };
        agent.methods.lock().await.push(method.clone());
        let id = msg.get("id").cloned();

        let reply = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
        let fail =
            |code: i64, message: &str| json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});

        let response = match method.as_str() {
            "initialize" => reply(json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": agent.config.load_session,
                    "sessionCapabilities": if agent.config.resume {
                        json!({"resume": {}})
                    } else {
                        json!({})
                    },
                },
                "authMethods": [],
            })),
            "session/resume" => {
                if agent.config.resume_no_answer {
                    continue; // never answer: the handshake hangs
                }
                match agent.config.resume_fail_code {
                    Some(code) => fail(code, "session/resume failed (injected)"),
                    None => reply(json!({})),
                }
            }
            "session/load" => {
                if agent.config.load_no_answer {
                    continue;
                }
                match agent.config.load_fail_code {
                    Some(code) => fail(code, "session/load failed (injected)"),
                    None => {
                        // Respond first; the historical replay notifications
                        // stream AFTER the response, exactly like a real
                        // session/load.
                        let _ = writer
                            .write_all(format!("{}\n", reply(json!({}))).as_bytes())
                            .await;
                        for i in 0..agent.config.replay_notifications {
                            let notif = json!({
                                "jsonrpc": "2.0",
                                "method": "session/update",
                                "params": {
                                    "sessionId": "ext-1",
                                    "update": {
                                        "sessionUpdate": "agent_message_chunk",
                                        "content": {"type": "text",
                                                    "text": format!("replay {i}")}
                                    },
                                },
                            });
                            let _ = writer.write_all(format!("{notif}\n").as_bytes()).await;
                        }
                        continue;
                    }
                }
            }
            "session/new" => reply(json!({"sessionId": "brand-new-session"})),
            "session/set_mode" => reply(json!({})),
            "session/set_config_option" => {
                match agent.config.set_config_fail_code {
                    Some(code) => fail(code, "set_config_option failed (injected)"),
                    None => reply(json!({})),
                }
            }
            "session/prompt" => {
                *agent.prompt_count.lock().await += 1;
                reply(json!({"stopReason": "end_turn"}))
            }
            // fs/* / terminal/* / anything else: generic success — none of the
            // scenarios under test should reach these anyway.
            _ => reply(json!({})),
        };
        let _ = writer.write_all(format!("{response}\n").as_bytes()).await;
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

const EXTERNAL_SESSION: &str = "ext-1";
const CONFIG_FINGERPRINT: &str = "fp-test";
const VERDICT_TIMEOUT: Duration = Duration::from_secs(10);

struct AttachOutcome {
    connection_id: String,
    connections:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, codeg_lib::acp::connection::AgentConnection>>>,
    verdict: Result<codeg_lib::acp::delegation::continuation::StrictReady, StrictAttachError>,
}

impl AttachOutcome {
    async fn teardown(&self) {
        let _ = self
            .connections
            .lock()
            .await
            .remove(&self.connection_id)
            .map(|conn| {
                conn.cmd_tx
                    .try_send(ConnectionCommand::Disconnect)
                    .is_ok()
            });
    }
}

async fn strict_attach(
    fake: Arc<FakeAgent>,
    _config: FakeAgentConfig,
    working_dir: &std::path::Path,
    recovery_fingerprint: &str,
) -> AttachOutcome {
    let (transport, ends) = in_memory_agent_pair(64 * 1024);
    // The driver task ends on its own once teardown drops the duplex halves.
    let _driver = tokio::spawn(run_fake_agent(fake.clone(), ends));

    let connections = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let (gate, mut verdict_rx) =
        codeg_lib::acp::delegation::continuation::StrictAttachGate::channel(
            working_dir.to_path_buf(),
            recovery_fingerprint.to_string(),
        );

    let connection_id = uuid::Uuid::new_v4().to_string();
    let spawn = spawn_agent_connection_with_transport(
        transport,
        connection_id.clone(),
        AgentType::ClaudeCode,
        Some(working_dir.to_string_lossy().into_owned()),
        Some(EXTERNAL_SESSION.to_string()),
        BTreeMap::new(), // runtime_env; the fingerprint is passed explicitly
        "test-window".to_string(),
        EventEmitter::Noop,
        connections.clone(),
        None,
        BTreeMap::new(),
        None,
        codeg_lib::acp::terminal_runtime::TerminalShellRuntimeConfig::default(),
        CONFIG_FINGERPRINT.to_string(),
        Arc::new(codeg_lib::acp::stderr_tail::StderrTail::new()),
        codeg_lib::acp::delegation::continuation::SessionRecovery::RequireExisting(gate),
    )
    .await;

    let verdict = match spawn {
        Ok(_session_started_rx) => {
            match tokio::time::timeout(VERDICT_TIMEOUT, &mut verdict_rx).await {
                Ok(Ok(StrictOutcome::Ready(r))) => Ok(r),
                Ok(Ok(StrictOutcome::Failed(e))) => Err(e),
                Ok(Err(_dropped)) => Err(StrictAttachError::new(
                    StrictAttachErrorCode::ResumeFailed,
                    "connection ended before verdict",
                )),
                Err(_elapsed) => Err(StrictAttachError::new(
                    StrictAttachErrorCode::ResumeTimeout,
                    "test timeout",
                )),
            }
        }
        Err(_spawn_err) => {
            // Failed before the driver thread started (e.g. binding
            // verification); the gate still carries the typed failure.
            match tokio::time::timeout(Duration::from_secs(5), &mut verdict_rx).await {
                Ok(Ok(StrictOutcome::Failed(e))) => Err(e),
                _ => Err(StrictAttachError::new(
                    StrictAttachErrorCode::ResumeFailed,
                    "spawn failed without a verdict",
                )),
            }
        }
    };

    AttachOutcome {
        connection_id,
        connections,
        verdict,
    }
}

async fn fresh_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "codeg-strict-{}-{tag}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

// ---------------------------------------------------------------------------
// A06: strict resume succeeds — initialize + session/resume, never new
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strict_resume_success_records_initialize_then_resume_and_readies() {
    let dir = fresh_dir("resume-ok").await;
    let fake = fake_agent(FakeAgentConfig {
        load_session: false,
        resume: true,
        ..Default::default()
    });
    let outcome = strict_attach(fake.clone(), FakeAgentConfig {
        load_session: false,
        resume: true,
        ..Default::default()
    }, &dir, CONFIG_FINGERPRINT)
    .await;

    let ready = outcome.verdict.as_ref().expect("strict attach must succeed").clone();
    assert_eq!(ready.external_session_id, EXTERNAL_SESSION);
    assert_eq!(ready.connection_id, outcome.connection_id);

    let methods = fake.recorded().await;
    assert_eq!(
        methods,
        vec!["initialize".to_string(), "session/resume".to_string()],
        "the wire must carry exactly initialize + session/resume"
    );
    assert_eq!(fake.prompts().await, 0, "no prompt before the caller sends one");
    outcome.teardown().await;
}

/// A06b: resume fails → load succeeds. The chain resumes trying, still never
/// touches session/new, and the replay notifications are drained before Ready.
#[tokio::test]
async fn strict_resume_failure_falls_back_to_load_then_readies() {
    let dir = fresh_dir("resume-load-ok").await;
    let config = FakeAgentConfig {
        load_session: true,
        resume: true,
        resume_fail_code: Some(-32601),
        replay_notifications: 3,
        ..Default::default()
    };
    let fake = fake_agent(config.clone());
    let outcome = strict_attach(fake.clone(), config, &dir, CONFIG_FINGERPRINT).await;

    let ready = outcome.verdict.as_ref().expect("strict attach via load must succeed").clone();
    assert_eq!(ready.external_session_id, EXTERNAL_SESSION);

    let methods = fake.recorded().await;
    assert_eq!(
        methods,
        vec![
            "initialize".to_string(),
            "session/resume".to_string(),
            "session/load".to_string(),
        ],
        "resume failure must fall through to session/load — and to nothing else"
    );
    assert_eq!(fake.prompts().await, 0);
    outcome.teardown().await;
}

// ---------------------------------------------------------------------------
// A07: strict failure — typed error, zero prompts, no session/new
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strict_resume_and_load_both_failing_never_starts_a_new_session() {
    let dir = fresh_dir("both-fail").await;
    let config = FakeAgentConfig {
        load_session: true,
        resume: true,
        resume_fail_code: Some(-32002),
        load_fail_code: Some(-32002),
        ..Default::default()
    };
    let fake = fake_agent(config.clone());
    let outcome = strict_attach(fake.clone(), config, &dir, CONFIG_FINGERPRINT).await;

    let err = outcome.verdict.as_ref().expect_err("both methods failing must fail");
    assert_eq!(err.code, StrictAttachErrorCode::ResumeFailed);

    let methods = fake.recorded().await;
    assert!(
        !methods.contains(&"session/new".to_string()),
        "session/new is FORBIDDEN on the strict path"
    );
    assert_eq!(fake.prompts().await, 0, "a failed attach sends nothing");
    outcome.teardown().await;
}

/// A07b: the agent supports neither resume nor load → resume_unsupported.
#[tokio::test]
async fn strict_attach_without_capabilities_reports_unsupported() {
    let dir = fresh_dir("no-caps").await;
    let config = FakeAgentConfig {
        load_session: false,
        resume: false,
        ..Default::default()
    };
    let fake = fake_agent(config.clone());
    let outcome = strict_attach(fake.clone(), config, &dir, CONFIG_FINGERPRINT).await;

    let err = outcome.verdict.as_ref().expect_err("no capabilities must fail");
    assert_eq!(err.code, StrictAttachErrorCode::ResumeUnsupported);
    assert!(!fake.recorded().await.contains(&"session/new".to_string()));
    assert_eq!(fake.prompts().await, 0);
    outcome.teardown().await;
}

/// A07c: the agent never answers session/resume → the verdict times out, no
/// session/new, no prompt.
#[tokio::test]
async fn strict_attach_hanging_handshake_times_out_without_sending() {
    let dir = fresh_dir("hang").await;
    let config = FakeAgentConfig {
        load_session: false,
        resume: true,
        resume_no_answer: true,
        ..Default::default()
    };
    let fake = fake_agent(config.clone());
    // Shorten the wait: the harness verdict timeout is the test's own bound.
    let outcome = {
        let (transport, ends) = in_memory_agent_pair(64 * 1024);
        let driver = tokio::spawn(run_fake_agent(fake.clone(), ends));
        let connections =
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let (gate, mut verdict_rx) =
            codeg_lib::acp::delegation::continuation::StrictAttachGate::channel(
                dir.clone(),
                CONFIG_FINGERPRINT.to_string(),
            );
        let connection_id = uuid::Uuid::new_v4().to_string();
        let spawn = spawn_agent_connection_with_transport(
            transport,
            connection_id.clone(),
            AgentType::ClaudeCode,
            Some(dir.to_string_lossy().into_owned()),
            Some(EXTERNAL_SESSION.to_string()),
            BTreeMap::new(),
            "test-window".to_string(),
            EventEmitter::Noop,
            connections.clone(),
            None,
            BTreeMap::new(),
            None,
            codeg_lib::acp::terminal_runtime::TerminalShellRuntimeConfig::default(),
            CONFIG_FINGERPRINT.to_string(),
            Arc::new(codeg_lib::acp::stderr_tail::StderrTail::new()),
            codeg_lib::acp::delegation::continuation::SessionRecovery::RequireExisting(gate),
        )
        .await;
        assert!(spawn.is_ok(), "spawn itself must succeed");
        let verdict = match tokio::time::timeout(Duration::from_millis(700), &mut verdict_rx).await
        {
            Ok(Ok(StrictOutcome::Ready(r))) => Ok(r),
            Ok(Ok(StrictOutcome::Failed(e))) => Err(e),
            Ok(Err(_)) => Err(StrictAttachError::new(
                StrictAttachErrorCode::ResumeFailed,
                "dropped",
            )),
            Err(_) => Err(StrictAttachError::new(
                StrictAttachErrorCode::ResumeTimeout,
                "handshake hung",
            )),
        };
        let outcome = AttachOutcome {
            connection_id,
            connections,
            verdict,
        };
        outcome.teardown().await;
        driver.abort();
        outcome
    };

    let err = outcome.verdict.as_ref().expect_err("a hanging handshake must time out");
    assert_eq!(err.code, StrictAttachErrorCode::ResumeTimeout);
    let methods = fake.recorded().await;
    assert!(!methods.contains(&"session/new".to_string()));
    assert_eq!(fake.prompts().await, 0);
}

// ---------------------------------------------------------------------------
// A08: config application failure is NOT readiness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strict_attach_config_failure_reports_failed_and_sends_nothing() {
    let dir = fresh_dir("config-fail").await;
    // A preferred config value makes run_connection apply
    // session/set_config_option; the fake agent rejects it.
    let mut preferred = BTreeMap::new();
    preferred.insert("model".to_string(), "gpt-x".to_string());
    let config = FakeAgentConfig {
        load_session: false,
        resume: true,
        set_config_fail_code: Some(-32603),
        ..Default::default()
    };
    let fake = fake_agent(config.clone());

    // Drive the same chain but with a preferred config value — a variant of
    // the harness (the shared helper passes empty preferences).
    let (transport, ends) = in_memory_agent_pair(64 * 1024);
    let driver = tokio::spawn(run_fake_agent(fake.clone(), ends));
    let connections = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let (gate, mut verdict_rx) =
        codeg_lib::acp::delegation::continuation::StrictAttachGate::channel(
            dir.clone(),
            CONFIG_FINGERPRINT.to_string(),
        );
    let connection_id = uuid::Uuid::new_v4().to_string();
    let spawn = spawn_agent_connection_with_transport(
        transport,
        connection_id.clone(),
        AgentType::ClaudeCode,
        Some(dir.to_string_lossy().into_owned()),
        Some(EXTERNAL_SESSION.to_string()),
        BTreeMap::new(),
        "test-window".to_string(),
        EventEmitter::Noop,
        connections.clone(),
        None,
        preferred,
        None,
        codeg_lib::acp::terminal_runtime::TerminalShellRuntimeConfig::default(),
        CONFIG_FINGERPRINT.to_string(),
        Arc::new(codeg_lib::acp::stderr_tail::StderrTail::new()),
        codeg_lib::acp::delegation::continuation::SessionRecovery::RequireExisting(gate),
    )
    .await;
    assert!(spawn.is_ok());
    let verdict: Result<codeg_lib::acp::delegation::continuation::StrictReady, StrictAttachError> = match tokio::time::timeout(VERDICT_TIMEOUT, &mut verdict_rx).await {
        Ok(Ok(StrictOutcome::Ready(_))) => panic!("a failed config application must NOT ready"),
        Ok(Ok(StrictOutcome::Failed(e))) => Err(e),
        other => panic!("unexpected verdict: {other:?}"),
    };
    let err = verdict.as_ref().expect_err("config failure must be a typed failure");
    assert_eq!(err.code, StrictAttachErrorCode::ResumeFailed);
    assert_eq!(fake.prompts().await, 0, "no prompt may follow a failed config");
    assert!(
        !fake.recorded().await.contains(&"session/new".to_string()),
        "session/new is FORBIDDEN even after a config failure"
    );
    let outcome = AttachOutcome {
        connection_id,
        connections,
        verdict: Ok(codeg_lib::acp::delegation::continuation::StrictReady {
            connection_id: String::new(),
            external_session_id: String::new(),
        }),
    };
    outcome.teardown().await;
    driver.abort();
}

/// A08b: binding mismatch (recorded fingerprint ≠ launch fingerprint) fails
/// BEFORE any protocol traffic.
#[tokio::test]
async fn strict_attach_binding_mismatch_refuses_before_any_traffic() {
    let dir = fresh_dir("fp-mismatch").await;
    let config = FakeAgentConfig {
        load_session: false,
        resume: true,
        ..Default::default()
    };
    let fake = fake_agent(config.clone());
    let outcome = strict_attach(
        fake.clone(),
        config,
        &dir,
        "a-different-recorded-fingerprint",
    )
    .await;

    let err = outcome
        .verdict
        .as_ref()
        .expect_err("fingerprint mismatch must be refused");
    assert_eq!(err.code, StrictAttachErrorCode::BindingMismatch);
    assert!(
        fake.recorded().await.is_empty(),
        "a binding mismatch must fail before initialize"
    );
    assert_eq!(fake.prompts().await, 0);
    outcome.teardown().await;
}

// ---------------------------------------------------------------------------
// AllowNewFallback: the ordinary chain keeps its session/new fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn allow_new_fallback_still_creates_a_new_session_when_recovery_fails() {
    let dir = fresh_dir("allow-new").await;
    // NOTE: a NOT-FOUND load failure (-32002) does NOT fall back — it
    // surfaces `SessionLoadFailed` so the user can choose Reload vs New.
    // A generic internal error (-32603) is the one that keeps the historical
    // session/new fallback.
    let config = FakeAgentConfig {
        load_session: true,
        resume: true,
        resume_fail_code: Some(-32601),
        load_fail_code: Some(-32603),
        ..Default::default()
    };
    let fake = fake_agent(config.clone());
    let (transport, ends) = in_memory_agent_pair(64 * 1024);
    let driver = tokio::spawn(run_fake_agent(fake.clone(), ends));
    let connections = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let connection_id = uuid::Uuid::new_v4().to_string();
    let spawn = spawn_agent_connection_with_transport(
        transport,
        connection_id.clone(),
        AgentType::ClaudeCode,
        Some(dir.to_string_lossy().into_owned()),
        Some(EXTERNAL_SESSION.to_string()),
        BTreeMap::new(),
        "test-window".to_string(),
        EventEmitter::Noop,
        connections.clone(),
        None,
        BTreeMap::new(),
        None,
        codeg_lib::acp::terminal_runtime::TerminalShellRuntimeConfig::default(),
        CONFIG_FINGERPRINT.to_string(),
        Arc::new(codeg_lib::acp::stderr_tail::StderrTail::new()),
        codeg_lib::acp::delegation::continuation::SessionRecovery::AllowNewFallback,
    )
    .await;
    assert!(spawn.is_ok(), "the ordinary spawn returns a connection id");
    // Give the chain a moment to walk resume → load → new.
    for _ in 0..100 {
        if fake.recorded().await.contains(&"session/new".to_string()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let methods = fake.recorded().await;
    assert!(
        methods.contains(&"session/new".to_string()),
        "AllowNewFallback must keep the ordinary session/new fallback: {methods:?}"
    );
    assert_eq!(
        fake.prompts().await, 0,
        "no prompt was sent in this scenario"
    );
    let _ = connections
        .lock()
        .await
        .remove(&connection_id)
        .map(|conn| conn.cmd_tx.try_send(ConnectionCommand::Disconnect).is_ok());
    driver.abort();
}
