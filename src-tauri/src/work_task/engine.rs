//! Work-task execution engine: drives the manual pipeline
//! `todo → queued → running ⇄ awaiting_input → review → merging → done`.
//!
//! Structure mirrors `automation::engine` (single per-process engine elected by
//! an exclusive data-dir file lock; a `tokio::select!` loop over the internal
//! event bus + a reconcile tick), with three task-specific additions:
//! - **run_seq generations**: every launch claims a new `run_seq`; events are
//!   matched on `(connection_id, run_seq)` and settle through CAS updates, so a
//!   cancel racing a late `TurnComplete` is a zero-side-effect no-op.
//! - **backend-driven awaiting_input**: the engine subscribes to
//!   Question/Permission/PlanApproval request+resolve events (the frontend has
//!   no global pending-question channel for unopened conversations) and flips
//!   `running ⇄ awaiting_input` from an outstanding-request-id set.
//! - **two-stage merge with persisted intent**: stage A merges base INTO the
//!   worktree (conflicts always land there); stage B lands on the base branch
//!   in the project folder under a per-folder git mutex, with the merge intent
//!   persisted before execution so crash recovery can replay git truth.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, Notify};
use tokio::time::MissedTickBehavior;

use crate::acp::manager::ConnectionManager;
use crate::acp::types::{AcpEvent, EventEnvelope, PromptCapabilitiesInfo, PromptInputBlock};
use crate::acp::work_task_tools::{TaskReportAck, WorkTaskToolAccess};
use crate::acp::InternalEventBus;
use crate::commands::acp::{build_session_runtime_env, verify_agent_installed};
use crate::commands::conversations::{create_conversation_core, emit_conversation_upsert};
use crate::commands::folders::{
    emit_folder_deleted, emit_folder_upsert, get_folder_core, git_worktree_add,
    open_worktree_folder_core, resolve_git_head,
};
use crate::db::entities::conversation::{self, ConversationStatus};
use crate::db::entities::work_task::WorkTaskStatus;
use crate::db::entities::{folder, folder_command};
use crate::db::service::{conversation_service, tab_service, work_task_service};
use crate::db::AppDatabase;
use crate::logging::throttle::{LagLogThrottle, LAG_LOG_WINDOW};
use crate::models::{
    AgentType, FollowUpIntent, WorkTaskConfig, WorkTaskFolderSettings, WorkTaskMergeState,
    WorkTaskPreflight, WorkTaskQueuedMerge, STAGE_PROMPT_ALL,
};
use crate::web::event_bridge::{
    emit_event, EventEmitter, WorkTaskChange, WORK_TASK_CHANGED_EVENT,
};
use crate::work_task::git as task_git;

/// Reconcile sweep cadence.
const RECONCILE_INTERVAL_SECS: u64 = 30;

/// How often planned starts are checked. Its own (tighter) tick rather than a
/// step of the reconcile sweep: this one is a cheap indexed-ish scan, and it
/// bounds how late a task the user planned for a given minute actually starts.
const SCHEDULE_INTERVAL_SECS: u64 = 15;

/// Cap on the preflight output tail persisted with a red light.
const PREFLIGHT_TAIL_CHARS: usize = 4000;

static ENGINE: OnceLock<Arc<TaskEngine>> = OnceLock::new();

/// The process-global engine, set once at boot by [`build_task_engine`]. Read
/// by the start/cancel/merge/cleanup commands.
pub fn engine() -> Option<Arc<TaskEngine>> {
    ENGINE.get().cloned()
}

pub struct TaskEngine {
    db: AppDatabase,
    manager: ConnectionManager,
    emitter: EventEmitter,
    bus: Arc<InternalEventBus>,
    data_dir: PathBuf,
    /// Live runs: `connection_id -> (task_id, run_seq)` — the only way events
    /// keyed by connection_id map back to a task generation. Lost on restart
    /// (boot reconcile covers that).
    index: Arc<Mutex<HashMap<String, (i32, i32)>>>,
    /// Outstanding blocking requests per task (`"q:<id>"`, `"p:<id>"`,
    /// `"a:<id>"` — namespaced so the three id spaces can't collide). Non-empty
    /// set ⇔ awaiting_input. Requests raised by a delegation sub-agent are
    /// additionally prefixed with the child's connection id
    /// (`"<child_conn>#p:<id>"`) so [`TaskEngine::forget_delegation_child`] can
    /// drop the whole group when that child goes away.
    awaiting: Arc<Mutex<HashMap<i32, HashSet<String>>>>,
    /// `child_connection_id -> parent_connection_id` for delegation children of
    /// a task run. A sub-agent's blocking prompts arrive on the CHILD's
    /// connection, which is not in `index` — without this mapping they are
    /// dropped and the board keeps saying "running" while the run is actually
    /// parked on the user (#447). Populated from `DelegationStarted` (only for
    /// connections that ARE task runs) and dropped on `DelegationCompleted`.
    delegation_parents: Arc<Mutex<HashMap<String, String>>>,
    /// Tasks currently being launched (`queued`, then `preparing` in DB, but
    /// owned by an in-flight launch), mapped to their folder so the pump's
    /// concurrency accounting stays per-folder. Keeps the pump from
    /// double-launching, and is the reconcile sweep's ownership test for
    /// `preparing` rows. Entries carry an ownership token: a slow launch that
    /// is still unwinding must not drop the entry a newer launch of the same
    /// task now depends on.
    launching: Arc<Mutex<HashMap<i32, LaunchOwner>>>,
    /// Source of `LaunchOwner` tokens.
    launch_token: Arc<std::sync::atomic::AtomicU64>,
    /// Live setup (init command) child processes, by task. A cancel kills the
    /// process TREE so a long `pnpm install` stops with the task instead of
    /// running to completion in the background.
    setup_children: Arc<Mutex<HashMap<i32, SetupChild>>>,
    /// Tasks whose merge/cleanup is executing in THIS process — the reconcile
    /// tick must not run crash recovery against them.
    merging: Arc<Mutex<HashSet<i32>>>,
    /// Per-task lock serializing launch vs cancel teardown (same role as the
    /// automation engine's fire lock).
    task_locks: Arc<Mutex<HashMap<i32, Arc<Mutex<()>>>>>,
    /// Per-project-folder git mutex: merge and worktree cleanup serialize here.
    folder_locks: Arc<Mutex<HashMap<i32, Arc<Mutex<()>>>>>,
    /// Per-folder pump lock so concurrent pumps can't over-launch past
    /// max_concurrent.
    pump_locks: Arc<Mutex<HashMap<i32, Arc<Mutex<()>>>>>,
    /// Held for the engine's lifetime: exclusive advisory lock on
    /// `<db>.tasks.lock`. Its existence proves this process is the sole task
    /// engine on the DB — the precondition for the destructive boot reconcile.
    _engine_lock: std::fs::File,
}

/// Build the engine and publish it to the process global. Fails closed like the
/// automation engine: `None` unless this process holds the exclusive task lock.
pub fn build_task_engine(
    db: AppDatabase,
    manager: ConnectionManager,
    emitter: EventEmitter,
    bus: Arc<InternalEventBus>,
    data_dir: PathBuf,
) -> Option<Arc<TaskEngine>> {
    let engine_lock = match acquire_engine_ownership(&data_dir) {
        Ownership::Exclusive(file) => file,
        Ownership::Taken => {
            tracing::info!(
                "[work_task] another codeg process owns the task engine for {}; \
                 this process will not drive tasks",
                data_dir.display()
            );
            return None;
        }
        Ownership::Unavailable => {
            tracing::warn!(
                "[work_task] could not establish the task engine lock for {}; \
                 tasks are disabled in this process",
                data_dir.display()
            );
            return None;
        }
    };
    let engine = Arc::new(TaskEngine {
        db,
        manager,
        emitter,
        bus,
        data_dir,
        index: Arc::new(Mutex::new(HashMap::new())),
        awaiting: Arc::new(Mutex::new(HashMap::new())),
        delegation_parents: Arc::new(Mutex::new(HashMap::new())),
        launching: Arc::new(Mutex::new(HashMap::new())),
        launch_token: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        setup_children: Arc::new(Mutex::new(HashMap::new())),
        merging: Arc::new(Mutex::new(HashSet::new())),
        task_locks: Arc::new(Mutex::new(HashMap::new())),
        folder_locks: Arc::new(Mutex::new(HashMap::new())),
        pump_locks: Arc::new(Mutex::new(HashMap::new())),
        _engine_lock: engine_lock,
    });
    let _ = ENGINE.set(engine.clone());
    Some(engine)
}

/// Hard stop when walking `TaskEngine::delegation_parents` up to its run. Well
/// above any usable `depth_limit` (the delegation config's, which bounds the
/// real chain), so it only ever fires against a map that shouldn't exist —
/// insurance against spinning, not a functional limit.
const MAX_DELEGATION_CHAIN_HOPS: usize = 16;

/// Build an engine WITHOUT taking the process-wide ownership lock or publishing
/// it to the `ENGINE` cell, so a test can drive the instance methods (event
/// handling, request tracking) directly. The subscriber loop is never started;
/// tests feed `on_event` themselves.
#[cfg(test)]
fn test_engine(db: AppDatabase) -> Arc<TaskEngine> {
    Arc::new(TaskEngine {
        db,
        manager: ConnectionManager::new(),
        emitter: EventEmitter::Noop,
        bus: Arc::new(InternalEventBus::new(Default::default())),
        // An anonymous temp file, not a handle on `data_dir`: opening a
        // DIRECTORY as a File succeeds on Unix but fails on Windows.
        _engine_lock: tempfile::tempfile().expect("temp file"),
        data_dir: std::env::temp_dir(),
        index: Arc::new(Mutex::new(HashMap::new())),
        awaiting: Arc::new(Mutex::new(HashMap::new())),
        delegation_parents: Arc::new(Mutex::new(HashMap::new())),
        launching: Arc::new(Mutex::new(HashMap::new())),
        launch_token: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        setup_children: Arc::new(Mutex::new(HashMap::new())),
        merging: Arc::new(Mutex::new(HashSet::new())),
        task_locks: Arc::new(Mutex::new(HashMap::new())),
        folder_locks: Arc::new(Mutex::new(HashMap::new())),
        pump_locks: Arc::new(Mutex::new(HashMap::new())),
    })
}

enum Ownership {
    Exclusive(std::fs::File),
    Taken,
    Unavailable,
}

/// `<db-file>.tasks.lock` — sibling of the automation engine's `<db>.lock`, so
/// the two engines elect independently but both contend exactly when the DB is
/// shared.
fn engine_lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join(format!("{}.tasks.lock", crate::db::database_file_name()))
}

fn acquire_engine_ownership(data_dir: &Path) -> Ownership {
    let path = engine_lock_path(data_dir);
    let file = match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("[work_task] engine lock open failed: {e}");
            return Ownership::Unavailable;
        }
    };
    match file.try_lock() {
        Ok(()) => Ownership::Exclusive(file),
        Err(std::fs::TryLockError::WouldBlock) => Ownership::Taken,
        Err(std::fs::TryLockError::Error(e)) => {
            tracing::warn!("[work_task] engine lock failed: {e}");
            Ownership::Unavailable
        }
    }
}

/// Long-running driver: boot recovery, then a select loop over the event bus +
/// the reconcile tick. Spawn once per process in each boot path.
pub async fn run_task_engine(engine: Arc<TaskEngine>) {
    // Boot recovery: no connections (and no setup child processes) survive a
    // restart, so queued / preparing / running / awaiting_input are
    // interruptions → failed(interrupted); retry is idempotent (the worktree is
    // reused, and an init command that never finished re-runs — see the setup
    // marker). merging is exempt — it recovers from git truth below, never from
    // connection liveness.
    match work_task_service::boot_reconcile_interrupted(&engine.db.conn).await {
        Ok(n) if n > 0 => {
            tracing::info!("[work_task] boot reconcile failed {n} interrupted task(s)");
            engine.emit_changed_all();
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("[work_task] boot reconcile error: {e}"),
    }
    match work_task_service::list_by_status(&engine.db.conn, &[WorkTaskStatus::Merging]).await {
        Ok(rows) => {
            for row in rows {
                engine.recover_merging(row.id).await;
            }
        }
        Err(e) => tracing::warn!("[work_task] boot merging scan error: {e}"),
    }

    let mut rx = engine.bus.subscribe();
    let mut reconcile = {
        let mut i = tokio::time::interval(Duration::from_secs(RECONCILE_INTERVAL_SECS));
        i.set_missed_tick_behavior(MissedTickBehavior::Delay);
        i
    };
    // Fires immediately on its first tick, which is also the catch-up pass: a
    // plan whose time passed while the app was closed runs late rather than
    // never.
    let mut schedule = {
        let mut i = tokio::time::interval(Duration::from_secs(SCHEDULE_INTERVAL_SECS));
        i.set_missed_tick_behavior(MissedTickBehavior::Delay);
        i
    };
    let mut lag_throttle = LagLogThrottle::new(LAG_LOG_WINDOW);

    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(env) => engine.on_event(&env).await,
                Err(RecvError::Lagged(n)) => {
                    if let Some(s) = lag_throttle.record(n) {
                        tracing::warn!(
                            "[work_task] event bus lagged: dropped {} events across \
                             {} occurrence(s) in the last {}s; reconcile will recover",
                            s.dropped,
                            s.occurrences,
                            LAG_LOG_WINDOW.as_secs()
                        );
                    }
                }
                Err(RecvError::Closed) => break,
            },
            _ = schedule.tick() => engine.claim_due_scheduled().await,
            _ = reconcile.tick() => engine.reconcile_once().await,
        }
    }
}

/// What a merge request actually did. Merges into one base branch are serial,
/// so a click that finds the folder's slot busy takes a place in line instead
/// of failing — and the caller has to be able to tell the user which happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeDispatch {
    /// The merge generation is running now.
    Dispatched,
    /// Parked on the row; the folder's merge pump dispatches it when the slot
    /// frees.
    Queued,
}

impl MergeDispatch {
    pub fn is_queued(self) -> bool {
        matches!(self, MergeDispatch::Queued)
    }
}

/// The queued merge a pump dispatch is FOR, carried from the scan down to the
/// CAS that spends it.
///
/// `raw` is the row's `pending_merge` JSON verbatim — an optimistic token, not
/// a re-serialization: every write that consumes a queued merge demands the
/// column still equal it. Between the scan and the dispatch the pump does a
/// worktree stat, takes the folder lock and runs three git subprocesses, and a
/// user can withdraw or edit the merge anywhere in that window. `run_seq` does
/// not move for either, so without this token a withdrawn merge would still
/// land on the base branch.
struct QueuedMergeClaim {
    raw: String,
    /// The instant the task took its place in line, so a re-park (the slot got
    /// taken first) keeps it rather than going to the back.
    queued_at: chrono::DateTime<chrono::Utc>,
}

/// What one pass over a folder's merge queue concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainOutcome {
    /// A merge is running (or was just dispatched / refused) — the folder's one
    /// merge slot is spoken for this round.
    Taken,
    /// Nothing is queued. The slot is free for the auto-merge sweep.
    Empty,
    /// The queue changed while this pass worked on it; the snapshot is out of
    /// date and must be re-read before anything else may use the slot.
    Stale,
}

/// How many times a drain re-reads a queue that changed under it before it
/// gives the slot back to the next pump. Each retry requires a fresh
/// concurrent change, so this only bounds a pathological click loop.
const MERGE_QUEUE_DRAIN_ATTEMPTS: usize = 3;

/// How a launch composes its prompt.
enum LaunchMode {
    /// First run: the task's own prompt blocks.
    Fresh,
    /// Retry after failure: resume the session if possible and ask to continue.
    Retry,
    /// A follow-up on a reviewed task: the user's text, framed by their intent,
    /// plus whatever the composer attached out of band (images, pasted bytes).
    Return {
        intent: FollowUpIntent,
        feedback: String,
        attachments: Vec<serde_json::Value>,
    },
    /// Merge generation: the agent lands the task onto the base branch itself
    /// (sync base into the worktree, resolve conflicts, merge into base). The
    /// task sits in `merging` for the whole turn; the engine settles from git
    /// truth, never from the agent's word.
    Merge {
        root_path: String,
        base_branch: String,
        work_branch: String,
        /// "squash" | "merge"
        strategy: String,
        /// `None` → the agent writes the commit message itself.
        message: Option<String>,
    },
}

impl LaunchMode {
    /// The task status a launch of this mode expects to find when it starts.
    fn expected_status(&self) -> WorkTaskStatus {
        match self {
            LaunchMode::Merge { .. } => WorkTaskStatus::Merging,
            _ => WorkTaskStatus::Queued,
        }
    }

    /// The status the task holds for the REST of the launch, once setup has
    /// begun — what the cancel gates re-check. A merge generation stays
    /// `merging` throughout; every other mode moves `queued → preparing` before
    /// it touches the worktree.
    fn in_flight_status(&self) -> WorkTaskStatus {
        match self {
            LaunchMode::Merge { .. } => WorkTaskStatus::Merging,
            _ => WorkTaskStatus::Preparing,
        }
    }

    /// Timeline `round` label for the prompt this mode composes. Every
    /// follow-up intent shares the `return` stage: the folder's per-stage
    /// prompt settings stay four stages wide, and the intent rides the `round`
    /// event beside this label for the transcript's phase divider.
    fn round_kind(&self) -> &'static str {
        match self {
            LaunchMode::Fresh => "work",
            LaunchMode::Retry => "retry",
            LaunchMode::Return { .. } => "return",
            LaunchMode::Merge { .. } => "merge",
        }
    }

    /// The follow-up intent this launch carries, for the `round` marker.
    fn round_intent(&self) -> Option<FollowUpIntent> {
        match self {
            LaunchMode::Return { intent, .. } => Some(*intent),
            _ => None,
        }
    }

    /// Whether this launch may write to the worktree. A question is answered in
    /// chat; everything else is work.
    fn is_read_only(&self) -> bool {
        matches!(
            self,
            LaunchMode::Return {
                intent: FollowUpIntent::Question,
                ..
            }
        )
    }
}

impl TaskEngine {
    // ── user entry points ───────────────────────────────────────────────────

    /// Manual start: claim todo → queued, then pump the folder.
    pub async fn start(self: &Arc<Self>, task_id: i32) -> Result<(), String> {
        let task = work_task_service::get_model(&self.db.conn, task_id)
            .await
            .map_err(|e| e.to_string())?;
        self.preflight_folder(task.folder_id).await?;
        match work_task_service::claim_for_run(&self.db.conn, task_id, WorkTaskStatus::Todo, "user")
            .await
            .map_err(|e| e.to_string())?
        {
            Some(_) => {
                self.emit_upsert(task_id);
                self.pump_folder(task.folder_id).await;
                Ok(())
            }
            None => Err("task is not in todo".to_string()),
        }
    }

    /// "Start all": claim every todo of the folder, then pump. With no folder
    /// selected this is the global sweep — every folder that holds todos, each
    /// on its own preflight (an invalid folder is skipped, not fatal).
    pub async fn start_all(self: &Arc<Self>, folder_id: Option<i32>) -> Result<u32, String> {
        let folder_ids = match folder_id {
            Some(id) => vec![id],
            None => work_task_service::folders_with_todos(&self.db.conn)
                .await
                .map_err(|e| e.to_string())?,
        };
        let explicit = folder_id.is_some();
        let mut claimed = 0u32;
        for fid in folder_ids {
            if let Err(e) = self.preflight_folder(fid).await {
                if explicit {
                    return Err(e);
                }
                tracing::info!("[work_task] start all skips folder {fid}: {e}");
                continue;
            }
            let ids = work_task_service::list_todo_ids(&self.db.conn, fid)
                .await
                .map_err(|e| e.to_string())?;
            let mut folder_claimed = 0u32;
            for id in ids {
                // The unplanned-only claim, not the generic one: `ids` is a
                // snapshot, and a plan set in the meantime must still be
                // honoured rather than silently overridden by a bulk button.
                if work_task_service::claim_unplanned_for_run(
                    &self.db.conn,
                    id,
                    WorkTaskStatus::Todo,
                    "user",
                )
                .await
                    .map_err(|e| e.to_string())?
                    .is_some()
                {
                    folder_claimed += 1;
                    self.emit_upsert(id);
                }
            }
            if folder_claimed > 0 {
                self.pump_folder(fid).await;
            }
            claimed += folder_claimed;
        }
        Ok(claimed)
    }

    /// Retry a failed task: claim failed → queued (same worktree / session
    /// reused by the launch), then pump. An optional note rides the claim's own
    /// transaction and reaches the retry prompt — a failure usually has a cause
    /// the user knows and the agent doesn't.
    pub async fn retry(
        self: &Arc<Self>,
        task_id: i32,
        note: Option<String>,
        attachments: Vec<serde_json::Value>,
    ) -> Result<(), String> {
        let task = work_task_service::get_model(&self.db.conn, task_id)
            .await
            .map_err(|e| e.to_string())?;
        self.preflight_folder(task.folder_id).await?;
        let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        // An attachment is an instruction on its own: a screenshot with no
        // sentence still has to reach the retry prompt, so the action is
        // recorded whenever EITHER part is present.
        let action = (note.is_some() || !attachments.is_empty()).then(|| {
            serde_json::json!({
                "action": "retry",
                "note": note.unwrap_or_default(),
                "blocks": attachments,
            })
        });
        match work_task_service::claim_for_run_with_action(
            &self.db.conn,
            task_id,
            WorkTaskStatus::Failed,
            "user",
            action,
        )
        .await
        .map_err(|e| e.to_string())?
        {
            Some(_) => {
                self.emit_upsert(task_id);
                self.pump_folder(task.folder_id).await;
                Ok(())
            }
            None => Err("task is not in failed".to_string()),
        }
    }

    /// Send a reviewed task back to the agent with a follow-up. Launches
    /// directly (explicit user action — does not wait behind the queue).
    ///
    /// The intent picks the wording the agent receives; the feedback itself is
    /// recorded inside the claim's transaction, so a pump that steals the
    /// freshly queued generation still finds the instruction.
    pub async fn return_task(
        self: &Arc<Self>,
        task_id: i32,
        intent: FollowUpIntent,
        feedback: String,
        attachments: Vec<serde_json::Value>,
    ) -> Result<(), String> {
        let task = work_task_service::get_model(&self.db.conn, task_id)
            .await
            .map_err(|e| e.to_string())?;
        self.preflight_folder(task.folder_id).await?;
        let Some(_) = work_task_service::claim_for_run_with_action(
            &self.db.conn,
            task_id,
            WorkTaskStatus::Review,
            "user",
            Some(serde_json::json!({
                "action": "return",
                "intent": intent.as_str(),
                "feedback": feedback,
                "blocks": attachments,
            })),
        )
        .await
        .map_err(|e| e.to_string())?
        else {
            return Err("task is not in review".to_string());
        };
        self.emit_upsert(task_id);
        self.spawn_launch(
            task_id,
            task.folder_id,
            LaunchMode::Return {
                intent,
                feedback,
                attachments,
            },
        );
        Ok(())
    }

