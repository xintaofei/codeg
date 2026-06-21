//! Automation execution engine: replays a saved composer snapshot through the
//! existing ACP launch chain, then settles the run from the event bus.
//!
//! Design (see docs/automations-spec.md §6/§9):
//! - Completion is correlated by `connection_id` (the `TurnComplete` event has no
//!   conversation_id), via an in-memory `connection_id -> (run_id, automation_id)`
//!   index. `stop_reason` is the settle authority.
//! - A per-tick reconcile backstop settles runs whose `TurnComplete` was dropped
//!   (broadcast lag) by reading the produced conversation's terminal status, and
//!   fails runs this process is not tracking that exceeded a generous deadline.
//! - The idle sweep is NOT a hazard: an in-flight turn sits in `Prompting`, which
//!   `sweep_idle` already skips (it only reaps `Connected`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::Utc;
use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Mutex;
use tokio::time::MissedTickBehavior;

use crate::acp::manager::ConnectionManager;
use crate::acp::types::{AcpEvent, EventEnvelope, PromptInputBlock};
use crate::acp::InternalEventBus;
use crate::commands::acp::{build_session_runtime_env, verify_agent_installed};
use crate::commands::conversations::{create_conversation_core, emit_conversation_upsert};
use crate::commands::folders::{
    emit_folder_upsert, get_folder_core, git_checkout, git_worktree_add, open_worktree_folder_core,
    resolve_worktree_folder_core,
};
use crate::db::entities::conversation::{self, ConversationStatus};
use crate::db::service::automation_service;
use crate::db::AppDatabase;
use crate::models::{
    AgentType, AutomationConfig, AutomationInfo, AutomationRunStatus, IsolationMode,
};
use crate::web::event_bridge::{
    emit_event, AutomationChange, EventEmitter, AUTOMATION_CHANGED_EVENT,
};

/// Generous absolute cap before a run we are no longer tracking (lost index, or
/// another process) is force-failed by the reconcile sweep. Not a turn timeout —
/// owned, live runs are never force-failed here.
const MAX_RUN_MINUTES: i64 = 180;

/// Reconcile sweep cadence.
const RECONCILE_INTERVAL_SECS: u64 = 30;

/// Scheduler poll cadence. Cron is minute-granular, so 30s catches each slot.
const SCHEDULER_INTERVAL_SECS: u64 = 30;

/// Run-history prune cadence + retention window.
const PRUNE_INTERVAL_SECS: u64 = 6 * 60 * 60;
const RUN_RETENTION_DAYS: i64 = 30;

static ENGINE: OnceLock<Arc<AutomationEngine>> = OnceLock::new();

/// The process-global engine, set once at boot by [`build_engine`]. Read by the
/// manual "run now" / cancel commands (and, later, the scheduler).
pub fn engine() -> Option<Arc<AutomationEngine>> {
    ENGINE.get().cloned()
}

pub struct AutomationEngine {
    db: AppDatabase,
    manager: ConnectionManager,
    emitter: EventEmitter,
    bus: Arc<InternalEventBus>,
    data_dir: PathBuf,
    /// Live automation runs: `connection_id -> (run_id, automation_id)`. The only
    /// way `TurnComplete` (keyed by connection_id) maps back to a run. Lost on
    /// restart — which is why boot reconcile + the conversation-status backstop
    /// exist.
    index: Arc<Mutex<HashMap<String, (i32, i32)>>>,
    /// Per-automation fire lock. Serializes the overlap-check + run-row insert (and
    /// the whole launch) so a manual run-now, a scheduled fire, and a double-click
    /// can't all pass `has_active_run` and start duplicate concurrent runs.
    automation_locks: Arc<Mutex<HashMap<i32, Arc<Mutex<()>>>>>,
    /// Serializes git checkout for `shared_in_root` runs on the same root folder.
    root_locks: Arc<Mutex<HashMap<i32, Arc<Mutex<()>>>>>,
}

