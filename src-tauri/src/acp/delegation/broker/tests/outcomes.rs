use super::*;

#[test]
fn resume_binding_json_holds_identities_only_and_requires_verifiable_facts() {
    // Missing the external session id or the config identity → null
    // binding (never a guess).
    assert_eq!(
        build_resume_binding_json(
            AgentType::ClaudeCode,
            &ResumeBindingFacts {
                external_session_id: None,
                cwd: Some("/work".into()),
                config_fingerprint: Some("fp".into()),
            }
        ),
        None
    );
    assert_eq!(
        build_resume_binding_json(
            AgentType::ClaudeCode,
            &ResumeBindingFacts {
                external_session_id: Some("ext-1".into()),
                cwd: Some("/work".into()),
                config_fingerprint: None,
            }
        ),
        None
    );

    let json = build_resume_binding_json(
        AgentType::ClaudeCode,
        &ResumeBindingFacts {
            external_session_id: Some("ext-1".into()),
            cwd: Some("/work".into()),
            config_fingerprint: Some("fp".into()),
        },
    )
    .expect("binding");
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["agent_type"], "claude_code");
    assert_eq!(value["external_session_id"], "ext-1");
    assert_eq!(value["cwd"], "/work");
    assert_eq!(value["config_fingerprint"], "fp");
    // Identities only — the key set is closed.
    let keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "agent_type",
            "config_fingerprint",
            "cwd",
            "external_session_id",
            "schema_version"
        ]
    );
}

#[test]
fn frozen_report_projects_the_original_bounded_text() {
    let row = crate::db::service::delegation_outcome_service::DelegationOutcomeRow {
        task_id: "task-1".into(),
        parent_conversation_id: 1,
        parent_tool_use_id: Some("pt-1".into()),
        child_conversation_id: Some(42),
        agent_type: "claude_code".into(),
        text: "原始成功结果".into(),
        duration_ms: 55,
        text_truncated: false,
        completed_at: chrono::Utc::now(),
        schema_version: 1,
        resume_binding_json: None,
    };
    let report = frozen_report("task-1", &row);
    assert_eq!(report.status, TaskStatus::Completed);
    assert_eq!(report.text.as_deref(), Some("原始成功结果"));
    assert_eq!(report.child_conversation_id, Some(42));
    assert_eq!(report.agent_type, Some(AgentType::ClaudeCode));
    assert_eq!(report.duration_ms, Some(55));
    assert_eq!(report.error_code, None);
    assert_eq!(report.blocked_on, None);
}

/// Acceptance F1 regression (was `review_late_old_completion_…`): a
/// canceled task resumed under the SAME id on a NEW connection must not
/// be consumed by the OLD connection's late completion. The late
/// terminal is rejected at the pending lock; the resumed execution keeps
/// running, completes with ITS text, and no teardown fires on the new
/// connection.
#[tokio::test]
async fn late_old_completion_must_not_freeze_resumed_execution() {
    let (mock, _lookup, broker) = resume_harness(Some(resume_ctx(TaskStatus::Canceled))).await;
    mock.queue_spawn(Ok("child-conn-1".into())).await;
    mock.queue_send(Ok(42)).await;
    let ack = broker.start_delegation(request(1, "pt-orig")).await;
    let task_id = ack.task_id.unwrap();
    assert_eq!(ack.status, TaskStatus::Running);

    // Cancel + resume under the same id on a new connection.
    broker
        .cancel_task_by_id("parent-conn", Some(1), &task_id)
        .await;
    mock.queue_resume_spawn(Ok(ResumedSpawn::fresh("child-conn-2")))
        .await;
    mock.queue_resume_send(Ok(())).await;
    let ack = broker.resume_delegation(resume_request(&task_id)).await;
    assert_eq!(ack.status, TaskStatus::Running);

    // The OLD connection's terminal arrives LATE. It must be rejected.
    let accepted = broker
        .complete_call_for_connection(
            "child-conn-1",
            &task_id,
            DelegationOutcome::Ok(DelegationSuccess {
                text: "OLD C1 RESULT".into(),
                child_conversation_id: 42,
                child_agent_type: AgentType::ClaudeCode,
                turn_count: 1,
                duration_ms: 1,
                token_usage: None,
            }),
        )
        .await;
    assert_eq!(accepted, CompleteCallResult::RejectedStale);
    // The stale terminal must not have torn down the NEW connection
    // (the canceled OLD connection was disconnected by the cancel itself).
    assert!(!mock
        .disconnects
        .lock()
        .await
        .contains(&"child-conn-2".to_string()));

    // The resumed execution is STILL running and finishes with ITS text.
    let status = broker
        .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
        .await;
    assert_eq!(status.status, TaskStatus::Running);
    broker
        .complete_call_for_connection(
            "child-conn-2",
            &task_id,
            DelegationOutcome::Ok(DelegationSuccess {
                text: "NEW C2 RESULT".into(),
                child_conversation_id: 42,
                child_agent_type: AgentType::ClaudeCode,
                turn_count: 1,
                duration_ms: 2,
                token_usage: None,
            }),
        )
        .await;
    let status = broker
        .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
        .await;
    assert_eq!(status.status, TaskStatus::Completed);
    assert_eq!(status.text.as_deref(), Some("NEW C2 RESULT"));
}

