use super::*;
use async_trait::async_trait;
use sea_orm::Database;
use serde_json::{json, Value};
use std::{future::Future, str::FromStr, time::Duration};

use crate::acp::delegation::broker::{DbDepthLookup, DelegationBroker, DelegationConfig};
use crate::acp::delegation::spawner::{
    ConnectionSpawner, DelegationDispatch, DelegationLink, ResumedSpawn, SpawnerError,
};
use crate::acp::delegation::types::{
    DelegationOutcome, DelegationRequest, DelegationSuccess, DelegationTaskReport, TaskStatus,
};
use crate::db::service::delegation_task_service as ledger;
use crate::db::service::{conversation_service, folder_service};
use crate::db::test_helpers::fresh_disk_db;

// Process creation and strict session recovery can approach five seconds on a
// saturated Windows CI runner. Keep protocol-response assertions at five
// seconds, but give process-bound phases a separate, still-bounded budget.
const FIXTURE_PROCESS_TIMEOUT: Duration = Duration::from_secs(30);

fn run_on_large_stack<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> T {
    std::thread::Builder::new()
        .name("continuation-protocol-test".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(future)
        })
        .unwrap()
        .join()
        .unwrap()
}

fn fixture_agent(mode: &str, log: &Path) -> AcpAgent {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/bounded_continuation_agent.py");
    let command = json!({
        "type": "stdio",
        "name": "bounded-continuation",
        "command": "python3",
        "args": [script, mode, log],
        "env": []
    });
    AcpAgent::from_str(&command.to_string()).unwrap()
}

async fn run_driver(
    mode: &'static str,
) -> (
    tempfile::TempDir,
    PathBuf,
    Arc<RwLock<SessionState>>,
    mpsc::Sender<ConnectionCommand>,
    tokio::task::JoinHandle<Result<(), AcpError>>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("wire.jsonl");
    let agent = fixture_agent(mode, &log).with_current_dir(dir.path());
    let mut initial = SessionState::new(
        "continuation-test".into(),
        AgentType::ClaudeCode,
        Some(dir.path().to_path_buf()),
        "test".into(),
        None,
    );
    let started = initial.install_session_started_signal();
    let state = Arc::new(RwLock::new(initial));
    let (tx, rx) = mpsc::channel(8);
    let worker_state = Arc::clone(&state);
    let cwd = dir.path().to_path_buf();
    let driver = tokio::spawn(async move {
        run_connection(
            agent,
            "continuation-test".into(),
            AgentType::ClaudeCode,
            Some(cwd.to_string_lossy().into_owned()),
            Some("source-session".into()),
            rx,
            EventEmitter::Noop,
            worker_state,
            BTreeMap::new(),
            TerminalShellRuntimeConfig::default(),
            Some("plan".into()),
            BTreeMap::from([("model".into(), "source-model".into())]),
            None,
            FsAccessPolicy::from_env(&cwd, AgentType::ClaudeCode, &BTreeMap::new()),
            HostToolsPolicy::Default,
            Arc::new(StderrTail::new()),
            SessionRecoveryPolicy::Strict,
            None,
        )
        .await
    });
    (dir, log, state, tx, driver, started)
}

