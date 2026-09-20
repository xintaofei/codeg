//! Pipeline execution engine: orchestrates multi-agent workflows through the ACP layer.
//!
//! Design:
//! - Each run has a unique `run_id`, each attempt has `attempt_id = (run_id, step, iteration)`.
//! - Attempts are correlated by `connection_id` (same as automation engine).
//! - Transitions use CAS: `pipeline_service::cas_attempt_status(attempt_id, from, to)`.
//! - Worktree per run via `PipelineIsolation::WorktreePerRun` (same as automation).
//! - Guard: read-only step checks if `git diff HEAD` + untracked hash changed after step.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Mutex;

use crate::acp::manager::ConnectionManager;
use crate::acp::pipeline_tools::PipelineToolAccess;
use crate::acp::types::{AcpEvent, EventEnvelope, PromptInputBlock};
use crate::acp::work_task_tools::TaskReportAck;
use crate::acp::InternalEventBus;
use crate::db::service::pipeline_service;
use crate::db::AppDatabase;
use crate::models::{
    AttemptStatus, PipelineGraph, PipelineIsolation, PipelineRole, PipelineRunInfo,
    PipelineRunRequest, PipelineRunStatus, PipelineVerdict,
};
use crate::pipeline::verdict;
use crate::web::event_bridge::{emit_event, EventEmitter, PipelineChange, PIPELINE_CHANGED_EVENT};

static PIPELINE_ENGINE: OnceLock<Arc<PipelineEngine>> = OnceLock::new();

const CANCEL_WAIT_TIMEOUT_SECS: u64 = 30;

pub fn engine() -> Option<Arc<PipelineEngine>> {
    PIPELINE_ENGINE.get().cloned()
}

#[derive(Debug, Clone)]
struct AttemptState {
    run_id: i32,
    attempt_id: i32,
    step_index: usize,
    iteration: u32,
    step_id: String,
    role: PipelineRole,
    read_only: bool,
    display_text: String,
    working_dir: String,
    folder_id: i32,
}


/// Memory context for a step that asks for it.
///
/// Entries are recalled by the run's task text and handed to the agent as
/// untrusted data: memory holds text written by earlier agent runs, so it must
/// never be read as instructions.
async fn memory_context(db: &AppDatabase, task: &str, folder_id: i32) -> String {
    const MAX_CHARS: usize = 2500;
    let hits = match crate::commands::memory::memory_search_scoped(
        db,
        task.to_string(),
        Some(8),
        Some(folder_id),
    )
    .await
    {
        Ok(hits) if !hits.is_empty() => hits,
        _ => return String::new(),
    };
    let mut body = String::new();
    for hit in hits {
        let line = format!("- [{}] {}: {}\n", hit.node.kind, hit.node.title, hit.node.body);
        if body.len() + line.len() > MAX_CHARS {
            break;
        }
        body.push_str(&line);
    }
    if body.is_empty() {
        return String::new();
    }
    format!("<memory untrusted=\"true\">\n{body}</memory>")
}


/// Record a run outcome in memory when the matching kind is enabled and set to
/// `auto`. Anything the user has not enabled stays unwritten, and the entry
/// carries where it came from so a wrong lesson can be traced back.
#[allow(clippy::too_many_arguments)]
async fn auto_record_memory(
    db: &AppDatabase,
    kind_key: &str,
    title: String,
    body: String,
    run_id: i32,
    folder_id: i32,
    verified_by_tests: bool,
) {
    use crate::memory::backend::{MemoryProvenance, NewMemoryNode};

    let Ok(kinds) = crate::db::service::memory_kind_service::list(&db.conn).await else {
        return;
    };
    let allowed = kinds
        .iter()
        .any(|k| k.key == kind_key && k.enabled && k.mode == crate::models::MemoryMode::Auto);
    if !allowed {
        return;
    }

    let node = NewMemoryNode {
        kind: kind_key.to_string(),
        title,
        body,
        scope: crate::models::MemoryScope::Project,
        folder_id: Some(folder_id),
        provenance: MemoryProvenance {
            run_id: Some(run_id),
            step_id: None,
            agent_type: None,
            verified_by_tests,
            source: "auto".into(),
        },
    };
    let _ = crate::commands::memory::memory_write_auto_core(db, node).await;
}


/// The option that lets the step continue.
///
/// Agents name these differently, so prefer an explicit allow-once kind and
/// fall back to the first option whose name reads as an approval.
fn pick_allow_option(options: &[crate::acp::types::PermissionOptionInfo]) -> Option<String> {
    options
        .iter()
        .find(|o| o.kind == "allow_once")
        .or_else(|| options.iter().find(|o| o.kind.starts_with("allow")))
        .or_else(|| {
            options
                .iter()
                .find(|o| o.name.to_lowercase().starts_with("allow") || o.name.to_lowercase().starts_with("yes"))
        })
        .map(|o| o.option_id.clone())
}

/// Verdict recorded by a tool call, keyed by attempt id.
type RecordedVerdicts = Arc<Mutex<HashMap<i32, (PipelineVerdict, Option<String>)>>>;

/// Prompts handed to each step, recorded only when the engine runs without
/// real agent processes (tests): `(attempt_id, step_id, iteration, prompt)`.
#[cfg(any(test, feature = "test-utils"))]
pub type LaunchLog = Arc<Mutex<Vec<(i32, String, u32, String)>>>;

