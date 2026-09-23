//! End to end coverage for a duet run: the engine drives every step itself,
//! the test only reports verdicts and asserts what the engine persisted.
//!
//! Steps run against synthetic connections (`simulate_launches`) so no agent
//! process is started; the rendered prompt of each step is captured instead.

#[cfg(test)]
mod duet_e2e {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use crate::acp::manager::ConnectionManager;
    use crate::acp::types::{AcpEvent, EventEnvelope};
    use crate::acp::InternalEventBus;
    use crate::db::service::pipeline_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::db::AppDatabase;
    use crate::models::{
        AttemptStatus, LoopBack, PipelineGraph, PipelineIsolation, PipelineRole,
        PipelineRunRequest, PipelineRunStatus, PipelineStep, PipelineVerdict,
    };
    use crate::pipeline::engine::{build_engine, LaunchLog, PipelineEngine};
    use crate::web::event_bridge::EventEmitter;

    fn duet_graph() -> PipelineGraph {
        PipelineGraph {
            steps: vec![
                PipelineStep {
                    id: "coder".into(),
                    role: PipelineRole::Coder,
                    label: "Coder".into(),
                    agent_type: "claude_code".into(),
                    mode_id: None,
                    config_values: BTreeMap::new(),
                    prompt_template: "TASK: $task\nREVIEW: $review".into(),
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
                    prompt_template: "REVIEW TASK: $task".into(),
                    timeout_secs: 1800,
                    read_memory: false,
                    read_only: false,
                },
            ],
            loops: vec![LoopBack {
                from_step: "reviewer".into(),
                to_step: "coder".into(),
                max_iterations: 3,
            }],
        }
    }

    async fn setup() -> (Arc<PipelineEngine>, AppDatabase, ConnectionManager, LaunchLog) {
        let db = fresh_in_memory_db().await;
        let metrics = Arc::new(crate::acp::internal_bus::EventBusMetrics::default());
        let bus = Arc::new(InternalEventBus::new(metrics));
        let manager = ConnectionManager::new();
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let engine = build_engine(
            AppDatabase {
                conn: db.conn.clone(),
            },
            manager.clone_ref(),
            EventEmitter::Noop,
            bus,
            temp_dir.path().to_path_buf(),
        )
        .expect("build engine");
        let log: LaunchLog = Arc::new(Mutex::new(Vec::new()));
        engine.simulate_launches(log.clone()).await;
        (engine, db, manager, log)
    }

    fn turn_complete(conn_id: &str) -> EventEnvelope {
        EventEnvelope {
            seq: 0,
            connection_id: conn_id.to_string(),
            payload: AcpEvent::TurnComplete {
                session_id: conn_id.to_string(),
                stop_reason: "end_turn".to_string(),
                agent_type: "claude_code".to_string(),
            },
        }
    }

    /// Report a verdict for the step that is currently running, then end its
    /// turn, exactly as a real agent would through the MCP tool and the event
    /// stream.
    async fn finish_step(engine: &PipelineEngine, log: &LaunchLog, verdict: PipelineVerdict, notes: Option<&str>) {
        let attempt_id = log.lock().await.last().expect("a step was launched").0;
        let conn_id = format!("sim-conn-{attempt_id}");
        let ack = engine.record_verdict(&conn_id, verdict, notes).await;
        assert!(ack.recorded, "verdict for the live step must be recorded");
        engine.on_event(&turn_complete(&conn_id)).await;
    }