    /// Cancel a task from any non-terminal state except merging. Worktree is
    /// kept (the card offers cleanup separately). `reason` is the user's own
    /// note for the timeline; internal cancels (a conversation the user stopped
    /// from the chat UI, a delete) pass None.
    pub async fn cancel(
        self: &Arc<Self>,
        task_id: i32,
        reason: Option<String>,
    ) -> Result<(), String> {
        let won = work_task_service::cancel(&self.db.conn, task_id, reason.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        if !won {
            return Err("task cannot be canceled in its current state".to_string());
        }
        self.emit_upsert(task_id);

        // Kill a running init command BEFORE waiting on the task lock: the
        // launch holds that lock for its whole setup, so waiting first would
        // mean waiting out the very `pnpm install` we are trying to stop. The
        // run_seq we just canceled scopes the kill to this generation (cancel
        // does not bump it, so the row still carries it).
        if let Ok(task) = work_task_service::get_model(&self.db.conn, task_id).await {
            self.kill_setup_child(task_id, task.run_seq).await;
        }

        // Serialize the teardown with a possibly in-flight launch: the launch
        // holds the task lock across spawn → prompt, and its status gates
        // re-read after each step — so we tear down either before the prompt
        // (gate aborts) or after the turn is truly in flight (manager.cancel
        // aborts a real turn), never interleaved with the prompt enqueue.
        let lock = self.task_lock(task_id).await;
        let _guard = lock.lock().await;

        let conn_id = {
            self.index
                .lock()
                .await
                .iter()
                .find(|(_, (tid, _))| *tid == task_id)
                .map(|(c, _)| c.clone())
        };
        if let Some(conn_id) = conn_id {
            let _ = self.manager.cancel(&self.db.conn, &conn_id).await;
            self.index.lock().await.remove(&conn_id);
            self.forget_delegation_children_of(&conn_id).await;
            let _ = self.manager.disconnect(&conn_id).await;
        }
        self.awaiting.lock().await.remove(&task_id);

        // Converge a stranded InProgress conversation.
        let task = work_task_service::get_model(&self.db.conn, task_id).await.ok();
        if let Some(conv_id) = task.as_ref().and_then(|t| t.conversation_id) {
            if self.conversation_status(conv_id).await == Some(ConversationStatus::InProgress) {
                self.cancel_conversation(conv_id).await;
            }
        }
        // The slot freed — refill from the queue (and an auto folder's todo).
        if let Some(folder_id) = task.map(|t| t.folder_id) {
            self.pump_folder(folder_id).await;
        }
        Ok(())
    }

    // ── scheduler ───────────────────────────────────────────────────────────

    /// Queue every to-do task whose planned start has arrived, then pump the
    /// folders that gained one. Runs on its own tick and also as a nudge right
    /// after a plan is set, so a time already in the past takes effect at once.
    ///
    /// Claiming does not launch: the task lands in `queued` like any manual
    /// start, and the folder's concurrency limit still governs what actually
    /// runs.
    pub async fn claim_due_scheduled(self: &Arc<Self>) {
        let claimed =
            match work_task_service::claim_due_scheduled(&self.db.conn, chrono::Utc::now()).await {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::warn!("[work_task] scheduled claim error: {e}");
                    return;
                }
            };
        if claimed.is_empty() {
            return;
        }
        let mut folders: Vec<i32> = Vec::new();
        for (task_id, folder_id) in claimed {
            tracing::info!("[work_task] scheduled start claimed task {task_id}");
            self.emit_upsert(task_id);
            if !folders.contains(&folder_id) {
                folders.push(folder_id);
            }
        }
        for folder_id in folders {
            self.pump_folder(folder_id).await;
        }
    }

    // ── pump ────────────────────────────────────────────────────────────────