fn wire_methods(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

struct FixtureConnection {
    tx: mpsc::Sender<ConnectionCommand>,
    state: Arc<RwLock<SessionState>>,
    driver: tokio::task::JoinHandle<Result<(), AcpError>>,
}

/// Production-shaped continuation spawner backed by the deterministic ACP
/// fixture. It performs the same durable admission immediately before the
/// prompt, while the connection itself exercises the real strict-resume
/// protocol in `run_connection`.
struct FixtureContinuationSpawner {
    db: Arc<crate::db::AppDatabase>,
    working_dir: PathBuf,
    log: PathBuf,
    child_conversation_id: i32,
    connection: tokio::sync::Mutex<Option<FixtureConnection>>,
}

#[async_trait]
impl ConnectionSpawner for FixtureContinuationSpawner {
    async fn spawn(
        &self,
        _parent_connection_id: &str,
        _agent_type: AgentType,
        _working_dir: Option<String>,
        _preferred_mode_id: Option<String>,
        _preferred_config_values: BTreeMap<String, String>,
    ) -> Result<String, SpawnerError> {
        Err(SpawnerError::Spawn(
            "fixture only supports durable continuation".into(),
        ))
    }

    async fn spawn_for_delegation(
        &self,
        _parent_connection_id: &str,
        agent_type: AgentType,
        working_dir: Option<String>,
        preferred_mode_id: Option<String>,
        preferred_config_values: BTreeMap<String, String>,
        _task_id: String,
        resume_binding: Option<ledger::ResumeBinding>,
    ) -> Result<String, SpawnerError> {
        let binding = resume_binding
            .ok_or_else(|| SpawnerError::Spawn("expected continuation binding".into()))?;
        if binding.agent_type != agent_type {
            return Err(SpawnerError::Spawn("agent binding changed".into()));
        }
        let cwd = working_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.working_dir.clone());
        let connection_id = "child-after-reopen".to_string();
        let agent = fixture_agent("resume_ok", &self.log).with_current_dir(&cwd);
        let mut initial = SessionState::new(
            connection_id.clone(),
            agent_type,
            Some(cwd.clone()),
            "reopened-parent-window".into(),
            None,
        );
        let started = initial.install_session_started_signal();
        let state = Arc::new(RwLock::new(initial));
        let (tx, rx) = mpsc::channel(8);
        let worker_state = Arc::clone(&state);
        let driver_connection_id = connection_id.clone();
        let driver = tokio::spawn(async move {
            run_connection(
                agent,
                driver_connection_id,
                agent_type,
                Some(cwd.to_string_lossy().into_owned()),
                Some(binding.external_session_id),
                rx,
                EventEmitter::Noop,
                worker_state,
                BTreeMap::new(),
                TerminalShellRuntimeConfig::default(),
                preferred_mode_id,
                preferred_config_values,
                None,
                FsAccessPolicy::from_env(&cwd, agent_type, &BTreeMap::new()),
                HostToolsPolicy::Default,
                Arc::new(StderrTail::new()),
                SessionRecoveryPolicy::Strict,
                None,
            )
            .await
        });
        match tokio::time::timeout(FIXTURE_PROCESS_TIMEOUT, started).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                driver.abort();
                return Err(SpawnerError::Spawn(
                    "fixture resume stopped before announcing its session".into(),
                ));
            }
            Err(_) => {
                driver.abort();
                return Err(SpawnerError::Spawn(format!(
                    "fixture resume timed out after {FIXTURE_PROCESS_TIMEOUT:?}; wire={:?}",
                    wire_methods(&self.log)
                )));
            }
        }
        *self.connection.lock().await = Some(FixtureConnection { tx, state, driver });
        Ok(connection_id)
    }

    async fn send_prompt_linked_for_delegation(
        &self,
        _conn_id: &str,
        task: String,
        link: DelegationLink,
    ) -> Result<DelegationDispatch, SpawnerError> {
        let admission = link
            .admission
            .ok_or_else(|| SpawnerError::Send("missing durable admission".into()))?;
        let resume_binding = admission
            .resume_binding
            .ok_or_else(|| SpawnerError::Send("missing continuation binding".into()))?;
        let result = ledger::admit_continuation(
            &self.db.conn,
            ledger::AdmissionInput {
                task_id: link.delegation_call_id,
                parent_conversation_id: link.parent_conversation_id,
                child_conversation_id: self.child_conversation_id,
                source_task_id: admission.source_task_id,
                task: admission.task,
                requested_working_dir: admission.requested_working_dir,
                resume_binding,
            },
        )
        .await
        .map_err(|error| SpawnerError::Send(error.to_string()))?;
        if let ledger::AdmissionResult::Existing { entry } = result {
            return Ok(DelegationDispatch::Existing(entry.report));
        }
        if let ledger::AdmissionResult::Conflict { reason, .. } = result {
            return Err(SpawnerError::Send(reason));
        }

        let (tx, state) = {
            let connection = self.connection.lock().await;
            let connection = connection
                .as_ref()
                .ok_or_else(|| SpawnerError::Send("fixture connection missing".into()))?;
            (connection.tx.clone(), Arc::clone(&connection.state))
        };
        tx.send(ConnectionCommand::Prompt {
            blocks: vec![PromptInputBlock::Text { text: task }],
            user_message: None,
        })
        .await
        .map_err(|error| SpawnerError::Send(error.to_string()))?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = state.read().await;
                if snapshot.status == ConnectionStatus::Connected
                    && snapshot.last_assistant_text.as_deref() == Some("immediate reply 1")
                {
                    break;
                }
                drop(snapshot);
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| SpawnerError::Send("fixture continuation prompt timed out".into()))?;
        Ok(DelegationDispatch::Started(self.child_conversation_id))
    }

    async fn spawn_for_resume(
        &self,
        _parent_connection_id: &str,
        _task_id: &str,
        _agent_type: AgentType,
        _working_dir: Option<String>,
        _external_session_id: &str,
        _preferred_mode_id: Option<String>,
        _preferred_config_values: BTreeMap<String, String>,
    ) -> Result<ResumedSpawn, SpawnerError> {
        Err(SpawnerError::Spawn(
            "legacy resume is outside this fixture".into(),
        ))
    }

    async fn send_resume_prompt(
        &self,
        _conn_id: &str,
        _prompt: String,
        _folder_id: i32,
        _child_conversation_id: i32,
        _link: DelegationLink,
    ) -> Result<(), SpawnerError> {
        Err(SpawnerError::Send(
            "legacy resume is outside this fixture".into(),
        ))
    }

    async fn has_live_connection_for_conversation(&self, conversation_id: i32) -> bool {
        conversation_id == self.child_conversation_id && self.connection.lock().await.is_some()
    }

    async fn cancel(&self, _conn_id: &str) -> Result<(), SpawnerError> {
        if let Some(connection) = self.connection.lock().await.as_ref() {
            connection
                .tx
                .send(ConnectionCommand::Cancel)
                .await
                .map_err(|error| SpawnerError::Cancel(error.to_string()))?;
        }
        Ok(())
    }

    async fn disconnect(&self, _conn_id: &str) -> Result<(), SpawnerError> {
        let Some(connection) = self.connection.lock().await.take() else {
            return Ok(());
        };
        connection
            .tx
            .send(ConnectionCommand::Disconnect)
            .await
            .map_err(|error| SpawnerError::Disconnect(error.to_string()))?;
        let mut driver = connection.driver;
        let joined = match tokio::time::timeout(FIXTURE_PROCESS_TIMEOUT, &mut driver).await {
            Ok(joined) => joined,
            Err(_) => {
                driver.abort();
                return Err(SpawnerError::Disconnect(format!(
                    "fixture disconnect timed out after {FIXTURE_PROCESS_TIMEOUT:?}; wire={:?}",
                    wire_methods(&self.log)
                )));
            }
        };
        joined
            .map_err(|error| SpawnerError::Disconnect(error.to_string()))?
            .map_err(|error| SpawnerError::Disconnect(error.to_string()))?;
        Ok(())
    }
}

