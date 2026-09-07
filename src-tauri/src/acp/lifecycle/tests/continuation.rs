use super::*;

/// Reacceptance R1: a TurnComplete on a strict-attached connection that
/// was BOUND to the reserved child conversation (what
/// `ConnectionManagerContinuationRuntime::bind_child_conversation` now
/// guarantees before any prompt flows) must settle the registered
/// collaboration round with the round's own result — while the original
/// task id on the child row stays a plain frozen source (no broker is
/// even involved). Before the binding, the unbound state made this arm
/// return early and the round stayed `running` forever.
#[tokio::test]
async fn turn_complete_on_bound_strict_connection_settles_the_collaboration_round() {
    use crate::acp::delegation::broker::NoopOutcomeStore;
    use crate::acp::delegation::continuation::{
        ContinuationCoordinator, NoopRuntime as ContinuationNoopRuntime,
    };
    use crate::db::entities::conversation::{self as conv_entity, ConversationKind};
    use crate::db::service::collaboration_service;
    use sea_orm::{ActiveModelTrait, NotSet, Set};

    let db = test_helpers::fresh_in_memory_db().await;
    let folder_id = test_helpers::seed_folder(&db, "/tmp/r1-lifecycle").await;
    let now = chrono::Utc::now();
    let child = conv_entity::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        title: Set(Some("r1-child".into())),
        title_locked: Set(false),
        agent_type: Set("claude_code".into()),
        status: Set(ConversationStatus::Completed),
        kind: Set(ConversationKind::Delegate),
        model: Set(None),
        git_branch: Set(None),
        external_id: Set(Some("ext-r1".into())),
        parent_id: Set(Some(1)),
        parent_tool_use_id: Set(Some("pt-r1".into())),
        delegation_call_id: Set(Some("task-r1".into())),
        message_count: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        pinned_at: Set(None),
        origin_cwd: Set(None),
    };
    let cid = child.insert(&db.conn).await.expect("child row").id;

    let coordinator = Arc::new(ContinuationCoordinator::new(
        Arc::new(crate::db::AppDatabase {
            conn: db.conn.clone(),
        }),
        Arc::new(ContinuationNoopRuntime),
        Arc::new(NoopOutcomeStore),
    ));
    let session = collaboration_service::upsert_session_once(
        &db.conn,
        collaboration_service::NewSession {
            id: uuid::Uuid::new_v4().to_string(),
            source_task_id: "task-r1".into(),
            parent_conversation_id: 1,
            child_conversation_id: cid,
            resume_binding_json: "{}".into(),
        },
    )
    .await
    .unwrap();
    let turn = collaboration_service::insert_turn(
        &db.conn,
        collaboration_service::NewTurn {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            ordinal: 1,
            request_id: "k1".into(),
            message: "fix the boundary".into(),
            initiator_parent_conversation_id: 1,
            initiator_tool_use_id: None,
        },
    )
    .await
    .unwrap();
    collaboration_service::cas_turn_state(&db.conn, &turn.id, None, &["accepted"], "running", true)
        .await
        .unwrap();
    coordinator
        .register_execution_for_test(
            "strict-conn",
            &turn.id,
            &turn.execution_id,
            &session.id,
            "parent-conn",
        )
        .await;

    // The strict connection, BOUND to the child row (the R1 latch), with
    // the round's last assistant text on its state.
    let mgr = ConnectionManager::new();
    {
        let mut map = mgr.connections.lock().await;
        let conn = fake_connection_with_state("strict-conn", Some(cid));
        conn.state.write().await.last_assistant_text = Some("REWORK RESULT".into());
        map.insert("strict-conn".to_string(), conn);
    }
    let env = EventEnvelope {
        seq: 1,
        connection_id: "strict-conn".to_string(),
        payload: AcpEvent::TurnComplete {
            session_id: "ext-r1".into(),
            stop_reason: "end_turn".into(),
            agent_type: "claude_code".into(),
        },
    };
    handle_event(&db.conn, &mgr, &env, None, Some(&coordinator))
        .await
        .unwrap();

    let settled = collaboration_service::find_turn(&db.conn, &turn.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        settled.state, "completed",
        "the bound connection's terminal must settle the round (R1)"
    );
    assert_eq!(settled.result_text.as_deref(), Some("REWORK RESULT"));
    // The child row itself advanced to PendingReview like any turn.
    assert_eq!(
        read_row_status(&db, cid).await,
        ConversationStatus::PendingReview
    );
}
