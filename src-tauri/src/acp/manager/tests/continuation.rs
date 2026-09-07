use super::*;

#[tokio::test]
async fn requested_external_id_blocks_manager_resume_admission_until_release_proof() {
    use crate::acp::connection::lifetime::{
        ConnectionProcessLifetime, ConnectionResource,
    };
    use crate::acp::delegation::continuation::StrictAttachErrorCode;

    let mgr = ConnectionManager::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path().to_path_buf();
    let cwd_text = cwd.to_string_lossy().into_owned();
    let connection_id = "pre-handshake-owner";
    let external_id = "requested-before-session-started";
    let agent_type = AgentType::Custom("task6b-missing-agent");
    let _cmd_rx = insert_live_connection(
        &mgr,
        connection_id,
        agent_type,
        Some(cwd.clone()),
    )
    .await;
    let (cmd_tx, state) = {
        let active = mgr.connections.lock().await;
        let connection = active.get(connection_id).expect("active owner");
        (connection.cmd_tx.clone(), Arc::clone(&connection.state))
    };
    assert!(
        state.read().await.external_id.is_none(),
        "fixture must cover the pre-SessionStarted identity gap"
    );

    let lifetime = ConnectionProcessLifetime::new();
    let resource = ConnectionResource::new(
        connection_id.to_string(),
        cmd_tx,
        Arc::clone(&lifetime),
        state,
        agent_type,
        cwd.clone(),
        Some(external_id.to_string()),
        None,
        true,
    );
    mgr.resources.insert(Arc::clone(&resource)).await;
    mgr.resources.retire_when_confirmed(Arc::clone(&resource));

    // The ordinary path does not launch a second process while the original
    // connection is active, even though its observed external id is absent.
    let reused = mgr
        .spawn_agent(
            agent_type,
            Some(cwd_text.clone()),
            Some(external_id.to_string()),
            BTreeMap::new(),
            "test-window".into(),
            EventEmitter::Noop,
            None,
            BTreeMap::new(),
        )
        .await
        .expect("ordinary resume reuses requested-id owner");
    assert_eq!(reused, connection_id);

    // Strict continuation ownership may never share that same live driver.
    let fingerprint = crate::commands::acp::fingerprint_config(agent_type, &BTreeMap::new());
    let strict_err = mgr
        .attach_existing_session_strict(
            agent_type,
            cwd_text.clone(),
            external_id.to_string(),
            BTreeMap::new(),
            "test-window".into(),
            EventEmitter::Noop,
            None,
            BTreeMap::new(),
            cwd.clone(),
            fingerprint,
            Duration::from_millis(20),
            "turn-admission",
            "exec-admission",
        )
        .await
        .expect_err("strict resume refuses the live requested-id owner");
    assert_eq!(strict_err.code, StrictAttachErrorCode::ResumeFailed);

    // Once the active entry disappears, the retained resource remains the
    // admission fence until both driver exit and no-spawn/reap are proven.
    mgr.connections.lock().await.remove(connection_id);
    let draining_err = mgr
        .spawn_agent(
            agent_type,
            Some(cwd_text.clone()),
            Some(external_id.to_string()),
            BTreeMap::new(),
            "test-window".into(),
            EventEmitter::Noop,
            None,
            BTreeMap::new(),
        )
        .await
        .expect_err("ordinary resume refuses an unreleased retained owner");
    assert!(draining_err.to_string().contains("being reclaimed"));

    lifetime.mark_driver_exited();
    let after_release = mgr
        .spawn_agent(
            agent_type,
            Some(cwd_text),
            Some(external_id.to_string()),
            BTreeMap::new(),
            "test-window".into(),
            EventEmitter::Noop,
            None,
            BTreeMap::new(),
        )
        .await
        .expect_err("missing custom agent fails only after admission succeeds");
    assert!(
        !after_release.to_string().contains("being reclaimed"),
        "release proof must reopen admission: {after_release}"
    );
    assert!(mgr.resources.get(connection_id).await.is_none());
}