struct ResolvedCwd {
    folder_id: i32,
    working_dir: String,
    worktree_folder_id: Option<i32>,
}

/// Build the engine and publish it to the process global. Returns the handle the
/// caller spawns via [`run_automation_engine`] (with its runtime's spawn fn).
pub fn build_engine(
    db: AppDatabase,
    manager: ConnectionManager,
    emitter: EventEmitter,
    bus: Arc<InternalEventBus>,
    data_dir: PathBuf,
) -> Arc<AutomationEngine> {
    let engine = Arc::new(AutomationEngine {
        db,
        manager,
        emitter,
        bus,
        data_dir,
        index: Arc::new(Mutex::new(HashMap::new())),
        automation_locks: Arc::new(Mutex::new(HashMap::new())),
        root_locks: Arc::new(Mutex::new(HashMap::new())),
    });
    let _ = ENGINE.set(engine.clone());
    engine
}

/// Long-running engine driver: boot recovery, then a single select loop over the
/// completion event stream + the reconcile interval. Spawn once per process in
/// each boot path (`lib.rs` setup via `tauri::async_runtime::spawn`, and
/// `bin/codeg_server.rs` via `tokio::spawn`).
pub async fn run_automation_engine(engine: Arc<AutomationEngine>) {
    // Boot recovery: a fresh process has no live connections, so any run still
    // `running` in the DB is an interruption — fail it (never re-fire here).
    match automation_service::boot_reconcile_interrupted(&engine.db.conn).await {
        Ok(n) if n > 0 => tracing::info!("[automation] boot reconcile failed {n} interrupted run(s)"),
        Ok(_) => {}
        Err(e) => tracing::warn!("[automation] boot reconcile error: {e}"),
    }

    let mut rx = engine.bus.subscribe();
    let mut reconcile = delay_interval(RECONCILE_INTERVAL_SECS);
    let mut schedule = delay_interval(SCHEDULER_INTERVAL_SECS);
    let mut prune = delay_interval(PRUNE_INTERVAL_SECS);

    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(env) => engine.on_event(&env).await,
                // Dropped events under lag — the reconcile backstop recovers them.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("[automation] event bus lagged {n}; reconcile will recover");
                }
                Err(RecvError::Closed) => break,
            },
            _ = reconcile.tick() => engine.reconcile_once().await,
            _ = schedule.tick() => {
                // Due-detection + CAS claim; each won slot fires off-thread so a
                // slow git/worktree launch never blocks the event/reconcile arms.
                let due = automation_service::list_due(&engine.db.conn, Utc::now())
                    .await
                    .unwrap_or_default();
                for id in due {
                    match automation_service::claim_due(&engine.db.conn, id, Utc::now()).await {
                        Ok(Some(slot)) => {
                            let eng = engine.clone();
                            tokio::spawn(async move {
                                if let Err(e) = eng.run_automation(id, "schedule", Some(slot)).await {
                                    tracing::info!("[automation] scheduled run {id}: {e}");
                                }
                            });
                        }
                        Ok(None) => {}
                        Err(e) => tracing::warn!("[automation] claim {id}: {e}"),
                    }
                }
            }
            _ = prune.tick() => {
                if let Err(e) =
                    automation_service::prune_old_runs(&engine.db.conn, RUN_RETENTION_DAYS).await
                {
                    tracing::warn!("[automation] prune error: {e}");
                }
            }
        }
    }
}

fn delay_interval(secs: u64) -> tokio::time::Interval {
    let mut i = tokio::time::interval(Duration::from_secs(secs));
    i.set_missed_tick_behavior(MissedTickBehavior::Delay);
    i
}

impl AutomationEngine {
    // ── fire ────────────────────────────────────────────────────────────────