/// Reacceptance R2 regression: while a resumed delegation is still in its
/// SETUP phase (prompt sent, park parked at the metadata write — the
/// setup reservation is live), the CURRENT connection's early completion
/// is buffered, and a LATE terminal from the OLD (superseded) connection
/// must NOT evict it. The old event is dropped at the buffer entry, the
/// park resolves the resume with the CURRENT execution's result, and the
/// frozen outcome carries that result.
#[tokio::test]
async fn late_old_early_terminal_must_not_replace_current_completion() {
    struct ParkGate {
        gate: tokio::sync::Mutex<
            Option<(
                tokio::sync::oneshot::Sender<()>,
                tokio::sync::oneshot::Receiver<()>,
            )>,
        >,
    }
    #[async_trait::async_trait]
    impl DelegationMetaWriter for ParkGate {
        async fn write_meta(&self, _parent: &str, _tool: &str, _meta: serde_json::Value) {
            let gate = self.gate.lock().await.take();
            if let Some((entered, release)) = gate {
                let _ = entered.send(());
                let _ = release.await;
            }
        }
    }

    let mock = Arc::new(MockSpawner::new());
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let broker = DelegationBroker::with_meta_writer(
        mock.clone() as Arc<dyn ConnectionSpawner>,
        shallow_lookup(),
        Arc::new(ParkGate {
            gate: tokio::sync::Mutex::new(Some((entered_tx, release_rx))),
        }),
    )
    .with_status_lookup(Arc::new(MockResumeLookup {
        ctx: tokio::sync::Mutex::new(Some(resume_ctx(TaskStatus::Canceled))),
    }));
    enable_delegation(&broker).await;
    mock.queue_resume_spawn(Ok(ResumedSpawn::fresh("new-C2")))
        .await;
    mock.queue_resume_send(Ok(())).await;

    let driver = {
        let broker = broker.clone();
        tokio::spawn(async move { broker.resume_delegation(resume_request("task-1")).await })
    };
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await
        .expect("the resume reached its metadata write")
        .unwrap();

    let ok = |text: &str| {
        DelegationOutcome::Ok(DelegationSuccess {
            text: text.into(),
            child_conversation_id: 42,
            child_agent_type: AgentType::ClaudeCode,
            turn_count: 1,
            duration_ms: 1,
            token_usage: None,
        })
    };
    // The CURRENT connection's completion lands first (buffered)…
    let current = broker
        .complete_call_for_connection("new-C2", "task-1", ok("NEW C2 RESULT"))
        .await;
    assert_eq!(current, CompleteCallResult::Buffered);
    // …then the OLD connection's late terminal tries to replace it.
    let stale = broker
        .complete_call_for_connection("old-C1", "task-1", ok("OLD C1 RESULT"))
        .await;
    assert_eq!(
        stale,
        CompleteCallResult::DroppedStale,
        "the superseded connection's terminal must be dropped at the buffer entry"
    );

    release_tx.send(()).unwrap();
    let report = driver.await.unwrap();
    assert_eq!(
        report.status,
        TaskStatus::Completed,
        "the current execution's buffered completion must win the park"
    );
    let status = broker
        .get_task_status("parent-conn", Some(1), "task-1", StatusWait::Immediate)
        .await;
    assert_eq!(status.text.as_deref(), Some("NEW C2 RESULT"));
}
