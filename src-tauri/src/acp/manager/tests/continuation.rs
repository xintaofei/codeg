use super::*;

/// Reacceptance R7: reclaiming a stuck connection must kill the whole
/// agent process tree even when the graceful command path is unavailable
/// (the receiver here is dropped — the stand-in for a driver parked in
/// the resume/load handshake that never reads `cmd_rx`), and must
/// CONFIRM the exit via the pid cell before reporting success.
/// Unix-only (relies on `sh` / `kill(2)`).
#[cfg(unix)]
#[tokio::test]
async fn disconnect_and_reclaim_kills_a_stuck_process_tree_and_confirms_exit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (mut child, gpid) = spawn_process_tree(&dir.path().join("g.pid")).await;

    let mgr = ConnectionManager::new();
    let conn = fake_connection("conn-stuck", None);
    conn.child_pid
        .store(child.id(), std::sync::atomic::Ordering::SeqCst);
    let cell = Arc::clone(&conn.child_pid);
    mgr.connections
        .lock()
        .await
        .insert("conn-stuck".to_string(), conn);
    // Stand-in for the real driver's `on_exit` zeroing: fires once the
    // process tree is dead. Watching the GRANDCHILD avoids the zombie
    // subtlety — the unreaped `sh` answers `kill(pid,0)` as alive, while
    // the reparented `sleep` disappears once the tree is truly killed.
    let tree_dead_pid = gpid;
    tokio::spawn(async move {
        for _ in 0..300 {
            if !is_alive(tree_dead_pid) {
                cell.store(0, std::sync::atomic::Ordering::SeqCst);
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    mgr.disconnect_and_reclaim("conn-stuck")
        .await
        .expect("reclaim with a confirmed exit succeeds");
    assert!(
        wait_until_dead(gpid).await,
        "grandchild {gpid} survived — the reclaim did not kill the tree"
    );
    let _ = child.wait();
    assert!(
        mgr.get_state("conn-stuck").await.is_none(),
        "the reclaimed connection must be deregistered"
    );
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