#[tokio::test]
async fn terminal_requested_id_entries_fence_ordinary_resume_until_release_proof() {
    use crate::acp::connection::lifetime::{
        ConnectionProcessLifetime, ConnectionResource,
    };

    for terminal in [ConnectionStatus::Error, ConnectionStatus::Disconnected] {
        let mgr = ConnectionManager::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().to_path_buf();
        let cwd_text = cwd.to_string_lossy().into_owned();
        let connection_id = format!("terminal-{terminal:?}");
        let external_id = format!("requested-{terminal:?}");
        let agent_type = AgentType::Custom("task6b-terminal-missing-agent");
        let _cmd_rx = insert_live_connection(
            &mgr,
            &connection_id,
            agent_type,
            Some(cwd.clone()),
        )
        .await;
        let (cmd_tx, state) = {
            let active = mgr.connections.lock().await;
            let connection = active.get(&connection_id).expect("active terminal entry");
            (connection.cmd_tx.clone(), Arc::clone(&connection.state))
        };
        {
            let mut state = state.write().await;
            state.status = terminal.clone();
            assert!(state.external_id.is_none());
        }
        let lifetime = ConnectionProcessLifetime::new();
        lifetime.mark_driver_running();
        let resource = ConnectionResource::new(
            connection_id.clone(),
            cmd_tx,
            Arc::clone(&lifetime),
            state,
            agent_type,
            cwd,
            Some(external_id.clone()),
            None,
            true,
        );
        mgr.resources.insert(Arc::clone(&resource)).await;
        mgr.resources.retire_when_confirmed(Arc::clone(&resource));

        let fenced = mgr
            .spawn_agent(
                agent_type,
                Some(cwd_text.clone()),
                Some(external_id.clone()),
                BTreeMap::new(),
                "test-window".into(),
                EventEmitter::Noop,
                None,
                BTreeMap::new(),
            )
            .await
            .expect_err("terminal entry with a live driver must fence a new launch");
        assert!(
            fenced.to_string().contains("being reclaimed"),
            "{terminal:?} returned the stale connection or bypassed the fence: {fenced}"
        );

        lifetime.mark_driver_exited();
        let after_release = mgr
            .spawn_agent(
                agent_type,
                Some(cwd_text),
                Some(external_id),
                BTreeMap::new(),
                "test-window".into(),
                EventEmitter::Noop,
                None,
                BTreeMap::new(),
            )
            .await
            .expect_err("missing custom agent is reached after release admission");
        assert!(
            !after_release.to_string().contains("being reclaimed"),
            "{terminal:?} release proof did not reopen admission: {after_release}"
        );
        assert!(mgr.resources.get(&connection_id).await.is_none());
        assert!(mgr.get_state(&connection_id).await.is_none());
    }
}

#[tokio::test]
async fn pid_zero_while_driver_can_spawn_times_out_retains_and_retries() {
    use crate::acp::connection::lifetime::{
        ConnectionProcessLifetime, ConnectionResource,
    };
    use crate::acp::session_state::SessionState;

    let mgr = ConnectionManager::new();
    let connection_id = "delayed-before-spawn";
    let cwd = std::env::current_dir().expect("cwd");
    let state = Arc::new(tokio::sync::RwLock::new(SessionState::new(
        connection_id.to_string(),
        AgentType::ClaudeCode,
        Some(cwd.clone()),
        "test-window".to_string(),
        None,
    )));
    let lifetime = ConnectionProcessLifetime::new();
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(1);
    let resource = ConnectionResource::new(
        connection_id.to_string(),
        cmd_tx,
        Arc::clone(&lifetime),
        state,
        AgentType::ClaudeCode,
        cwd.clone(),
        Some("delayed-external".to_string()),
        None,
        false,
    );
    mgr.resources.insert(Arc::clone(&resource)).await;
    mgr.resources.retire_when_confirmed(Arc::clone(&resource));

    let first = mgr
        .disconnect_and_reclaim_with_timing(
            connection_id,
            Duration::from_millis(5),
            Duration::from_millis(30),
        )
        .await;
    assert!(first.is_err(), "zero pid while driver may spawn is unconfirmed");
    assert!(mgr.resources.get(connection_id).await.is_some());
    assert!(mgr
        .resources
        .find_target(AgentType::ClaudeCode, &cwd, "delayed-external")
        .await
        .is_some());

    lifetime.mark_driver_exited();
    mgr.disconnect_and_reclaim(connection_id)
        .await
        .expect("retry consumes the retained exact resource");
    assert!(mgr.resources.get(connection_id).await.is_none());
}