#[test]
fn strict_resume_never_falls_back_to_new() {
    for mode in ["unsupported", "load_fail"] {
        run_on_large_stack(async move {
            let (_dir, log, state, _tx, mut driver, _started) = run_driver(mode).await;
            let error = match tokio::time::timeout(FIXTURE_PROCESS_TIMEOUT, &mut driver).await {
                Ok(joined) => joined
                    .expect("strict-recovery fixture driver panicked")
                    .expect_err("strict recovery unexpectedly succeeded"),
                Err(_) => {
                    driver.abort();
                    panic!(
                        "strict-recovery fixture timed out after {FIXTURE_PROCESS_TIMEOUT:?}; mode={mode}; wire={:?}",
                        wire_methods(&log)
                    );
                }
            };
            assert!(error.to_string().contains("strict session recovery failed"));
            assert!(
                state.read().await.external_id.is_none(),
                "a failed strict recovery must not announce a session"
            );
            let wire = wire_methods(&log);
            assert!(!wire
                .iter()
                .any(|message| message["method"] == "session/new"));
            assert!(!wire
                .iter()
                .any(|message| message["method"] == "session/prompt"));
        });
    }
}

#[test]
fn immediate_text_is_reduced_before_prompt_response_every_time() {
    run_on_large_stack(async {
        let (_dir, log, state, tx, mut driver, started) = run_driver("resume_ok").await;
        match tokio::time::timeout(FIXTURE_PROCESS_TIMEOUT, started).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => panic!("fixture stopped before announcing its session"),
            Err(_) => {
                driver.abort();
                panic!(
                    "fixture session startup timed out after {FIXTURE_PROCESS_TIMEOUT:?}; wire={:?}",
                    wire_methods(&log)
                );
            }
        }

        for turn in 1..=50 {
            tx.send(ConnectionCommand::Prompt {
                blocks: vec![PromptInputBlock::Text {
                    text: format!("turn {turn}"),
                }],
                user_message: None,
            })
            .await
            .unwrap();
            let expected = format!("immediate reply {turn}");
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let snapshot = state.read().await;
                    if snapshot.status == ConnectionStatus::Connected
                        && snapshot.last_assistant_text.as_deref() == Some(expected.as_str())
                    {
                        break;
                    }
                    drop(snapshot);
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("turn {turn} completed without its queued text"));
        }

        tx.send(ConnectionCommand::Disconnect).await.unwrap();
        match tokio::time::timeout(FIXTURE_PROCESS_TIMEOUT, &mut driver).await {
            Ok(joined) => joined
                .expect("fixture driver panicked during disconnect")
                .expect("fixture driver failed during disconnect"),
            Err(_) => {
                driver.abort();
                panic!(
                    "fixture disconnect timed out after {FIXTURE_PROCESS_TIMEOUT:?}; wire={:?}",
                    wire_methods(&log)
                );
            }
        }
        let wire = wire_methods(&log);
        let prompt = wire
            .iter()
            .position(|message| message["method"] == "session/prompt")
            .unwrap();
        for method in ["session/set_mode", "session/set_config_option"] {
            assert!(
                wire.iter()
                    .position(|message| message["method"] == method)
                    .unwrap()
                    < prompt
            );
        }
        assert_eq!(wire[prompt]["params"]["sessionId"], "source-session");
        assert!(!wire
            .iter()
            .any(|message| message["method"] == "session/new"));
    });
}