    /// Fire one run of `automation_id`. Records the run row, launches the agent,
    /// and returns the new run id. Does NOT wait for completion (the event
    /// subscriber settles it). On any pre-completion failure the run is settled
    /// `failed` with a visible error (never a silent hang).
    pub async fn run_automation(
        &self,
        automation_id: i32,
        trigger: &str,
        scheduled_for: Option<chrono::DateTime<Utc>>,
    ) -> Result<i32, String> {
        // Serialize every fire of this automation: the overlap check + run-row
        // insert below is otherwise a check-then-act race that a manual run-now
        // racing a scheduled fire (or a double-click) defeats, starting two runs.
        let fire_lock = self.fire_lock(automation_id).await;
        let _fire_guard = fire_lock.lock().await;

        let auto = automation_service::get(&self.db.conn, automation_id)
            .await
            .map_err(|e| e.to_string())?;

        // Overlap guard: never run two of the same automation concurrently.
        if automation_service::has_active_run(&self.db.conn, automation_id)
            .await
            .map_err(|e| e.to_string())?
        {
            let _ =
                automation_service::record_skipped_run(&self.db.conn, automation_id, trigger, scheduled_for)
                    .await;
            self.emit(AutomationChange::Upsert { id: automation_id });
            return Err("previous run still active".to_string());
        }

        let run = automation_service::start_run(&self.db.conn, automation_id, trigger, scheduled_for)
            .await
            .map_err(|e| e.to_string())?;
        // RunStarted is emitted inside `launch` once the run is fully wired (its
        // connection + conversation attached), so the broadcast carries the live
        // "View conversation" link. A launch that fails before that point still
        // surfaces via the RunSettled(failed) emit in the `Err` arm below.

        match self.launch(&auto, run.id).await {
            Ok(()) => Ok(run.id),
            Err(e) => {
                let _ = automation_service::settle_run(
                    &self.db.conn,
                    run.id,
                    AutomationRunStatus::Failed,
                    None,
                    Some(e.clone()),
                    None,
                )
                .await;
                self.emit(AutomationChange::RunSettled {
                    automation_id,
                    run_id: run.id,
                    status: "failed".to_string(),
                });
                Err(e)
            }
        }
    }