/// Reacceptance R7: reclaiming a real stuck transport must force its whole
/// process tree, then wait for both the vendored owned-Child reaper callback
/// and the real connection driver guard before reporting success.
/// Unix-only (relies on sh / kill(2)).
#[cfg(unix)]
#[tokio::test]
async fn disconnect_and_reclaim_kills_a_stuck_process_tree_and_confirms_exit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mgr = ConnectionManager::new();
    let root_file = dir.path().join("root.pid");
    let descendant_file = dir.path().join("descendant.pid");
    let ready_file = dir.path().join("ready");
    let script = format!(
        "trap '' TERM HUP; echo $$ > '{}'; \
         (trap '' TERM HUP; while :; do :; done) & echo $! > '{}'; \
         echo ready > '{}'; wait",
        root_file.display(),
        descendant_file.display(),
        ready_file.display(),
    );
    let agent = sacp_tokio::AcpAgent::from_args(["/bin/sh", "-c", &script])
        .expect("fixture agent");
    crate::acp::connection::spawn_agent_connection_with_transport_managed(
        agent,
        "conn-stuck".to_string(),
        AgentType::ClaudeCode,
        Some(dir.path().to_string_lossy().into_owned()),
        Some("fixture-session".to_string()),
        BTreeMap::new(),
        "test-window".to_string(),
        EventEmitter::Noop,
        Arc::clone(&mgr.connections),
        mgr.resources.clone(),
        None,
        BTreeMap::new(),
        None,
        mgr.terminal_shell_config(),
        "fixture-fingerprint".to_string(),
        Arc::new(crate::acp::stderr_tail::StderrTail::new()),
        crate::acp::delegation::continuation::SessionRecovery::AllowNewFallback,
    )
    .await
    .expect("spawn real fixture transport");

    for _ in 0..200 {
        if ready_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ready_file.exists(), "fixture process tree did not become ready");
    let root_pid: i32 = std::fs::read_to_string(&root_file)
        .expect("root pid")
        .trim()
        .parse()
        .expect("numeric root pid");
    let descendant_pid: i32 = std::fs::read_to_string(&descendant_file)
        .expect("descendant pid")
        .trim()
        .parse()
        .expect("numeric descendant pid");

    let reclaim = mgr.disconnect_and_reclaim("conn-stuck").await;
    let root_dead = wait_until_dead(root_pid).await;
    let descendant_dead = wait_until_dead(descendant_pid).await;
    // Fixture cleanup is restricted to the exact PIDs recorded while the owned
    // root was alive.
    if !root_dead {
        unsafe { libc::kill(root_pid, libc::SIGKILL) };
    }
    if !descendant_dead {
        unsafe { libc::kill(descendant_pid, libc::SIGKILL) };
    }
    reclaim.expect("reclaim with actual driver + reaper proof succeeds");
    assert!(root_dead, "owned root {root_pid} survived reclaim");
    assert!(
        descendant_dead,
        "TERM-ignoring descendant {descendant_pid} survived reclaim"
    );
    assert!(
        mgr.get_state("conn-stuck").await.is_none(),
        "the reclaimed connection must be deregistered"
    );
    assert_eq!(mgr.resources.len().await, 0, "confirmed resource retires");
}

