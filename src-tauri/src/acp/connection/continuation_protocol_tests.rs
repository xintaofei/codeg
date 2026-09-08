use super::*;
use serde_json::{json, Value};
use std::{future::Future, str::FromStr, time::Duration};

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

#[test]
fn strict_resume_never_falls_back_to_new() {
    for mode in ["unsupported", "load_fail"] {
        run_on_large_stack(async move {
            let (_dir, log, state, _tx, driver, _started) = run_driver(mode).await;
            let error = tokio::time::timeout(Duration::from_secs(5), driver)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert!(error.to_string().contains("strict session recovery failed"));
            assert!(
                state.read().await.external_id.is_none(),
                "a failed strict recovery must not announce a session"
            );
            let wire = wire_methods(&log);
            assert!(!wire.iter().any(|message| message["method"] == "session/new"));
            assert!(!wire.iter().any(|message| message["method"] == "session/prompt"));
        });
    }
}

#[test]
fn immediate_text_is_reduced_before_prompt_response_every_time() {
    run_on_large_stack(async {
        let (_dir, log, state, tx, driver, started) = run_driver("resume_ok").await;
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .unwrap()
            .unwrap();

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
        tokio::time::timeout(Duration::from_secs(5), driver)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
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
        assert!(!wire.iter().any(|message| message["method"] == "session/new"));
    });
}