    /// Replay the captured composer snapshot through the existing launch chain.
    async fn launch(&self, auto: &AutomationInfo, run_id: i32) -> Result<(), String> {
        let cfg: AutomationConfig =
            serde_json::from_value(auto.config.clone()).map_err(|e| format!("bad config: {e}"))?;
        let agent_type = parse_agent_type(&auto.agent_type)?;
        let blocks = cfg
            .prompt_blocks
            .iter()
            .map(|v| serde_json::from_value::<PromptInputBlock>(v.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("bad prompt blocks: {e}"))?;
        if blocks.is_empty() {
            return Err("prompt is empty".to_string());
        }

        let cwd = self.resolve_cwd(auto, run_id).await?;

        // Announce the resolved working folder so every client's sidebar knows it
        // BEFORE the conversation upsert below lands — a conversation in a fresh
        // per-run worktree has no (client-)known folder to group under and would
        // never render otherwise. Re-broadcasting an already-open root folder
        // (shared_in_root) is an idempotent no-op on the client.
        if let Ok(detail) = get_folder_core(&self.db, cwd.folder_id).await {
            emit_folder_upsert(&self.emitter, detail);
        }

        // Recompute env from current settings (never snapshotted); hard-fail
        // visibly if the agent is disabled or not installed.
        let runtime_env = build_session_runtime_env(&self.db, agent_type, None, &self.data_dir)
            .await
            .map_err(|e| e.to_string())?;
        verify_agent_installed(agent_type)
            .await
            .map_err(|e| e.to_string())?;

        // Fresh connection (session_id=None), owner-labelled "automation".
        let conn_id = self
            .manager
            .spawn_agent(
                agent_type,
                Some(cwd.working_dir.clone()),
                None,
                runtime_env,
                "automation".to_string(),
                self.emitter.clone(),
                cfg.mode_id.clone(),
                cfg.config_values.clone(),
            )
            .await
            .map_err(|e| e.to_string())?;

        // Create the conversation row, then adopt it in send_prompt (Branch A).
        let title = first_chars(&cfg.display_text, 80);
        let conversation_id =
            match create_conversation_core(&self.db.conn, cwd.folder_id, agent_type, Some(title)).await
            {
                Ok(id) => id,
                Err(e) => {
                    let _ = self.manager.disconnect(&conn_id).await;
                    return Err(e.to_string());
                }
            };

        // Surface the produced conversation in every client's sidebar the instant
        // it exists (InProgress) — independent of the implicit upsert inside
        // send_prompt_linked. Its folder was announced just above, so it can be
        // grouped/rendered right away; live status then rides the existing
        // ConversationStatusChanged → conversation://changed bridge.
        emit_conversation_upsert(&self.emitter, &self.db.conn, conversation_id).await;

        // Register for completion correlation BEFORE prompting, so a fast
        // TurnComplete can't race ahead of the index entry.
        self.index
            .lock()
            .await
            .insert(conn_id.clone(), (run_id, auto.id));
        let _ = automation_service::attach_run_runtime(
            &self.db.conn,
            run_id,
            Some(conversation_id),
            Some(conn_id.clone()),
            cwd.worktree_folder_id,
        )
        .await;

        // The run row now carries its connection + conversation, so RunStarted
        // here gives run history a running row whose "View conversation" link is
        // live during the run (not only after settle).
        self.emit(AutomationChange::RunStarted {
            automation_id: auto.id,
            run_id,
        });

        match self
            .manager
            .send_prompt_linked_with_message_id(
                &self.db,
                &conn_id,
                blocks,
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
                // The conversation row was created `InProgress`; with the prompt
                // never sent and the connection gone, no TurnComplete will flip it
                // and reconcile won't revisit a Failed run — so flip it terminal
                // here to avoid stranding a stuck-in-progress conversation.
                self.cancel_conversation(conversation_id).await;
                Err(e.to_string())
            }
        }
    }

    /// Resolve the working directory for a run from `(root_folder_id, isolation,
    /// branch)`, reusing the existing worktree/checkout machinery. v1 requires a
    /// target folder (folderless deferred).
    async fn resolve_cwd(&self, auto: &AutomationInfo, run_id: i32) -> Result<ResolvedCwd, String> {
        let Some(root_folder_id) = auto.root_folder_id else {
            return Err("automation has no target folder".to_string());
        };
        let root = get_folder_core(&self.db, root_folder_id)
            .await
            .map_err(|e| e.to_string())?;

        match auto.isolation {
            IsolationMode::WorktreePerRun => {
                // Fresh isolated worktree per run; names carry the automation +
                // run id so `git worktree list` / the branch tree groups them.
                let branch = format!("automation/{}/run-{}", auto.id, run_id);
                let dir = format!(
                    "{}-automation-{}-run-{}",
                    basename(&root.path),
                    auto.id,
                    run_id
                );
                let mut wt_path = sibling_path(&root.path, &dir);

                // Retry once with a short suffix if a leftover collides (a prior
                // attempt for this run id that failed before cleanup).
                if let Err(e) =
                    git_worktree_add(root.path.clone(), branch.clone(), wt_path.clone()).await
                {
                    let suffix = short_suffix(run_id);
                    let branch2 = format!("{branch}-{suffix}");
                    wt_path = sibling_path(&root.path, &format!("{dir}-{suffix}"));
                    git_worktree_add(root.path.clone(), branch2, wt_path.clone())
                        .await
                        .map_err(|_| format!("worktree add failed: {e}"))?;
                }

                let wt = open_worktree_folder_core(&self.db, wt_path, root_folder_id)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(ResolvedCwd {
                    folder_id: wt.id,
                    working_dir: wt.path,
                    worktree_folder_id: Some(wt.id),
                })
            }
            IsolationMode::SharedInRoot => {
                let Some(branch) = auto.branch.clone() else {
                    // No branch pinned: run in the root tree as-is.
                    return Ok(ResolvedCwd {
                        folder_id: root_folder_id,
                        working_dir: root.path,
                        worktree_folder_id: None,
                    });
                };

                // Serialize checkout per root so concurrent shared runs can't
                // corrupt each other's index during the switch.
                let lock = self.root_lock(root_folder_id).await;
                let _guard = lock.lock().await;

                let resolution =
                    resolve_worktree_folder_core(&self.db, root.path.clone(), branch.clone())
                        .await
                        .map_err(|e| e.to_string())?;
                match resolution.path {
                    Some(path) => Ok(ResolvedCwd {
                        folder_id: resolution.folder_id.unwrap_or(root_folder_id),
                        working_dir: path,
                        worktree_folder_id: resolution.folder_id,
                    }),
                    None => {
                        git_checkout(root.path.clone(), branch)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(ResolvedCwd {
                            folder_id: root_folder_id,
                            working_dir: root.path,
                            worktree_folder_id: None,
                        })
                    }
                }
            }
        }
    }