#[test]
fn durable_continuation_reopens_the_source_session_and_admits_one_successor() {
    run_on_large_stack(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let working_dir = std::fs::canonicalize(dir.path())
            .expect("canonical working directory")
            .to_string_lossy()
            .into_owned();
        let db = fresh_disk_db(dir.path()).await;
        let folder = folder_service::add_folder(&db.conn, &working_dir)
            .await
            .expect("folder");
        let parent = conversation_service::create(
            &db.conn,
            folder.id,
            AgentType::ClaudeCode,
            Some("parent".into()),
            None,
        )
        .await
        .expect("parent conversation");
        let source_task_id = "source-task";
        let child = conversation_service::create_with_delegation(
            &db.conn,
            folder.id,
            AgentType::ClaudeCode,
            Some("source child".into()),
            None,
            Some(DelegationLink {
                parent_conversation_id: parent.id,
                parent_tool_use_id: "source-tool".into(),
                delegation_call_id: source_task_id.into(),
                admission: None,
            }),
        )
        .await
        .expect("child conversation");
        conversation_service::bind_external_id(&db.conn, child.id, "source-session", &[])
            .await
            .expect("bind source session");

        let source_binding = ledger::ResumeBinding {
            agent_type: AgentType::ClaudeCode,
            external_session_id: "source-session".into(),
            child_conversation_id: child.id,
            working_dir: working_dir.clone(),
            preferred_mode_id: Some("plan".into()),
            preferred_config_values: BTreeMap::from([("model".into(), "source-model".into())]),
            config_fingerprint: "source-config".into(),
        };
        let admitted = ledger::admit(
            &db.conn,
            ledger::AdmissionInput {
                task_id: source_task_id.into(),
                parent_conversation_id: parent.id,
                child_conversation_id: child.id,
                source_task_id: None,
                task: "first round".into(),
                requested_working_dir: Some(working_dir.clone()),
                resume_binding: source_binding,
            },
        )
        .await
        .expect("admit source task");
        assert!(matches!(admitted, ledger::AdmissionResult::New { .. }));
        let source_report = DelegationTaskReport {
            task_id: Some(source_task_id.into()),
            status: TaskStatus::Completed,
            child_conversation_id: Some(child.id),
            agent_type: Some(AgentType::ClaudeCode),
            text: Some("first round complete".into()),
            error_code: None,
            message: None,
            duration_ms: Some(1),
            blocked_on: None,
        };
        assert!(
            ledger::finish(&db.conn, parent.id, source_task_id, &source_report)
                .await
                .expect("finish source task")
        );
        assert!(ledger::mark_released(&db.conn, parent.id, source_task_id)
            .await
            .expect("release source task"));
        db.conn.close().await.expect("close source database");

        let reopened_conn = Database::connect(format!(
            "sqlite:{}?mode=rwc",
            dir.path().join("source.db").to_string_lossy()
        ))
        .await
        .expect("reopen database");
        let reopened = Arc::new(crate::db::AppDatabase {
            conn: reopened_conn,
        });
        let log = dir.path().join("reopened-wire.jsonl");
        let spawner = Arc::new(FixtureContinuationSpawner {
            db: Arc::clone(&reopened),
            working_dir: dir.path().to_path_buf(),
            log: log.clone(),
            child_conversation_id: child.id,
            connection: tokio::sync::Mutex::new(None),
        });
        let broker = DelegationBroker::new(
            spawner.clone() as Arc<dyn ConnectionSpawner>,
            Arc::new(DbDepthLookup {
                db: Arc::clone(&reopened),
            }),
        )
        .with_ledger(Arc::clone(&reopened));
        broker
            .set_config(DelegationConfig {
                enabled: true,
                ..DelegationConfig::default()
            })
            .await;

        let ack = broker
            .start_delegation(DelegationRequest {
                parent_connection_id: "parent-after-reopen".into(),
                parent_conversation_id: parent.id,
                parent_tool_use_id: "follow-up-tool".into(),
                agent_type: AgentType::ClaudeCode,
                task: "second round".into(),
                working_dir: Some(working_dir.clone()),
                requested_working_dir: Some(working_dir),
                continue_from_task_id: Some(source_task_id.into()),
                external_handle: None,
            })
            .await;
        assert_eq!(ack.status, TaskStatus::Running, "{ack:?}");
        assert_eq!(ack.child_conversation_id, Some(child.id));
        let successor_task_id = ack.task_id.clone().expect("successor task id");
        assert_ne!(successor_task_id, source_task_id);

        let wire = wire_methods(&log);
        let resumed = wire
            .iter()
            .position(|message| message["method"] == "session/resume")
            .expect("strict resume request");
        let prompted = wire
            .iter()
            .position(|message| message["method"] == "session/prompt")
            .expect("continuation prompt");
        assert_eq!(
            wire.iter()
                .filter(|message| message["method"] == "session/prompt")
                .count(),
            1
        );
        assert!(resumed < prompted);
        assert_eq!(wire[resumed]["params"]["sessionId"], "source-session");
        assert_eq!(wire[prompted]["params"]["sessionId"], "source-session");
        assert_eq!(
            wire[prompted]["params"]["prompt"][0]["text"],
            "second round"
        );
        assert!(!wire
            .iter()
            .any(|message| message["method"] == "session/new"));

        let successor = ledger::successor(&reopened.conn, parent.id, source_task_id)
            .await
            .expect("successor lookup")
            .expect("successor ledger row");
        assert_eq!(successor.task_id, successor_task_id);
        assert_eq!(successor.source_task_id.as_deref(), Some(source_task_id));
        assert_eq!(successor.status, TaskStatus::Running);
        assert_eq!(
            successor.resume_binding.external_session_id,
            "source-session"
        );

        broker
            .complete_call(
                &successor_task_id,
                DelegationOutcome::Ok(DelegationSuccess {
                    text: "second round complete".into(),
                    child_conversation_id: child.id,
                    child_agent_type: AgentType::ClaudeCode,
                    turn_count: 1,
                    duration_ms: 1,
                    token_usage: None,
                }),
            )
            .await;
        let terminal = ledger::lookup(&reopened.conn, parent.id, &successor_task_id)
            .await
            .expect("terminal lookup")
            .expect("terminal ledger row");
        assert_eq!(terminal.status, TaskStatus::Completed);
        assert_eq!(
            terminal.report.text.as_deref(),
            Some("second round complete")
        );
        let original = ledger::lookup(&reopened.conn, parent.id, source_task_id)
            .await
            .expect("source lookup after successor completion")
            .expect("source ledger row");
        assert_eq!(original.status, TaskStatus::Completed);
        assert_eq!(
            original.report.text.as_deref(),
            Some("first round complete")
        );
        assert!(spawner.connection.lock().await.is_none());
    });
}