pub struct PipelineEngine {
    db: AppDatabase,
    manager: ConnectionManager,
    emitter: EventEmitter,
    bus: Arc<InternalEventBus>,
    data_dir: PathBuf,
    /// Live runs: `connection_id -> AttemptState`.
    index: Arc<Mutex<HashMap<String, AttemptState>>>,
    /// Per-folder start serialization locks.
    folder_locks: Arc<Mutex<HashMap<i32, Arc<Mutex<()>>>>>,
    /// Active run per folder: `folder_id -> run_id`.
    active_runs: Arc<Mutex<HashMap<i32, i32>>>,
    /// Pre-step working tree hashes for read-only step guards: `attempt_id -> hash`.
    pre_step_hashes: Arc<Mutex<HashMap<i32, String>>>,
    /// Recorded verdicts from tool calls: `attempt_id -> (verdict, notes)`.
    recorded_verdicts: RecordedVerdicts,
    /// When set, steps run against synthetic connections instead of spawning
    /// agent processes, and their prompts are recorded here.
    #[cfg(any(test, feature = "test-utils"))]
    launch_log: Arc<Mutex<Option<LaunchLog>>>,
}

#[async_trait]
impl PipelineToolAccess for PipelineEngine {
    async fn record_verdict(
        &self,
        parent_connection_id: &str,
        verdict: &str,
        notes: Option<&str>,
    ) -> TaskReportAck {
        let v = match verdict.to_lowercase().as_str() {
            "pass" => PipelineVerdict::Pass,
            "changes_requested" => PipelineVerdict::ChangesRequested,
            "inconclusive" => PipelineVerdict::Inconclusive,
            _ => return TaskReportAck::rejected(&format!("unknown verdict: {verdict}")),
        };
        self.record_verdict_internal(parent_connection_id, v, notes)
            .await
    }
}

pub fn build_engine(
    db: AppDatabase,
    manager: ConnectionManager,
    emitter: EventEmitter,
    bus: Arc<InternalEventBus>,
    data_dir: PathBuf,
) -> Option<Arc<PipelineEngine>> {
    let engine = Arc::new(PipelineEngine {
        db,
        manager,
        emitter,
        bus,
        data_dir,
        index: Arc::new(Mutex::new(HashMap::new())),
        folder_locks: Arc::new(Mutex::new(HashMap::new())),
        active_runs: Arc::new(Mutex::new(HashMap::new())),
        pre_step_hashes: Arc::new(Mutex::new(HashMap::new())),
        recorded_verdicts: Arc::new(Mutex::new(HashMap::new())),
        #[cfg(any(test, feature = "test-utils"))]
        launch_log: Arc::new(Mutex::new(None)),
    });
    let _ = PIPELINE_ENGINE.set(engine.clone());
    Some(engine)
}

/// Task that recovers pipeline state on boot and processes events.
pub async fn run_pipeline_engine(engine: Arc<PipelineEngine>) {
    engine.recover_on_boot().await;
    let mut rx = engine.bus.subscribe();
    loop {
        match rx.recv().await {
            Ok(env) => engine.on_event(&env).await,
            Err(RecvError::Lagged(n)) => {
                tracing::warn!("[pipeline] event bus lagged: dropped {n} events");
            }
            Err(RecvError::Closed) => break,
        }
    }
}

struct ResolvedCwd {
    folder_id: i32,
    working_dir: String,
    worktree_folder_id: Option<i32>,
}

impl PipelineEngine {
    pub fn db(&self) -> &AppDatabase {
        &self.db
    }

    pub fn manager(&self) -> &ConnectionManager {
        &self.manager
    }

    pub fn emitter(&self) -> &EventEmitter {
        &self.emitter
    }

    pub async fn record_verdict(
        &self,
        connection_id: &str,
        verdict: PipelineVerdict,
        notes: Option<&str>,
    ) -> TaskReportAck {
        self.record_verdict_internal(connection_id, verdict, notes)
            .await
    }

    async fn record_verdict_internal(
        &self,
        connection_id: &str,
        verdict: PipelineVerdict,
        notes: Option<&str>,
    ) -> TaskReportAck {
        let entry = self.attempt_state_for_connection(connection_id).await;
        let Some(attempt) = entry else {
            tracing::debug!(
                "[pipeline] record_verdict for unknown or expired connection {connection_id}"
            );
            return TaskReportAck::rejected(
                "connection not associated with an active pipeline attempt",
            );
        };

        self.recorded_verdicts
            .lock()
            .await
            .insert(attempt.attempt_id, (verdict, notes.map(|s| s.to_string())));

        let _ = pipeline_service::set_attempt_verdict(
            &self.db.conn,
            attempt.attempt_id,
            verdict,
            Some("tool".into()),
            notes.map(|s| s.to_string()),
        )
        .await;

        TaskReportAck::recorded()
    }