    /// Launch queued tasks of a folder up to its `max_concurrent` (0 =
    /// unlimited). Serialized per folder so concurrent pumps can't over-launch.
    pub async fn pump_folder(self: &Arc<Self>, folder_id: i32) {
        let lock = {
            let mut locks = self.pump_locks.lock().await;
            locks
                .entry(folder_id)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        let settings = work_task_service::settings_get_effective(&self.db.conn, folder_id)
            .await
            .unwrap_or_default();
        let max = settings.max_concurrent.max(0) as u64;

        // Scheduler arm: an auto_process folder claims todo heads into the
        // queue until the budget (which counts queued) is spent; the drain
        // loop below then launches them like any manually queued task.
        if settings.auto_process {
            loop {
                match work_task_service::auto_claim_next(
                    &self.db.conn,
                    folder_id,
                    settings.max_concurrent,
                )
                .await
                {
                    Ok(Some(id)) => self.emit_upsert(id),
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("[work_task] auto claim error: {e}");
                        break;
                    }
                }
            }
        }

        loop {
            // Only THIS folder's in-flight launches count against its limit.
            let launching: Vec<i32> = self
                .launching
                .lock()
                .await
                .iter()
                .filter(|(_, owner)| owner.folder_id == folder_id)
                .map(|(tid, _)| *tid)
                .collect();
            let active = match work_task_service::active_launched_count(&self.db.conn, folder_id)
                .await
            {
                Ok(n) => n + launching.len() as u64,
                Err(e) => {
                    tracing::warn!("[work_task] pump count error: {e}");
                    return;
                }
            };
            if max != 0 && active >= max {
                return;
            }
            let next = match work_task_service::next_queued(&self.db.conn, folder_id, &launching)
                .await
            {
                Ok(Some(t)) => t,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!("[work_task] pump next error: {e}");
                    return;
                }
            };
            // Claimed synchronously: this loop iterates immediately, and the
            // task must already read as in-flight when it does.
            let token = self.claim_launch_slot(next.id, folder_id).await;
            self.spawn_launch_owned(next.id, folder_id, launch_mode_for(&next), Some(token));
        }
    }

    /// Mark `task_id` as owned by a new launch and return the ownership token.
    async fn claim_launch_slot(&self, task_id: i32, folder_id: i32) -> u64 {
        let token = self
            .launch_token
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.launching
            .lock()
            .await
            .insert(task_id, LaunchOwner { folder_id, token });
        token
    }

    /// Give up ownership — but only if the entry is still ours. A launch that
    /// is slowly unwinding must not drop the slot a NEWER launch of the same
    /// task now holds, or the reconcile sweep would requeue a live setup and
    /// the folder's accounting would lose a slot.
    async fn release_launch_slot(&self, task_id: i32, token: u64) {
        let mut launching = self.launching.lock().await;
        if launching.get(&task_id).is_some_and(|o| o.token == token) {
            launching.remove(&task_id);
        }
    }

    fn spawn_launch(self: &Arc<Self>, task_id: i32, folder_id: i32, mode: LaunchMode) {
        self.spawn_launch_owned(task_id, folder_id, mode, None);
    }

    /// `token: Some(_)` when the caller already claimed the slot (the pump,
    /// whose loop must see the task as in-flight the moment it continues);
    /// `None` to claim it here.
    fn spawn_launch_owned(
        self: &Arc<Self>,
        task_id: i32,
        folder_id: i32,
        mode: LaunchMode,
        token: Option<u64>,
    ) {
        let engine = self.clone();
        tokio::spawn(async move {
            let token = match token {
                Some(token) => token,
                None => engine.claim_launch_slot(task_id, folder_id).await,
            };
            // A merge generation stays `merging` and is NEVER failed: it
            // recovers from git truth (`recover_merging`, driven by the
            // reconcile tick) so a half-merge can go back to review.
            let is_merge = matches!(mode, LaunchMode::Merge { .. });
            // The generation the launch actually operated on, published as soon
            // as it has read the row. A setup failure must be attributed to THAT
            // generation: a cancel + requeue (which bumps run_seq) can land
            // while a slow setup is still unwinding, and failing the row by its
            // *current* sequence would kill the fresh run instead.
            let launched_seq = LaunchSeq::default();
            let result = engine.launch(task_id, mode, &launched_seq).await;
            let errored = result.is_err();
            if let Err(e) = result {
                tracing::info!("[work_task] launch {task_id}: {e}");
                if let (false, Some(seq)) = (is_merge, launched_seq.get()) {
                    let failed = work_task_service::fail(
                        &engine.db.conn,
                        task_id,
                        &[WorkTaskStatus::Queued, WorkTaskStatus::Preparing],
                        Some(seq),
                        "setup_error",
                        Some(e),
                    )
                    .await
                    .unwrap_or(false);
                    if failed {
                        engine.emit_upsert(task_id);
                    }
                }
            }
            // Released only after the failure is settled: while the entries are
            // held, the reconcile sweep cannot mistake this row for an orphaned
            // `preparing`, and a cancel still has a kill slot to write to.
            engine.release_setup_slot(task_id, launched_seq.get()).await;
            engine.release_launch_slot(task_id, token).await;
            if errored {
                // A slot may have opened up (or this task left the queue) —
                // keep draining.
                engine.pump_folder(folder_id).await;
            }
        });
    }

    // ── launch ──────────────────────────────────────────────────────────────

    async fn launch(
        self: &Arc<Self>,
        task_id: i32,
        mode: LaunchMode,
        launched_seq: &LaunchSeq,
    ) -> Result<(), String> {
        let lock = self.task_lock(task_id).await;
        let _guard = lock.lock().await;

        let task = work_task_service::get_model(&self.db.conn, task_id)
            .await
            .map_err(|e| e.to_string())?;
        if task.status != mode.expected_status() {
            return Ok(()); // canceled (or otherwise moved on) before we got here
        }
        let run_seq = task.run_seq;
        launched_seq.set(run_seq);
        let root = get_folder_core(&self.db, task.folder_id)
            .await
            .map_err(|e| e.to_string())?;

        // Effective agent + config: task override > folder task settings >
        // folder default agent. Audited via a config_effective event (values
        // are inherited live, never frozen).
        let cfg: WorkTaskConfig =
            serde_json::from_str(&task.config).unwrap_or_default();
        let settings = work_task_service::settings_get_effective(&self.db.conn, task.folder_id)
            .await
            .unwrap_or_default();
        let (agent_str, mode_id, config_values) = effective_agent_config(&cfg, &settings, &root);
        let agent_str = agent_str.ok_or_else(|| {
            "no agent configured: set a task agent or a folder default".to_string()
        })?;
        let agent_type = parse_agent_type(&agent_str)?;

        // Cheap validation before any side effects; the full prompt is composed
        // after the spawn, once we know whether the session actually resumed.
        if matches!(mode, LaunchMode::Fresh) && cfg.prompt_blocks.is_empty() {
            return Err("prompt is empty".to_string());
        }

        // Out of the queue: from here the task holds its slot and does real
        // work (worktree, init command, agent spawn), so the board must stop
        // calling it "queued". Losing the CAS means a concurrent cancel or a
        // newer generation took over. A merge generation stays `merging`.
        if !matches!(mode, LaunchMode::Merge { .. }) {
            // Reserve the kill slot BEFORE the row can read as `preparing`, and
            // hold it for the whole phase. A cancel cannot wait for the task
            // lock (this launch holds it across the entire init command), so it
            // must always find somewhere to record itself — including during
            // `ensure_worktree`, which takes seconds on a big repo.
            // `spawn_launch_owned` releases the slot, including when the CAS
            // below loses.
            self.reserve_setup_slot(task_id, run_seq).await;
            if !work_task_service::begin_setup(&self.db.conn, task_id, run_seq)
                .await
                .map_err(|e| e.to_string())?
            {
                return Ok(());
            }
            self.emit_upsert(task_id);
        }

        // Cancel gate before the expensive part of setup, so a cancel that
        // landed while we were reading config never reaches `git worktree add`.
        if !still_expected(&self.db.conn, task_id, run_seq, mode.in_flight_status()).await {
            return Ok(());
        }

        // Worktree: reuse the recorded one when it still exists (retry/return),
        // else mint a fresh one pinned to the base recorded FIRST (no drift
        // window between reading the branch and creating the worktree). A merge
        // generation never creates one — merging a fresh empty worktree would
        // "land" as a no-op tree match.
        let wt = if matches!(mode, LaunchMode::Merge { .. }) {
            self.existing_worktree(&task).await?
        } else {
            let wt = self.ensure_worktree(&task, &root, &settings).await?;
            // Run the folder's init command (deps install etc.) before the
            // agent ever sees the tree. Gated on the setup marker, NOT on "the
            // tree was just created": an init that was killed (cancel) or cut
            // short (restart) leaves a half-installed tree that must initialize
            // again. A failure is a setup error — the task must not start
            // half-initialized.
            if let Some(command) = settings
                .init_command
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                if !setup_marker_present(&wt.path).await {
                    self.run_init_command(task_id, run_seq, command, &wt.path)
                        .await?;
                    write_setup_marker(&wt.path).await;
                }
            }
            wt
        };

        let _ = work_task_service::record_event(
            &self.db.conn,
            task_id,
            "config_effective",
            "engine",
            Some(serde_json::json!({
                "agent": agent_str,
                "mode": mode_id,
                "model": config_values.get("model"),
            })),
        )
        .await;

        // Announce the worktree folder before any conversation upsert so every
        // client can group the conversation (idempotent re-broadcast).
        if let Ok(detail) = get_folder_core(&self.db, wt.folder_id).await {
            emit_folder_upsert(&self.emitter, detail);
        }

        // Resume the previous session for retry/return/merge when we have one.
        let resume_session_id = match mode {
            LaunchMode::Fresh => None,
            LaunchMode::Retry | LaunchMode::Return { .. } | LaunchMode::Merge { .. } => {
                match task.conversation_id {
                    Some(conv_id) => conversation::Entity::find_by_id(conv_id)
                        .one(&self.db.conn)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|c| c.external_id),
                    None => None,
                }
            }
        };

        let runtime_env =
            build_session_runtime_env(&self.db, agent_type, resume_session_id.as_deref(), &self.data_dir)
                .await
                .map_err(|e| e.to_string())?;
        verify_agent_installed(agent_type)
            .await
            .map_err(|e| e.to_string())?;

        // Cancel gate before spawning the CLI.
        if !still_expected(&self.db.conn, task_id, run_seq, mode.in_flight_status()).await {
            return Ok(());
        }

        let mut resumed = resume_session_id.is_some();
        let conn_id = match self
            .manager
            .spawn_agent(
                agent_type,
                Some(wt.path.clone()),
                resume_session_id.clone(),
                runtime_env.clone(),
                "work_task".to_string(),
                self.emitter.clone(),
                mode_id.clone(),
                config_values.clone(),
            )
            .await
        {
            Ok(id) => id,
            Err(e) if resumed => {
                // Resume failed (e.g. the agent lost the session) → fall back
                // to a fresh session in the same worktree, recorded on the
                // timeline.
                tracing::info!("[work_task] resume failed for task {task_id}: {e}; falling back");
                let _ = work_task_service::record_event(
                    &self.db.conn,
                    task_id,
                    "resume_fallback",
                    "engine",
                    Some(serde_json::json!({ "error": e.to_string() })),
                )
                .await;
                resumed = false;
                self.manager
                    .spawn_agent(
                        agent_type,
                        Some(wt.path.clone()),
                        None,
                        runtime_env,
                        "work_task".to_string(),
                        self.emitter.clone(),
                        mode_id.clone(),
                        config_values.clone(),
                    )
                    .await
                    .map_err(|e| e.to_string())?
            }
            Err(e) => return Err(e.to_string()),
        };

        // Conversation row: reuse when resuming the same session; otherwise a
        // fresh row (fresh runs and resume fallbacks).
        let conversation_id = if resumed {
            task.conversation_id.expect("resumed implies conversation")
        } else {
            let title = conversation_title_for_task(&task.title);
            let id = match create_conversation_core(
                &self.db.conn,
                wt.folder_id,
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
            // The card's name IS this session's identity — freeze it the way a
            // manual rename would, or the per-turn auto-title backfill replaces
            // it with whatever the agent's session file parses to (for agents
            // with no title of their own: the first line of the composed
            // prompt, e.g. "项目：/Users/…"). Issue #495.
            //
            // Strictly before the upsert below: that broadcast is how any
            // client first learns this id, so locking first makes a backfill on
            // this row impossible rather than merely unlikely. A failure here
            // only costs the nice title — never the launch.
            if let Err(e) = conversation_service::lock_title(&self.db.conn, id).await {
                tracing::warn!(
                    "[work_task] task {task_id}: could not lock conversation {id} title: {e}"
                );
            }
            id
        };
        emit_conversation_upsert(&self.emitter, &self.db.conn, conversation_id).await;

        let mut blocks =
            compose_prompt(&cfg, &task, &mode, &settings, resumed, &self.db.conn).await?;
        // Re-encode attached images for the agent that actually answered the
        // handshake. A task's blocks are STORED — the composer picked their
        // encoding from a transient probe that may not have landed, possibly
        // months ago, and the task's agent can change between then and this
        // run. The live session's advertised capabilities are the only truth.
        //
        // Only for a prompt that actually carries an image, and only then do we
        // wait: `spawn_agent` returns before a FRESH session's handshake
        // completes, so the capabilities are typically still unpublished here.
        // Every other launch (the overwhelming majority) pays nothing.
        if blocks.iter().any(carries_image) {
            match self
                .manager
                .wait_for_prompt_capabilities(&conn_id, IMAGE_CAPABILITY_WAIT)
                .await
            {
                Some(caps) => reencode_images(&mut blocks, &caps),
                None => tracing::warn!(
                    "[work_task] task {task_id}: agent never advertised prompt capabilities; \
                     sending attached images as stored"
                ),
            }
        }

        // Register for completion correlation BEFORE prompting so a fast
        // TurnComplete can't race ahead of the index entry.
        self.index
            .lock()
            .await
            .insert(conn_id.clone(), (task_id, run_seq));

        // queued → running (CAS on run_seq) — or, for a merge generation, just
        // record the live coordinates while the status stays merging. Losing
        // means a concurrent cancel/settle — tear down without side effects.
        let marked = if matches!(mode, LaunchMode::Merge { .. }) {
            work_task_service::mark_merging_live(
                &self.db.conn,
                task_id,
                run_seq,
                conversation_id,
                &conn_id,
            )
            .await
            .map_err(|e| e.to_string())?
        } else {
            work_task_service::mark_running(
                &self.db.conn,
                task_id,
                run_seq,
                conversation_id,
                &conn_id,
            )
            .await
            .map_err(|e| e.to_string())?
        };
        if !marked {
            self.index.lock().await.remove(&conn_id);
            let _ = self.manager.disconnect(&conn_id).await;
            if !resumed {
                self.cancel_conversation(conversation_id).await;
            }
            return Ok(());
        }
        self.emit_upsert(task_id);

        let prompt_head = prompt_head(&blocks);
        match self
            .manager
            .send_prompt_linked_with_message_id(
                &self.db,
                &conn_id,
                blocks,
                Some(wt.folder_id),
                Some(conversation_id),
                None,
                None,
            )
            .await
        {
            Ok(_) => {
                // Timeline round marker: lets the transcript viewer label this
                // prompt's turn with its phase (work / retry / return / merge).
                let _ = work_task_service::record_event(
                    &self.db.conn,
                    task_id,
                    "round",
                    "engine",
                    Some(serde_json::json!({
                        "kind": mode.round_kind(),
                        "intent": mode.round_intent().map(FollowUpIntent::as_str),
                        "run_seq": run_seq,
                        "prompt_head": prompt_head,
                    })),
                )
                .await;
                Ok(())
            }
            Err(e) => {
                self.index.lock().await.remove(&conn_id);
                let _ = self.manager.disconnect(&conn_id).await;
                if !resumed {
                    self.cancel_conversation(conversation_id).await;
                }
                Err(e.to_string())
            }
        }
    }

    /// Resolve (and if needed create) the task's worktree. On creation the base
    /// branch + sha are recorded BEFORE `git worktree add` runs against that
    /// exact sha, and the directory goes wherever the folder's settings put it
    /// (next to the project folder by default).
    async fn ensure_worktree(
        &self,
        task: &crate::db::entities::work_task::Model,
        root: &crate::models::FolderDetail,
        settings: &WorkTaskFolderSettings,
    ) -> Result<WorktreeRef, String> {
        if let Some(wt_id) = task.worktree_folder_id {
            if let Ok(detail) = get_folder_core(&self.db, wt_id).await {
                if Path::new(&detail.path).exists() {
                    return Ok(WorktreeRef {
                        folder_id: detail.id,
                        path: detail.path,
                    });
                }
            }
        }

        // The directory is gone, but the branch may not be: a retry / follow-up
        // after the checkout was removed must continue the work already
        // committed on the branch — not restart on a fresh base while those
        // commits sit stranded on a branch nothing points to. Only when the
        // branch cannot be re-checked-out does the fresh mint below take over
        // (re-recording base + branch).
        if let Some(wt) = self.recreate_worktree_from_branch(task, root, settings).await {
            return Ok(wt);
        }

        let head = resolve_git_head(&root.path).await.map_err(|e| e.to_string())?;
        let base_branch = head
            .branch
            .ok_or_else(|| "project folder is not on a branch (detached HEAD?)".to_string())?;
        let base_sha = task_git::rev_parse(&root.path, "HEAD")
            .await
            .map_err(|e| e.to_string())?;

        let branch = format!("task/{}", task.id);
        let dir = format!("{}-task-{}", basename(&root.path), task.id);
        // A configured root that does not exist yet (the folder's first task)
        // needs no `mkdir`: `git worktree add` creates the leading directories
        // along with the checkout.
        let mut wt_path = worktree_path_in(&root.path, settings.worktree_root.as_deref(), &dir);
        let mut branch_used = branch.clone();

        if let Err(e) = git_worktree_add(
            root.path.clone(),
            branch.clone(),
            wt_path.clone(),
            Some(base_sha.clone()),
        )
        .await
        {
            // A leftover from a prior attempt may collide — retry once with a
            // generation-scoped suffix.
            let suffix = format!("r{}b", task.run_seq);
            branch_used = format!("{branch}-{suffix}");
            wt_path = worktree_path_in(
                &root.path,
                settings.worktree_root.as_deref(),
                &format!("{dir}-{suffix}"),
            );
            git_worktree_add(
                root.path.clone(),
                branch_used.clone(),
                wt_path.clone(),
                Some(base_sha.clone()),
            )
            .await
            .map_err(|_| format!("worktree add failed: {e}"))?;
        }

        let wt = open_worktree_folder_core(&self.db, wt_path, task.folder_id)
            .await
            .map_err(|e| e.to_string())?;
        work_task_service::attach_worktree(
            &self.db.conn,
            task.id,
            wt.id,
            &base_branch,
            &base_sha,
            &branch_used,
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(WorktreeRef {
            folder_id: wt.id,
            path: wt.path,
        })
    }

    /// Try to re-create the task's worktree from its still-existing work
    /// branch (`git worktree add <path> <branch>`, prior commits intact), at
    /// the recorded folder's path — or the standard naming when that row is
    /// gone. `None` when the branch is gone too, the path is occupied, or git
    /// refuses (e.g. the branch is checked out elsewhere) — the caller then
    /// mints a fresh worktree.
    async fn recreate_worktree_from_branch(
        &self,
        task: &crate::db::entities::work_task::Model,
        root: &crate::models::FolderDetail,
        settings: &WorkTaskFolderSettings,
    ) -> Option<WorktreeRef> {
        let branch = task.work_branch.as_deref()?;
        // The recorded base stays authoritative for the branch's history; the
        // three are only ever written together, but a row missing them must
        // fall through to the fresh mint that records them.
        let base_branch = task.base_branch.as_deref()?;
        let base_sha = task.base_sha.as_deref()?;
        // The LOCAL branch specifically — an unqualified lookup would let a
        // same-name tag answer here and the add below would check out a
        // detached HEAD instead of the branch.
        match task_git::local_branch_tip(&root.path, branch).await {
            Ok(Some(_)) => {}
            _ => return None, // branch gone too — nothing to continue from
        }
        // Prefer the recorded folder row's path so the row binds back to the
        // same workspace entry; fall back to the standard naming.
        let recorded = match task.worktree_folder_id {
            Some(wt_id) => get_folder_core(&self.db, wt_id).await.ok().map(|d| d.path),
            None => None,
        };
        let path = recorded.unwrap_or_else(|| {
            worktree_path_in(
                &root.path,
                settings.worktree_root.as_deref(),
                &format!("{}-task-{}", basename(&root.path), task.id),
            )
        });
        if Path::new(&path).exists() {
            return None; // something else lives there now
        }
        if let Err(e) = task_git::worktree_add_existing_branch(&root.path, &path, branch).await {
            tracing::info!(
                "[work_task] task {}: could not re-create the worktree from branch \
                 {branch}: {e}",
                task.id
            );
            return None;
        }
        let wt = match open_worktree_folder_core(&self.db, path.clone(), task.folder_id).await {
            Ok(wt) => wt,
            Err(e) => {
                // The checkout exists but has no folder row; the fresh-mint
                // fallback will collide on the path and suffix itself, so the
                // launch still proceeds.
                tracing::warn!(
                    "[work_task] task {}: recreated worktree at {path} but could not open \
                     its folder: {e}",
                    task.id
                );
                return None;
            }
        };
        // Re-attach under the ORIGINAL base: the branch's history is diffed
        // against it, and re-recording today's HEAD would misstate the change
        // set the review shows.
        if let Err(e) = work_task_service::attach_worktree(
            &self.db.conn,
            task.id,
            wt.id,
            base_branch,
            base_sha,
            branch,
        )
        .await
        {
            tracing::warn!(
                "[work_task] task {}: could not re-attach the recreated worktree: {e}",
                task.id
            );
        }
        Some(WorktreeRef {
            folder_id: wt.id,
            path: wt.path,
        })
    }

    /// The task's recorded worktree, required to exist on disk — the merge
    /// generation must never mint a fresh (empty) one.
    async fn existing_worktree(&self, task: &crate::db::entities::work_task::Model) -> Result<WorktreeRef, String> {
        let wt_id = task
            .worktree_folder_id
            .ok_or_else(|| "task has no worktree".to_string())?;
        let detail = get_folder_core(&self.db, wt_id)
            .await
            .map_err(|e| e.to_string())?;
        if !Path::new(&detail.path).exists() {
            return Err("the task worktree no longer exists on disk".to_string());
        }
        Ok(WorktreeRef {
            folder_id: detail.id,
            path: detail.path,
        })
    }

    /// Run the folder's worktree init command in an uninitialized worktree.
    /// The outcome is always recorded on the timeline; a failure aborts the
    /// launch (setup error) so the agent never starts half-initialized.
    async fn run_init_command(
        &self,
        task_id: i32,
        run_seq: i32,
        command: &str,
        cwd: &str,
    ) -> Result<(), String> {
        let run = self.run_setup_shell(task_id, run_seq, command, cwd).await;
        let (exit_code, tail) = match &run {
            Ok((code, tail)) => (*code, tail.clone()),
            Err(e) => (None, e.clone()),
        };
        let ok = matches!(run, Ok((Some(0), _)));
        let _ = work_task_service::record_event(
            &self.db.conn,
            task_id,
            "init_command",
            "engine",
            Some(serde_json::json!({
                "command": command,
                "exit_code": exit_code,
                "output_tail": (!ok && !tail.is_empty()).then_some(tail.clone()),
            })),
        )
        .await;
        if ok {
            Ok(())
        } else {
            let code = exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
            Err(format!("worktree init command failed (exit {code}): {tail}"))
        }
    }

    /// Open the kill slot for a generation entering setup. Held for the whole
    /// preparing phase so a cancel always has somewhere to record itself, even
    /// before (or after) a child process exists.
    async fn reserve_setup_slot(&self, task_id: i32, run_seq: i32) {
        self.setup_children.lock().await.insert(
            task_id,
            SetupChild {
                kill_requested: false,
                run_seq,
                wake: Arc::new(Notify::new()),
            },
        );
    }

    /// Close the slot once the launch is over — but only if it is still this
    /// generation's, so a slow unwinding launch cannot drop the slot a newer
    /// one is relying on.
    async fn release_setup_slot(&self, task_id: i32, run_seq: Option<i32>) {
        let Some(run_seq) = run_seq else { return };
        let mut children = self.setup_children.lock().await;
        if children.get(&task_id).is_some_and(|s| s.run_seq == run_seq) {
            children.remove(&task_id);
        }
    }

    /// `run_shell_capture` for the setup phase: the child is bound to the
    /// task's kill slot so a cancel kills its process TREE instead of letting a
    /// multi-minute install run to completion after the user gave up.
    ///
    /// Every cancel window is covered, and the kill always happens HERE, in the
    /// task that owns the child:
    /// - before the spawn → the slot already says `kill_requested`, so no child
    ///   is ever started;
    /// - during the spawn → `notify_one` leaves a permit, so the wait loop's
    ///   first `notified()` returns immediately and kills;
    /// - while waiting → the wake fires and we kill, then keep awaiting the
    ///   child so it is still reaped normally;
    /// - after the child was reaped → nobody holds its pid any more, so a
    ///   recycled pid can never be signalled.
    async fn run_setup_shell(
        &self,
        task_id: i32,
        run_seq: i32,
        line: &str,
        cwd: &str,
    ) -> Result<(Option<i32>, String), String> {
        let wake = {
            // The slot is opened by `launch` before setup begins; re-assert it
            // if it somehow went missing, so the child is never unkillable.
            let mut children = self.setup_children.lock().await;
            let slot = children.entry(task_id).or_insert_with(|| SetupChild {
                kill_requested: false,
                run_seq,
                wake: Arc::new(Notify::new()),
            });
            if slot.kill_requested {
                return Err("canceled before the init command started".to_string());
            }
            slot.run_seq = run_seq;
            slot.wake.clone()
        };
        let child = shell_command(line, cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let pid = child.id();

        let wait = child.wait_with_output();
        tokio::pin!(wait);
        let mut killed = false;
        let out = loop {
            tokio::select! {
                out = &mut wait => break out,
                // Kill once, then go back to awaiting the child: the process
                // is only reaped by the `wait` branch, so `pid` is still ours.
                _ = wake.notified(), if !killed => {
                    killed = true;
                    kill_process_tree(pid).await;
                }
            }
        };
        let out = out.map_err(|e| e.to_string())?;
        Ok(combine_capture(&out))
    }

    /// Stop the setup of `task_id`, but only when the slot belongs to
    /// generation `run_seq` — a stale cancel must not stop a task that was
    /// already requeued and relaunched. Records the request and wakes the
    /// runner; the actual kill happens in `run_setup_shell`, which is the only
    /// place that knows the child is still alive.
    async fn kill_setup_child(&self, task_id: i32, run_seq: i32) {
        let wake = {
            let mut children = self.setup_children.lock().await;
            match children.get_mut(&task_id) {
                Some(slot) if slot.run_seq == run_seq => {
                    slot.kill_requested = true;
                    slot.wake.clone()
                }
                _ => return,
            }
        };
        wake.notify_one();
    }

    /// Start preflight: the target folder must exist, be live, and be a
    /// project root (not a worktree).
    async fn preflight_folder(&self, folder_id: i32) -> Result<(), String> {
        let row = folder::Entity::find_by_id(folder_id)
            .one(&self.db.conn)
            .await
            .map_err(|e| e.to_string())?
            .filter(|f| f.deleted_at.is_none())
            .ok_or_else(|| "folder not found".to_string())?;
        if row.parent_id.is_some() {
            return Err("tasks must run from a project folder, not a worktree".to_string());
        }
        Ok(())
    }

    // ── event settlement ────────────────────────────────────────────────────

    async fn on_event(self: &Arc<Self>, env: &EventEnvelope) {
        match &env.payload {
            AcpEvent::TurnComplete { stop_reason, .. } => {
                self.on_turn_complete(&env.connection_id, stop_reason).await;
            }
            AcpEvent::QuestionRequest { question_id, .. } => {
                self.track_request(&env.connection_id, format!("q:{question_id}"), true)
                    .await;
            }
            AcpEvent::QuestionResolved { question_id } => {
                self.track_request(&env.connection_id, format!("q:{question_id}"), false)
                    .await;
            }
            AcpEvent::PermissionRequest { request_id, .. } => {
                self.track_request(&env.connection_id, format!("p:{request_id}"), true)
                    .await;
            }
            AcpEvent::PermissionResolved { request_id } => {
                self.track_request(&env.connection_id, format!("p:{request_id}"), false)
                    .await;
            }
            AcpEvent::PlanApprovalRequest { approval_id, .. } => {
                self.track_request(&env.connection_id, format!("a:{approval_id}"), true)
                    .await;
            }
            AcpEvent::PlanApprovalResolved { approval_id } => {
                self.track_request(&env.connection_id, format!("a:{approval_id}"), false)
                    .await;
            }
            // Both delegation events ride the PARENT's stream, so
            // `env.connection_id` is the (possibly task-owning) parent and the
            // child id is in the payload.
            AcpEvent::DelegationStarted {
                child_connection_id,
                ..
            } => {
                // Scoped to task runs: a delegation from an ordinary chat tab
                // has no board row to flip, and mapping it would grow this map
                // for the life of the process. Resolved through
                // `task_for_connection` rather than `index` alone so a nested
                // delegation (a sub-agent delegating further) maps to the same
                // run — its prompts block the task just as much.
                if self.task_for_connection(&env.connection_id).await.is_some() {
                    self.delegation_parents
                        .lock()
                        .await
                        .insert(child_connection_id.clone(), env.connection_id.clone());
                    // The broker starts the child's turn BEFORE announcing it
                    // (`send_prompt_linked_for_delegation` precedes
                    // `emit_started_if_real`), so a child that blocks
                    // immediately raised its prompt while we had no mapping and
                    // we dropped it. Recover from live state now that we do —
                    // otherwise the board sits at `running` for a run that is
                    // already parked on the user.
                    self.backfill_child_blocking_prompts(child_connection_id)
                        .await;
                }
            }
            AcpEvent::DelegationCompleted {
                child_connection_id,
                ..
            } => {
                self.forget_delegation_child(child_connection_id).await;
            }
            _ => {}
        }
    }

    /// Resolve the task generation an event belongs to, and whether `conn_id` is
    /// the run's OWN connection. A connection that isn't itself a task run may
    /// still be a delegation child of one, in which case the run is its
    /// parent's and the caller must namespace anything it records by
    /// `conn_id` — see [`Self::track_request`].
    ///
    /// Walks the delegation chain rather than one hop: with `depth_limit > 1` a
    /// sub-agent can delegate further, and a grandchild parked on a permission
    /// blocks the run exactly as much as a direct child does. Bounded by
    /// [`MAX_DELEGATION_CHAIN_HOPS`] so a malformed map can never spin.
    ///
    /// Never holds two of the engine's maps at once (each guard is a statement
    /// temporary), matching the discipline the rest of these helpers keep.
    async fn task_for_connection(&self, conn_id: &str) -> Option<((i32, i32), bool)> {
        if let Some(entry) = self.index.lock().await.get(conn_id).copied() {
            return Some((entry, true));
        }
        let mut cursor = conn_id.to_string();
        for _ in 0..MAX_DELEGATION_CHAIN_HOPS {
            let parent = self.delegation_parents.lock().await.get(&cursor).cloned()?;
            if let Some(entry) = self.index.lock().await.get(&parent).copied() {
                return Some((entry, false));
            }
            cursor = parent;
        }
        None
    }

    /// Detach `root`'s delegation-child mappings, transitively, and return every
    /// connection id removed (`root` itself only when `include_root`).
    ///
    /// Transitive because a sub-agent may have delegated further: dropping only
    /// the direct children would strand the deeper links, and — since
    /// [`Self::task_for_connection`] deliberately doesn't prune as it walks —
    /// nothing else would ever collect them.
    ///
    /// Terminates unconditionally: each iteration removes the entries it
    /// enqueues, so the map strictly shrinks.
    async fn detach_delegation_subtree(&self, root: &str, include_root: bool) -> Vec<String> {
        let mut parents = self.delegation_parents.lock().await;
        let mut detached = Vec::new();
        let mut frontier = vec![root.to_string()];
        while let Some(node) = frontier.pop() {
            let children: Vec<String> = parents
                .iter()
                .filter(|(_, parent)| parent.as_str() == node)
                .map(|(child, _)| child.clone())
                .collect();
            for child in children {
                parents.remove(&child);
                frontier.push(child.clone());
                detached.push(child);
            }
        }
        if include_root && parents.remove(root).is_some() {
            detached.push(root.to_string());
        }
        detached
    }

    /// Track any blocking prompt the child is ALREADY parked on, reading its
    /// live `SessionState` (authoritative, and updated independently of the
    /// event bus). Idempotent: re-tracking a key already in the set is a
    /// no-op insert, so a prompt we did see plus this backfill can't
    /// double-count.
    ///
    /// Reads all three prompt kinds rather than the single top-precedence one,
    /// so resolving whichever comes first can't empty the set while another is
    /// still outstanding.
    async fn backfill_child_blocking_prompts(&self, child_conn_id: &str) {
        let Some(state) = self.manager.get_state(child_conn_id).await else {
            return;
        };
        // Collect under the read lock, track after releasing it — `track_request`
        // takes the engine's own maps and must not nest inside a session lock.
        let keys: Vec<String> = {
            let s = state.read().await;
            let mut keys = Vec::new();
            if let Some(p) = s.pending_permission.as_ref() {
                keys.push(format!("p:{}", p.request_id));
            }
            if let Some(q) = s.pending_question.as_ref() {
                keys.push(format!("q:{}", q.question_id));
            }
            if let Some(a) = s.pending_plan_approval.as_ref() {
                keys.push(format!("a:{}", a.approval_id));
            }
            keys
        };
        for key in keys {
            self.track_request(child_conn_id, key, true).await;
        }
    }

    /// Drop every delegation-child mapping belonging to a retired run. Called
    /// wherever a run leaves `index`; the run's outstanding-request set is
    /// cleared wholesale by those same call sites.
    async fn forget_delegation_children_of(&self, parent_conn_id: &str) {
        // `parent_conn_id` is a run connection, never a key in this map.
        self.detach_delegation_subtree(parent_conn_id, false).await;
    }

    /// Drop a finished delegation child (and anything it delegated in turn) plus
    /// any blocking requests they left unanswered.
    ///
    /// The cleanup is the load-bearing half: a child torn down while a
    /// permission was still pending (cancel, crash, parent teardown) would
    /// otherwise leave its key in the task's outstanding set forever, pinning
    /// the row at `awaiting_input` with nothing left that could ever resolve it.
    /// Empties the set through the same flip path as `track_request` so the row
    /// returns to `running`.
    async fn forget_delegation_child(&self, child_conn_id: &str) {
        // Resolve the run BEFORE detaching — afterwards the chain is gone.
        let entry = self.task_for_connection(child_conn_id).await;
        let detached = self.detach_delegation_subtree(child_conn_id, true).await;
        let Some(((task_id, run_seq), _)) = entry else {
            return;
        };
        let prefixes: Vec<String> = detached.iter().map(|c| format!("{c}#")).collect();
        let emptied = {
            let mut awaiting = self.awaiting.lock().await;
            let Some(set) = awaiting.get_mut(&task_id) else {
                return;
            };
            let before = set.len();
            set.retain(|k| !prefixes.iter().any(|p| k.starts_with(p.as_str())));
            if set.len() == before {
                return; // nothing of this subtree's was outstanding
            }
            if set.is_empty() {
                awaiting.remove(&task_id);
                true
            } else {
                false
            }
        };
        if emptied {
            let flipped = work_task_service::flip_awaiting(&self.db.conn, task_id, run_seq, false)
                .await
                .unwrap_or(false);
            if flipped {
                self.emit_upsert(task_id);
            }
        }
    }

    async fn on_turn_complete(self: &Arc<Self>, conn_id: &str, stop_reason: &str) {
        let entry = { self.index.lock().await.get(conn_id).copied() };
        let Some((task_id, run_seq)) = entry else {
            return; // not a task run
        };

        let summary = self.capture_summary(conn_id).await;
        self.index.lock().await.remove(conn_id);
        self.awaiting.lock().await.remove(&task_id);
        self.forget_delegation_children_of(conn_id).await;
        let _ = self.manager.disconnect(conn_id).await;

        let task = work_task_service::get_model(&self.db.conn, task_id).await.ok();

        // A merge generation settles from git truth whatever the stop reason —
        // the agent may have landed the merge and then errored or been stopped.
        if let Some(t) = task
            .as_ref()
            .filter(|t| t.status == WorkTaskStatus::Merging && t.run_seq == run_seq)
        {
            self.settle_merge_generation(t, stop_reason, summary.as_deref())
                .await;
            self.pump_folder(t.folder_id).await;
            // The folder's one merge slot just freed — the next merge the user
            // queued (or the auto-merge train) can land now.
            self.spawn_merge_pump(t.folder_id);
            return;
        }

        let changed = match stop_reason {
            "end_turn" => {
                // A `task_complete` report from this generation decides the
                // settle: blocked → failed(verdict_blocked); success /
                // needs_review → review. The verdict column is cleared on every
                // claim, so a present verdict is always this generation's — and
                // its summary (written with it) outranks the captured
                // last-assistant text.
                let verdict = task
                    .as_ref()
                    .filter(|t| t.run_seq == run_seq)
                    .and_then(|t| t.verdict.clone());
                if verdict.as_deref() == Some("blocked") {
                    let error = task
                        .as_ref()
                        .and_then(|t| t.result_summary.clone())
                        .unwrap_or_else(|| "agent reported the task as blocked".to_string());
                    work_task_service::fail(
                        &self.db.conn,
                        task_id,
                        &[WorkTaskStatus::Running, WorkTaskStatus::AwaitingInput],
                        Some(run_seq),
                        "verdict_blocked",
                        Some(error),
                    )
                    .await
                    .unwrap_or(false)
                } else {
                    let own_summary = if verdict.is_some() {
                        task.as_ref().and_then(|t| t.result_summary.clone())
                    } else {
                        None
                    };
                    let stats = self.snapshot_diff_stats(task_id).await;
                    let settled = work_task_service::settle_review(
                        &self.db.conn,
                        task_id,
                        run_seq,
                        own_summary.or(summary),
                        stats,
                    )
                    .await
                    .unwrap_or(false);
                    if settled {
                        self.spawn_post_review(task_id, run_seq);
                    }
                    settled
                }
            }
            "cancelled" => {
                // The user stopped the agent from the conversation UI — that is
                // a task cancel, not an agent failure.
                work_task_service::cancel(&self.db.conn, task_id, None)
                    .await
                    .unwrap_or(false)
            }
            other => work_task_service::fail(
                &self.db.conn,
                task_id,
                &[WorkTaskStatus::Running, WorkTaskStatus::AwaitingInput],
                Some(run_seq),
                "agent_error",
                Some(format!("agent stopped: {other}")),
            )
            .await
            .unwrap_or(false),
        };
        if changed {
            self.emit_upsert(task_id);
        }
        // A slot freed up — keep the queue draining.
        if let Ok(task) = work_task_service::get_model(&self.db.conn, task_id).await {
            self.pump_folder(task.folder_id).await;
        }
    }

    /// Track an outstanding blocking request and flip running ⇄ awaiting_input
    /// on the empty↔non-empty edges of the per-task set.
    ///
    /// `conn_id` may be the task's own connection or one of its delegation
    /// children: a sub-agent parked on a permission blocks the run just as
    /// surely as the top-level agent does, and is in fact harder to notice
    /// (#447). A child's keys are prefixed with its connection id so
    /// [`Self::forget_delegation_child`] can retract them as a group.
    async fn track_request(&self, conn_id: &str, key: String, outstanding: bool) {
        let Some(((task_id, run_seq), is_own_connection)) =
            self.task_for_connection(conn_id).await
        else {
            return;
        };
        let key = if is_own_connection {
            key
        } else {
            format!("{conn_id}#{key}")
        };
        let flip = {
            let mut awaiting = self.awaiting.lock().await;
            let set = awaiting.entry(task_id).or_default();
            if outstanding {
                set.insert(key);
                set.len() == 1
            } else {
                set.remove(&key);
                if set.is_empty() {
                    awaiting.remove(&task_id);
                    true
                } else {
                    false
                }
            }
        };
        if flip {
            let flipped =
                work_task_service::flip_awaiting(&self.db.conn, task_id, run_seq, outstanding)
                    .await
                    .unwrap_or(false);
            if flipped {
                self.emit_upsert(task_id);
            }
        }
    }

    // ── codeg-mcp task reporting tools ──────────────────────────────────────

    /// `task_progress`: attribute the report through the connection index and
    /// append an `agent_progress` event (the card/timeline milestone).
    pub async fn record_progress(&self, conn_id: &str, message: &str) -> TaskReportAck {
        let entry = { self.index.lock().await.get(conn_id).copied() };
        let Some((task_id, run_seq)) = entry else {
            return TaskReportAck::rejected("this session is not executing a work task");
        };
        // Generation guard: a stale connection's report is a no-op.
        match work_task_service::get_model(&self.db.conn, task_id).await {
            Ok(task) if task.run_seq == run_seq => {}
            _ => return TaskReportAck::rejected("the task moved on to a new run"),
        }
        if let Err(e) = work_task_service::record_event(
            &self.db.conn,
            task_id,
            "agent_progress",
            "agent",
            Some(serde_json::json!({ "message": message })),
        )
        .await
        {
            return TaskReportAck::rejected(&format!("could not record progress: {e}"));
        }
        self.emit_upsert(task_id);
        TaskReportAck::recorded()
    }

    /// `task_complete`: stash the verdict + summary on the current generation;
    /// the TurnComplete settle reads them to decide review vs failed.
    pub async fn record_complete(
        &self,
        conn_id: &str,
        verdict: &str,
        summary: Option<&str>,
    ) -> TaskReportAck {
        let entry = { self.index.lock().await.get(conn_id).copied() };
        let Some((task_id, run_seq)) = entry else {
            return TaskReportAck::rejected("this session is not executing a work task");
        };
        match work_task_service::set_verdict(&self.db.conn, task_id, run_seq, verdict, summary)
            .await
        {
            Ok(true) => {
                self.emit_upsert(task_id);
                TaskReportAck::recorded()
            }
            Ok(false) => TaskReportAck::rejected("the task is not running anymore"),
            Err(e) => TaskReportAck::rejected(&format!("could not record verdict: {e}")),
        }
    }

    // ── preflight (acceptance red/green light) ──────────────────────────────

    /// After a settle into review: run the folder's preflight command (if one
    /// is configured), then give auto-merge its chance. One spawned task, in
    /// that order — the sweep requires a green light whenever a preflight is
    /// configured, so the light must be on the row before the sweep reads it.
    /// Fire-and-forget: the light is written CAS (review + run_seq), so a task
    /// that moved on ignores a slow finish, and the sweep re-checks status.
    fn spawn_post_review(self: &Arc<Self>, task_id: i32, run_seq: i32) {
        let engine = self.clone();
        tokio::spawn(async move {
            engine.run_preflight(task_id, run_seq).await;
            if let Ok(task) = work_task_service::get_model(&engine.db.conn, task_id).await {
                engine.merge_pump(task.folder_id).await;
            }
        });
    }

    async fn run_preflight(&self, task_id: i32, run_seq: i32) {
        let Ok(task) = work_task_service::get_model(&self.db.conn, task_id).await else {
            return;
        };
        let settings = work_task_service::settings_get_effective(&self.db.conn, task.folder_id)
            .await
            .unwrap_or_default();
        // A free-form command wins over a folder-command reference; the
        // reference must still exist and belong to this project folder.
        let custom = settings
            .preflight_command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty());
        let (display_name, command_line) = match custom {
            Some(cmd) => (cmd.to_string(), cmd.to_string()),
            None => {
                let Some(command_id) = settings.preflight_command_id else {
                    return;
                };
                let Ok(Some(command)) = folder_command::Entity::find_by_id(command_id)
                    .one(&self.db.conn)
                    .await
                else {
                    return;
                };
                if command.folder_id != task.folder_id {
                    return;
                }
                (command.name, command.command)
            }
        };
        let Some(wt_id) = task.worktree_folder_id else {
            return;
        };
        let Ok(wt) = get_folder_core(&self.db, wt_id).await else {
            return;
        };
        if !Path::new(&wt.path).exists() {
            return;
        }

        let mut light = WorkTaskPreflight {
            status: "running".to_string(),
            command: display_name,
            exit_code: None,
            output_tail: None,
        };
        match work_task_service::set_preflight(&self.db.conn, task_id, run_seq, &light).await {
            Ok(true) => self.emit_upsert(task_id),
            _ => return, // task already moved on
        }

        match run_shell_capture(&command_line, &wt.path).await {
            Ok((code, tail)) => {
                let passed = code == Some(0);
                light.status = if passed { "passed" } else { "failed" }.to_string();
                light.exit_code = code;
                // The tail only matters when the light is red.
                light.output_tail = (!passed && !tail.is_empty()).then_some(tail);
            }
            Err(e) => {
                light.status = "failed".to_string();
                light.output_tail = Some(format!("could not run the command: {e}"));
            }
        }
        if work_task_service::set_preflight(&self.db.conn, task_id, run_seq, &light)
            .await
            .unwrap_or(false)
        {
            self.emit_upsert(task_id);
        }
    }

    /// Best-effort diff-stat snapshot of the task worktree vs its base.
    async fn snapshot_diff_stats(&self, task_id: i32) -> Option<(i32, i32, i32)> {
        let task = work_task_service::get_model(&self.db.conn, task_id).await.ok()?;
        let wt_id = task.worktree_folder_id?;
        let base = task.base_sha.clone()?;
        let wt = get_folder_core(&self.db, wt_id).await.ok()?;
        let files = task_git::diff_numstat(&wt.path, &base).await.ok()?;
        let adds: i32 = files.iter().map(|f| f.additions).sum();
        let dels: i32 = files.iter().map(|f| f.deletions).sum();
        Some((files.len() as i32, adds, dels))
    }

    /// Best-effort: the turn's final assistant text (cleared at next turn
    /// start) becomes the task's result summary.
    async fn capture_summary(&self, conn_id: &str) -> Option<String> {
        let (state, _) = self.manager.get_state_and_emitter(conn_id).await?;
        let text = state.read().await.last_assistant_text.clone();
        text.filter(|t| !t.trim().is_empty())
    }

    // ── accept without merging ─────────────────────────────────────────────

    /// Accept a reviewed task without dispatching a merge generation: review →
    /// done, optionally taking the worktree with it.
    ///
    /// Two ways in, and the board offers exactly one of them per task:
    /// - the recorded diff stat is empty — nothing to land. The stat is a
    ///   snapshot from when the run settled, so git (not the row) re-decides
    ///   under the lock;
    /// - the worktree is GONE (folder row removed, or its directory deleted
    ///   from disk) — a merge generation cannot run at all, so completing is
    ///   the only acceptance left. The leftovers are converged like any other
    ///   removal, except the work branch survives whenever it still holds
    ///   commits the base never received.
    ///
    /// The whole decision runs inside the folder's git lock, with the CAS in
    /// the middle: check, settle and remove form one critical section, leaving
    /// no window in which the task can gain a commit between the check that
    /// cleared it and the `branch -D` that would take it away.
    ///
    /// Two probes on the live-worktree path, because neither alone sees
    /// everything the removal destroys: a commit made on the work branch is
    /// invisible to `git status` but shows up in the diff against the base, and
    /// an untracked file is the reverse. Anything either one finds keeps the
    /// worktree on disk.
    pub async fn complete_task(&self, task_id: i32, delete_worktree: bool) -> Result<(), String> {
        let task = work_task_service::get_model(&self.db.conn, task_id)
            .await
            .map_err(|e| e.to_string())?;
        if task.status != WorkTaskStatus::Review {
            return Err("task is not in review".to_string());
        }

        // Held across the check → CAS → removal. Uncontended in the common
        // case: the merge path holds this only for its dispatch, not for the
        // generation that follows.
        let lock = self.folder_lock(task.folder_id).await;
        let _guard = lock.lock().await;

        let live_wt = self.live_worktree(&task).await;
        let reason = match &live_wt {
            Some(wt) => {
                if self.has_landable_changes(&task, &wt.path).await? {
                    return Err(
                        "this task changed files after all — merge it instead of completing it"
                            .to_string(),
                    );
                }
                "completed without merging: no changes"
            }
            None => "completed without merging: the worktree is gone",
        };
        if !work_task_service::complete_without_merge(&self.db.conn, task_id, reason)
            .await
            .map_err(|e| e.to_string())?
        {
            return Err("task left review before it could be completed".to_string());
        }
        self.emit_upsert(task_id);

        if live_wt.is_none() {
            // Nothing usable is left behind the worktree pointer — converge
            // the bookkeeping regardless of the checkbox (there is no worktree
            // left to keep), sparing only a branch that still holds work.
            self.converge_missing_worktree(&task).await;
            self.emit_upsert(task_id);
        } else if delete_worktree {
            if self.worktree_holds_uncommitted(&task).await {
                let _ = work_task_service::set_cleanup_state(
                    &self.db.conn,
                    task_id,
                    true,
                    Some(
                        "the worktree was kept: it still holds uncommitted files. Remove them, \
                         then retry the cleanup."
                            .to_string(),
                    ),
                )
                .await;
            } else {
                self.remove_worktree_locked(task_id).await;
            }
            self.emit_upsert(task_id);
        }
        Ok(())
    }

    /// The task's recorded worktree while it can still serve a merge: folder
    /// row live and directory on disk. `None` covers "never recorded", "row
    /// removed" and "directory gone" alike.
    async fn live_worktree(
        &self,
        task: &crate::db::entities::work_task::Model,
    ) -> Option<crate::models::FolderDetail> {
        let wt_id = task.worktree_folder_id?;
        let detail = get_folder_core(&self.db, wt_id).await.ok()?;
        Path::new(&detail.path).exists().then_some(detail)
    }

    /// Converge the leftovers of a worktree that is no longer usable (folder
    /// row removed, or directory gone from disk): prune the stale git
    /// registration, soft-delete the folder row, re-parent its conversations —
    /// the same sequence every other removal runs — EXCEPT that the work
    /// branch survives whenever it still holds commits the base never
    /// received. This path is reached without the user ever confirming a
    /// deletion of work, and `branch -D` is its only irreversible step; a kept
    /// branch stays visible in the branch selector for a manual merge or
    /// delete. Caller holds the folder git lock.
    async fn converge_missing_worktree(&self, task: &crate::db::entities::work_task::Model) {
        let task_id = task.id;
        let Some(wt_id) = task.worktree_folder_id else {
            return; // already detached — nothing to converge
        };
        let root = get_folder_core(&self.db, task.folder_id).await.ok();
        let wt = get_folder_core(&self.db, wt_id).await.ok();
        let (Some(root), Some(wt)) = (root, wt) else {
            // The folder rows are already gone — detach, and tell clients
            // still holding a stale copy to drop it.
            let _ = work_task_service::clear_worktree(&self.db.conn, task_id).await;
            emit_folder_deleted(&self.emitter, wt_id);
            return;
        };
        let mut branch_to_delete = task.work_branch.as_deref();
        if let Some(branch) = branch_to_delete {
            if task_git::branch_holds_unlanded_work(
                &root.path,
                branch,
                task.base_branch.as_deref(),
                task.base_sha.as_deref(),
            )
            .await
            {
                tracing::info!(
                    "[work_task] task {task_id}: keeping branch {branch} — it still holds \
                     unlanded commits"
                );
                let _ = work_task_service::record_event(
                    &self.db.conn,
                    task_id,
                    "user_action",
                    "engine",
                    Some(serde_json::json!({ "action": "branch_kept", "branch": branch })),
                )
                .await;
                branch_to_delete = None;
            }
        }
        if let Err(e) =
            task_git::remove_worktree_and_branch(&root.path, &wt.path, branch_to_delete).await
        {
            let _ = work_task_service::set_cleanup_state(
                &self.db.conn,
                task_id,
                true,
                Some(e.to_string()),
            )
            .await;
            return;
        }
        converge_worktree_removal(&self.db, &self.emitter, task_id, wt_id, task.folder_id, &wt.path)
            .await;
    }

    /// Whether the task worktree still has anything uncommitted (tracked edits
    /// or untracked files). A git error reads as "clean": the removal path is
    /// itself tolerant of a worktree that is already off disk, which is the
    /// likeliest reason git could not answer here.
    async fn worktree_holds_uncommitted(
        &self,
        task: &crate::db::entities::work_task::Model,
    ) -> bool {
        let Some(wt_id) = task.worktree_folder_id else {
            return false;
        };
        let Ok(wt) = get_folder_core(&self.db, wt_id).await else {
            return false;
        };
        task_git::has_changes(&wt.path).await.unwrap_or(false)
    }

    /// Live git truth for "would a merge still have something to take from this
    /// task?" — including work committed on the branch after the run settled,
    /// which `git status` reports as a clean worktree. Mirrors the changed-files
    /// view the user reviewed (same base fallback), so the board's offer and
    /// this check cannot disagree. `wt_path` is the already-verified live
    /// worktree (see [`Self::live_worktree`]).
    async fn has_landable_changes(
        &self,
        task: &crate::db::entities::work_task::Model,
        wt_path: &str,
    ) -> Result<bool, String> {
        let Some(base) = task.base_sha.clone().or_else(|| task.base_branch.clone()) else {
            return Ok(false);
        };
        let files = task_git::diff_numstat(wt_path, &base)
            .await
            .map_err(|e| format!("could not read the task's changes: {e}"))?;
        Ok(!files.is_empty())
    }

    // ── merge (two-stage, persisted intent, per-folder git mutex) ───────────

    /// Land a reviewed task on its base branch. Runs the full stage 0/A/B
    /// pipeline; on any failure the task returns to review with a readable
    /// error. Optionally removes the worktree after landing.
    /// Accept a reviewed task: dispatch a MERGE GENERATION — the agent lands
    /// the task onto the base branch itself in its session, resolving any
    /// conflicts in the same turn. Validation runs before the review→merging
    /// CAS, so a refused merge leaves the task untouched; after dispatch the
    /// settle comes from git truth (`settle_merge_generation` / recovery),
    /// never from the agent's word. `auto` marks a dispatch the engine's
    /// auto-merge sweep issued rather than a click — same pipeline (including
    /// the folder's per-stage prompts), different actor on the timeline.
    ///
    /// A click that arrives while the folder's one merge slot is busy is
    /// QUEUED rather than refused: the intent is parked on the row and the
    /// merge pump dispatches it when the slot frees, so accepting a whole
    /// review column is one pass of clicks instead of a wait per landing. An
    /// unattended dispatch never queues — the sweep runs its own train.
    ///
    /// `claim` is set only when the pump is spending a merge the user queued
    /// earlier; it binds every write here to that exact parked intent (see
    /// [`QueuedMergeClaim`]). A click passes `None` and always wins.
    pub async fn merge_task(
        self: &Arc<Self>,
        task_id: i32,
        message: Option<String>,
        delete_worktree: bool,
        auto: bool,
    ) -> Result<MergeDispatch, String> {
        self.merge_task_inner(task_id, message, delete_worktree, auto, None)
            .await
    }

    async fn merge_task_inner(
        self: &Arc<Self>,
        task_id: i32,
        message: Option<String>,
        delete_worktree: bool,
        auto: bool,
        claim: Option<&QueuedMergeClaim>,
    ) -> Result<MergeDispatch, String> {
        let task = work_task_service::get_model(&self.db.conn, task_id)
            .await
            .map_err(|e| e.to_string())?;
        if task.status != WorkTaskStatus::Review {
            return Err("task is not in review".to_string());
        }
        let message = message
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty());
        let settings = work_task_service::settings_get_effective(&self.db.conn, task.folder_id)
            .await
            .unwrap_or_default();
        let strategy = if settings.merge_strategy == "merge" {
            "merge"
        } else {
            "squash"
        }
        .to_string();
        let (root, _wt, base_branch, work_branch) = self.merge_coordinates(&task).await?;

        // Dispatch under the per-folder git lock: one merge per project at a
        // time, with the base state validated right before the CAS.
        let lock = self.folder_lock(task.folder_id).await;
        let _guard = lock.lock().await;

        let another_merging =
            work_task_service::list_by_status(&self.db.conn, &[WorkTaskStatus::Merging])
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .any(|t| t.folder_id == task.folder_id);
        if another_merging {
            if auto {
                // The sweep drains its column one landing at a time and
                // re-sweeps on every settle; a parked unattended intent would
                // outlive the setting that asked for it.
                return Err(
                    "another task of this project is already merging — wait for it".to_string()
                );
            }
            // Take (or keep) a place in line. Reusing the existing `queued_at`
            // is what makes re-queuing — the user reopening the dialog to
            // change the message, or the pump re-parking a merge whose slot got
            // taken first — an edit rather than a trip to the back of the queue.
            let queued_at = claim
                .map(|c| c.queued_at)
                .or_else(|| {
                    work_task_service::queued_merge(task.pending_merge.as_deref())
                        .map(|q| q.queued_at)
                })
                .unwrap_or_else(chrono::Utc::now);
            let intent = WorkTaskQueuedMerge {
                message,
                delete_worktree,
                queued_at,
            };
            let queued = work_task_service::queue_merge(
                &self.db.conn,
                task_id,
                &intent,
                task.run_seq,
                claim.map(|c| c.raw.as_str()),
            )
            .await
            .map_err(|e| e.to_string())?;
            if !queued {
                return Err(missed_queue_cas(claim));
            }
            self.emit_upsert(task_id);
            return Ok(MergeDispatch::Queued);
        }
        let head = resolve_git_head(&root.path).await.map_err(|e| e.to_string())?;
        if head.branch.as_deref() != Some(base_branch.as_str()) {
            return Err(format!(
                "project folder is on '{}', expected '{base_branch}' — switch back to merge",
                head.branch.as_deref().unwrap_or("detached HEAD")
            ));
        }
        match task_git::staged_clean(&root.path).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(
                    "the project folder has staged changes — commit or unstage them first"
                        .to_string(),
                )
            }
            Err(e) => return Err(e.to_string()),
        }
        let pre_merge_head = task_git::rev_parse(&root.path, "HEAD")
            .await
            .map_err(|e| e.to_string())?;

        let state = WorkTaskMergeState {
            pre_merge_head,
            message: message.clone().unwrap_or_default(),
            strategy: strategy.clone(),
            delete_worktree,
            auto_message: message.is_none(),
        };
        // Keep recovery away from the dispatch window (begin → live conn).
        self.merging.lock().await.insert(task_id);
        // `task.run_seq` was read before the folder lock; the CAS binds the
        // dispatch to that exact generation, so waiting out the lock behind an
        // attempt that failed (and bannered the row) misses instead of
        // redispatching — the auto path's no-retry latch depends on this.
        let result = match work_task_service::begin_merge(
            &self.db.conn,
            task_id,
            &state,
            task.run_seq,
            auto,
            claim.map(|c| c.raw.as_str()),
        )
        .await
        {
            Err(e) => Err(e.to_string()),
            Ok(None) => Err(match claim {
                Some(_) => missed_queue_cas(claim),
                None => "task left review before the merge began".to_string(),
            }),
            Ok(Some(_run_seq)) => {
                self.emit_upsert(task_id);
                // A merge generation never transitions out of `merging` here,
                // so the sequence sink stays unused — the failure is handled by
                // the match below (residue cleanup + back to review).
                let merge_seq = LaunchSeq::default();
                match self
                    .launch(
                        task_id,
                        LaunchMode::Merge {
                            root_path: root.path.clone(),
                            base_branch: base_branch.clone(),
                            work_branch: work_branch.clone(),
                            strategy,
                            message,
                        },
                        &merge_seq,
                    )
                    .await
                {
                    Ok(()) => Ok(MergeDispatch::Dispatched),
                    Err(e) => {
                        self.back_to_review(task_id, format!("merge dispatch failed: {e}"), None)
                            .await;
                        Err(e)
                    }
                }
            }
        };
        self.merging.lock().await.remove(&task_id);
        self.emit_upsert(task_id);
        result
    }

    /// Settle a finished merge generation from git truth: landed ⟺ the base
    /// HEAD moved and contains the work (branch ancestry for merge commits,
    /// tree equality for squashes). Anything else goes back to review with the
    /// reason, after cleaning any half-done merge out of the project folder.
    async fn settle_merge_generation(
        self: &Arc<Self>,
        task: &crate::db::entities::work_task::Model,
        stop_reason: &str,
        summary: Option<&str>,
    ) {
        let task_id = task.id;
        let Some(state) = task
            .merge_state
            .as_deref()
            .and_then(|s| serde_json::from_str::<WorkTaskMergeState>(s).ok())
        else {
            self.back_to_review(task_id, "merge state lost — please merge again".to_string(), None)
                .await;
            return;
        };
        let root = match get_folder_core(&self.db, task.folder_id).await {
            Ok(r) => r,
            Err(e) => {
                self.back_to_review(task_id, e.to_string(), None).await;
                return;
            }
        };
        let lock = self.folder_lock(task.folder_id).await;
        let _guard = lock.lock().await;

        match self.merge_landed_commit(task, &state, &root.path).await {
            Ok(Some(commit)) => {
                let landed = work_task_service::merge_landed(&self.db.conn, task_id, &commit)
                    .await
                    .unwrap_or(false);
                if landed {
                    self.emit_upsert(task_id);
                    if state.delete_worktree {
                        self.remove_worktree_locked(task_id).await;
                    }
                }
            }
            Ok(None) => {
                self.clean_merge_residue(&root.path).await;
                let reason = match stop_reason {
                    "end_turn" => match summary.map(str::trim).filter(|s| !s.is_empty()) {
                        Some(s) => format!(
                            "the agent finished without landing the merge: {}",
                            first_chars(s, 300)
                        ),
                        None => "the agent finished without landing the merge — review and \
                                 merge again"
                            .to_string(),
                    },
                    "cancelled" => "the merge run was stopped before landing".to_string(),
                    other => format!("the merge run failed before landing: {other}"),
                };
                self.back_to_review(task_id, reason, None).await;
            }
            Err(e) => {
                self.back_to_review(task_id, format!("could not verify the merge: {e}"), None)
                    .await;
            }
        }
    }

    /// `Some(base HEAD)` when git truth says this task landed on the base.
    /// The commit message is deliberately not consulted — it may have been
    /// written by the agent.
    async fn merge_landed_commit(
        &self,
        task: &crate::db::entities::work_task::Model,
        state: &WorkTaskMergeState,
        root_path: &str,
    ) -> Result<Option<String>, String> {
        let head = task_git::rev_parse(root_path, "HEAD")
            .await
            .map_err(|e| e.to_string())?;
        if head == state.pre_merge_head {
            return Ok(None);
        }
        let Some(work_branch) = task.work_branch.as_deref() else {
            return Ok(None);
        };
        let ancestor = task_git::is_ancestor(root_path, work_branch, &head)
            .await
            .unwrap_or(false);
        let same_tree = task_git::trees_equal(root_path, &head, work_branch)
            .await
            .unwrap_or(false);
        Ok((ancestor || same_tree).then_some(head))
    }

    /// Clean a half-done landing out of the project folder: an in-progress
    /// merge (MERGE_HEAD) or a staged-but-uncommitted squash.
    async fn clean_merge_residue(&self, root_path: &str) {
        let residue = task_git::has_merge_head(root_path).await.unwrap_or(false)
            || !task_git::staged_clean(root_path).await.unwrap_or(true);
        if residue {
            let _ = task_git::reset_merge(root_path).await;
        }
    }

    async fn back_to_review(
        &self,
        task_id: i32,
        error: String,
        conflict_files: Option<Vec<String>>,
    ) {
        let _ = work_task_service::merge_back_to_review(
            &self.db.conn,
            task_id,
            Some(error),
            conflict_files,
        )
        .await;
        self.emit_upsert(task_id);
    }

    /// Resolve (project folder, worktree folder, base branch, work branch) or
    /// explain what's missing.
    async fn merge_coordinates(
        &self,
        task: &crate::db::entities::work_task::Model,
    ) -> Result<
        (
            crate::models::FolderDetail,
            crate::models::FolderDetail,
            String,
            String,
        ),
        String,
    > {
        let root = get_folder_core(&self.db, task.folder_id)
            .await
            .map_err(|e| e.to_string())?;
        let wt_id = task
            .worktree_folder_id
            .ok_or_else(|| "task has no worktree".to_string())?;
        let wt = get_folder_core(&self.db, wt_id)
            .await
            .map_err(|e| e.to_string())?;
        if !Path::new(&wt.path).exists() {
            return Err("the task worktree no longer exists on disk".to_string());
        }
        let base_branch = task
            .base_branch
            .clone()
            .ok_or_else(|| "task has no recorded base branch".to_string())?;
        let work_branch = task
            .work_branch
            .clone()
            .ok_or_else(|| "task has no recorded work branch".to_string())?;
        Ok((root, wt, base_branch, work_branch))
    }

    // ── merge pump (the folder's one merge slot) ────────────────────────────

    /// Advance the folder's merge slot: the user's merge queue first, then the
    /// auto-merge train. Called wherever that slot can have freed (a settled
    /// merge, crash recovery, a fresh review, the reconcile tick) — the two
    /// sources of landings share one slot, so they share one pump.
    async fn merge_pump(self: &Arc<Self>, folder_id: i32) {
        if self.drain_merge_queue(folder_id).await {
            return;
        }
        self.auto_merge_sweep(folder_id).await;
    }

    /// Dispatch the oldest merge the user queued on this folder. Returns
    /// whether the merge slot is spoken for — the auto sweep must not chase a
    /// user's landing into the same slot, and a queued column drains one
    /// landing per pump (every settle pumps again).
    ///
    /// A queue that changed under the scan (the user withdrew or edited a merge
    /// while this was working) is re-scanned rather than worked from the stale
    /// picture: continuing would dispatch out of order, and reporting an empty
    /// queue would hand the slot to the auto sweep — which has neither the
    /// user's commit message nor their worktree choice. Bounded, because each
    /// retry needs a fresh concurrent change to happen at all; still churning
    /// after that leaves the slot to the next pump.
    async fn drain_merge_queue(self: &Arc<Self>, folder_id: i32) -> bool {
        for _ in 0..MERGE_QUEUE_DRAIN_ATTEMPTS {
            match self.drain_merge_queue_once(folder_id).await {
                DrainOutcome::Taken => return true,
                DrainOutcome::Empty => return false,
                DrainOutcome::Stale => continue,
            }
        }
        true
    }

    /// One pass over the folder's queue. See [`Self::drain_merge_queue`].
    ///
    /// Refusals are handled like the sweep's: a task that left the queue on its
    /// own moves on to the next one, and a real refusal (wrong base branch,
    /// staged changes, a worktree that vanished) banners the row and drops its
    /// queue entry, so a hopeless intent is attempted once rather than looped
    /// over by the reconcile tick.
    async fn drain_merge_queue_once(self: &Arc<Self>, folder_id: i32) -> DrainOutcome {
        // Fast path only — merge_task re-checks under the folder lock.
        let merging = work_task_service::list_by_status(&self.db.conn, &[WorkTaskStatus::Merging])
            .await
            .unwrap_or_default();
        if merging.iter().any(|t| t.folder_id == folder_id) {
            return DrainOutcome::Taken;
        }
        let mut queued: Vec<_> =
            work_task_service::list_by_status(&self.db.conn, &[WorkTaskStatus::Review])
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.folder_id == folder_id)
                .filter_map(|t| {
                    // The raw column value travels with the parsed intent: it is
                    // the token every write below CASes on (see
                    // `QueuedMergeClaim`).
                    let raw = t.pending_merge.clone()?;
                    let intent = work_task_service::queued_merge(Some(raw.as_str()))?;
                    let claim = QueuedMergeClaim {
                        raw,
                        queued_at: intent.queued_at,
                    };
                    Some((intent, claim, t))
                })
                .collect();
        queued.sort_by_key(|(intent, _, task)| queue_order(intent, task));
        for (intent, claim, task) in queued {
            if self.live_worktree(&task).await.is_none() {
                // Only the intent we scanned can be refused; anything else on
                // the row now is a change we have to re-read.
                if !self
                    .refuse_queued_merge(&task, &claim, "the task worktree no longer exists on disk")
                    .await
                {
                    return DrainOutcome::Stale;
                }
                continue;
            }
            match self
                .merge_task_inner(
                    task.id,
                    intent.message.clone(),
                    intent.delete_worktree,
                    false,
                    Some(&claim),
                )
                .await
            {
                // Queued again: the slot was taken between the scan and the
                // folder lock. The row keeps its place in line untouched.
                Ok(_) => return DrainOutcome::Taken,
                // The user withdrew or edited THIS merge while we worked on it:
                // the whole snapshot is stale (their edit may now sort first,
                // and it must not be left to the auto sweep's defaults).
                Err(e) if is_queued_merge_superseded(&e) => return DrainOutcome::Stale,
                // The task left review on its own — it is out of the queue for
                // good, so the rest of the snapshot still holds.
                Err(e) if is_benign_merge_race(&e) => continue,
                Err(e) => {
                    tracing::warn!("[work_task] queued merge of task {} refused: {e}", task.id);
                    if !self.refuse_queued_merge(&task, &claim, &e).await {
                        return DrainOutcome::Stale;
                    }
                    return DrainOutcome::Taken;
                }
            }
        }
        DrainOutcome::Empty
    }

    /// Drop a queued merge that cannot run and leave the reason on the card —
    /// the queue must not hold an intent the user has no way to see failing.
    ///
    /// Both writes are CAS'd on the refused intent: while the dispatch was
    /// failing the user may have withdrawn it or queued different options, and
    /// neither a silent delete of that newer request nor a banner about an
    /// older one would be true.
    /// `false` = the row moved on, so nothing was written: its current state is
    /// not this refusal's to judge, and the caller must re-read the queue.
    async fn refuse_queued_merge(
        &self,
        task: &crate::db::entities::work_task::Model,
        claim: &QueuedMergeClaim,
        error: &str,
    ) -> bool {
        let dropped = work_task_service::clear_queued_merge(&self.db.conn, task.id, &claim.raw)
            .await
            .unwrap_or(false);
        if !dropped {
            return false;
        }
        let _ = work_task_service::set_review_error(
            &self.db.conn,
            task.id,
            task.run_seq,
            &format!("queued merge failed: {error}"),
        )
        .await;
        self.emit_upsert(task.id);
        true
    }

    // ── auto-merge (unattended landing) ─────────────────────────────────────

    /// Fire-and-forget [`Self::merge_pump`] — a dispatch holds the folder's git
    /// lock for the whole launch, which must not stall the engine's event loop.
    fn spawn_merge_pump(self: &Arc<Self>, folder_id: i32) {
        let engine = self.clone();
        tokio::spawn(async move {
            engine.merge_pump(folder_id).await;
        });
    }

    /// Pump one folder — or, with `None`, every folder that currently holds a
    /// reviewed task. `None` serves the reconcile tick and a change of the
    /// global settings row, which can switch auto-merge on for any folder that
    /// follows it. Each folder's pump runs spawned, so one folder's dispatch
    /// cannot delay another's.
    pub async fn sweep_merge_backlog(self: &Arc<Self>, folder_id: Option<i32>) {
        match folder_id {
            Some(folder_id) => self.spawn_merge_pump(folder_id),
            None => {
                let review = work_task_service::list_by_status(
                    &self.db.conn,
                    &[WorkTaskStatus::Review],
                )
                .await
                .unwrap_or_default();
                let mut swept: HashSet<i32> = HashSet::new();
                for task in review {
                    if swept.insert(task.folder_id) {
                        self.spawn_merge_pump(task.folder_id);
                    }
                }
            }
        }
    }

    /// When the folder's settings ask for it, dispatch the same merge the
    /// button would — agent-written commit message, worktree per the folder's
    /// default — for the oldest reviewed task that is actually mergeable. One
    /// dispatch per sweep: merges are serial per folder anyway, and every
    /// settle re-sweeps, so a column of reviewed tasks drains one landing at a
    /// time (the merge train).
    ///
    /// Concurrent sweeps, clicks and settles are all safe: `merge_task`
    /// serializes dispatches on the folder git lock and CAS-guards
    /// review→merging, so the worst case is a benign "someone got there first"
    /// refusal, which the sweep swallows. A dispatch refused for a real reason
    /// (wrong base branch, staged changes, …) leaves that reason on the row as
    /// the card's error banner — and a row with an error is no longer
    /// eligible, so a hopeless dispatch is attempted once, not looped.
    async fn auto_merge_sweep(self: &Arc<Self>, folder_id: i32) {
        let settings = work_task_service::settings_get_effective(&self.db.conn, folder_id)
            .await
            .unwrap_or_default();
        if !settings.auto_merge {
            return;
        }
        // Fast path only — merge_task re-checks under the folder lock.
        let merging = work_task_service::list_by_status(&self.db.conn, &[WorkTaskStatus::Merging])
            .await
            .unwrap_or_default();
        if merging.iter().any(|t| t.folder_id == folder_id) {
            return;
        }
        let mut candidates: Vec<_> =
            work_task_service::list_by_status(&self.db.conn, &[WorkTaskStatus::Review])
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.folder_id == folder_id && auto_merge_candidate(t, &settings))
                .collect();
        candidates.sort_by_key(|t| (t.settled_at, t.id));
        for task in candidates {
            // A gone worktree cannot serve a merge generation; that task's
            // acceptance is the "complete" button — a user decision.
            if self.live_worktree(&task).await.is_none() {
                continue;
            }
            match self
                .merge_task(task.id, None, settings.delete_worktree_default, true)
                .await
            {
                // An unattended dispatch never queues (see `merge_task`), so
                // this is always the live generation.
                Ok(_) => {}
                Err(e) if is_benign_merge_race(&e) => {}
                Err(e) => {
                    tracing::warn!(
                        "[work_task] auto-merge of task {} refused: {e}",
                        task.id
                    );
                    if work_task_service::set_review_error(
                        &self.db.conn,
                        task.id,
                        task.run_seq,
                        &format!("auto-merge failed: {e}"),
                    )
                    .await
                    .unwrap_or(false)
                    {
                        self.emit_upsert(task.id);
                    }
                }
            }
            // One dispatch (or one refusal) per sweep. Cascading on after a
            // refusal would stamp one environmental problem onto every card
            // in the column.
            return;
        }
    }

    // ── merging crash recovery (git truth) ──────────────────────────────────

    /// Recover a task stuck in `merging` (crash / lost process) from git
    /// truth in the project folder. A merge generation with a live agent
    /// connection is not stuck — its TurnComplete settles it.
    pub async fn recover_merging(self: &Arc<Self>, task_id: i32) {
        if self.merging.lock().await.contains(&task_id) {
            return; // merge dispatch in flight in this process
        }
        let Ok(task) = work_task_service::get_model(&self.db.conn, task_id).await else {
            return;
        };
        if task.status != WorkTaskStatus::Merging {
            return;
        }
        if let Some(conn_id) = task.connection_id.as_deref() {
            if self.manager.get_state_and_emitter(conn_id).await.is_some() {
                return; // live merge generation — on_turn_complete owns the settle
            }
        }
        let Some(state) = task
            .merge_state
            .as_deref()
            .and_then(|s| serde_json::from_str::<WorkTaskMergeState>(s).ok())
        else {
            self.back_to_review(
                task_id,
                "merge state lost — please merge again".to_string(),
                None,
            )
            .await;
            return;
        };
        let Ok(root) = get_folder_core(&self.db, task.folder_id).await else {
            return;
        };

        let lock = self.folder_lock(task.folder_id).await;
        let _guard = lock.lock().await;
        // Re-read under the lock; an in-flight settle may have finished it.
        let Ok(current) = work_task_service::get_model(&self.db.conn, task_id).await else {
            return;
        };
        if current.status != WorkTaskStatus::Merging || current.run_seq != task.run_seq {
            return;
        }

        match self.merge_landed_commit(&current, &state, &root.path).await {
            Ok(Some(commit)) => {
                let landed = work_task_service::merge_landed(&self.db.conn, task_id, &commit)
                    .await
                    .unwrap_or(false);
                if landed {
                    self.emit_upsert(task_id);
                    if state.delete_worktree {
                        self.remove_worktree_locked(task_id).await;
                    }
                }
            }
            Ok(None) => {
                self.clean_merge_residue(&root.path).await;
                self.back_to_review(
                    task_id,
                    "the merge was interrupted before landing — merge again".to_string(),
                    None,
                )
                .await;
            }
            Err(e) => {
                self.back_to_review(task_id, format!("could not verify the merge: {e}"), None)
                    .await;
            }
        }
    }

    // ── worktree cleanup ────────────────────────────────────────────────────

    /// Remove a task's worktree + branch (user action from the card, or the
    /// post-merge checkbox). Takes the per-folder git lock.
    pub async fn cleanup_task(&self, task_id: i32) -> Result<(), String> {
        let task = work_task_service::get_model(&self.db.conn, task_id)
            .await
            .map_err(|e| e.to_string())?;
        if matches!(
            task.status,
            WorkTaskStatus::Queued
                | WorkTaskStatus::Preparing
                | WorkTaskStatus::Running
                | WorkTaskStatus::AwaitingInput
                | WorkTaskStatus::Merging
        ) {
            return Err("cancel or finish the task before removing its worktree".to_string());
        }
        let lock = self.folder_lock(task.folder_id).await;
        let _guard = lock.lock().await;
        self.remove_worktree_locked(task_id).await;
        self.emit_upsert(task_id);
        Ok(())
    }

    /// Git-first-then-DB worktree removal. Caller holds the folder git lock.
    ///
    /// Order matters: the git removal runs first; only after it succeeds does
    /// the DB transaction re-parent the worktree's conversations onto the
    /// project folder (stamping `origin_cwd`), close its tabs, and soft-delete
    /// the folder row. A git failure flags `cleanup_state='failed'` (retryable
    /// from the card) and leaves the DB untouched; a `done` task never leaves
    /// `done` either way.
    async fn remove_worktree_locked(&self, task_id: i32) {
        let Ok(task) = work_task_service::get_model(&self.db.conn, task_id).await else {
            return;
        };
        let Some(wt_id) = task.worktree_folder_id else {
            // Nothing left to remove. A cleanup flag surviving past the
            // detach would offer a retry that can never succeed — clear it.
            if task.cleanup_state.is_some() {
                let _ =
                    work_task_service::set_cleanup_state(&self.db.conn, task_id, false, None).await;
            }
            return;
        };
        // Precondition: no live connection of ours on this task.
        let has_conn = {
            self.index
                .lock()
                .await
                .values()
                .any(|(tid, _)| *tid == task_id)
        };
        if has_conn {
            let _ = work_task_service::set_cleanup_state(
                &self.db.conn,
                task_id,
                true,
                Some("task still has a live agent connection".to_string()),
            )
            .await;
            return;
        }

        let root = match get_folder_core(&self.db, task.folder_id).await {
            Ok(r) => r,
            Err(e) => {
                let _ = work_task_service::set_cleanup_state(
                    &self.db.conn,
                    task_id,
                    true,
                    Some(e.to_string()),
                )
                .await;
                return;
            }
        };
        let Ok(wt) = get_folder_core(&self.db, wt_id).await else {
            // Folder row already gone (a retried cleanup) — just detach, and
            // still tell clients to drop it: one holding a stale copy would
            // otherwise keep rendering a worktree no refetch would return.
            let _ = work_task_service::clear_worktree(&self.db.conn, task_id).await;
            emit_folder_deleted(&self.emitter, wt_id);
            return;
        };

        if let Err(e) = task_git::remove_worktree_and_branch(
            &root.path,
            &wt.path,
            task.work_branch.as_deref(),
        )
        .await
        {
            let _ = work_task_service::set_cleanup_state(
                &self.db.conn,
                task_id,
                true,
                Some(e.to_string()),
            )
            .await;
            return;
        }

        converge_worktree_removal(
            &self.db,
            &self.emitter,
            task_id,
            wt_id,
            task.folder_id,
            &wt.path,
        )
        .await;
    }

    // ── reconcile ───────────────────────────────────────────────────────────

    async fn reconcile_once(self: &Arc<Self>) {
        // running / awaiting_input whose worker died without a TurnComplete.
        let active = work_task_service::list_by_status(
            &self.db.conn,
            &[WorkTaskStatus::Running, WorkTaskStatus::AwaitingInput],
        )
        .await
        .unwrap_or_default();
        for task in active {
            let Some(conn_id) = task.connection_id.clone() else {
                continue;
            };
            if self.manager.get_state_and_emitter(&conn_id).await.is_some() {
                continue; // live — on_event settles it authoritatively
            }
            // Connection gone. If the produced conversation reached a terminal
            // status the TurnComplete was merely dropped — settle from it.
            self.index.lock().await.remove(&conn_id);
            self.awaiting.lock().await.remove(&task.id);
            self.forget_delegation_children_of(&conn_id).await;
            let conv_status = match task.conversation_id {
                Some(cid) => self.conversation_status(cid).await,
                None => None,
            };
            let changed = match conv_status {
                Some(ConversationStatus::PendingReview) | Some(ConversationStatus::Completed) => {
                    let stats = self.snapshot_diff_stats(task.id).await;
                    let settled = work_task_service::settle_review(
                        &self.db.conn,
                        task.id,
                        task.run_seq,
                        None,
                        stats,
                    )
                    .await
                    .unwrap_or(false);
                    if settled {
                        // The dropped TurnComplete owed review its preflight
                        // and its auto-merge chance — same hook as the live
                        // settle path.
                        self.spawn_post_review(task.id, task.run_seq);
                    }
                    settled
                }
                Some(ConversationStatus::Cancelled) => {
                    work_task_service::cancel(&self.db.conn, task.id, None)
                        .await
                        .unwrap_or(false)
                }
                _ => work_task_service::fail(
                    &self.db.conn,
                    task.id,
                    &[WorkTaskStatus::Running, WorkTaskStatus::AwaitingInput],
                    Some(task.run_seq),
                    "interrupted",
                    Some("task lost its worker".to_string()),
                )
                .await
                .unwrap_or(false),
            };
            if changed {
                self.emit_upsert(task.id);
            }
        }

        // preparing rows that no in-flight launch owns → back to the queue.
        // `launching` is authoritative here: this process holds the exclusive
        // engine lock, and the entry outlives the whole launch (including its
        // failure handling). Without this sweep an orphan would sit in
        // `preparing` forever — `next_queued` only picks `queued`, so it would
        // lose the self-healing a stuck `queued` row gets today.
        let preparing =
            work_task_service::list_by_status(&self.db.conn, &[WorkTaskStatus::Preparing])
                .await
                .unwrap_or_default();
        if !preparing.is_empty() {
            let launching: HashSet<i32> = self.launching.lock().await.keys().copied().collect();
            for task in preparing {
                if launching.contains(&task.id) {
                    continue;
                }
                match work_task_service::abandon_setup(&self.db.conn, task.id, task.run_seq).await {
                    Ok(true) => {
                        tracing::info!(
                            "[work_task] requeued orphaned setup of task {}",
                            task.id
                        );
                        self.emit_upsert(task.id);
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!("[work_task] abandon setup error: {e}"),
                }
            }
        }

        // merging not owned by this process's in-flight merges → git truth.
        // Spawned off-thread: recovery waits on the per-folder git lock, and an
        // in-flight merge on that folder must not stall the event loop here.
        let merging = work_task_service::list_by_status(&self.db.conn, &[WorkTaskStatus::Merging])
            .await
            .unwrap_or_default();
        for task in merging {
            let engine = self.clone();
            tokio::spawn(async move {
                engine.recover_merging(task.id).await;
                engine.pump_folder(task.folder_id).await;
                // Recovery freed the folder's merge slot (landed or bounced) —
                // resume the queue / auto-merge train where the crash cut it.
                engine.merge_pump(task.folder_id).await;
            });
        }

        // Pending backlog: queued tasks whose slot freed while no event fired,
        // plus todo tasks of auto_process folders (the pump checks the flag).
        for folder_id in work_task_service::folders_with_pending(&self.db.conn)
            .await
            .unwrap_or_default()
        {
            self.pump_folder(folder_id).await;
        }

        // Review backlog: merges the user queued whose pump never ran (queued
        // before a restart, or a settle lost in the crash window), plus the
        // auto-merge folders' own backlog (a dispatch lost between the settle
        // and the merge, and tasks already sitting in review when the setting
        // was switched on). The pump re-checks per folder, so this is a cheap
        // scan for folders with neither.
        self.sweep_merge_backlog(None).await;
    }

    // ── helpers ─────────────────────────────────────────────────────────────

    async fn task_lock(&self, task_id: i32) -> Arc<Mutex<()>> {
        let mut locks = self.task_locks.lock().await;
        locks
            .entry(task_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn folder_lock(&self, folder_id: i32) -> Arc<Mutex<()>> {
        let mut locks = self.folder_locks.lock().await;
        locks
            .entry(folder_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn emit_upsert(&self, task_id: i32) {
        emit_event(
            &self.emitter,
            WORK_TASK_CHANGED_EVENT,
            WorkTaskChange::Upsert { id: task_id },
        );
    }

    fn emit_changed_all(&self) {
        emit_event(
            &self.emitter,
            WORK_TASK_CHANGED_EVENT,
            WorkTaskChange::Refresh,
        );
    }

    async fn conversation_status(&self, conv_id: i32) -> Option<ConversationStatus> {
        conversation::Entity::find_by_id(conv_id)
            .one(&self.db.conn)
            .await
            .ok()
            .flatten()
            .map(|m| m.status)
    }

    async fn cancel_conversation(&self, conversation_id: i32) {
        if let Ok(Some(row)) = conversation::Entity::find_by_id(conversation_id)
            .one(&self.db.conn)
            .await
        {
            let mut active = row.into_active_model();
            active.status = Set(ConversationStatus::Cancelled);
            if active.update(&self.db.conn).await.is_ok() {
                emit_conversation_upsert(&self.emitter, &self.db.conn, conversation_id).await;
            }
        }
    }
}

struct WorktreeRef {
    folder_id: i32,
    path: String,
}

/// Converge DB + clients once a task worktree is off disk. Split out of
/// [`TaskEngine::remove_worktree_locked`] so the ordering contract below is
/// unit-testable without a real git worktree.
///
/// Write order: conversations first (never orphaned under a vanishing folder),
/// then its tabs, then the folder row. Each step ends in a broadcast, because a
/// removal no client hears about is a removal no client shows: the sidebar keeps
/// the dead worktree until the next full `fetchFolders` (i.e. a reload).
async fn converge_worktree_removal(
    db: &AppDatabase,
    emitter: &EventEmitter,
    task_id: i32,
    wt_id: i32,
    project_folder_id: i32,
    wt_path: &str,
) {
    let moved = conversation_service::reparent_folder_conversations(
        &db.conn,
        wt_id,
        project_folder_id,
        wt_path,
    )
    .await
    .unwrap_or(0);

    match tab_service::delete_folder_tabs_and_bump(&db.conn, wt_id).await {
        Ok(inv) => {
            if let Some(tabs) = inv.emit {
                emit_event(
                    emitter,
                    crate::web::event_bridge::TABS_CHANGED_EVENT,
                    crate::web::event_bridge::TabsChanged {
                        version: inv.version,
                        origin: "server".to_string(),
                        tabs,
                    },
                );
            }
        }
        Err(e) => tracing::warn!("[work_task] tab cleanup failed for folder {wt_id}: {e}"),
    }

    // Announce the folder drop only when the row really is gone, so a failed
    // write can't leave clients disagreeing with what a refetch would return.
    let folder_gone = match folder::Entity::find_by_id(wt_id).one(&db.conn).await {
        Ok(Some(row)) => {
            let now = chrono::Utc::now();
            let mut active = row.into_active_model();
            active.is_open = Set(false);
            active.deleted_at = Set(Some(now));
            active.updated_at = Set(now);
            match active.update(&db.conn).await {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!("[work_task] worktree folder {wt_id} soft-delete failed: {e}");
                    false
                }
            }
        }
        // Already soft-deleted / never there: a client still holding it is stale.
        Ok(None) => true,
        Err(e) => {
            tracing::warn!("[work_task] worktree folder {wt_id} lookup failed: {e}");
            false
        }
    };

    let _ = work_task_service::clear_worktree(&db.conn, task_id).await;
    let _ = work_task_service::record_event(
        &db.conn,
        task_id,
        "user_action",
        "engine",
        Some(serde_json::json!({ "action": "cleanup", "reparented": moved })),
    )
    .await;

    // Conversations first: this starts every client's refetch, so the moved
    // rows have the shortest possible window with no folder to render under.
    emit_event(
        emitter,
        crate::web::event_bridge::CONVERSATIONS_BULK_CHANGED_EVENT,
        crate::web::event_bridge::ConversationsBulkChanged {
            imported: 0,
            updated: moved as u32,
            folder_ids: vec![project_folder_id],
        },
    );
    if folder_gone {
        emit_folder_deleted(emitter, wt_id);
    }
}

/// Row-level auto-merge eligibility: everything the sweep can decide without
/// touching git. Mirrors what the board offers — a task whose merge button the
/// user would not see (nothing to land) or would not press yet (red or pending
/// preflight, an error banner from an earlier attempt) is not auto-merged.
///
/// - `files_changed == 0` is the "complete" button's territory: dispatching a
///   merge generation there would land nothing and bounce. `None` (stats
///   unreadable) merges, exactly like the UI defaults to the merge button.
/// - With a preflight configured, only a green light qualifies; `None` means
///   the light is not written yet (the post-preflight hook re-sweeps) and a
///   red light waits for the user. Without one, there is no gate.
/// - `last_error` present = an earlier merge failed or was refused; retrying
///   unattended would loop, so the row waits for the user (any claim or a
///   fresh dispatch clears it).
fn auto_merge_candidate(
    task: &crate::db::entities::work_task::Model,
    settings: &WorkTaskFolderSettings,
) -> bool {
    if task.status != WorkTaskStatus::Review {
        return false;
    }
    if task.last_error.is_some() {
        return false;
    }
    // A merge the user queued is theirs to land, with the commit message and
    // the worktree choice THEY picked. The unattended dispatch has neither, so
    // the queue's own drain owns this row (and clears the intent in the same
    // CAS that starts the merge, which makes the row a normal candidate again).
    //
    // Any value in the column counts, parseable or not: the authoritative gate
    // is `begin_merge`'s `PendingMerge IS NULL` filter, SQL cannot parse the
    // JSON, and a looser predicate here would just dispatch into a CAS that
    // misses every time — a sweep spinning on a row it can never land.
    if task.pending_merge.is_some() {
        return false;
    }
    if task.files_changed == Some(0) {
        return false;
    }
    if preflight_configured(settings) {
        let passed = task
            .preflight
            .as_deref()
            .and_then(|s| serde_json::from_str::<WorkTaskPreflight>(s).ok())
            .is_some_and(|p| p.status == "passed");
        if !passed {
            return false;
        }
    }
    true
}

/// Whether the folder's settings would make `run_preflight` attempt a command
/// at all — the sweep's gate must not wait for a light that will never be
/// written.
fn preflight_configured(settings: &WorkTaskFolderSettings) -> bool {
    settings
        .preflight_command
        .as_deref()
        .map(str::trim)
        .is_some_and(|c| !c.is_empty())
        || settings.preflight_command_id.is_some()
}

/// Why a queue-bound CAS missed. With a claim it can be either "the row left
/// review" or "this queued merge is no longer the one on the row" (withdrawn,
/// or edited into a different one) — the pump treats both the same way, and
/// both are races it should lose quietly rather than banner.
fn missed_queue_cas(claim: Option<&QueuedMergeClaim>) -> String {
    match claim {
        Some(_) => "the queued merge was changed or withdrawn before it could start".to_string(),
        None => "task left review before the merge was queued".to_string(),
    }
}

/// The order the folder's merge queue drains in: oldest request first, task id
/// breaking a tie. Mirrored client-side by `mergeQueueRanks`, so the place in
/// line a card shows is the order the pump actually dispatches in.
fn queue_order(
    intent: &WorkTaskQueuedMerge,
    task: &crate::db::entities::work_task::Model,
) -> (chrono::DateTime<chrono::Utc>, i32) {
    (intent.queued_at, task.id)
}

/// Merge-dispatch refusals that mean "someone else is (or just was) handling
/// this" rather than "this merge cannot work": the losing side of a race with
/// a click, another sweep or a user action. Matched on `merge_task`'s own
/// wording (same file, a screen up) — these are skipped silently, because the
/// next settle re-sweeps, while every other refusal banners the row.
fn is_benign_merge_race(error: &str) -> bool {
    error.contains("not in review")
        || error.contains("left review")
        || error.contains("already merging")
        || is_queued_merge_superseded(error)
}

/// The one benign race the drain cannot simply step over: the user withdrew or
/// edited the very merge it was dispatching (see [`missed_queue_cas`]). Their
/// word replaces the whole snapshot — a re-read, not a skip.
fn is_queued_merge_superseded(error: &str) -> bool {
    error.contains("changed or withdrawn")
}

/// Pick the launch mode for a pump-driven launch from the task's history: a
/// task with a prior conversation continues (retry semantics); a pristine one
/// starts fresh. Explicit returns launch directly with `LaunchMode::Return`.
fn launch_mode_for(task: &crate::db::entities::work_task::Model) -> LaunchMode {
    if task.conversation_id.is_some() {
        LaunchMode::Retry
    } else {
        LaunchMode::Fresh
    }
}

/// Layered agent config: task override wins wholesale; else the folder's task
/// settings; else the folder's default agent with no extra options.
fn effective_agent_config(
    cfg: &WorkTaskConfig,
    settings: &WorkTaskFolderSettings,
    root: &crate::models::FolderDetail,
) -> (
    Option<String>,
    Option<String>,
    std::collections::BTreeMap<String, String>,
) {
    if let Some(agent) = cfg.agent_type.clone() {
        return (Some(agent), cfg.mode_id.clone(), cfg.config_values.clone());
    }
    if let Some(agent) = settings.default_agent_type.clone() {
        return (
            Some(agent),
            settings.mode_id.clone(),
            settings.config_values.clone(),
        );
    }
    let folder_default = root
        .default_agent_type
        .as_ref()
        .and_then(|a| serde_json::to_value(a).ok())
        .and_then(|v| v.as_str().map(String::from));
    (
        folder_default,
        settings.mode_id.clone(),
        settings.config_values.clone(),
    )
}

/// Compose the prompt for a launch mode. Fresh runs replay the task's blocks.
/// Retry/return compose against the session we actually got: a resumed session
/// already carries the task context, while a fresh fallback session needs the
/// full original description again. Every prompt ends with the worktree guard,
/// then with whatever the folder's settings add for this stage.
async fn compose_prompt(
    cfg: &WorkTaskConfig,
    task: &crate::db::entities::work_task::Model,
    mode: &LaunchMode,
    settings: &WorkTaskFolderSettings,
    resumed: bool,
    conn: &sea_orm::DatabaseConnection,
) -> Result<Vec<PromptInputBlock>, String> {
    let mut blocks: Vec<PromptInputBlock> = Vec::new();
    let original: Vec<PromptInputBlock> = cfg
        .prompt_blocks
        .iter()
        .map(|v| serde_json::from_value::<PromptInputBlock>(v.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("bad prompt blocks: {e}"))?;

    match mode {
        LaunchMode::Fresh => {
            if original.is_empty() {
                return Err("prompt is empty".to_string());
            }
            blocks.extend(original);
            // A task can reach a fresh launch carrying a restart note: it was
            // canceled (or failed during setup) before it ever had a session,
            // and the user attached a note when re-queueing it. Review feedback
            // cannot exist here — that needs a session — but match on the kind
            // rather than assume it.
            if let Some(outstanding) = outstanding_instruction(conn, task.id).await {
                if matches!(outstanding.kind, OutstandingKind::Restart) {
                    blocks.push(restart_note_block(&outstanding.text));
                    blocks.extend(attachment_blocks(&outstanding.attachments, task.id));
                }
            }
        }
        LaunchMode::Retry => {
            blocks.push(PromptInputBlock::Text {
                text: "The previous run of this task was interrupted. Continue working in \
                       this worktree and complete the task."
                    .to_string(),
            });
            if !resumed {
                blocks.push(PromptInputBlock::Text {
                    text: "The original task follows for reference:".to_string(),
                });
                blocks.extend(original);
            }
            // Replay whatever instruction the interrupted generation still
            // owed the user, framed the way they meant it — an unanswered
            // question must not come back as a work order.
            if let Some(outstanding) = outstanding_instruction(conn, task.id).await {
                blocks.push(match outstanding.kind {
                    OutstandingKind::Restart => restart_note_block(&outstanding.text),
                    OutstandingKind::Review(FollowUpIntent::Question) => PromptInputBlock::Text {
                        text: format!(
                            "The user asked this question before the interruption and never \
                             got an answer. Answer it, and do not change any files for it:\
                             \n\n{}",
                            outstanding.text
                        ),
                    },
                    OutstandingKind::Review(_) => PromptInputBlock::Text {
                        text: format!("Latest review feedback to address:\n{}", outstanding.text),
                    },
                });
                // Whatever the user attached to that instruction follows it, so
                // the replay carries the screenshot as well as the sentence.
                blocks.extend(attachment_blocks(&outstanding.attachments, task.id));
            }
        }
        LaunchMode::Return {
            intent,
            feedback,
            attachments,
        } => {
            if !resumed {
                // Session resume failed — the fresh session has no context, so
                // replay the task before the feedback.
                blocks.push(PromptInputBlock::Text {
                    text: "You are picking up a task whose previous session could not be \
                           resumed. The worktree already contains that session's work. \
                           The original task was:"
                        .to_string(),
                });
                blocks.extend(original);
            }
            blocks.push(PromptInputBlock::Text {
                text: follow_up_text(*intent, feedback),
            });
            blocks.extend(attachment_blocks(attachments, task.id));
        }
        LaunchMode::Merge {
            root_path,
            base_branch,
            work_branch,
            strategy,
            message,
        } => {
            if !resumed {
                blocks.push(PromptInputBlock::Text {
                    text: "You are picking up a task whose previous session could not be \
                           resumed. The worktree already contains that session's work. \
                           The original task was:"
                        .to_string(),
                });
                blocks.extend(original);
            }
            let land_command = if strategy == "merge" {
                format!(
                    "git -C \"{root_path}\" merge --no-ff -m \"<message>\" {work_branch}"
                )
            } else {
                format!(
                    "git -C \"{root_path}\" merge --squash {work_branch} && \
                     git -C \"{root_path}\" commit -m \"<message>\""
                )
            };
            let message_rule = match message {
                Some(m) => format!("Use exactly this commit message:\n{m}"),
                None => "Write a concise Conventional Commits message yourself, \
                         summarizing what this task changed."
                    .to_string(),
            };
            blocks.push(PromptInputBlock::Text {
                text: format!(
                    "The user accepted this task — land it onto the base branch \
                     `{base_branch}` now, doing all git operations yourself:\n\
                     1. Commit any uncommitted changes in this worktree to the current \
                     branch (`{work_branch}`).\n\
                     2. Run `git merge {base_branch}` here and resolve every conflict so \
                     both the base's changes and this task's intent survive; complete the \
                     merge commit.\n\
                     3. Land onto the base checkout at `{root_path}`:\n   {land_command}\n\
                     {message_rule}\n\
                     Do NOT push, do NOT delete this worktree or its branch, and do not \
                     change anything else on the base branch. Finish with one short line \
                     saying what landed."
                ),
            });
        }
    }

    // The standing worktree guard — a merge generation replaces it with its
    // own instructions (it exists to forbid exactly what a merge must do).
    //
    // It is the LAST built-in block, so its licence clause is the last thing
    // the agent reads: a read-only turn has to swap that clause out, or "commit
    // to the current branch as you like" would quietly undo the intent's own
    // "don't touch any file" instruction several blocks earlier.
    if !matches!(mode, LaunchMode::Merge { .. }) {
        let branch = task
            .work_branch
            .as_deref()
            .map(|b| format!(" (branch `{b}`)"))
            .unwrap_or_default();
        let base = task
            .base_branch
            .as_deref()
            .map(|b| format!(" (`{b}`)"))
            .unwrap_or_default();
        let licence = if mode.is_read_only() {
            format!(
                "This turn is a question, not a work order: answer it in your reply and do NOT \
                 create, edit, delete or commit any file, and do not merge into, rebase onto, or \
                 push the base branch{base}. If answering would require a change, describe the \
                 change instead of making it."
            )
        } else {
            format!(
                "Commit to the current branch as you like, but do NOT merge into, rebase onto, \
                 or push the base branch{base} — the user lands the result after review. Finish \
                 with a short summary of what you did."
            )
        };
        blocks.push(PromptInputBlock::Text {
            text: format!(
                "—— Work task context ——\nYou are working inside a dedicated git worktree for \
                 this task{branch}. {licence}\nIf the `task_progress` and `task_complete` tools \
                 are available to you, report milestones with `task_progress` as you go, and \
                 call `task_complete` once right before you finish (verdict `success`, \
                 `needs_review`, or `blocked`, plus a short summary).",
            ),
        });
    }

    // Whatever the user added in task settings, always last: it refines the
    // built-in wording above it, and staying at the end keeps `prompt_head`
    // (the transcript's round marker) on the prompt's own opening text.
    blocks.extend(stage_prompt_block(settings, mode.round_kind()));
    Ok(blocks)
}

/// The user's own instructions for a stage: the `all` text (every stage) then
/// the stage's own, as one trailing block. Empty when neither is configured.
fn stage_prompt_block(
    settings: &WorkTaskFolderSettings,
    stage: &str,
) -> Option<PromptInputBlock> {
    let extras: Vec<&str> = [STAGE_PROMPT_ALL, stage]
        .into_iter()
        .filter_map(|key| settings.stage_prompts.get(key))
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .collect();
    if extras.is_empty() {
        return None;
    }
    Some(PromptInputBlock::Text {
        text: format!("—— Additional instructions ——\n{}", extras.join("\n\n")),
    })
}

/// The prompt text for a follow-up on a reviewed task. The intent decides the
/// framing, which is the whole point of having intents: the same sentence from
/// the user means "fix this", "also do this" or "explain this" depending on it,
/// and an agent told it was *returned* work will start editing either way.
fn follow_up_text(intent: FollowUpIntent, feedback: &str) -> String {
    match intent {
        // Historical wording, kept verbatim: this is what an unlabelled
        // follow-up composes, so the default path is unchanged.
        FollowUpIntent::Revise => format!(
            "The user reviewed your work on this task and returned it with the following \
             feedback. Address it in this same worktree:\n\n{feedback}"
        ),
        FollowUpIntent::Continue => format!(
            "The user reviewed your work on this task and accepted it as it stands. Keep going \
             in this same worktree with the following additional work — do not redo or second-\
             guess what is already there:\n\n{feedback}"
        ),
        FollowUpIntent::Question => format!(
            "The user has a question about your work on this task. Answer it directly in your \
             reply. This is a question, not a work order: do not create, edit, delete or commit \
             any file unless the user explicitly asks you to. If answering would require a \
             change, describe the change instead of making it.\n\n{feedback}"
        ),
        FollowUpIntent::Verify => {
            let base = "The user wants this task checked over before they accept it. Review your \
                        own work: read the full diff of this worktree against the base branch \
                        looking for bugs, leftovers, debug code and anything the task asked for \
                        but you did not do; run the project's own checks or tests; and fix what \
                        you find. Report what you checked and what you changed.";
            if feedback.is_empty() {
                base.to_string()
            } else {
                format!("{base}\n\nWhat they want you to pay attention to:\n\n{feedback}")
            }
        }
    }
}

/// The attachment blocks recorded on a `user_action` payload, if any. Absent on
/// every event written before follow-ups could carry attachments, so a missing
/// or malformed field simply means "nothing was attached".
fn payload_blocks(payload: &serde_json::Value) -> Vec<serde_json::Value> {
    payload
        .get("blocks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Parse stored attachment blocks, dropping (with a warning) any that no longer
/// deserialize. Unlike the task's own prompt, an attachment is an addition to
/// the instruction — losing one must not stop the run that carries the rest.
fn attachment_blocks(raw: &[serde_json::Value], task_id: i32) -> Vec<PromptInputBlock> {
    raw.iter()
        .filter_map(|v| match serde_json::from_value::<PromptInputBlock>(v.clone()) {
            Ok(block) => Some(block),
            Err(e) => {
                tracing::warn!("[work_task] task {task_id}: dropping bad attachment block: {e}");
                None
            }
        })
        .collect()
}

/// How long a launch waits for the agent to advertise what a prompt may carry,
/// before sending its attached images in whatever shape they were stored. Only
/// image-bearing prompts ever wait, and only until the handshake lands — which
/// is the same round trip the prompt itself is about to need anyway.
const IMAGE_CAPABILITY_WAIT: Duration = Duration::from_secs(20);

/// Whether this block carries image bytes in either of the two wire encodings.
fn carries_image(block: &PromptInputBlock) -> bool {
    match block {
        PromptInputBlock::Image { .. } => true,
        PromptInputBlock::Resource {
            mime_type, blob, ..
        } => {
            blob.is_some()
                && mime_type
                    .as_deref()
                    .is_some_and(|m| m.starts_with("image/"))
        }
        _ => false,
    }
}

/// Swap every attached image into the shape the connected agent advertised.
///
/// Two wire encodings carry the same bytes: a native `Image` block, and a
/// `Resource` whose `blob` holds them under an image mime type (what an agent
/// that rejects image content but accepts embedded context takes). The composer
/// picks one at compose time from a probe; this picks again at dispatch, when
/// the session has actually said what it accepts.
///
/// Grok needs neither swap — it advertises both bits — but its images still get
/// sorted per mime further downstream, in `acp::connection`, which is the last
/// point that sees the blocks.
///
/// Only image-carrying blocks are touched, and only to move between those two
/// encodings — never to invent or drop content. An agent that advertises
/// neither is left alone: there is no third shape to reach for, and rewriting
/// into one it also rejects would only obscure the error it is about to raise.
fn reencode_images(blocks: &mut [PromptInputBlock], caps: &PromptCapabilitiesInfo) {
    for (index, block) in blocks.iter_mut().enumerate() {
        match block {
            PromptInputBlock::Image {
                data,
                mime_type,
                uri,
            } if !caps.image && caps.embedded_context => {
                *block = PromptInputBlock::Resource {
                    // A path-less image (a pasted screenshot) needs some stable
                    // identifier; its position in this prompt is one.
                    uri: uri
                        .clone()
                        .unwrap_or_else(|| format!("clipboard://work-task-image-{index}")),
                    mime_type: Some(mime_type.clone()),
                    text: None,
                    blob: Some(std::mem::take(data)),
                };
            }
            PromptInputBlock::Resource {
                uri,
                mime_type,
                text: None,
                blob: Some(blob),
            } if caps.image
                && !caps.embedded_context
                && mime_type
                    .as_deref()
                    .is_some_and(|m| m.starts_with("image/")) =>
            {
                *block = PromptInputBlock::Image {
                    data: std::mem::take(blob),
                    mime_type: mime_type.clone().unwrap_or_default(),
                    uri: Some(uri.clone()),
                };
            }
            _ => {}
        }
    }
}

/// A restart note reaches the agent as context, not as the task itself.
fn restart_note_block(note: &str) -> PromptInputBlock {
    PromptInputBlock::Text {
        text: format!(
            "The user restarted this task and left this note — take it into account:\n\n{note}"
        ),
    }
}

/// What the user last asked for, and how they meant it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OutstandingKind {
    /// A follow-up on a reviewed task.
    Review(FollowUpIntent),
    /// A note attached to a retry or a re-queue.
    Restart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Outstanding {
    kind: OutstandingKind,
    text: String,
    /// Raw prompt blocks the user attached to that instruction (images, pasted
    /// bytes). Kept unparsed here — `attachment_blocks` validates at the point
    /// the prompt is composed.
    attachments: Vec<serde_json::Value>,
}

/// The user instruction the agent still owes a turn to, if any.
///
/// Scans the task's events newest-first and stops at whichever comes first:
/// - a follow-up `user_action` (return / retry / requeue) → that one is
///   outstanding;
/// - a settle into `review` → a generation ran to completion after anything
///   earlier, so nothing is owed.
///
/// It is a barrier, not a filter. Filtering (e.g. "skip questions") would walk
/// *past* the newest instruction and resurrect an older one the agent already
/// carried out — replaying "add the tests" long after they were added. And the
/// review barrier is what stops a note from being re-injected into every
/// subsequent generation for the rest of the task's life.
async fn outstanding_instruction(
    conn: &sea_orm::DatabaseConnection,
    task_id: i32,
) -> Option<Outstanding> {
    // Newest-first, and narrowed IN THE QUERY to the two kinds that can settle
    // the question. Scanning raw events would let a chatty run bury the answer:
    // `agent_progress` volume is up to the agent, so a few hundred milestones
    // would push the instruction past any limit. Filtered this way the limit
    // bounds a log of decisions, which is small.
    let events = work_task_service::recent_events_of_kinds(
        conn,
        task_id,
        &["user_action", "status_changed"],
        200,
    )
    .await
    .ok()?;
    for event in events {
        match event.kind.as_str() {
            "status_changed" => {
                let settled = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("to"))
                    .and_then(|v| v.as_str())
                    == Some("review");
                if settled {
                    return None;
                }
            }
            "user_action" => {
                let payload = match event.payload {
                    Some(p) => p,
                    None => continue,
                };
                let action = payload.get("action").and_then(|v| v.as_str())?;
                match action {
                    "return" => {
                        let intent = FollowUpIntent::from_wire(
                            payload.get("intent").and_then(|v| v.as_str()),
                        )
                        .unwrap_or_default();
                        let text = payload.get("feedback").and_then(|v| v.as_str())?;
                        return Some(Outstanding {
                            kind: OutstandingKind::Review(intent),
                            text: text.to_string(),
                            attachments: payload_blocks(&payload),
                        });
                    }
                    "retry" | "requeue" => {
                        let text = payload.get("note").and_then(|v| v.as_str())?;
                        return Some(Outstanding {
                            kind: OutstandingKind::Restart,
                            text: text.to_string(),
                            attachments: payload_blocks(&payload),
                        });
                    }
                    // Other user actions (delete, …) neither carry nor consume
                    // an instruction.
                    _ => continue,
                }
            }
            _ => continue,
        }
    }
    None
}

/// One-shot sink for the generation a launch actually operated on. The launch
/// fills it as soon as it has read the row; `spawn_launch` reads it afterwards
/// so a setup failure is attributed to the right generation.
#[derive(Default)]
struct LaunchSeq(std::sync::Mutex<Option<i32>>);

impl LaunchSeq {
    fn set(&self, run_seq: i32) {
        *self.0.lock().expect("launch seq mutex") = Some(run_seq);
    }

    fn get(&self) -> Option<i32> {
        *self.0.lock().expect("launch seq mutex")
    }
}

/// Ownership of a task's in-flight launch slot: which folder it counts
/// against, plus a token so only the launch that took the slot can release it.
struct LaunchOwner {
    folder_id: i32,
    token: u64,
}

/// A task's setup (init command) kill slot, open for the whole preparing phase.
///
/// The slot deliberately does NOT hold the child's pid. Only the task that
/// spawned the child ever kills it, so a pid can never be signalled after it
/// has been reaped (and possibly recycled by the OS onto an unrelated
/// process). A canceller just raises `kill_requested` and rings `wake`.
struct SetupChild {
    /// A cancel arrived: refuse to start an init command, and kill a live one.
    kill_requested: bool,
    /// Generation that owns the slot — a stale cancel must not stop a newer run.
    run_seq: i32,
    /// Rung by the canceller, awaited by whoever is running the child.
    wake: Arc<Notify>,
}

/// Marker file (in the worktree's PRIVATE git dir, so it can never show up in
/// `git status` or be committed) recording that the folder's init command
/// completed successfully in this worktree.
const SETUP_MARKER: &str = "codeg-task-init-ok";

async fn setup_marker_present(wt_path: &str) -> bool {
    match task_git::git_dir(wt_path).await {
        Ok(dir) => dir.join(SETUP_MARKER).exists(),
        // Can't resolve the git dir → assume not initialized. Re-running an
        // init command is wasteful at worst; skipping one is a broken tree.
        Err(_) => false,
    }
}

async fn write_setup_marker(wt_path: &str) {
    match task_git::git_dir(wt_path).await {
        Ok(dir) => {
            if let Err(e) = tokio::fs::write(dir.join(SETUP_MARKER), b"").await {
                tracing::warn!("[work_task] could not write the setup marker: {e}");
            }
        }
        Err(e) => tracing::warn!("[work_task] could not resolve the worktree git dir: {e}"),
    }
}

/// Best-effort kill of a child's whole process tree. Killing only the direct
/// child is not enough: the init command runs as `sh -c "<line>"`, and the real
/// work (`pnpm install` and its downloads) happens in descendants that would
/// otherwise keep running. Deliberately not `kill_on_drop`, which SIGKILLs the
/// shell first and reparents the descendants out of reach — same rationale as
/// `commands::office_tools::stream_install_or_kill_tree`.
async fn kill_process_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    if let Err(e) = kill_tree::tokio::kill_tree(pid).await {
        tracing::warn!("[work_task] kill_tree failed for setup pid {pid}: {e}");
    }
}

async fn still_expected(
    conn: &sea_orm::DatabaseConnection,
    task_id: i32,
    run_seq: i32,
    expected: WorkTaskStatus,
) -> bool {
    matches!(
        work_task_service::get_model(conn, task_id).await,
        Ok(t) if t.status == expected && t.run_seq == run_seq
    )
}

/// Head of the first text block of a prompt — the transcript viewer matches
/// user turns against it to place round dividers.
fn prompt_head(blocks: &[PromptInputBlock]) -> String {
    blocks
        .iter()
        .find_map(|b| match b {
            PromptInputBlock::Text { text } => Some(first_chars(text.trim(), 160)),
            _ => None,
        })
        .unwrap_or_default()
}

/// Run one shell command line in `cwd`, capturing combined output. stdin is
/// null (a command waiting on input sees EOF); no timeout by design — the
/// result write is generation-guarded, so a runaway command can only waste its
/// own process. Returns (exit code, trailing output capped to
/// `PREFLIGHT_TAIL_CHARS`).
async fn run_shell_capture(line: &str, cwd: &str) -> Result<(Option<i32>, String), String> {
    let out = shell_command(line, cwd)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    Ok(combine_capture(&out))
}

/// The platform's shell invocation for one command line, ready to `output()` or
/// `spawn()`. stdin is null so a command waiting on input sees EOF.
fn shell_command(line: &str, cwd: &str) -> tokio::process::Command {
    #[cfg(not(windows))]
    let mut command = {
        let mut c = crate::process::tokio_command("/bin/sh");
        c.arg("-c").arg(line);
        c
    };
    #[cfg(windows)]
    let mut command = {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut c = crate::process::tokio_command(comspec);
        c.arg("/C").arg(line);
        c
    };
    command.current_dir(cwd);
    command.stdin(std::process::Stdio::null());
    command
}

/// (exit code, trailing combined output) of a finished shell capture.
fn combine_capture(out: &std::process::Output) -> (Option<i32>, String) {
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    (
        out.status.code(),
        tail_chars(&combined, PREFLIGHT_TAIL_CHARS),
    )
}

fn tail_chars(s: &str, n: usize) -> String {
    let trimmed = s.trim_end();
    let count = trimmed.chars().count();
    if count <= n {
        return trimmed.to_string();
    }
    trimmed.chars().skip(count - n).collect()
}

fn parse_agent_type(s: &str) -> Result<AgentType, String> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| format!("unknown agent type: {s}"))
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Cap on a task-derived conversation title. Long enough for a real card name,
/// short enough that the sidebar row is not one giant ellipsis.
const CONVERSATION_TITLE_MAX_CHARS: usize = 80;

/// The title the session a task produces carries: the task's own name, capped.
/// It is LOCKED on the conversation row at launch (`conversation_service::
/// lock_title`), so a board card and the session it spawned always read the
/// same in the sidebar (issue #495).
///
/// One definition, because two callers must agree byte-for-byte: the launch-time
/// seed here, and the rename propagation in `commands::work_task`, which
/// recognises "still the task's name" by comparing against this exact value.
pub(crate) fn conversation_title_for_task(task_title: &str) -> String {
    first_chars(task_title.trim(), CONVERSATION_TITLE_MAX_CHARS)
}

/// The project folder's own name — what a task worktree directory is named
/// after. `Path` semantics rather than a `/` split, so `C:\src\repo` yields
/// `repo` too.
///
/// The answer is always RELATIVE, empty included: a path with no final
/// component (a filesystem root, or one ending in `..`) must not fall back to
/// the path itself, because the name is JOINED onto the configured worktree
/// root — and an absolute one replaces that root instead of landing under it.
fn basename(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
}

/// `name` placed next to the project folder — where task worktrees live when
/// the folder configures no root of its own. A project folder with no parent
/// (a drive or filesystem root) falls back to the bare name, which git then
/// resolves against the repository itself.
fn sibling_path(root_path: &str, name: &str) -> String {
    match Path::new(root_path).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            parent.join(name).to_string_lossy().into_owned()
        }
        _ => name.to_string(),
    }
}

/// Where the task worktree directory `name` goes for a project at
/// `root_path`. `worktree_root` is the folder setting: blank (or absent) keeps
/// the historical layout — right next to the project folder — and any other
/// value becomes the directory every new worktree of that folder is created
/// in. A typed path is taken at face value in the two ways a user expects: `~`
/// is the home directory, and something relative hangs off the project folder.
fn worktree_path_in(root_path: &str, worktree_root: Option<&str>, name: &str) -> String {
    worktree_path_in_home(root_path, worktree_root, name, dirs::home_dir().as_deref())
}

/// [`worktree_path_in`] with the home directory injected — `dirs::home_dir()`
/// reads the real user's home even under a pinned `HOME`, so tests must hand
/// one in rather than set the environment.
fn worktree_path_in_home(
    root_path: &str,
    worktree_root: Option<&str>,
    name: &str,
    home: Option<&Path>,
) -> String {
    let Some(configured) = worktree_root.map(str::trim).filter(|r| !r.is_empty()) else {
        return sibling_path(root_path, name);
    };
    let base = expand_home(configured, home);
    let base = if base.is_absolute() {
        base
    } else {
        Path::new(root_path).join(base)
    };
    base.join(name).to_string_lossy().into_owned()
}

/// Expand a leading `~` against `home`. Left as-is when the home directory is
/// unknown, so the path still resolves as literally written instead of
/// silently becoming a different directory.
fn expand_home(path: &str, home: Option<&Path>) -> PathBuf {
    let Some(home) = home else {
        return PathBuf::from(path);
    };
    if path == "~" {
        return home.to_path_buf();
    }
    match path
        .strip_prefix("~/")
        .or_else(|| path.strip_prefix("~\\"))
    {
        Some(rest) => home.join(rest),
        None => PathBuf::from(path),
    }
}

/// codeg-mcp `task_progress` / `task_complete` access handed to the delegation
/// listener at boot. Resolves the process-global engine at CALL time — the
/// listener is constructed before the engine, and a process that never wins the
/// engine lock cleanly rejects every report.
pub struct EngineWorkTaskTools;

#[async_trait::async_trait]
impl WorkTaskToolAccess for EngineWorkTaskTools {
    async fn report_progress(&self, parent_connection_id: &str, message: &str) -> TaskReportAck {
        let Some(engine) = engine() else {
            return TaskReportAck::rejected("no task engine running in this process");
        };
        engine.record_progress(parent_connection_id, message).await
    }

    async fn complete(
        &self,
        parent_connection_id: &str,
        verdict: &str,
        summary: Option<&str>,
    ) -> TaskReportAck {
        let Some(engine) = engine() else {
            return TaskReportAck::rejected("no task engine running in this process");
        };
        engine
            .record_complete(parent_connection_id, verdict, summary)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows has no absolute path without a drive, and the resolver branches
    /// on `is_absolute` — so the fixtures need a prefix that makes them count
    /// as absolute on both platforms.
    #[cfg(windows)]
    const ABS_PREFIX: &str = "C:";
    #[cfg(not(windows))]
    const ABS_PREFIX: &str = "";

    #[test]
    fn worktree_names_carry_ids() {
        assert_eq!(basename("/home/me/repo"), "repo");
        assert_eq!(basename("/home/me/repo/"), "repo");
        assert_eq!(
            sibling_path("/home/me/repo", "repo-task-7"),
            Path::new("/home/me")
                .join("repo-task-7")
                .to_string_lossy()
                .into_owned()
        );
        // No parent to be a sibling of: the bare name, which git resolves
        // against the repository it runs in.
        assert_eq!(sibling_path("repo", "repo-task-7"), "repo-task-7");
    }

    /// The derived directory name is joined ONTO the configured root, so it
    /// has to stay relative even for the paths that have no final component
    /// to borrow: a filesystem root, and anything ending in `..`. An absolute
    /// name would replace the configured root instead of landing under it —
    /// silently putting the worktree somewhere the user never chose.
    #[test]
    fn a_project_without_a_name_still_lands_under_the_configured_root() {
        let trees = format!("{ABS_PREFIX}/var/worktrees");
        for root in [format!("{ABS_PREFIX}/"), format!("{ABS_PREFIX}/home/me/..")] {
            let dir = format!("{}-task-7", basename(&root));
            assert!(
                !Path::new(&dir).is_absolute(),
                "the derived name must be relative, got {dir:?} for root {root:?}"
            );
            assert_eq!(
                worktree_path_in_home(&root, Some(&trees), &dir, None),
                Path::new(&trees).join(&dir).to_string_lossy().into_owned(),
                "root {root:?} must not escape the configured worktree directory"
            );
        }
    }

    /// Where a fresh task worktree lands. The default layout is load-bearing:
    /// every worktree already on disk was minted next to its project folder,
    /// so an unset (or blanked) setting has to keep producing exactly that
    /// path — the recreate-from-branch fallback looks for it by name.
    #[test]
    fn a_blank_worktree_root_keeps_worktrees_next_to_the_project() {
        let root = format!("{ABS_PREFIX}/home/me/repo");
        let expected = Path::new(&format!("{ABS_PREFIX}/home/me"))
            .join("repo-task-7")
            .to_string_lossy()
            .into_owned();
        for setting in [None, Some(""), Some("   ")] {
            assert_eq!(
                worktree_path_in_home(&root, setting, "repo-task-7", None),
                expected,
                "setting {setting:?} must not move the worktree"
            );
        }
    }

    /// A configured directory holds the worktree DIRECTORY, not the checkout
    /// itself — two tasks of the same folder have to be able to share it.
    #[test]
    fn a_configured_root_holds_one_directory_per_task() {
        let root = format!("{ABS_PREFIX}/home/me/repo");
        let trees = format!("{ABS_PREFIX}/var/worktrees");
        // Compared as paths, not strings: a separator the setting carried in
        // survives into the result — `Path::join` reuses a trailing one rather
        // than adding its own — and on Windows `/` and `\` are the same
        // separator. Only the directory the worktree lands in is pinned here,
        // never the spelling of the separators.
        let at = |name: &str| Path::new(&trees).join(name);
        let landed = |setting: &str, name: &str| {
            PathBuf::from(worktree_path_in_home(&root, Some(setting), name, None))
        };
        assert_eq!(landed(&trees, "repo-task-7"), at("repo-task-7"));
        assert_eq!(landed(&trees, "repo-task-8"), at("repo-task-8"));
        // Typed paths arrive with stray whitespace and trailing separators.
        assert_eq!(
            landed(&format!("  {trees}/  "), "repo-task-7"),
            at("repo-task-7")
        );
    }

    /// The two shorthands a typed path is expected to honour: `~` is the home
    /// directory, and a relative path hangs off the project folder. An unknown
    /// home leaves `~` alone rather than guessing a different directory.
    #[test]
    fn a_typed_root_expands_home_and_resolves_relatives() {
        let root = format!("{ABS_PREFIX}/home/me/repo");
        let home = PathBuf::from(format!("{ABS_PREFIX}/home/me"));
        assert_eq!(
            worktree_path_in_home(&root, Some("~/trees"), "repo-task-7", Some(&home)),
            home.join("trees")
                .join("repo-task-7")
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(
            worktree_path_in_home(&root, Some("~"), "repo-task-7", Some(&home)),
            home.join("repo-task-7").to_string_lossy().into_owned()
        );
        assert_eq!(
            worktree_path_in_home(&root, Some(".worktrees"), "repo-task-7", Some(&home)),
            Path::new(&root)
                .join(".worktrees")
                .join("repo-task-7")
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(
            worktree_path_in_home(&root, Some("~/trees"), "repo-task-7", None),
            Path::new(&root)
                .join("~/trees")
                .join("repo-task-7")
                .to_string_lossy()
                .into_owned(),
            "an unknown home must not silently relocate the worktree"
        );
    }

    /// Native Windows paths, where the naming and the join have to agree: a
    /// project folder whose "name" came back as the whole path (`C:\src\repo`)
    /// makes `<root>/<name>` an absolute join that REPLACES the configured
    /// root, so every setting would silently resolve back to the sibling
    /// layout. Nothing here is exotic — it is what the OS folder picker hands
    /// back on Windows.
    #[cfg(windows)]
    #[test]
    fn a_windows_project_path_still_honours_the_configured_root() {
        // Expectations are built with `Path::join` so they pin the DIRECTORY
        // the worktree lands in without also asserting separator spelling.
        let at = |dir: &str| {
            Path::new(dir)
                .join("repo-task-7")
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(basename("C:\\src\\repo"), "repo");
        assert_eq!(
            worktree_path_in_home("C:\\src\\repo", Some("D:\\worktrees"), "repo-task-7", None),
            at("D:\\worktrees"),
            "a configured root on another drive must not collapse to the sibling layout"
        );
        assert_eq!(
            worktree_path_in_home("C:\\src\\repo", Some(".worktrees"), "repo-task-7", None),
            at("C:\\src\\repo\\.worktrees")
        );
        assert_eq!(
            worktree_path_in_home("C:\\src\\repo", None, "repo-task-7", None),
            at("C:\\src"),
            "the default layout is unchanged on Windows"
        );
        assert_eq!(
            worktree_path_in_home(
                "C:\\src\\repo",
                Some("~\\trees"),
                "repo-task-7",
                Some(Path::new("C:\\Users\\me"))
            ),
            at("C:\\Users\\me\\trees")
        );
        // A UNC project folder keeps its share prefix.
        assert_eq!(
            sibling_path("\\\\server\\share\\repo", "repo-task-7"),
            at("\\\\server\\share")
        );
    }

    #[test]
    fn prompt_head_takes_the_first_text_block() {
        let blocks = vec![PromptInputBlock::Text {
            text: "  Fix the login flow and add tests.  ".to_string(),
        }];
        assert_eq!(prompt_head(&blocks), "Fix the login flow and add tests.");
        assert_eq!(prompt_head(&[]), "");
    }

    /// The sweep's row-level gate: exactly the tasks whose merge button the
    /// board would show — and whose light is green — land unattended.
    #[test]
    fn auto_merge_candidate_mirrors_the_board() {
        let settings = WorkTaskFolderSettings::default();
        let mut task = task_row();
        task.status = WorkTaskStatus::Review;
        assert!(auto_merge_candidate(&task, &settings));

        // Unknown stats merge (the UI defaults to the merge button there); an
        // empty change set is the complete button's territory.
        task.files_changed = Some(3);
        assert!(auto_merge_candidate(&task, &settings));
        task.files_changed = Some(0);
        assert!(!auto_merge_candidate(&task, &settings));
        task.files_changed = None;

        // An earlier failed or refused merge waits for the user.
        task.last_error = Some("merge dispatch failed: conflict".into());
        assert!(!auto_merge_candidate(&task, &settings));
        task.last_error = None;

        // A merge the user queued belongs to the queue's own drain, with THEIR
        // commit message and worktree choice — the unattended dispatch has
        // neither, so it must not take this row (a drain that skipped it after
        // losing a race would otherwise hand it straight to the sweep).
        task.pending_merge = Some(
            serde_json::to_string(&WorkTaskQueuedMerge {
                message: Some("feat: land it".into()),
                delete_worktree: false,
                queued_at: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            })
            .unwrap(),
        );
        assert!(!auto_merge_candidate(&task, &settings));
        // Deliberately the same predicate `begin_merge`'s auto CAS uses —
        // "anything in the column" rather than "a parseable intent". A gate
        // looser than the CAS that finally decides would dispatch into a miss
        // on every sweep; a manual merge clears the column either way.
        task.pending_merge = Some(r#"{"legacy":true}"#.into());
        assert!(!auto_merge_candidate(&task, &settings));
        task.pending_merge = None;

        // Only review is mergeable.
        task.status = WorkTaskStatus::Running;
        assert!(!auto_merge_candidate(&task, &settings));
        task.status = WorkTaskStatus::Review;

        // A configured preflight gates on a green light — absent, running and
        // red all wait. A blank custom command is no gate; a legacy id-only
        // reference is one.
        let gated = WorkTaskFolderSettings {
            preflight_command: Some("pnpm test".into()),
            ..Default::default()
        };
        assert!(!auto_merge_candidate(&task, &gated));
        for status in ["running", "failed"] {
            task.preflight =
                Some(format!(r#"{{"status":"{status}","command":"pnpm test"}}"#));
            assert!(!auto_merge_candidate(&task, &gated), "{status} light");
        }
        task.preflight = Some(r#"{"status":"passed","command":"pnpm test"}"#.into());
        assert!(auto_merge_candidate(&task, &gated));
        assert!(!preflight_configured(&WorkTaskFolderSettings {
            preflight_command: Some("  ".into()),
            ..Default::default()
        }));
        assert!(preflight_configured(&WorkTaskFolderSettings {
            preflight_command_id: Some(4),
            ..Default::default()
        }));
    }

    /// The refusal wordings that mean "lost a race" are skipped silently by
    /// the sweep; the strings live in `merge_task`, and this pin keeps the
    /// match and the wording from drifting apart.
    #[test]
    fn benign_merge_races_are_recognized() {
        assert!(is_benign_merge_race("task is not in review"));
        assert!(is_benign_merge_race(
            "task left review before the merge began"
        ));
        // The queue's own CAS miss: the row moved on (a follow-up, a stop)
        // while its queued dispatch waited for the folder lock.
        assert!(is_benign_merge_race(
            "task left review before the merge was queued"
        ));
        // A withdrawal / edit under the pump is benign too, but it is the one
        // the drain must RE-READ for instead of stepping over: the user's new
        // word may sort first, and leaving the slot to the auto sweep would
        // land it with the unattended defaults.
        let superseded = missed_queue_cas(Some(&QueuedMergeClaim {
            raw: "{}".to_string(),
            queued_at: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
        }));
        assert!(is_benign_merge_race(&superseded));
        assert!(is_queued_merge_superseded(&superseded));
        assert!(!is_queued_merge_superseded("task is not in review"));
        assert!(!is_queued_merge_superseded(&missed_queue_cas(None)));
        assert!(is_benign_merge_race(
            "another task of this project is already merging — wait for it"
        ));
        assert!(!is_benign_merge_race(
            "project folder is on 'feature', expected 'main' — switch back to merge"
        ));
        assert!(!is_benign_merge_race(
            "the task worktree no longer exists on disk"
        ));
    }

    /// The merge queue is first-asked-first-served, with the task id breaking a
    /// tie — the same rule `mergeQueueRanks` draws on the cards, so the "第 2
    /// 位" a user reads is the order they actually land in.
    #[test]
    fn the_merge_queue_drains_oldest_request_first() {
        let intent = |secs: i64| WorkTaskQueuedMerge {
            message: None,
            delete_worktree: true,
            queued_at: chrono::DateTime::from_timestamp(1_800_000_000 + secs, 0)
                .expect("valid instant"),
        };
        let row = |id: i32| crate::db::entities::work_task::Model {
            id,
            ..task_row()
        };

        let mut queue = [
            (intent(2), row(1)),
            (intent(1), row(3)),
            // Same instant as task 3 — id decides, exactly as the client does.
            (intent(1), row(2)),
        ];
        queue.sort_by_key(|(intent, task)| queue_order(intent, task));
        assert_eq!(
            queue.iter().map(|(_, t)| t.id).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
    }

    /// Minimal task row for prompt composition (nothing here touches the DB).
    fn task_row() -> crate::db::entities::work_task::Model {
        let now = chrono::Utc::now();
        crate::db::entities::work_task::Model {
            id: 7,
            folder_id: 1,
            title: "Fix the login flow".to_string(),
            config: "{}".to_string(),
            status: WorkTaskStatus::Queued,
            failure_reason: None,
            last_error: None,
            run_seq: 1,
            sort_order: 0,
            worktree_folder_id: None,
            conversation_id: None,
            connection_id: None,
            base_branch: Some("main".to_string()),
            base_sha: None,
            work_branch: Some("task/7".to_string()),
            merge_state: None,
            pending_merge: None,
            cleanup_state: None,
            verdict: None,
            result_summary: None,
            files_changed: None,
            additions: None,
            deletions: None,
            merge_commit: None,
            preflight: None,
            archived_at: None,
            scheduled_at: None,
            created_at: now,
            updated_at: now,
            started_at: None,
            settled_at: None,
            finished_at: None,
            deleted_at: None,
        }
    }

    fn task_config() -> WorkTaskConfig {
        WorkTaskConfig {
            prompt_blocks: vec![serde_json::json!({
                "type": "text",
                "text": "Fix the login flow and add tests."
            })],
            ..Default::default()
        }
    }

    fn settings_with(pairs: &[(&str, &str)]) -> WorkTaskFolderSettings {
        WorkTaskFolderSettings {
            stage_prompts: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    fn texts(blocks: &[PromptInputBlock]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                PromptInputBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn merge_mode() -> LaunchMode {
        LaunchMode::Merge {
            root_path: "/repo".to_string(),
            base_branch: "main".to_string(),
            work_branch: "task/7".to_string(),
            strategy: "squash".to_string(),
            message: None,
        }
    }

    fn return_mode(intent: FollowUpIntent) -> LaunchMode {
        LaunchMode::Return {
            intent,
            feedback: "please fix the copy".to_string(),
            attachments: Vec::new(),
        }
    }

    /// Insert a task row so events have something to hang off, then compose.
    async fn seeded_task(conn: &sea_orm::DatabaseConnection) -> i32 {
        use sea_orm::{ActiveModelTrait, Set};
        let now = chrono::Utc::now();
        let row = crate::db::entities::work_task::ActiveModel {
            folder_id: Set(1),
            title: Set("Fix the login flow".to_string()),
            config: Set("{}".to_string()),
            status: Set(WorkTaskStatus::Failed),
            run_seq: Set(1),
            sort_order: Set(0),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };
        row.insert(conn).await.expect("insert task").id
    }

    async fn user_action(conn: &sea_orm::DatabaseConnection, id: i32, payload: serde_json::Value) {
        work_task_service::record_event(conn, id, "user_action", "user", Some(payload))
            .await
            .expect("record user action");
    }

    async fn settled_into_review(conn: &sea_orm::DatabaseConnection, id: i32) {
        work_task_service::record_event(
            conn,
            id,
            "status_changed",
            "engine",
            Some(serde_json::json!({ "to": "review" })),
        )
        .await
        .expect("record settle");
    }

    /// Each scenario reframes the SAME user text — that is the whole point of
    /// having scenarios, and `revise` must stay byte-identical to the wording
    /// the action had before they existed.
    #[tokio::test]
    async fn follow_up_scenarios_reframe_the_same_feedback() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let cases = [
            (
                FollowUpIntent::Revise,
                "The user reviewed your work on this task and returned it with the following \
                 feedback. Address it in this same worktree:\n\nplease fix the copy",
            ),
            (FollowUpIntent::Continue, "accepted it as it stands"),
            (FollowUpIntent::Question, "This is a question, not a work order"),
            (FollowUpIntent::Verify, "read the full diff of this worktree"),
        ];
        for (intent, expected) in cases {
            let blocks = compose_prompt(
                &task_config(),
                &task_row(),
                &return_mode(intent),
                &WorkTaskFolderSettings::default(),
                true,
                &db.conn,
            )
            .await
            .expect("compose");
            let joined = texts(&blocks).join("\n");
            assert!(
                joined.contains(expected),
                "{}: missing its own framing in {joined}",
                intent.as_str()
            );
            assert!(
                joined.contains("please fix the copy"),
                "{}: dropped the user's text",
                intent.as_str()
            );
        }
    }

    /// The standing guard licenses committing, and it is the LAST block the
    /// agent reads — so a question turn has to replace it, not merely say
    /// "don't edit" a few blocks earlier.
    #[tokio::test]
    async fn a_question_turn_withdraws_the_licence_to_commit() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let blocks = compose_prompt(
            &task_config(),
            &task_row(),
            &return_mode(FollowUpIntent::Question),
            &WorkTaskFolderSettings::default(),
            true,
            &db.conn,
        )
        .await
        .expect("compose");
        let guard = texts(&blocks)
            .into_iter()
            .find(|t| t.starts_with("—— Work task context ——"))
            .expect("guard block");
        assert!(!guard.contains("Commit to the current branch as you like"));
        assert!(guard.contains("do NOT create, edit, delete or commit any file"));
        // The base-branch rules survive the swap.
        assert!(guard.contains("push the base branch"));

        // …and a working scenario keeps the original licence.
        let working = compose_prompt(
            &task_config(),
            &task_row(),
            &return_mode(FollowUpIntent::Revise),
            &WorkTaskFolderSettings::default(),
            true,
            &db.conn,
        )
        .await
        .expect("compose");
        assert!(texts(&working)
            .iter()
            .any(|t| t.contains("Commit to the current branch as you like")));
    }

    /// The one scenario that stands alone without user text.
    #[tokio::test]
    async fn a_self_check_composes_without_any_user_text() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let blocks = compose_prompt(
            &task_config(),
            &task_row(),
            &LaunchMode::Return {
                intent: FollowUpIntent::Verify,
                feedback: String::new(),
                attachments: Vec::new(),
            },
            &WorkTaskFolderSettings::default(),
            true,
            &db.conn,
        )
        .await
        .expect("compose");
        let joined = texts(&blocks).join("\n");
        assert!(joined.contains("read the full diff of this worktree"));
        assert!(!joined.contains("What they want you to pay attention to"));
    }

    /// A retry replays what the interrupted generation still owed — and an
    /// unanswered question must not come back as a work order.
    #[tokio::test]
    async fn a_retry_replays_an_unanswered_question_as_a_question() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let id = seeded_task(&db.conn).await;
        user_action(
            &db.conn,
            id,
            serde_json::json!({
                "action": "return", "intent": "question", "feedback": "why the extra table?",
            }),
        )
        .await;

        let mut row = task_row();
        row.id = id;
        let blocks = compose_prompt(
            &task_config(),
            &row,
            &LaunchMode::Retry,
            &WorkTaskFolderSettings::default(),
            true,
            &db.conn,
        )
        .await
        .expect("compose");
        let joined = texts(&blocks).join("\n");
        assert!(joined.contains("never got an answer"));
        assert!(joined.contains("why the extra table?"));
        assert!(!joined.contains("Latest review feedback to address"));
    }

    /// The lookup is a barrier, not a filter: skipping past the newest
    /// instruction would resurrect an older one the agent already carried out.
    #[tokio::test]
    async fn a_newer_instruction_hides_an_older_one() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let id = seeded_task(&db.conn).await;
        user_action(
            &db.conn,
            id,
            serde_json::json!({
                "action": "return", "intent": "revise", "feedback": "rename the column",
            }),
        )
        .await;
        settled_into_review(&db.conn, id).await;
        user_action(
            &db.conn,
            id,
            serde_json::json!({
                "action": "return", "intent": "question", "feedback": "why the extra table?",
            }),
        )
        .await;

        let outstanding = outstanding_instruction(&db.conn, id)
            .await
            .expect("an outstanding instruction");
        assert_eq!(
            outstanding.kind,
            OutstandingKind::Review(FollowUpIntent::Question)
        );
        assert_eq!(outstanding.text, "why the extra table?");
    }

    /// A completed turn consumes the instruction it carried, so it stops being
    /// re-injected into every generation for the rest of the task's life.
    #[tokio::test]
    async fn settling_into_review_consumes_the_instruction() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let id = seeded_task(&db.conn).await;
        user_action(
            &db.conn,
            id,
            serde_json::json!({ "action": "requeue", "note": "install deps first" }),
        )
        .await;
        assert!(outstanding_instruction(&db.conn, id).await.is_some());

        settled_into_review(&db.conn, id).await;
        assert!(outstanding_instruction(&db.conn, id).await.is_none());
    }

    /// A task canceled before it ever had a session comes back through `Fresh`,
    /// so the note the user attached has to reach that arm too.
    #[tokio::test]
    async fn a_restart_note_reaches_a_fresh_launch() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let id = seeded_task(&db.conn).await;
        user_action(
            &db.conn,
            id,
            serde_json::json!({ "action": "requeue", "note": "target the v2 API this time" }),
        )
        .await;

        let mut row = task_row();
        row.id = id;
        let blocks = compose_prompt(
            &task_config(),
            &row,
            &LaunchMode::Fresh,
            &WorkTaskFolderSettings::default(),
            false,
            &db.conn,
        )
        .await
        .expect("compose");
        let joined = texts(&blocks).join("\n");
        assert!(joined.contains("target the v2 API this time"));
        assert!(joined.contains("The user restarted this task"));
        // The task's own brief still opens the prompt, so the transcript's
        // phase divider keeps matching on it.
        assert_eq!(prompt_head(&blocks), "Fix the login flow and add tests.");
    }

    /// A screenshot pasted into the follow-up box has to reach the agent as an
    /// image block, right behind the sentence that framed it — dropping it
    /// would leave the framing pointing at nothing.
    #[tokio::test]
    async fn a_follow_up_carries_its_attachments_after_the_framing() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let blocks = compose_prompt(
            &task_config(),
            &task_row(),
            &LaunchMode::Return {
                intent: FollowUpIntent::Revise,
                feedback: "the header is wrong, see this".to_string(),
                attachments: vec![
                    serde_json::json!({
                        "type": "image", "data": "aGk=", "mime_type": "image/png", "uri": null,
                    }),
                    // A block that no longer deserializes is dropped, not fatal:
                    // one bad attachment must not stop the run carrying the rest.
                    serde_json::json!({ "type": "not_a_block" }),
                ],
            },
            &WorkTaskFolderSettings::default(),
            true,
            &db.conn,
        )
        .await
        .expect("compose");
        // The framing sentence, then the image it refers to, then the worktree
        // guard every prompt ends with.
        let framing = blocks
            .iter()
            .position(
                |b| matches!(b, PromptInputBlock::Text { text } if text.contains("the header is wrong, see this")),
            )
            .expect("the feedback is in there");
        assert!(
            matches!(
                blocks.get(framing + 1),
                Some(PromptInputBlock::Image { data, .. }) if data == "aGk="
            ),
            "the image follows the framing text, unparseable blocks aside: {blocks:?}"
        );
        assert_eq!(
            blocks
                .iter()
                .filter(|b| matches!(b, PromptInputBlock::Image { .. }))
                .count(),
            1
        );
    }

    /// The same attachments have to survive the replay path: a run interrupted
    /// before it answered owes the user the screenshot as well as the sentence.
    #[tokio::test]
    async fn an_outstanding_instruction_replays_its_attachments() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let id = seeded_task(&db.conn).await;
        user_action(
            &db.conn,
            id,
            serde_json::json!({
                "action": "retry",
                "note": "it looked like this",
                "blocks": [
                    { "type": "image", "data": "aGk=", "mime_type": "image/png", "uri": null },
                ],
            }),
        )
        .await;

        let outstanding = outstanding_instruction(&db.conn, id)
            .await
            .expect("an outstanding instruction");
        assert_eq!(outstanding.attachments.len(), 1);

        let mut row = task_row();
        row.id = id;
        let blocks = compose_prompt(
            &task_config(),
            &row,
            &LaunchMode::Retry,
            &WorkTaskFolderSettings::default(),
            true,
            &db.conn,
        )
        .await
        .expect("compose");
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, PromptInputBlock::Image { data, .. } if data == "aGk=")),
            "the replayed instruction keeps its image: {blocks:?}"
        );
    }

    /// A task's image blocks are stored, so the encoding the composer chose can
    /// be stale by the time the run happens (a slow probe then, a different
    /// agent now). Dispatch re-encodes for whoever actually answered.
    #[test]
    fn images_are_reencoded_for_the_agent_that_answered() {
        let caps = |image, embedded_context| PromptCapabilitiesInfo {
            image,
            audio: false,
            embedded_context,
        };
        let image = || PromptInputBlock::Image {
            data: "aGk=".to_string(),
            mime_type: "image/png".to_string(),
            uri: None,
        };
        let embedded = || PromptInputBlock::Resource {
            uri: "file:///shot.png".to_string(),
            mime_type: Some("image/png".to_string()),
            text: None,
            blob: Some("aGk=".to_string()),
        };

        // Native image → embedded blob for an agent that takes only the latter,
        // with a stable synthetic uri for a path-less screenshot.
        let mut blocks = vec![PromptInputBlock::Text { text: "see".into() }, image()];
        reencode_images(&mut blocks, &caps(false, true));
        assert!(
            matches!(
                &blocks[1],
                PromptInputBlock::Resource { uri, mime_type, text: None, blob: Some(b) }
                    if uri == "clipboard://work-task-image-1"
                        && mime_type.as_deref() == Some("image/png")
                        && b == "aGk="
            ),
            "{:?}",
            blocks[1]
        );
        // The prose beside it is untouched.
        assert!(matches!(&blocks[0], PromptInputBlock::Text { text } if text == "see"));

        // …and back, for an agent that takes images but not embedded context.
        let mut blocks = vec![embedded()];
        reencode_images(&mut blocks, &caps(true, false));
        assert!(
            matches!(
                &blocks[0],
                PromptInputBlock::Image { data, mime_type, uri: Some(u) }
                    if data == "aGk=" && mime_type == "image/png" && u == "file:///shot.png"
            ),
            "{:?}",
            blocks[0]
        );

        // An agent that takes both, or neither, gets exactly what was stored:
        // there is no better shape to reach for in either case.
        for c in [caps(true, true), caps(false, false)] {
            let mut blocks = vec![image(), embedded()];
            reencode_images(&mut blocks, &c);
            assert!(matches!(&blocks[0], PromptInputBlock::Image { uri: None, .. }));
            assert!(matches!(&blocks[1], PromptInputBlock::Resource { blob: Some(_), .. }));
        }

        // A non-image embedded resource (a pasted text file) is never turned
        // into an image, whatever the agent accepts.
        let mut blocks = vec![PromptInputBlock::Resource {
            uri: "clipboard://notes".to_string(),
            mime_type: Some("text/markdown".to_string()),
            text: None,
            blob: Some("aGk=".to_string()),
        }];
        reencode_images(&mut blocks, &caps(true, false));
        assert!(matches!(
            &blocks[0],
            PromptInputBlock::Resource { mime_type, .. }
                if mime_type.as_deref() == Some("text/markdown")
        ));
    }

    /// An event written before follow-ups could carry attachments has no
    /// `blocks` field at all — that has to read as "nothing was attached".
    #[tokio::test]
    async fn a_legacy_instruction_without_blocks_still_replays() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let id = seeded_task(&db.conn).await;
        user_action(
            &db.conn,
            id,
            serde_json::json!({ "action": "return", "intent": "revise", "feedback": "redo it" }),
        )
        .await;

        let outstanding = outstanding_instruction(&db.conn, id)
            .await
            .expect("an outstanding instruction");
        assert_eq!(outstanding.text, "redo it");
        assert!(outstanding.attachments.is_empty());
    }

    /// Two ways a busy task could hide its own instruction: `list_events`
    /// returns the OLDEST rows within its limit, and an agent's progress
    /// milestones are unbounded in number, so scanning raw events would bury
    /// the instruction under them.
    #[tokio::test]
    async fn a_chatty_run_cannot_bury_the_latest_instruction() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let id = seeded_task(&db.conn).await;
        user_action(
            &db.conn,
            id,
            serde_json::json!({ "action": "retry", "note": "the DB was locked" }),
        )
        .await;
        for _ in 0..520 {
            work_task_service::record_event(&db.conn, id, "agent_progress", "agent", None)
                .await
                .expect("filler event");
        }

        let outstanding = outstanding_instruction(&db.conn, id)
            .await
            .expect("found under the filler");
        assert_eq!(outstanding.kind, OutstandingKind::Restart);
        assert_eq!(outstanding.text, "the DB was locked");
    }

    /// A follow-up recorded by `claim_for_run_with_action` must be readable the
    /// moment the claim commits — a pump can launch the generation right after.
    #[tokio::test]
    async fn a_claim_carries_its_instruction_atomically() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let id = seeded_task(&db.conn).await;
        let seq = work_task_service::claim_for_run_with_action(
            &db.conn,
            id,
            WorkTaskStatus::Failed,
            "user",
            Some(serde_json::json!({ "action": "retry", "note": "install deps first" })),
        )
        .await
        .expect("claim")
        .expect("claim won");
        assert_eq!(seq, 2);

        let outstanding = outstanding_instruction(&db.conn, id)
            .await
            .expect("instruction visible right after the claim");
        assert_eq!(outstanding.text, "install deps first");

        // A LOST claim must not leave an instruction behind for some later
        // generation to pick up.
        let lost = work_task_service::claim_for_run_with_action(
            &db.conn,
            id,
            WorkTaskStatus::Failed,
            "user",
            Some(serde_json::json!({ "action": "retry", "note": "orphan" })),
        )
        .await
        .expect("claim");
        assert!(lost.is_none());
        assert_eq!(
            outstanding_instruction(&db.conn, id).await.unwrap().text,
            "install deps first"
        );
    }

    #[tokio::test]
    async fn stage_prompts_land_after_the_built_in_guard() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let settings = settings_with(&[("all", "Reply in Chinese."), ("work", "Run pnpm test.")]);
        let blocks = compose_prompt(
            &task_config(),
            &task_row(),
            &LaunchMode::Fresh,
            &settings,
            false,
            &db.conn,
        )
        .await
        .expect("compose");

        let texts = texts(&blocks);
        // …task blocks…, worktree guard, then the user's own instructions.
        assert!(texts[texts.len() - 2].starts_with("—— Work task context ——"));
        let last = texts.last().expect("trailing block");
        assert!(last.starts_with("—— Additional instructions ——"));
        // "all" first, then the stage's own text.
        let all_at = last.find("Reply in Chinese.").expect("all text");
        let work_at = last.find("Run pnpm test.").expect("stage text");
        assert!(all_at < work_at);
        // The round marker still keys off the task's own opening line, so the
        // transcript's phase dividers are unaffected.
        assert_eq!(prompt_head(&blocks), "Fix the login flow and add tests.");
    }

    #[tokio::test]
    async fn stage_prompts_select_only_their_own_stage() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let settings = settings_with(&[
            ("all", "EVERY-STAGE"),
            ("work", "WORK-ONLY"),
            ("retry", "RETRY-ONLY"),
            ("return", "RETURN-ONLY"),
            ("merge", "MERGE-ONLY"),
        ]);
        let modes = [
            (LaunchMode::Fresh, "WORK-ONLY"),
            (LaunchMode::Retry, "RETRY-ONLY"),
            (return_mode(FollowUpIntent::Revise), "RETURN-ONLY"),
            // Every scenario shares the `return` stage, so the settings dialog
            // stays four stages wide however many scenarios exist.
            (return_mode(FollowUpIntent::Continue), "RETURN-ONLY"),
            (return_mode(FollowUpIntent::Question), "RETURN-ONLY"),
            (return_mode(FollowUpIntent::Verify), "RETURN-ONLY"),
            (merge_mode(), "MERGE-ONLY"),
        ];
        for (mode, expected) in modes {
            let blocks = compose_prompt(
                &task_config(),
                &task_row(),
                &mode,
                &settings,
                false,
                &db.conn,
            )
            .await
            .expect("compose");
            let joined = texts(&blocks).join("\n");
            assert!(joined.contains("EVERY-STAGE"), "{expected}: missing all-stage text");
            assert!(joined.contains(expected), "{expected}: missing own text");
            for other in ["WORK-ONLY", "RETRY-ONLY", "RETURN-ONLY", "MERGE-ONLY"] {
                if other != expected {
                    assert!(!joined.contains(other), "{expected}: leaked {other}");
                }
            }
        }
    }

    #[tokio::test]
    async fn merge_stage_keeps_its_extra_without_the_guard() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let settings = settings_with(&[("merge", "Write the commit message in Chinese.")]);
        let blocks = compose_prompt(
            &task_config(),
            &task_row(),
            &merge_mode(),
            &settings,
            true,
            &db.conn,
        )
        .await
        .expect("compose");

        let texts = texts(&blocks);
        // The merge generation replaces the guard (it forbids exactly what a
        // merge must do) — the user's extra still trails it.
        assert!(texts.iter().all(|t| !t.starts_with("—— Work task context ——")));
        assert!(texts[0].contains("land it onto the base branch"));
        assert!(texts
            .last()
            .expect("trailing block")
            .contains("Write the commit message in Chinese."));
    }

    #[tokio::test]
    async fn blank_stage_prompts_add_nothing() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let bare = compose_prompt(
            &task_config(),
            &task_row(),
            &LaunchMode::Fresh,
            &WorkTaskFolderSettings::default(),
            false,
            &db.conn,
        )
        .await
        .expect("compose");
        let blank = compose_prompt(
            &task_config(),
            &task_row(),
            &LaunchMode::Fresh,
            &settings_with(&[("all", "  \n "), ("work", "")]),
            false,
            &db.conn,
        )
        .await
        .expect("compose");
        assert_eq!(texts(&bare), texts(&blank));
    }

    /// The worktree is off disk; everything below is what the clients see.
    /// Without the `folder://changed` delete the removed worktree keeps
    /// rendering in every sidebar until the next full `fetchFolders`.
    #[tokio::test]
    async fn worktree_removal_announces_the_dropped_folder() {
        use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
        use crate::web::event_bridge::WebEventBroadcaster;

        let db = fresh_in_memory_db().await;
        let root_id = seed_folder(&db, "/tmp/repo").await;
        let wt = open_worktree_folder_core(&db, "/tmp/repo-task-1".to_string(), root_id)
            .await
            .expect("worktree folder");
        let conv = conversation_service::create(&db.conn, wt.id, AgentType::ClaudeCode, None, None)
            .await
            .expect("conversation");
        let task = work_task_service::create(
            &db.conn,
            crate::models::WorkTaskDraft {
                folder_id: root_id,
                title: "fix login".to_string(),
                config: serde_json::json!({
                    "display_text": "fix login",
                    "prompt_blocks": [{ "type": "text", "text": "fix login" }],
                }),
            },
        )
        .await
        .expect("task");
        work_task_service::attach_worktree(&db.conn, task.id, wt.id, "main", "abc", "task/1")
            .await
            .expect("attach");

        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let mut rx = broadcaster.subscribe();
        let emitter = EventEmitter::test_web_only(broadcaster.clone());

        converge_worktree_removal(&db, &emitter, task.id, wt.id, root_id, &wt.path).await;

        // The folder row is gone from every list a client can fetch…
        assert!(
            crate::db::service::folder_service::get_folder_by_id(&db.conn, wt.id)
                .await
                .expect("lookup")
                .is_none()
        );
        // …its conversation moved to the project folder (stamped with where it ran)…
        let moved = conversation_service::get_by_id(&db.conn, conv.id)
            .await
            .expect("conversation row");
        assert_eq!(moved.folder_id, root_id);
        assert_eq!(moved.origin_cwd.as_deref(), Some("/tmp/repo-task-1"));
        // …and the task no longer points at it.
        let after = work_task_service::get_model(&db.conn, task.id)
            .await
            .expect("task row");
        assert_eq!(after.worktree_folder_id, None);

        // Conversations first (that refetch is what re-places the moved rows),
        // then the folder drop.
        let bulk = rx.try_recv().expect("bulk change should broadcast");
        assert_eq!(
            bulk.channel,
            crate::web::event_bridge::CONVERSATIONS_BULK_CHANGED_EVENT
        );
        let dropped = rx.try_recv().expect("folder delete should broadcast");
        assert_eq!(
            dropped.channel,
            crate::web::event_bridge::FOLDER_CHANGED_EVENT
        );
        assert_eq!(dropped.payload["kind"], "deleted");
        assert_eq!(dropped.payload["id"], wt.id);
    }

    /// A retried cleanup finds the row already soft-deleted. It must still
    /// announce the drop — a client that missed the first broadcast is the
    /// reason the user retried.
    #[tokio::test]
    async fn worktree_removal_re_announces_an_already_deleted_folder() {
        use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
        use crate::web::event_bridge::WebEventBroadcaster;

        let db = fresh_in_memory_db().await;
        let root_id = seed_folder(&db, "/tmp/repo2").await;
        let task = work_task_service::create(
            &db.conn,
            crate::models::WorkTaskDraft {
                folder_id: root_id,
                title: "fix login".to_string(),
                config: serde_json::json!({
                    "display_text": "fix login",
                    "prompt_blocks": [{ "type": "text", "text": "fix login" }],
                }),
            },
        )
        .await
        .expect("task");

        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let mut rx = broadcaster.subscribe();
        let emitter = EventEmitter::test_web_only(broadcaster.clone());

        // Folder id 9999 never existed — the "already gone" branch.
        converge_worktree_removal(&db, &emitter, task.id, 9999, root_id, "/tmp/repo2-task-1").await;

        let _bulk = rx.try_recv().expect("bulk change should broadcast");
        let dropped = rx.try_recv().expect("folder delete should broadcast");
        assert_eq!(dropped.payload["kind"], "deleted");
        assert_eq!(dropped.payload["id"], 9999);
    }

    #[test]
    fn engine_lock_is_per_data_dir() {
        let dir = tempfile::tempdir().expect("temp dir");
        let guard = match acquire_engine_ownership(dir.path()) {
            Ownership::Exclusive(f) => f,
            _ => panic!("first acquisition should win"),
        };
        assert!(matches!(
            acquire_engine_ownership(dir.path()),
            Ownership::Taken
        ));
        // Independent of the automation engine's lock file in the same dir.
        let automation_lock = dir
            .path()
            .join(format!("{}.lock", crate::db::database_file_name()));
        assert!(!automation_lock.exists());
        drop(guard);
    }

    // -- blocking prompts raised by a delegation sub-agent (#447) -----------

    const PARENT_CONN: &str = "conn-task";
    const CHILD_CONN: &str = "conn-child";

    /// A task driven to `running` on `PARENT_CONN`, with the engine's live
    /// index seeded as the launch path would. Returns `(engine, task_id)`.
    async fn running_task() -> (Arc<TaskEngine>, i32) {
        use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};

        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/task-deleg").await;
        let conv =
            conversation_service::create(&db.conn, folder_id, AgentType::ClaudeCode, None, None)
                .await
                .expect("conversation");
        let task = work_task_service::create(
            &db.conn,
            crate::models::WorkTaskDraft {
                folder_id,
                title: "fix login".to_string(),
                config: serde_json::json!({
                    "display_text": "fix login",
                    "prompt_blocks": [{ "type": "text", "text": "fix login" }],
                }),
            },
        )
        .await
        .expect("task");
        // Walk the real transition chain rather than writing the row directly,
        // so the run_seq the engine flips against is the one the CAS minted.
        let run_seq = work_task_service::claim_for_run(
            &db.conn,
            task.id,
            WorkTaskStatus::Todo,
            "test",
        )
        .await
        .expect("claim")
        .expect("claimed");
        assert!(work_task_service::begin_setup(&db.conn, task.id, run_seq)
            .await
            .expect("begin_setup"));
        assert!(
            work_task_service::mark_running(&db.conn, task.id, run_seq, conv.id, PARENT_CONN)
                .await
                .expect("mark_running")
        );

        let engine = test_engine(db);
        engine
            .index
            .lock()
            .await
            .insert(PARENT_CONN.into(), (task.id, run_seq));
        (engine, task.id)
    }

    async fn status_of(engine: &TaskEngine, task_id: i32) -> WorkTaskStatus {
        work_task_service::get_model(&engine.db.conn, task_id)
            .await
            .expect("task row")
            .status
    }

    fn env(conn_id: &str, payload: AcpEvent) -> EventEnvelope {
        EventEnvelope {
            seq: 0,
            connection_id: conn_id.to_string(),
            payload,
        }
    }

    fn delegation_started(parent: &str, child: &str) -> EventEnvelope {
        env(
            parent,
            AcpEvent::DelegationStarted {
                parent_connection_id: parent.into(),
                parent_tool_use_id: "tu-1".into(),
                child_connection_id: child.into(),
                child_conversation_id: 1,
                agent_type: AgentType::Codex,
                task_preview: "run the tests".into(),
                task_id: "task-1".into(),
            },
        )
    }

    fn delegation_completed(parent: &str, child: &str) -> EventEnvelope {
        env(
            parent,
            AcpEvent::DelegationCompleted {
                parent_connection_id: parent.into(),
                parent_tool_use_id: "tu-1".into(),
                child_connection_id: child.into(),
                child_conversation_id: 1,
                agent_type: AgentType::Codex,
                result: crate::acp::types::DelegationResultSummary::Ok {
                    duration_ms: 0,
                    text_preview: None,
                },
            },
        )
    }

    fn permission_request(conn: &str, request_id: &str) -> EventEnvelope {
        env(
            conn,
            AcpEvent::PermissionRequest {
                request_id: request_id.into(),
                tool_call: serde_json::json!({}),
                options: vec![],
                queued: 0,
            },
        )
    }

    #[tokio::test]
    async fn a_delegated_child_permission_flips_the_task_to_awaiting_input() {
        // #447: the prompt arrives on the CHILD's connection, which is not in
        // `index`. Before the parent lookup existed it was dropped outright and
        // the board kept saying "running" for a run that was parked on the user.
        let (engine, task_id) = running_task().await;
        engine.on_event(&delegation_started(PARENT_CONN, CHILD_CONN)).await;
        engine.on_event(&permission_request(CHILD_CONN, "r1")).await;

        assert_eq!(status_of(&engine, task_id).await, WorkTaskStatus::AwaitingInput);

        engine
            .on_event(&env(
                CHILD_CONN,
                AcpEvent::PermissionResolved {
                    request_id: "r1".into(),
                },
            ))
            .await;
        assert_eq!(status_of(&engine, task_id).await, WorkTaskStatus::Running);
    }

    #[tokio::test]
    async fn a_child_torn_down_mid_permission_does_not_wedge_awaiting_input() {
        // The failure this guards: a sub-agent cancelled/crashed while its
        // permission was still pending leaves a key nothing can ever resolve,
        // pinning the row at awaiting_input for the rest of the run.
        let (engine, task_id) = running_task().await;
        engine.on_event(&delegation_started(PARENT_CONN, CHILD_CONN)).await;
        engine.on_event(&permission_request(CHILD_CONN, "r1")).await;
        assert_eq!(status_of(&engine, task_id).await, WorkTaskStatus::AwaitingInput);

        engine.on_event(&delegation_completed(PARENT_CONN, CHILD_CONN)).await;
        assert_eq!(
            status_of(&engine, task_id).await,
            WorkTaskStatus::Running,
            "an unanswerable request must not outlive its sub-agent"
        );
        assert!(engine.delegation_parents.lock().await.is_empty());
    }

    #[tokio::test]
    async fn a_child_permission_does_not_clear_the_parents_own_block() {
        // Both connections' keys live in one set, so they must be independently
        // namespaced — otherwise resolving one would flip the row back while
        // the other is still waiting.
        let (engine, task_id) = running_task().await;
        engine.on_event(&delegation_started(PARENT_CONN, CHILD_CONN)).await;
        engine.on_event(&permission_request(PARENT_CONN, "r1")).await;
        // Same request_id on the child: a plausible collision, since the two
        // agents mint ids independently.
        engine.on_event(&permission_request(CHILD_CONN, "r1")).await;

        engine
            .on_event(&env(
                CHILD_CONN,
                AcpEvent::PermissionResolved {
                    request_id: "r1".into(),
                },
            ))
            .await;
        assert_eq!(
            status_of(&engine, task_id).await,
            WorkTaskStatus::AwaitingInput,
            "the parent's own permission is still outstanding"
        );

        engine
            .on_event(&env(
                PARENT_CONN,
                AcpEvent::PermissionResolved {
                    request_id: "r1".into(),
                },
            ))
            .await;
        assert_eq!(status_of(&engine, task_id).await, WorkTaskStatus::Running);
    }

    #[tokio::test]
    async fn a_permission_racing_ahead_of_delegation_started_is_recovered() {
        // The broker starts the child's turn BEFORE announcing the delegation
        // (`send_prompt_linked_for_delegation` precedes `emit_started_if_real`),
        // so a child that blocks immediately raises its prompt while the engine
        // still has no mapping for it — and drops it. Without the backfill the
        // board sits at `running` for a run already parked on the user, which is
        // the very bug #447 is about.
        use crate::acp::session_state::PendingPermissionState;
        use crate::models::agent::AgentType;
        use crate::web::event_bridge::EventEmitter;

        let (engine, task_id) = running_task().await;
        engine
            .manager
            .insert_test_connection(CHILD_CONN, AgentType::Codex, None, EventEmitter::Noop)
            .await;

        // Permission arrives first and is dropped (no mapping yet).
        engine.on_event(&permission_request(CHILD_CONN, "r1")).await;
        assert_eq!(status_of(&engine, task_id).await, WorkTaskStatus::Running);

        // The child's live state carries it regardless — that is what the
        // backfill reads when the announcement finally lands.
        {
            let state = engine.manager.get_state(CHILD_CONN).await.expect("state");
            let mut s = state.write().await;
            s.pending_permission = Some(PendingPermissionState {
                request_id: "r1".into(),
                tool_call_id: "tc-1".into(),
                tool_call: serde_json::json!({}),
                options: vec![],
                created_at: chrono::Utc::now(),
                queued: 0,
            });
        }
        engine.on_event(&delegation_started(PARENT_CONN, CHILD_CONN)).await;
        assert_eq!(
            status_of(&engine, task_id).await,
            WorkTaskStatus::AwaitingInput
        );

        // The recovered key matches what a normal resolve retracts — otherwise
        // the row would wedge at awaiting_input.
        engine
            .on_event(&env(
                CHILD_CONN,
                AcpEvent::PermissionResolved {
                    request_id: "r1".into(),
                },
            ))
            .await;
        assert_eq!(status_of(&engine, task_id).await, WorkTaskStatus::Running);
    }

    #[tokio::test]
    async fn a_nested_delegation_child_still_maps_to_the_run() {
        // A sub-agent that delegates further blocks the task just as much, so
        // the grandchild has to resolve through the chain rather than requiring
        // its parent to be the run's own connection.
        let (engine, task_id) = running_task().await;
        engine.on_event(&delegation_started(PARENT_CONN, CHILD_CONN)).await;
        engine.on_event(&delegation_started(CHILD_CONN, "conn-grandchild")).await;

        engine
            .on_event(&permission_request("conn-grandchild", "r1"))
            .await;
        assert_eq!(
            status_of(&engine, task_id).await,
            WorkTaskStatus::AwaitingInput
        );
    }

    #[tokio::test]
    async fn completing_a_child_retracts_its_whole_subtree() {
        // The middle child finishing takes its own delegations down with it, so
        // a grandchild's unanswered prompt must be retracted too — otherwise
        // the row wedges at awaiting_input on a key nothing can resolve, and
        // the grandchild's mapping is stranded in the map forever.
        let (engine, task_id) = running_task().await;
        engine.on_event(&delegation_started(PARENT_CONN, CHILD_CONN)).await;
        engine.on_event(&delegation_started(CHILD_CONN, "conn-grandchild")).await;
        engine
            .on_event(&permission_request("conn-grandchild", "r1"))
            .await;
        assert_eq!(
            status_of(&engine, task_id).await,
            WorkTaskStatus::AwaitingInput
        );

        engine.on_event(&delegation_completed(PARENT_CONN, CHILD_CONN)).await;
        assert_eq!(status_of(&engine, task_id).await, WorkTaskStatus::Running);
        assert!(
            engine.delegation_parents.lock().await.is_empty(),
            "the grandchild's mapping must not be stranded"
        );
    }

    #[tokio::test]
    async fn delegations_from_a_non_task_connection_are_not_tracked() {
        // A delegation from an ordinary chat tab has no board row to flip; the
        // map must not grow for it.
        let (engine, _task_id) = running_task().await;
        engine.on_event(&delegation_started("conn-chat", "conn-other-child")).await;
        assert!(engine.delegation_parents.lock().await.is_empty());
    }
}