/// Reacceptance R8 + R1: once a child conversation is reserved by an
/// open collaboration session, EVERY ordinary prompt entry refuses it —
/// including the already-linked path with the conversation id OMITTED
/// (previously the bypass) — while the continuation runtime's explicit
/// privileged send still flows, after binding the strict connection to
/// the child row (the R1 latch the lifecycle's settlement routing reads)
/// and flipping the row to InProgress.
#[tokio::test]
async fn reserved_child_refuses_ordinary_entries_but_continuation_send_flows() {
    use crate::acp::delegation::continuation::runtime::AttachTarget;
    use crate::acp::delegation::continuation::ContinuationRuntime as _;
    use crate::db::service::collaboration_service;
    use crate::db::test_helpers;
    use sea_orm::{ActiveModelTrait, NotSet, Set};

    let db = test_helpers::fresh_in_memory_db().await;
    let folder_id = test_helpers::seed_folder(&db, "/tmp/r8-res").await;
    let mgr = ConnectionManager::new();
    let stack_dir = tempfile::tempdir().expect("stack dir");
    let (_b, _t, _s, _f, _a, _si, _c, coordinator) = crate::app_state::build_delegation_stack(
        &mgr,
        db.conn.clone(),
        stack_dir.path().to_path_buf(),
    );
    coordinator.set_enabled(true).await;

    // The child conversation row holding the external session.
    let now = chrono::Utc::now();
    let child = conversation::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        title: Set(Some("reserved-child".into())),
        title_locked: Set(false),
        agent_type: Set("claude_code".into()),
        status: Set(ConversationStatus::Completed),
        kind: Set(ConversationKind::Delegate),
        model: Set(None),
        git_branch: Set(None),
        external_id: Set(Some("ext-r8".into())),
        parent_id: Set(Some(1)),
        parent_tool_use_id: Set(Some("pt-r8".into())),
        delegation_call_id: Set(Some("task-r8".into())),
        message_count: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        pinned_at: Set(None),
        origin_cwd: Set(None),
    };
    let cid = child.insert(&db.conn).await.expect("child row").id;

    // The open collaboration session that reserves the child.
    collaboration_service::upsert_session_once(
        &db.conn,
        collaboration_service::NewSession {
            id: uuid::Uuid::new_v4().to_string(),
            source_task_id: "task-r8".into(),
            parent_conversation_id: 1,
            child_conversation_id: cid,
            resume_binding_json: "{}".into(),
        },
    )
    .await
    .expect("session");

    // An ALREADY-LINKED idle connection to the reserved child (the
    // original user connection from before the admission).
    let mut ordinary_rx =
        insert_live_connection(&mgr, "ordinary-conn", AgentType::ClaudeCode, None).await;
    {
        let state = mgr.get_state("ordinary-conn").await.unwrap();
        state.write().await.conversation_id = Some(cid);
    }

    let blocks = || {
        vec![PromptInputBlock::Text {
            text: "ordinary write".into(),
        }]
    };
    // Plain entry with the id omitted: previously the bypass (R8).
    let err = mgr
        .send_prompt("ordinary-conn", blocks())
        .await
        .expect_err("plain send");
    assert!(
        err.to_string().contains("session_reserved_for_delegation"),
        "plain entry: {err}"
    );
    // Linked entry with the id omitted on an already-linked connection.
    let err = mgr
        .send_prompt_linked(&db, "ordinary-conn", blocks(), None, None, None)
        .await
        .expect_err("linked send, omitted id");
    assert!(
        err.to_string().contains("session_reserved_for_delegation"),
        "linked omitted-id entry: {err}"
    );
    // Linked entry naming the reserved row explicitly.
    let err = mgr
        .send_prompt_linked(
            &db,
            "ordinary-conn",
            blocks(),
            Some(folder_id),
            Some(cid),
            None,
        )
        .await
        .expect_err("linked send, explicit id");
    assert!(
        err.to_string().contains("session_reserved_for_delegation"),
        "linked explicit-id entry: {err}"
    );
    // A caller-supplied target that disagrees with the binding.
    let err = mgr
        .send_prompt_linked(
            &db,
            "ordinary-conn",
            blocks(),
            Some(folder_id),
            Some(cid + 999),
            None,
        )
        .await
        .expect_err("mismatched target");
    assert!(
        err.to_string().contains("already linked to conversation"),
        "mismatched target: {err}"
    );
    // Nothing was enqueued on the ordinary connection.
    assert!(ordinary_rx.try_recv().is_err());

    // The continuation runtime: bind a fresh strict connection to the
    // child row (R1), then the privileged send flows.
    let mgr_arc = Arc::new(mgr);
    let runtime = ConnectionManagerContinuationRuntime {
        manager: mgr_arc.clone(),
        db: Arc::new(crate::db::AppDatabase {
            conn: db.conn.clone(),
        }),
        data_dir: Arc::new(stack_dir.path().to_path_buf()),
    };
    let mut strict_rx =
        insert_live_connection(&mgr_arc, "strict-conn", AgentType::ClaudeCode, None).await;
    let target = AttachTarget {
        agent_type: AgentType::ClaudeCode,
        external_session_id: "ext-r8".into(),
        cwd: "/tmp/r8-res".into(),
        config_fingerprint: "fp".into(),
    };
    runtime
        .bind_child_conversation("strict-conn", cid, &target)
        .await
        .expect("bind");
    {
        let state = mgr_arc.get_state("strict-conn").await.unwrap();
        assert_eq!(
            state.read().await.conversation_id,
            Some(cid),
            "the bind latched the conversation identity (R1)"
        );
    }
    // A drifted row (wrong external session) must refuse the bind.
    let drifted = AttachTarget {
        external_session_id: "ext-OTHER".into(),
        ..target.clone()
    };
    assert!(
        runtime
            .bind_child_conversation("strict-conn", cid, &drifted)
            .await
            .is_err(),
        "a drifted external id must not rebind"
    );

    runtime
        .send_prompt("strict-conn", "the rework instruction")
        .await
        .expect("privileged continuation send flows");
    match strict_rx.recv().await {
        Some(crate::acp::connection::ConnectionCommand::Prompt { .. }) => {}
        Some(_) => panic!("expected the rework prompt on the wire"),
        None => panic!("the strict connection's command channel closed"),
    }
    let row = conversation::Entity::find_by_id(cid)
        .one(&db.conn)
        .await
        .expect("row")
        .expect("row exists");
    assert_eq!(
        row.status,
        ConversationStatus::InProgress,
        "the privileged send flipped the child row to InProgress"
    );
}