    async fn folder_lock(&self, folder_id: i32) -> Arc<Mutex<()>> {
        let mut locks = self.folder_locks.lock().await;
        locks
            .entry(folder_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn resolve_cwd(
        &self,
        root_folder_id: i32,
        isolation: PipelineIsolation,
        pipeline_id: Option<i32>,
        run_id: i32,
    ) -> Result<ResolvedCwd, String> {
        let root = crate::commands::folders::get_folder_core(&self.db, root_folder_id)
            .await
            .map_err(|e| e.to_string())?;

        match isolation {
            PipelineIsolation::WorktreePerRun => {
                let pid = pipeline_id.unwrap_or(0);
                let branch = format!("pipeline/{pid}/run-{run_id}");
                let repo_name = match basename(&root.path) {
                    "" => "workspace",
                    name => name,
                };
                let dir = format!("{repo_name}-pipeline-run-{run_id}");
                let mut wt_path = sibling_path(&root.path, &dir);

                if let Err(e) = crate::commands::folders::git_worktree_add(
                    root.path.clone(),
                    branch.clone(),
                    wt_path.clone(),
                    None,
                )
                .await
                {
                    let suffix = short_suffix(run_id);
                    let branch2 = format!("{branch}-{suffix}");
                    wt_path = sibling_path(&root.path, &format!("{dir}-{suffix}"));
                    crate::commands::folders::git_worktree_add(
                        root.path.clone(),
                        branch2,
                        wt_path.clone(),
                        None,
                    )
                    .await
                    .map_err(|_| format!("worktree add failed: {e}"))?;
                }

                let wt = crate::commands::folders::open_worktree_folder_core(
                    &self.db,
                    wt_path,
                    root_folder_id,
                )
                .await
                .map_err(|e| e.to_string())?;

                Ok(ResolvedCwd {
                    folder_id: wt.id,
                    working_dir: wt.path,
                    worktree_folder_id: Some(wt.id),
                })
            }
            PipelineIsolation::SharedInRoot => Ok(ResolvedCwd {
                folder_id: root_folder_id,
                working_dir: root.path,
                worktree_folder_id: None,
            }),
        }
    }

    /// Start a new pipeline run. Returns error if a run is already active in this folder.
    pub async fn start(&self, req: PipelineRunRequest) -> Result<PipelineRunInfo, String> {
        let lock = self.folder_lock(req.folder_id).await;
        let _guard = lock.lock().await;

        let active_in_db = pipeline_service::has_active_run(&self.db.conn, req.folder_id)
            .await
            .map_err(|e| format!("db error: {e}"))?;
        if active_in_db || self.active_runs.lock().await.contains_key(&req.folder_id) {
            return Err("pipeline already running in this folder".into());
        }

        let graph = if let Some(graph) = req.graph {
            graph
        } else if let Some(pipeline_id) = req.pipeline_id {
            let pipeline = pipeline_service::get_pipeline(&self.db.conn, pipeline_id)
                .await
                .map_err(|e| format!("failed to load pipeline: {e}"))?;
            pipeline.graph
        } else {
            return Err("either pipeline_id or graph must be provided".into());
        };

        crate::pipeline::validate::validate_graph(&graph)
            .map_err(|e| format!("invalid graph: {e}"))?;

        let isolation = req.isolation.unwrap_or(PipelineIsolation::WorktreePerRun);

        let run = pipeline_service::create_run(
            &self.db.conn,
            req.pipeline_id,
            req.folder_id,
            &graph,
            isolation,
            req.parent_conversation_id,
            Some(req.display_text.clone()),
        )
        .await
        .map_err(|e| format!("failed to create run: {e}"))?;

        self.active_runs.lock().await.insert(req.folder_id, run.id);

        emit_event(
            &self.emitter,
            PIPELINE_CHANGED_EVENT,
            PipelineChange::RunStarted {
                run_id: run.id,
                folder_id: req.folder_id,
            },
        );

        if let Err(e) = self.launch_step(run.id, 0, 1, req.display_text, None).await {
            let _ = pipeline_service::update_run_status(
                &self.db.conn,
                run.id,
                PipelineRunStatus::Failed,
                Some(e.clone()),
            )
            .await;
            self.active_runs.lock().await.remove(&req.folder_id);
            emit_event(
                &self.emitter,
                PIPELINE_CHANGED_EVENT,
                PipelineChange::RunSettled {
                    run_id: run.id,
                    status: PipelineRunStatus::Failed,
                },
            );
            return Err(e);
        }

        pipeline_service::get_run_info(&self.db.conn, run.id)
            .await
            .map_err(|e| e.to_string())
    }

    /// Run steps without spawning agent processes, recording their prompts.
    /// Test-only: the item does not exist in release builds.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn simulate_launches(&self, log: LaunchLog) {
        *self.launch_log.lock().await = Some(log);
    }


    /// Settle a run that could not continue.
    ///
    /// A launch failure used to be swallowed: the run stayed `running`, the
    /// folder stayed locked and no further event ever arrived, so the card span
    /// forever. Failing loudly keeps both the UI and the folder usable.
    async fn fail_run(&self, run_id: i32, folder_id: i32, error: String) {
        let _ = pipeline_service::update_run_status(
            &self.db.conn,
            run_id,
            PipelineRunStatus::Failed,
            Some(error),
        )
        .await;
        self.active_runs.lock().await.remove(&folder_id);
        emit_event(
            &self.emitter,
            PIPELINE_CHANGED_EVENT,
            PipelineChange::RunSettled {
                run_id,
                status: PipelineRunStatus::Failed,
            },
        );
    }

    async fn launch_step(
        &self,
        run_id: i32,
        step_index: usize,
        iteration: u32,
        display_text: String,
        previous_attempt_id: Option<i32>,
    ) -> Result<(), String> {
        let run = pipeline_service::get_run_raw(&self.db.conn, run_id)
            .await
            .map_err(|e| format!("failed to fetch run: {e}"))?;

        if run.status != "running" {
            return Ok(());
        }

        let graph: PipelineGraph =
            serde_json::from_str(&run.graph).map_err(|e| format!("invalid graph: {e}"))?;

        if step_index >= graph.steps.len() {
            let _ = pipeline_service::update_run_status(
                &self.db.conn,
                run_id,
                PipelineRunStatus::Succeeded,
                None,
            )
            .await;
            self.active_runs.lock().await.remove(&run.folder_id);
            emit_event(
                &self.emitter,
                PIPELINE_CHANGED_EVENT,
                PipelineChange::RunSettled {
                    run_id,
                    status: PipelineRunStatus::Succeeded,
                },
            );
            return Ok(());
        }

        let step = &graph.steps[step_index];
        let agent_type: crate::models::AgentType =
            serde_json::from_value(serde_json::Value::String(step.agent_type.clone()))
                .map_err(|_| format!("unknown agent type: {}", step.agent_type))?;

        let isolation = match run.isolation.as_str() {
            "shared_in_root" => PipelineIsolation::SharedInRoot,
            _ => PipelineIsolation::WorktreePerRun,
        };

        let cwd = if let Some(wt_id) = run.worktree_folder_id {
            if let Ok(detail) = crate::commands::folders::get_folder_core(&self.db, wt_id).await {
                ResolvedCwd {
                    folder_id: detail.id,
                    working_dir: detail.path,
                    worktree_folder_id: Some(detail.id),
                }
            } else {
                self.resolve_cwd(run.folder_id, isolation, run.pipeline_id, run_id)
                    .await?
            }
        } else {
            self.resolve_cwd(run.folder_id, isolation, run.pipeline_id, run_id)
                .await?
        };

        if let Ok(detail) = crate::commands::folders::get_folder_core(&self.db, cwd.folder_id).await
        {
            crate::commands::folders::emit_folder_upsert(&self.emitter, detail);
        }

        let _ = pipeline_service::update_run_current_step(
            &self.db.conn,
            run_id,
            Some(step.id.clone()),
            iteration,
            cwd.worktree_folder_id,
        )
        .await;

        let pre_hash = if step.read_only {
            Some(compute_worktree_hash(&cwd.working_dir).await)
        } else {
            None
        };

        let model_requested = step.config_values.get("model").cloned();
        let attempt = pipeline_service::create_attempt(
            &self.db.conn,
            run_id,
            step.id.clone(),
            iteration,
            None,
            model_requested,
        )
        .await
        .map_err(|e| format!("failed to create attempt: {e}"))?;

        if let Some(hash) = pre_hash {
            self.pre_step_hashes.lock().await.insert(attempt.id, hash);
        }

        let plan = pipeline_service::get_last_planner_summary(&self.db.conn, run_id, &graph)
            .await
            .unwrap_or(None);
        let review = pipeline_service::get_last_changes_requested_notes(&self.db.conn, run_id)
            .await
            .unwrap_or(None);
        let summary = if let Some(prev_id) = previous_attempt_id {
            if let Ok(Some(prev)) = pipeline_service::get_attempt(&self.db.conn, prev_id).await {
                prev.summary.map(|s| {
                    let truncated = if s.len() > 4000 {
                        let mut t: String = s.chars().take(4000).collect();
                        t.push_str("... (truncated)");
                        t
                    } else {
                        s
                    };
                    if let Some(cid) = prev.conversation_id {
                        format!("Summary from conversation #{cid}:\n{truncated}")
                    } else {
                        truncated
                    }
                })
            } else {
                None
            }
        } else {
            None
        };

        let memory = if step.read_memory {
            memory_context(&self.db, &display_text, cwd.folder_id).await
        } else {
            String::new()
        };

        let prompt_vars = verdict::PromptVars {
            task: display_text.clone(),
            plan,
            summary,
            review,
            memory,
        };
        let rendered_prompt = verdict::render_prompt(&step.prompt_template, &prompt_vars);

        let runtime_env = crate::commands::acp::build_session_runtime_env(
            &self.db,
            agent_type,
            None,
            &self.data_dir,
        )
        .await
        .map_err(|e| e.to_string())?;

        // Steps run unattended: in the agent's default mode every tool call
        // raises a permission prompt nobody answers, and the step just sits
        // there until its timeout. A read-only step reviews in `plan`, a step
        // that has to change files accepts its own edits. A mode set on the
        // step wins over both.
        let preferred_mode_id = step.mode_id.clone().or_else(|| {
            Some(if step.read_only {
                "plan".to_string()
            } else {
                "acceptEdits".to_string()
            })
        });

        #[cfg(any(test, feature = "test-utils"))]
        if let Some(log) = self.launch_log.lock().await.clone() {
            let conn_id = format!("sim-conn-{}", attempt.id);
            self.manager
                .insert_test_connection(
                    &conn_id,
                    agent_type,
                    Some(std::path::PathBuf::from(&cwd.working_dir)),
                    self.emitter.clone(),
                )
                .await;
            let _ = pipeline_service::attach_attempt_runtime(
                &self.db.conn,
                attempt.id,
                None,
                Some(conn_id.clone()),
                None,
            )
            .await;
            log.lock().await.push((
                attempt.id,
                step.id.clone(),
                iteration,
                rendered_prompt.clone(),
            ));
            self.index.lock().await.insert(
                conn_id,
                AttemptState {
                    run_id,
                    attempt_id: attempt.id,
                    step_index,
                    iteration,
                    step_id: step.id.clone(),
                    role: step.role,
                    read_only: step.read_only,
                    display_text: display_text.clone(),
                    working_dir: cwd.working_dir.clone(),
                    folder_id: cwd.folder_id,
                },
            );
            return Ok(());
        }

        let conn_id = self
            .manager
            .spawn_agent(
                agent_type,
                Some(cwd.working_dir.clone()),
                None,
                runtime_env,
                "pipeline".to_string(),
                self.emitter.clone(),
                preferred_mode_id,
                step.config_values.clone(),
            )
            .await;
        // The attempt row already exists, so a spawn failure has to close it:
        // an attempt left running blocks the UI and can later be matched by a
        // stale connection id.
        let conn_id = match conn_id {
            Ok(id) => id,
            Err(e) => {
                let _ = pipeline_service::cas_attempt_status(
                    &self.db.conn,
                    attempt.id,
                    AttemptStatus::Running,
                    AttemptStatus::Failed,
                )
                .await;
                return Err(e.to_string());
            }
        };

        let title = format!("{}: {}", step.label, first_chars(&display_text, 40));
        let conversation_id = match crate::commands::conversations::create_conversation_core(
            &self.db.conn,
            cwd.folder_id,
            agent_type,
            Some(title),
        )
        .await
        {
            Ok(id) => id,
            Err(e) => {
                let _ = self.manager.disconnect(&conn_id).await;
                return Err(e.to_string());
            }
        };

        let _ =
            crate::db::service::conversation_service::lock_title(&self.db.conn, conversation_id)
                .await;

        crate::commands::conversations::emit_conversation_upsert(
            &self.emitter,
            &self.db.conn,
            conversation_id,
        )
        .await;

        let _ = pipeline_service::attach_attempt_runtime(
            &self.db.conn,
            attempt.id,
            Some(conversation_id),
            Some(conn_id.clone()),
            None,
        )
        .await;

        let attempt_state = AttemptState {
            run_id,
            attempt_id: attempt.id,
            step_index,
            iteration,
            step_id: step.id.clone(),
            role: step.role,
            read_only: step.read_only,
            display_text: display_text.clone(),
            working_dir: cwd.working_dir.clone(),
            folder_id: run.folder_id,
        };

        self.index
            .lock()
            .await
            .insert(conn_id.clone(), attempt_state);

        emit_event(
            &self.emitter,
            PIPELINE_CHANGED_EVENT,
            PipelineChange::StepStarted {
                run_id,
                attempt_id: attempt.id,
                step_id: step.id.clone(),
                iteration,
            },
        );

        // Step timeout watcher
        if step.timeout_secs > 0 {
            let timeout_secs = step.timeout_secs;
            let attempt_id = attempt.id;
            let c_id = conn_id.clone();
            let db_conn = self.db.conn.clone();
            let mgr = self.manager.clone_ref();
            let emitter = self.emitter.clone();
            let active_runs = self.active_runs.clone();
            let index = self.index.clone();
            let pre_step_hashes = self.pre_step_hashes.clone();
            let recorded_verdicts = self.recorded_verdicts.clone();
            let folder_id = run.folder_id;
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(timeout_secs)).await;
                if let Ok(true) = pipeline_service::cas_attempt_status(
                    &db_conn,
                    attempt_id,
                    AttemptStatus::Running,
                    AttemptStatus::TimedOut,
                )
                .await
                {
                    let _ = mgr.cancel(&db_conn, &c_id).await;
                    let _ = mgr.disconnect(&c_id).await;
                    let _ = pipeline_service::update_run_status(
                        &db_conn,
                        run_id,
                        PipelineRunStatus::Failed,
                        Some("step timed out".into()),
                    )
                    .await;
                    // Without this the folder stays locked for the rest of the
                    // process lifetime: `start` refuses a second run while the
                    // folder is in `active_runs`.
                    active_runs.lock().await.remove(&folder_id);
                    index.lock().await.remove(&c_id);
                    pre_step_hashes.lock().await.remove(&attempt_id);
                    recorded_verdicts.lock().await.remove(&attempt_id);
                    emit_event(
                        &emitter,
                        PIPELINE_CHANGED_EVENT,
                        PipelineChange::RunSettled {
                            run_id,
                            status: PipelineRunStatus::Failed,
                        },
                    );
                }
            });
        }

        let prompt_blocks = vec![PromptInputBlock::Text {
            text: rendered_prompt,
        }];
        match self
            .manager
            .send_prompt_linked_with_message_id(
                &self.db,
                &conn_id,
                prompt_blocks,
                Some(cwd.folder_id),
                Some(conversation_id),
                None,
                None,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                self.index.lock().await.remove(&conn_id);
                let _ = self.manager.disconnect(&conn_id).await;
                Err(e.to_string())
            }
        }
    }


    /// Resolve the attempt a child connection belongs to.
    ///
    /// Normally the live index answers this. After a restart the index is empty
    /// while child agents may still be connected, so fall back to the running
    /// attempt recorded for that connection in the database.
    async fn attempt_state_for_connection(&self, connection_id: &str) -> Option<AttemptState> {
        if let Some(state) = self.index.lock().await.get(connection_id).cloned() {
            return Some(state);
        }
        let attempt =
            pipeline_service::find_running_attempt_by_connection(&self.db.conn, connection_id)
                .await
                .ok()
                .flatten()?;
        let run = pipeline_service::get_run_raw(&self.db.conn, attempt.run_id)
            .await
            .ok()?;
        let graph: PipelineGraph = serde_json::from_str(&run.graph).ok()?;
        let step_index = graph
            .steps
            .iter()
            .position(|s| s.id == attempt.step_id)?;
        let step = &graph.steps[step_index];
        // A missing folder row must not strand the attempt: the working dir is
        // only needed to launch further steps, not to close this one.
        let working_dir = crate::commands::folders::get_folder_core(
            &self.db,
            run.worktree_folder_id.unwrap_or(run.folder_id),
        )
        .await
        .map(|f| f.path)
        .unwrap_or_default();
        Some(AttemptState {
            run_id: attempt.run_id,
            attempt_id: attempt.id,
            step_index,
            iteration: attempt.iteration as u32,
            step_id: attempt.step_id.clone(),
            role: step.role,
            read_only: step.read_only,
            display_text: run.display_text.clone().unwrap_or_default(),
            working_dir,
            folder_id: run.folder_id,
        })
    }

    pub async fn on_event(&self, env: &EventEnvelope) {
        // Steps run unattended, so a permission card would sit on screen with
        // nobody to answer it and the step would burn its whole timeout. Answer
        // for our own connections: the user already approved this pipeline, and
        // the run is confined to its own worktree.
        if let AcpEvent::PermissionRequest {
            request_id,
            options,
            ..
        } = &env.payload
        {
            if self.index.lock().await.contains_key(&env.connection_id) {
                if let Some(option) = pick_allow_option(options) {
                    let _ = self
                        .manager
                        .respond_permission(&env.connection_id, request_id, &option)
                        .await;
                }
            }
            return;
        }

        let AcpEvent::TurnComplete { stop_reason, .. } = &env.payload else {
            return;
        };
        let conn_id = &env.connection_id;
        let entry = self.attempt_state_for_connection(conn_id).await;
        self.index.lock().await.remove(conn_id);
        let Some(attempt) = entry else {
            tracing::debug!("[pipeline] TurnComplete for unindexed connection {conn_id}");
            return;
        };

        let attempt_status = match stop_reason.as_str() {
            "cancelled" => AttemptStatus::Cancelled,
            _ => AttemptStatus::Done,
        };

        let cas_ok = pipeline_service::cas_attempt_status(
            &self.db.conn,
            attempt.attempt_id,
            AttemptStatus::Running,
            attempt_status,
        )
        .await
        .unwrap_or(false);

        if !cas_ok {
            tracing::debug!(
                "[pipeline] duplicate or stale TurnComplete for attempt {}",
                attempt.attempt_id
            );
            let _ = self.manager.disconnect(conn_id).await;
            return;
        }

        let summary = self.capture_summary(conn_id).await;
        let _ = pipeline_service::set_attempt_summary(
            &self.db.conn,
            attempt.attempt_id,
            summary.clone(),
        )
        .await;

        let _ = self.manager.disconnect(conn_id).await;

        if stop_reason == "cancelled" {
            return;
        }

        let (mut verdict, mut source, mut notes) = if let Some((v, n)) = self
            .recorded_verdicts
            .lock()
            .await
            .remove(&attempt.attempt_id)
        {
            (v, "tool".to_string(), n)
        } else if let Some(ref s) = summary {
            if let Some((v, n)) = verdict::parse_marker(s) {
                (v, "marker".to_string(), n)
            } else if attempt.role == PipelineRole::Reviewer || attempt.role == PipelineRole::Tests
            {
                (
                    PipelineVerdict::Inconclusive,
                    "none".to_string(),
                    Some("no verdict reported by reviewer/tests".into()),
                )
            } else if stop_reason == "end_turn" {
                (PipelineVerdict::Pass, "none".to_string(), None)
            } else {
                (
                    PipelineVerdict::Inconclusive,
                    "none".to_string(),
                    Some(format!("agent stopped: {stop_reason}")),
                )
            }
        } else if attempt.role == PipelineRole::Reviewer || attempt.role == PipelineRole::Tests {
            (
                PipelineVerdict::Inconclusive,
                "none".to_string(),
                Some("no verdict reported by reviewer/tests".into()),
            )
        } else if stop_reason == "end_turn" {
            (PipelineVerdict::Pass, "none".to_string(), None)
        } else {
            (
                PipelineVerdict::Inconclusive,
                "none".to_string(),
                Some(format!("agent stopped: {stop_reason}")),
            )
        };

        if attempt.read_only {
            let post_hash = compute_worktree_hash(&attempt.working_dir).await;
            let pre_hash = self
                .pre_step_hashes
                .lock()
                .await
                .remove(&attempt.attempt_id);
            if let Some(pre) = pre_hash {
                if pre != post_hash {
                    verdict = PipelineVerdict::Inconclusive;
                    source = "guard".to_string();
                    notes = Some("reviewer modified files".to_string());
                }
            }
        }

        let _ = pipeline_service::set_attempt_verdict(
            &self.db.conn,
            attempt.attempt_id,
            verdict,
            Some(source),
            notes.clone(),
        )
        .await;

        emit_event(
            &self.emitter,
            PIPELINE_CHANGED_EVENT,
            PipelineChange::StepSettled {
                run_id: attempt.run_id,
                attempt_id: attempt.attempt_id,
                verdict: Some(verdict),
            },
        );

        let Ok(run) = pipeline_service::get_run_raw(&self.db.conn, attempt.run_id).await else {
            return;
        };
        if run.status != "running" {
            self.active_runs.lock().await.remove(&attempt.folder_id);
            return;
        }

        let Ok(graph) = serde_json::from_str::<PipelineGraph>(&run.graph) else {
            return;
        };

        match verdict {
            PipelineVerdict::Pass => {
                if attempt.step_index + 1 >= graph.steps.len() {
                    let _ = pipeline_service::update_run_status(
                        &self.db.conn,
                        attempt.run_id,
                        PipelineRunStatus::Succeeded,
                        None,
                    )
                    .await;
                    self.active_runs.lock().await.remove(&attempt.folder_id);
                    let had_fix_round = attempt.iteration > 1;
                    let tests_ran = graph
                        .steps
                        .iter()
                        .any(|s| s.role == PipelineRole::Tests);
                    auto_record_memory(
                        &self.db,
                        "task_summary",
                        first_chars(&attempt.display_text, 80).to_string(),
                        summary.clone().unwrap_or_default(),
                        attempt.run_id,
                        attempt.folder_id,
                        tests_ran,
                    )
                    .await;
                    if had_fix_round {
                        if let Some(review_notes) =
                            pipeline_service::get_last_changes_requested_notes(
                                &self.db.conn,
                                attempt.run_id,
                            )
                            .await
                            .unwrap_or(None)
                        {
                            auto_record_memory(
                                &self.db,
                                "fixed_bug",
                                first_chars(&attempt.display_text, 80).to_string(),
                                review_notes,
                                attempt.run_id,
                                attempt.folder_id,
                                tests_ran,
                            )
                            .await;
                        }
                    }
                    emit_event(
                        &self.emitter,
                        PIPELINE_CHANGED_EVENT,
                        PipelineChange::RunSettled {
                            run_id: attempt.run_id,
                            status: PipelineRunStatus::Succeeded,
                        },
                    );
                } else if let Err(e) = self
                    .launch_step(
                        attempt.run_id,
                        attempt.step_index + 1,
                        attempt.iteration,
                        attempt.display_text.clone(),
                        Some(attempt.attempt_id),
                    )
                    .await
                {
                    self.fail_run(attempt.run_id, attempt.folder_id, e).await;
                }
            }
            PipelineVerdict::ChangesRequested => {
                let loop_edge = graph.loops.iter().find(|l| l.from_step == attempt.step_id);
                if let Some(loop_back) = loop_edge {
                    if attempt.iteration < loop_back.max_iterations {
                        if let Some(target_idx) =
                            graph.steps.iter().position(|s| s.id == loop_back.to_step)
                        {
                            let new_iteration = attempt.iteration + 1;
                            if let Err(e) = self
                                .launch_step(
                                    attempt.run_id,
                                    target_idx,
                                    new_iteration,
                                    attempt.display_text.clone(),
                                    Some(attempt.attempt_id),
                                )
                                .await
                            {
                                self.fail_run(attempt.run_id, attempt.folder_id, e).await;
                            }
                        } else {
                            let _ = pipeline_service::update_run_status(
                                &self.db.conn,
                                attempt.run_id,
                                PipelineRunStatus::Failed,
                                Some(format!(
                                    "loop target step '{}' not found",
                                    loop_back.to_step
                                )),
                            )
                            .await;
                            self.active_runs.lock().await.remove(&attempt.folder_id);
                            emit_event(
                                &self.emitter,
                                PIPELINE_CHANGED_EVENT,
                                PipelineChange::RunSettled {
                                    run_id: attempt.run_id,
                                    status: PipelineRunStatus::Failed,
                                },
                            );
                        }
                    } else {
                        let _ = pipeline_service::update_run_status(
                            &self.db.conn,
                            attempt.run_id,
                            PipelineRunStatus::StoppedMaxIterations,
                            Some(format!(
                                "reached maximum fix iterations ({})",
                                loop_back.max_iterations
                            )),
                        )
                        .await;
                        self.active_runs.lock().await.remove(&attempt.folder_id);
                        emit_event(
                            &self.emitter,
                            PIPELINE_CHANGED_EVENT,
                            PipelineChange::RunSettled {
                                run_id: attempt.run_id,
                                status: PipelineRunStatus::StoppedMaxIterations,
                            },
                        );
                    }
                } else {
                    let _ = pipeline_service::update_run_status(
                        &self.db.conn,
                        attempt.run_id,
                        PipelineRunStatus::Failed,
                        Some("changes requested but no loopback edge configured".into()),
                    )
                    .await;
                    self.active_runs.lock().await.remove(&attempt.folder_id);
                    emit_event(
                        &self.emitter,
                        PIPELINE_CHANGED_EVENT,
                        PipelineChange::RunSettled {
                            run_id: attempt.run_id,
                            status: PipelineRunStatus::Failed,
                        },
                    );
                }
            }
            PipelineVerdict::Inconclusive => {
                let _ = pipeline_service::update_run_status(
                    &self.db.conn,
                    attempt.run_id,
                    PipelineRunStatus::Inconclusive,
                    notes,
                )
                .await;
                self.active_runs.lock().await.remove(&attempt.folder_id);
                emit_event(
                    &self.emitter,
                    PIPELINE_CHANGED_EVENT,
                    PipelineChange::RunSettled {
                        run_id: attempt.run_id,
                        status: PipelineRunStatus::Inconclusive,
                    },
                );
            }
        }
    }

    async fn capture_summary(&self, conn_id: &str) -> Option<String> {
        let (state, _) = self.manager.get_state_and_emitter(conn_id).await?;
        let text = state.read().await.last_assistant_text.clone();
        text.filter(|t| !t.trim().is_empty())
    }

    pub async fn cancel(&self, run_id: i32) -> Result<(), String> {
        let run = match pipeline_service::get_run_raw(&self.db.conn, run_id).await {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };

        let settled = pipeline_service::update_run_status(
            &self.db.conn,
            run_id,
            PipelineRunStatus::Cancelled,
            Some("cancelled by user".into()),
        )
        .await
        .unwrap_or(false);

        if settled {
            emit_event(
                &self.emitter,
                PIPELINE_CHANGED_EVENT,
                PipelineChange::RunSettled {
                    run_id,
                    status: PipelineRunStatus::Cancelled,
                },
            );
        }

        let conn_id = {
            self.index
                .lock()
                .await
                .iter()
                .find(|(_, state)| state.run_id == run_id)
                .map(|(c, _)| c.clone())
        };

        if let Some(ref cid) = conn_id {
            let _ = self.manager.cancel(&self.db.conn, cid).await;
            let start_time = std::time::Instant::now();
            while start_time.elapsed().as_secs() < CANCEL_WAIT_TIMEOUT_SECS {
                let still_in_index = self.index.lock().await.contains_key(cid);
                if !still_in_index {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }

            self.index.lock().await.remove(cid);
            let _ = self.manager.disconnect(cid).await;
        }

        self.active_runs.lock().await.remove(&run.folder_id);
        let _ = pipeline_service::interrupt_running_attempts(&self.db.conn, run_id).await;

        Ok(())
    }

    pub async fn status(&self, run_id: i32) -> Result<PipelineRunInfo, String> {
        pipeline_service::get_run_info(&self.db.conn, run_id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn request_changes(&self, run_id: i32, notes: String) -> Result<(), String> {
        let run = pipeline_service::get_run_raw(&self.db.conn, run_id)
            .await
            .map_err(|e| format!("failed to load run: {e}"))?;

        if run.status == "running" {
            return Err("run is still running".into());
        }

        let graph: PipelineGraph =
            serde_json::from_str(&run.graph).map_err(|e| format!("invalid graph: {e}"))?;

        let coder_step_idx = graph
            .steps
            .iter()
            .position(|s| s.role == PipelineRole::Coder)
            .unwrap_or(0);

        let next_iteration = (run.current_iteration as u32) + 1;
        pipeline_service::reset_run_to_running(&self.db.conn, run_id, next_iteration)
            .await
            .map_err(|e| format!("db error: {e}"))?;

        self.active_runs.lock().await.insert(run.folder_id, run.id);

        emit_event(
            &self.emitter,
            PIPELINE_CHANGED_EVENT,
            PipelineChange::RunStarted {
                run_id,
                folder_id: run.folder_id,
            },
        );

        let last_attempt = pipeline_service::get_run_info(&self.db.conn, run_id)
            .await
            .ok()
            .and_then(|info| info.attempts.last().cloned());
        if let Some(att) = last_attempt {
            let _ = pipeline_service::set_attempt_verdict(
                &self.db.conn,
                att.id,
                PipelineVerdict::ChangesRequested,
                Some("user".into()),
                Some(notes.clone()),
            )
            .await;
        }

        // The coder's prompt renders `$task`, so a fix round must carry the
        // original task, not a placeholder.
        let display_text = run
            .display_text
            .clone()
            .unwrap_or_else(|| "Requested changes".to_string());
        self.launch_step(run_id, coder_step_idx, next_iteration, display_text, None)
            .await
    }

    pub async fn stop_for_manual_fix(&self, run_id: i32) -> Result<(), String> {
        self.cancel(run_id).await
    }

    pub async fn recover_on_boot(&self) {
        if let Err(e) = pipeline_service::interrupt_running_runs(&self.db.conn).await {
            tracing::warn!("[pipeline] failed to interrupt running runs on boot: {e}");
        }
        // Attempts outlive their run row otherwise: the UI keeps showing a
        // running step, and a stale attempt could still be matched by
        // connection id and accept a verdict from a dead agent.
        if let Err(e) = pipeline_service::interrupt_all_running_attempts(&self.db.conn).await {
            tracing::warn!("[pipeline] failed to close running attempts on boot: {e}");
        }
        self.active_runs.lock().await.clear();
        self.index.lock().await.clear();
    }
}

async fn compute_worktree_hash(working_dir: &str) -> String {
    let diff_out = crate::process::tokio_command("git")
        .args(["diff", "HEAD"])
        .current_dir(working_dir)
        .output()
        .await
        .map(|o| o.stdout)
        .unwrap_or_default();

    let untracked_out = crate::process::tokio_command("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(working_dir)
        .output()
        .await
        .map(|o| o.stdout)
        .unwrap_or_default();

    let mut untracked_lines: Vec<&str> = std::str::from_utf8(&untracked_out)
        .unwrap_or("")
        .lines()
        .collect();
    untracked_lines.sort_unstable();

    let mut hasher = Sha256::new();
    hasher.update(&diff_out);
    hasher.update(b"\n---UNTRACKED---\n");
    for line in untracked_lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn path_separator(path: &str) -> char {
    if path.contains('\\') {
        '\\'
    } else if path.contains('/') {
        '/'
    } else if is_drive_designator(path) {
        '\\'
    } else {
        '/'
    }
}

fn is_drive_designator(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_root_designator(trimmed: &str) -> bool {
    if trimmed.is_empty() || is_drive_designator(trimmed) {
        return true;
    }
    let unc = trimmed
        .strip_prefix("\\\\")
        .or_else(|| trimmed.strip_prefix("//"));
    match unc {
        Some(rest) => rest.split(['/', '\\']).filter(|s| !s.is_empty()).count() <= 2,
        None => false,
    }
}

fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if is_drive_designator(trimmed) {
        return "";
    }
    match trimmed.rfind(['/', '\\']) {
        Some(idx) => &trimmed[idx + 1..],
        None => trimmed,
    }
}

fn sibling_path(root_path: &str, name: &str) -> String {
    let trimmed = root_path.trim_end_matches(['/', '\\']);
    let separator = path_separator(root_path);
    if is_root_designator(trimmed) {
        return format!("{trimmed}{separator}{name}");
    }
    match trimmed.rfind(['/', '\\']) {
        Some(idx) => format!("{}{}{}", &trimmed[..idx], separator, name),
        None => name.to_string(),
    }
}

fn short_suffix(run_id: i32) -> String {
    format!("r{run_id}p")
}