    #[tokio::test]
    async fn duet_loops_back_on_changes_requested_then_completes_on_pass() {
        let (engine, db, _manager, log) = setup().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-duet-e2e").await;

        let run = engine
            .start(PipelineRunRequest {
                folder_id,
                pipeline_id: None,
                graph: Some(duet_graph()),
                isolation: Some(PipelineIsolation::SharedInRoot),
                prompt_blocks: vec![],
                display_text: "add a readiness probe".into(),
                parent_conversation_id: None,
            })
            .await
            .expect("start run");

        // The engine launched the coder itself.
        {
            let entries = log.lock().await;
            assert_eq!(entries.len(), 1, "coder must be launched by the engine");
            assert_eq!(entries[0].1, "coder");
            assert_eq!(entries[0].2, 1, "first iteration");
            assert!(entries[0].3.contains("add a readiness probe"));
        }

        // Coder finishes, reviewer asks for changes.
        finish_step(&engine, &log, PipelineVerdict::Pass, None).await;
        {
            let entries = log.lock().await;
            assert_eq!(entries.len(), 2, "reviewer must follow the coder");
            assert_eq!(entries[1].1, "reviewer");
        }

        finish_step(
            &engine,
            &log,
            PipelineVerdict::ChangesRequested,
            Some("initialDelaySeconds is too low; use 20"),
        )
        .await;

        // The loop sent the work back to the coder with the reviewer's notes.
        {
            let entries = log.lock().await;
            assert_eq!(entries.len(), 3, "coder must run again");
            let (_, step_id, iteration, prompt) = entries[2].clone();
            assert_eq!(step_id, "coder");
            assert_eq!(iteration, 2, "iteration is monotonic across the run");
            assert!(
                prompt.contains("initialDelaySeconds is too low"),
                "the coder must receive the reviewer notes in full, got: {prompt}"
            );
        }

        // Second round passes review, so the run completes.
        finish_step(&engine, &log, PipelineVerdict::Pass, None).await;
        finish_step(&engine, &log, PipelineVerdict::Pass, None).await;

        let info = pipeline_service::get_run_info(&db.conn, run.id)
            .await
            .expect("run info");
        assert_eq!(info.status, PipelineRunStatus::Succeeded);

        let attempts = info.attempts;
        assert_eq!(attempts.len(), 4, "two coder and two reviewer attempts");
        assert!(
            attempts.iter().all(|a| a.status == AttemptStatus::Done),
            "every attempt must be closed"
        );
        let coder_iterations: Vec<u32> = attempts
            .iter()
            .filter(|a| a.step_id == "coder")
            .map(|a| a.iteration)
            .collect();
        assert_eq!(coder_iterations, vec![1, 2]);
        assert_eq!(
            attempts
                .iter()
                .filter(|a| a.verdict == Some(PipelineVerdict::ChangesRequested))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn duet_stops_when_the_fix_round_limit_is_reached() {
        let (engine, db, _manager, log) = setup().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-duet-limit").await;
        let mut graph = duet_graph();
        graph.loops[0].max_iterations = 1;

        let run = engine
            .start(PipelineRunRequest {
                folder_id,
                pipeline_id: None,
                graph: Some(graph),
                isolation: Some(PipelineIsolation::SharedInRoot),
                prompt_blocks: vec![],
                display_text: "tighten the alert rule".into(),
                parent_conversation_id: None,
            })
            .await
            .expect("start run");

        finish_step(&engine, &log, PipelineVerdict::Pass, None).await;
        finish_step(
            &engine,
            &log,
            PipelineVerdict::ChangesRequested,
            Some("missing severity label"),
        )
        .await;

        let info = pipeline_service::get_run_info(&db.conn, run.id)
            .await
            .expect("run info");
        assert_eq!(
            info.status,
            PipelineRunStatus::StoppedMaxIterations,
            "one fix round is allowed, the second request must stop the run"
        );
        assert_eq!(log.lock().await.len(), 2, "no third step may be launched");
    }

    #[tokio::test]
    async fn inconclusive_review_stops_the_run_without_a_fix_round() {
        let (engine, db, _manager, log) = setup().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-duet-inconclusive").await;

        let run = engine
            .start(PipelineRunRequest {
                folder_id,
                pipeline_id: None,
                graph: Some(duet_graph()),
                isolation: Some(PipelineIsolation::SharedInRoot),
                prompt_blocks: vec![],
                display_text: "rotate the deploy key".into(),
                parent_conversation_id: None,
            })
            .await
            .expect("start run");

        finish_step(&engine, &log, PipelineVerdict::Pass, None).await;
        finish_step(
            &engine,
            &log,
            PipelineVerdict::Inconclusive,
            Some("cannot tell whether the change is correct"),
        )
        .await;

        let info = pipeline_service::get_run_info(&db.conn, run.id)
            .await
            .expect("run info");
        assert_eq!(info.status, PipelineRunStatus::Inconclusive);
        assert_eq!(log.lock().await.len(), 2, "no fix round after inconclusive");
    }
}