    async fn root_lock(&self, root_folder_id: i32) -> Arc<Mutex<()>> {
        let mut locks = self.root_locks.lock().await;
        locks
            .entry(root_folder_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn fire_lock(&self, automation_id: i32) -> Arc<Mutex<()>> {
        let mut locks = self.automation_locks.lock().await;
        locks
            .entry(automation_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    // ── completion ────────────────────────────────────────────────────────────

    async fn on_event(&self, env: &EventEnvelope) {
        let AcpEvent::TurnComplete { stop_reason, .. } = &env.payload else {
            return;
        };
        let conn_id = env.connection_id.clone();
        let entry = { self.index.lock().await.get(&conn_id).copied() };
        let Some((run_id, automation_id)) = entry else {
            return; // not an automation run
        };

        let (status, status_str) = classify_stop_reason(stop_reason);
        let summary = self.capture_summary(&conn_id).await;
        let error = if status == AutomationRunStatus::Failed {
            Some(format!("agent stopped: {stop_reason}"))
        } else {
            None
        };

        let settled = automation_service::settle_run(
            &self.db.conn,
            run_id,
            status,
            Some(stop_reason.clone()),
            error,
            summary,
        )
        .await;

        // One prompt, one turn, then disconnect (last_assistant_text is cleared
        // at the next turn start, so an automation connection is never reused).
        self.index.lock().await.remove(&conn_id);
        let _ = self.manager.disconnect(&conn_id).await;

        if let Ok(true) = settled {
            self.emit(AutomationChange::RunSettled {
                automation_id,
                run_id,
                status: status_str.to_string(),
            });
        }
    }

    /// Best-effort: capture the turn's final assistant text on the TurnComplete
    /// tick (it's process-local and cleared at the next turn start).
    async fn capture_summary(&self, conn_id: &str) -> Option<String> {
        let (state, _) = self.manager.get_state_and_emitter(conn_id).await?;
        let text = state.read().await.last_assistant_text.clone();
        text.filter(|t| !t.trim().is_empty())
    }

    // ── reconcile backstop ────────────────────────────────────────────────────

    async fn reconcile_once(&self) {
        let active = match automation_service::list_active_runs(&self.db.conn).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[automation] reconcile list error: {e}");
                return;
            }
        };
        if active.is_empty() {
            return;
        }
        // run_id -> connection_id for runs THIS process launched and is tracking.
        let owned: HashMap<i32, String> = {
            self.index
                .lock()
                .await
                .iter()
                .map(|(conn_id, (run_id, _))| (*run_id, conn_id.clone()))
                .collect()
        };
        let now = Utc::now();

        for run in active {
            if let Some(conn_id) = owned.get(&run.id) {
                // We own this run; `on_event` settles it authoritatively (with the
                // real stop_reason). Leave it alone while its connection is live —
                // settling here from coarse conversation status would race on_event
                // and discard stop_reason fidelity (and could mislabel a cancel).
                if self.manager.get_state_and_emitter(conn_id).await.is_some() {
                    continue;
                }
                // Connection gone but still Running: the TurnComplete can never
                // arrive. Settle from the conversation's terminal status, else
                // fail. Either way drop the now-dead index entry.
                let handled = self.settle_from_conversation(&run).await;
                self.index.lock().await.remove(conn_id);
                if !handled
                    && automation_service::settle_run(
                        &self.db.conn,
                        run.id,
                        AutomationRunStatus::Failed,
                        None,
                        Some("run lost its worker".to_string()),
                        None,
                    )
                    .await
                    .unwrap_or(false)
                {
                    self.emit_settled(&run, "failed");
                }
                continue;
            }

            // Not owned (lost index, or another process). Recover from the
            // conversation's terminal status (a dropped TurnComplete), else fail
            // once the run blows past a generous absolute deadline.
            if self.settle_from_conversation(&run).await {
                continue;
            }
            if let Some(started) = run.started_at {
                if now.signed_duration_since(started) > chrono::Duration::minutes(MAX_RUN_MINUTES)
                    && automation_service::settle_run(
                        &self.db.conn,
                        run.id,
                        AutomationRunStatus::Failed,
                        None,
                        Some("run exceeded max duration or lost its worker".to_string()),
                        None,
                    )
                    .await
                    .unwrap_or(false)
                {
                    self.emit_settled(&run, "failed");
                }
            }
        }
    }

    /// If the produced conversation reached a terminal status, settle the run
    /// accordingly (CAS) and emit. Returns true if the conversation was terminal
    /// (run handled — even if the CAS was lost to a concurrent settle); false if
    /// still InProgress or there is no produced conversation.
    async fn settle_from_conversation(&self, run: &crate::models::AutomationRunInfo) -> bool {
        let Some(conv_id) = run.conversation_id else {
            return false;
        };
        let Some(status) = self.conversation_status(conv_id).await else {
            return false;
        };
        let (run_status, status_str, error) = match status {
            ConversationStatus::PendingReview | ConversationStatus::Completed => {
                (AutomationRunStatus::Succeeded, "succeeded", None)
            }
            ConversationStatus::Cancelled => (
                AutomationRunStatus::Failed,
                "failed",
                Some("agent cancelled or refused".to_string()),
            ),
            ConversationStatus::InProgress => return false,
        };
        if automation_service::settle_run(&self.db.conn, run.id, run_status, None, error, None)
            .await
            .unwrap_or(false)
        {
            self.emit_settled(run, status_str);
        }
        true
    }

    async fn conversation_status(&self, conv_id: i32) -> Option<ConversationStatus> {
        conversation::Entity::find_by_id(conv_id)
            .one(&self.db.conn)
            .await
            .ok()
            .flatten()
            .map(|m| m.status)
    }

    // ── cancel ────────────────────────────────────────────────────────────────

    /// Cancel a run: stop the live turn if we own it, then settle `cancelled`.
    /// Settling a run with no live connection clears a wedged row.
    pub async fn cancel_run(&self, run_id: i32) -> Result<(), String> {
        // Settle first (CAS) so a racing reconcile tick can't relabel this user
        // cancel as Failed via the conversation-status path.
        let settled = automation_service::settle_run(
            &self.db.conn,
            run_id,
            AutomationRunStatus::Cancelled,
            Some("cancelled".to_string()),
            None,
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

        // Best-effort: stop the live turn and drop the index entry / connection.
        let conn_id = {
            self.index
                .lock()
                .await
                .iter()
                .find(|(_, (rid, _))| *rid == run_id)
                .map(|(c, _)| c.clone())
        };
        if let Some(conn_id) = conn_id {
            let _ = self.manager.cancel(&self.db.conn, &conn_id).await;
            self.index.lock().await.remove(&conn_id);
            let _ = self.manager.disconnect(&conn_id).await;
        }

        if settled {
            if let Ok(Some(run)) = run_by_id(&self.db.conn, run_id).await {
                self.emit(AutomationChange::RunSettled {
                    automation_id: run.automation_id,
                    run_id,
                    status: "cancelled".to_string(),
                });
            }
        }
        Ok(())
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn emit(&self, change: AutomationChange) {
        emit_event(&self.emitter, AUTOMATION_CHANGED_EVENT, change);
    }

    fn emit_settled(&self, run: &crate::models::AutomationRunInfo, status: &str) {
        self.emit(AutomationChange::RunSettled {
            automation_id: run.automation_id,
            run_id: run.id,
            status: status.to_string(),
        });
    }

    /// Flip a produced conversation to a terminal status — used when a launch
    /// fails after the row was created `InProgress`, so it isn't left stranded.
    async fn cancel_conversation(&self, conversation_id: i32) {
        if let Ok(Some(row)) = conversation::Entity::find_by_id(conversation_id)
            .one(&self.db.conn)
            .await
        {
            let mut active = row.into_active_model();
            active.status = Set(ConversationStatus::Cancelled);
            if active.update(&self.db.conn).await.is_ok() {
                // The create-time upsert announced this row as InProgress; converge
                // every sidebar to the terminal status (this direct flip emits no
                // ConversationStatusChanged of its own).
                emit_conversation_upsert(&self.emitter, &self.db.conn, conversation_id).await;
            }
        }
    }
}

async fn run_by_id(
    conn: &sea_orm::DatabaseConnection,
    run_id: i32,
) -> Result<Option<crate::db::entities::automation_run::Model>, sea_orm::DbErr> {
    crate::db::entities::automation_run::Entity::find_by_id(run_id)
        .one(conn)
        .await
}

fn parse_agent_type(s: &str) -> Result<AgentType, String> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| format!("unknown agent type: {s}"))
}

/// `end_turn` → succeeded; explicit cancel → cancelled; everything else
/// (refusal / max_tokens / max_turn_requests / empty / unknown) → failed.
fn classify_stop_reason(stop_reason: &str) -> (AutomationRunStatus, &'static str) {
    match stop_reason {
        "end_turn" => (AutomationRunStatus::Succeeded, "succeeded"),
        "cancelled" => (AutomationRunStatus::Cancelled, "cancelled"),
        _ => (AutomationRunStatus::Failed, "failed"),
    }
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

fn sibling_path(root_path: &str, name: &str) -> String {
    let trimmed = root_path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(idx) => format!("{}/{}", &trimmed[..idx], name),
        None => name.to_string(),
    }
}

fn short_suffix(run_id: i32) -> String {
    // Deterministic, leftover-avoiding suffix (no RNG needed at this layer).
    format!("r{run_id}b")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_stop_reason_maps_outcomes() {
        assert_eq!(classify_stop_reason("end_turn").1, "succeeded");
        assert_eq!(classify_stop_reason("cancelled").1, "cancelled");
        assert_eq!(classify_stop_reason("refusal").1, "failed");
        assert_eq!(classify_stop_reason("max_tokens").1, "failed");
        assert_eq!(classify_stop_reason("").1, "failed");
    }

    #[test]
    fn worktree_names_carry_ids() {
        assert_eq!(basename("/home/me/repo"), "repo");
        assert_eq!(basename("/home/me/repo/"), "repo");
        assert_eq!(sibling_path("/home/me/repo", "repo-automation-3-run-7"), "/home/me/repo-automation-3-run-7");
    }

    #[test]
    fn first_chars_truncates_on_char_boundary() {
        assert_eq!(first_chars("hello world", 5), "hello");
        assert_eq!(first_chars("日本語テスト", 3), "日本語");
    }
}
