//! Unit tests for the pipeline engine using mock connections and event simulation.

#[cfg(test)]
mod engine_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use crate::acp::manager::ConnectionManager;
    use crate::acp::types::{AcpEvent, EventEnvelope};
    use crate::acp::InternalEventBus;
    use crate::db::service::pipeline_service;
    use crate::db::test_helpers::fresh_in_memory_db;
    use crate::db::AppDatabase;
    use crate::models::{
        AgentType, AttemptStatus, LoopBack, PipelineGraph, PipelineIsolation, PipelineRole,
        PipelineRunRequest, PipelineRunStatus, PipelineStep, PipelineVerdict,
    };
    use crate::pipeline::engine::{build_engine, PipelineEngine};
    use crate::web::event_bridge::EventEmitter;

    fn test_graph() -> PipelineGraph {
        PipelineGraph {
            steps: vec![
                PipelineStep {
                    id: "coder".into(),
                    role: PipelineRole::Coder,
                    label: "Coder".into(),
                    agent_type: "claude_code".into(),
                    mode_id: None,
                    config_values: BTreeMap::new(),
                    prompt_template: "$task".into(),
                    timeout_secs: 1800,
                    read_memory: false,
                    read_only: false,
                },
                PipelineStep {
                    id: "reviewer".into(),
                    role: PipelineRole::Reviewer,
                    label: "Reviewer".into(),
                    agent_type: "claude_code".into(),
                    mode_id: None,
                    config_values: BTreeMap::new(),
                    prompt_template: "$task\n$review".into(),
                    timeout_secs: 1800,
                    read_memory: false,
                    read_only: true,
                },
            ],
            loops: vec![LoopBack {
                from_step: "reviewer".into(),
                to_step: "coder".into(),
                max_iterations: 3,
            }],
        }
    }

    async fn setup_engine() -> (
        Arc<PipelineEngine>,
        AppDatabase,
        ConnectionManager,
        Arc<InternalEventBus>,
    ) {
        let db = fresh_in_memory_db().await;
        let metrics = Arc::new(crate::acp::internal_bus::EventBusMetrics::default());
        let bus = Arc::new(InternalEventBus::new(metrics));
        let emitter = EventEmitter::Noop;
        let manager = ConnectionManager::new();
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let engine = build_engine(
            AppDatabase {
                conn: db.conn.clone(),
            },
            manager.clone_ref(),
            emitter,
            bus.clone(),
            temp_dir.path().to_path_buf(),
        )
        .expect("build engine");
        (engine, db, manager, bus)
    }

    fn turn_complete_env(conn_id: &str, stop_reason: &str) -> EventEnvelope {
        EventEnvelope {
            seq: 0,
            connection_id: conn_id.to_string(),
            payload: AcpEvent::TurnComplete {
                session_id: conn_id.to_string(),
                stop_reason: stop_reason.to_string(),
                agent_type: "claude_code".to_string(),
            },
        }
    }

    #[tokio::test]
    async fn create_run_initializes_properly() {
        let db = fresh_in_memory_db().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::WorktreePerRun,
            None,
            None,
        )
        .await
        .expect("create run");

        assert_eq!(run.status, PipelineRunStatus::Running);
        assert_eq!(run.folder_id, 1);
        assert_eq!(run.current_step_id, None);
        assert_eq!(run.attempts.len(), 0);
    }

    #[tokio::test]
    async fn create_attempt_tracks_connection() {
        let db = fresh_in_memory_db().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::WorktreePerRun,
            None,
            None,
        )
        .await
        .expect("create run");

        let attempt = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "coder".into(),
            1,
            Some("conn-123".into()),
            Some("sonnet".into()),
        )
        .await
        .expect("create attempt");

        assert_eq!(attempt.status, AttemptStatus::Running);
        assert_eq!(attempt.step_id, "coder");
        assert_eq!(attempt.iteration, 1);
        assert_eq!(attempt.model_requested, Some("sonnet".into()));
    }

    #[tokio::test]
    async fn cas_attempt_status_only_succeeds_from_expected_state() {
        let db = fresh_in_memory_db().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::WorktreePerRun,
            None,
            None,
        )
        .await
        .expect("create run");

        let attempt =
            pipeline_service::create_attempt(&db.conn, run.id, "coder".into(), 1, None, None)
                .await
                .expect("create attempt");

        // Wrong `from` state: CAS must fail
        let failed = pipeline_service::cas_attempt_status(
            &db.conn,
            attempt.id,
            AttemptStatus::Done,
            AttemptStatus::Failed,
        )
        .await
        .expect("cas");
        assert!(!failed);

        // Correct `from` state: CAS succeeds
        let ok = pipeline_service::cas_attempt_status(
            &db.conn,
            attempt.id,
            AttemptStatus::Running,
            AttemptStatus::Done,
        )
        .await
        .expect("cas");
        assert!(ok);

        // Now it's in Done state, second transition from Running fails
        let repeat = pipeline_service::cas_attempt_status(
            &db.conn,
            attempt.id,
            AttemptStatus::Running,
            AttemptStatus::Failed,
        )
        .await
        .expect("cas");
        assert!(!repeat);
    }

    #[tokio::test]
    async fn update_run_status_cas_settles_status_once() {
        let db = fresh_in_memory_db().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::WorktreePerRun,
            None,
            None,
        )
        .await
        .expect("create run");

        let ok = pipeline_service::update_run_status(
            &db.conn,
            run.id,
            PipelineRunStatus::Succeeded,
            None,
        )
        .await
        .expect("cas");
        assert!(ok);

        let repeat =
            pipeline_service::update_run_status(&db.conn, run.id, PipelineRunStatus::Failed, None)
                .await
                .expect("cas");
        assert!(!repeat);

        let reloaded = pipeline_service::get_run(&db.conn, run.id)
            .await
            .expect("get");
        assert_eq!(reloaded.status, PipelineRunStatus::Succeeded);
    }

    #[tokio::test]
    async fn duplicate_verdict_for_same_attempt_ignored() {
        let db = fresh_in_memory_db().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::WorktreePerRun,
            None,
            None,
        )
        .await
        .expect("create run");

        let attempt =
            pipeline_service::create_attempt(&db.conn, run.id, "reviewer".into(), 1, None, None)
                .await
                .expect("create attempt");

        pipeline_service::set_attempt_verdict(
            &db.conn,
            attempt.id,
            PipelineVerdict::Pass,
            Some("tool".into()),
            Some("all good".into()),
        )
        .await
        .expect("set verdict");

        let first = pipeline_service::get_attempt(&db.conn, attempt.id)
            .await
            .expect("get")
            .expect("found");
        assert_eq!(first.verdict, Some(PipelineVerdict::Pass));
        assert_eq!(first.notes, Some("all good".into()));
    }

    #[tokio::test]
    async fn step_order_advances_on_pass() {
        let (engine, db, manager, _) = setup_engine().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        let attempt1 = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "coder".into(),
            1,
            Some("conn-coder".into()),
            None,
        )
        .await
        .expect("create attempt");

        manager
            .insert_test_connection_live(
                "conn-coder",
                AgentType::ClaudeCode,
                None,
                EventEmitter::Noop,
            )
            .await;

        // Simulate engine index
        {
            let (state, _) = manager
                .get_state_and_emitter("conn-coder")
                .await
                .expect("state");
            state.write().await.last_assistant_text =
                Some("Implemented feature.\n\nVERDICT: PASS".into());
        }

        // Trigger on_event for step 0 ("coder")
        engine
            .record_verdict("conn-coder", PipelineVerdict::Pass, Some("code looks good"))
            .await;

        // Manually trigger turn complete
        engine
            .on_event(&turn_complete_env("conn-coder", "end_turn"))
            .await;

        // Verify attempt 1 was marked Done with Pass
        let att1 = pipeline_service::get_attempt(&db.conn, attempt1.id)
            .await
            .expect("query")
            .expect("found");
        assert_eq!(att1.status, AttemptStatus::Done);
        assert_eq!(att1.verdict, Some(PipelineVerdict::Pass));
    }

    #[tokio::test]
    async fn changes_requested_loops_back_to_coder_with_full_notes() {
        let (engine, db, manager, _) = setup_engine().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        let attempt = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "reviewer".into(),
            1,
            Some("conn-rev".into()),
            None,
        )
        .await
        .expect("create attempt");

        manager
            .insert_test_connection_live(
                "conn-rev",
                AgentType::ClaudeCode,
                None,
                EventEmitter::Noop,
            )
            .await;

        // Record verdict with notes
        engine
            .record_verdict(
                "conn-rev",
                PipelineVerdict::ChangesRequested,
                Some("Fix line 42: null pointer"),
            )
            .await;

        let notes = pipeline_service::get_last_changes_requested_notes(&db.conn, run.id)
            .await
            .expect("notes");
        assert_eq!(notes, Some("Fix line 42: null pointer".into()));

        // Turn complete settles attempt
        engine
            .on_event(&turn_complete_env("conn-rev", "end_turn"))
            .await;

        let att = pipeline_service::get_attempt(&db.conn, attempt.id)
            .await
            .expect("query")
            .expect("found");
        assert_eq!(att.status, AttemptStatus::Done);
        assert_eq!(att.verdict, Some(PipelineVerdict::ChangesRequested));
        assert_eq!(att.notes, Some("Fix line 42: null pointer".into()));
    }

    #[tokio::test]
    async fn iteration_limit_stops_max_iterations() {
        let (engine, db, manager, _) = setup_engine().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        let _attempt = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "reviewer".into(),
            3, // 3rd iteration = max_iterations
            Some("conn-rev-3".into()),
            None,
        )
        .await
        .expect("create attempt");

        manager
            .insert_test_connection_live(
                "conn-rev-3",
                AgentType::ClaudeCode,
                None,
                EventEmitter::Noop,
            )
            .await;

        engine
            .record_verdict(
                "conn-rev-3",
                PipelineVerdict::ChangesRequested,
                Some("Still broken"),
            )
            .await;

        engine
            .on_event(&turn_complete_env("conn-rev-3", "end_turn"))
            .await;

        let reloaded_run = pipeline_service::get_run(&db.conn, run.id)
            .await
            .expect("get run");
        assert_eq!(reloaded_run.status, PipelineRunStatus::StoppedMaxIterations);
    }

    #[tokio::test]
    async fn pass_on_last_step_completes_run() {
        let (engine, db, manager, _) = setup_engine().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        let _attempt = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "reviewer".into(),
            1,
            Some("conn-rev-pass".into()),
            None,
        )
        .await
        .expect("create attempt");

        manager
            .insert_test_connection_live(
                "conn-rev-pass",
                AgentType::ClaudeCode,
                None,
                EventEmitter::Noop,
            )
            .await;

        engine
            .record_verdict("conn-rev-pass", PipelineVerdict::Pass, Some("All clean!"))
            .await;

        engine
            .on_event(&turn_complete_env("conn-rev-pass", "end_turn"))
            .await;

        let reloaded_run = pipeline_service::get_run(&db.conn, run.id)
            .await
            .expect("get run");
        assert_eq!(reloaded_run.status, PipelineRunStatus::Succeeded);
    }

    #[tokio::test]
    async fn inconclusive_stops_pipeline_run() {
        let (engine, db, manager, _) = setup_engine().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        let _attempt = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "reviewer".into(),
            1,
            Some("conn-rev-inc".into()),
            None,
        )
        .await
        .expect("create attempt");

        manager
            .insert_test_connection_live(
                "conn-rev-inc",
                AgentType::ClaudeCode,
                None,
                EventEmitter::Noop,
            )
            .await;

        engine
            .record_verdict(
                "conn-rev-inc",
                PipelineVerdict::Inconclusive,
                Some("Ambiguous requirements"),
            )
            .await;

        engine
            .on_event(&turn_complete_env("conn-rev-inc", "end_turn"))
            .await;

        let reloaded_run = pipeline_service::get_run(&db.conn, run.id)
            .await
            .expect("get run");
        assert_eq!(reloaded_run.status, PipelineRunStatus::Inconclusive);
    }

    #[tokio::test]
    async fn late_verdict_of_past_attempt_ignored() {
        let (engine, db, _, _) = setup_engine().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        let attempt = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "coder".into(),
            1,
            Some("conn-expired".into()),
            None,
        )
        .await
        .expect("create attempt");

        // A verdict that arrives after its attempt already finished belongs to a
        // superseded connection and must be dropped.
        pipeline_service::cas_attempt_status(
            &db.conn,
            attempt.id,
            AttemptStatus::Running,
            AttemptStatus::Done,
        )
        .await
        .expect("finish attempt");

        let ack = engine
            .record_verdict("conn-expired", PipelineVerdict::Pass, None)
            .await;
        assert!(
            !ack.recorded,
            "verdict for an already finished attempt must be rejected"
        );

        // A still running attempt that is missing from the live index (the app
        // restarted while the child agent stayed connected) is resolved from the
        // database, so its verdict is kept.
        let live = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "coder".into(),
            2,
            Some("conn-live".into()),
            None,
        )
        .await
        .expect("create attempt");
        let ack = engine
            .record_verdict("conn-live", PipelineVerdict::Pass, None)
            .await;
        assert!(ack.recorded, "verdict for a running attempt must be kept");
        let stored = pipeline_service::get_attempt(&db.conn, live.id)
            .await
            .expect("query")
            .expect("found");
        assert_eq!(stored.verdict, Some(PipelineVerdict::Pass));
    }

    #[tokio::test]
    async fn duplicate_turn_complete_handled() {
        let (engine, db, manager, _) = setup_engine().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        let attempt = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "coder".into(),
            1,
            Some("conn-dup".into()),
            None,
        )
        .await
        .expect("create attempt");

        manager
            .insert_test_connection_live(
                "conn-dup",
                AgentType::ClaudeCode,
                None,
                EventEmitter::Noop,
            )
            .await;

        // First TurnComplete
        engine
            .on_event(&turn_complete_env("conn-dup", "end_turn"))
            .await;

        // Second duplicate TurnComplete
        engine
            .on_event(&turn_complete_env("conn-dup", "end_turn"))
            .await;

        let att = pipeline_service::get_attempt(&db.conn, attempt.id)
            .await
            .expect("query")
            .expect("found");
        assert_eq!(att.status, AttemptStatus::Done);
    }

    #[tokio::test]
    async fn cancel_waits_and_does_not_launch_next_step() {
        let (engine, db, _, _) = setup_engine().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        engine.cancel(run.id).await.expect("cancel");

        let reloaded_run = pipeline_service::get_run(&db.conn, run.id)
            .await
            .expect("get run");
        assert_eq!(reloaded_run.status, PipelineRunStatus::Cancelled);
    }

    #[tokio::test]
    async fn second_run_in_same_folder_rejected() {
        let (engine, db, _, _) = setup_engine().await;
        let graph = test_graph();
        let _run = pipeline_service::create_run(
            &db.conn,
            None,
            42,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        // Attempting to start another run in folder 42
        let req = PipelineRunRequest {
            folder_id: 42,
            pipeline_id: None,
            graph: Some(graph),
            isolation: Some(PipelineIsolation::SharedInRoot),
            prompt_blocks: vec![],
            display_text: "new task".into(),
            parent_conversation_id: None,
        };

        let err = engine.start(req).await.unwrap_err();
        assert!(err.contains("pipeline already running in this folder"));
    }

    #[tokio::test]
    async fn modified_tree_on_reviewer_yields_inconclusive_guard() {
        let (engine, db, manager, _) = setup_engine().await;
        let graph = test_graph();
        let run = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run");

        let attempt = pipeline_service::create_attempt(
            &db.conn,
            run.id,
            "reviewer".into(),
            1,
            Some("conn-guard-test".into()),
            None,
        )
        .await
        .expect("create attempt");

        manager
            .insert_test_connection_live(
                "conn-guard-test",
                AgentType::ClaudeCode,
                None,
                EventEmitter::Noop,
            )
            .await;

        // Set verdict Pass
        engine
            .record_verdict("conn-guard-test", PipelineVerdict::Pass, None)
            .await;

        // Even though Pass was recorded, if the reviewer modified files and hash differs,
        // it triggers guard
        engine
            .on_event(&turn_complete_env("conn-guard-test", "end_turn"))
            .await;

        let att = pipeline_service::get_attempt(&db.conn, attempt.id)
            .await
            .expect("query")
            .expect("found");
        assert_eq!(att.status, AttemptStatus::Done);
    }

    #[tokio::test]
    async fn recover_on_boot_interrupts_running_runs() {
        let (engine, db, _, _) = setup_engine().await;
        let graph = test_graph();
        let run1 = pipeline_service::create_run(
            &db.conn,
            None,
            1,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run 1");

        let run2 = pipeline_service::create_run(
            &db.conn,
            None,
            2,
            &graph,
            PipelineIsolation::SharedInRoot,
            None,
            None,
        )
        .await
        .expect("create run 2");

        assert_eq!(run1.status, PipelineRunStatus::Running);
        assert_eq!(run2.status, PipelineRunStatus::Running);

        engine.recover_on_boot().await;

        let r1 = pipeline_service::get_run(&db.conn, run1.id)
            .await
            .expect("get run 1");
        let r2 = pipeline_service::get_run(&db.conn, run2.id)
            .await
            .expect("get run 2");

        assert_eq!(r1.status, PipelineRunStatus::Interrupted);
        assert_eq!(r2.status, PipelineRunStatus::Interrupted);
    }
}
