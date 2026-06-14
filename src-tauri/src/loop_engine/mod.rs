//! Loop engineering engine: drives each running issue through triage → refine →
//! design → plan → implement → verify → review → finalize, autonomously.
//!
//! The engine holds cheap clones of the shared runtime handles (database,
//! connection manager, event emitter) plus a per-issue driver registry. A
//! single instance lives per process — desktop manages it as Tauri state, the
//! web/server `AppState` reuses (or builds) the same `Arc<LoopEngine>` — so a
//! trigger from either entry point drives the same drivers.
//!
//! State is DB-authoritative (§4.1a dispatch leases); the in-process registry
//! below is only a single-instance guard, never the concurrency authority.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tokio::sync::{broadcast, Mutex, Notify};
use tokio::task::AbortHandle;

use crate::acp::internal_bus::InternalEventBus;
use crate::acp::manager::ConnectionManager;
use crate::acp::types::AcpEvent;
use crate::db::entities::loop_iteration::{self, IterationStatus};
use crate::db::AppDatabase;
use crate::web::event_bridge::EventEmitter;

pub mod actions;
pub mod briefing;
pub mod config_resolver;
pub mod dispatch;
pub mod driver;
pub mod error;
pub mod gates;
pub mod ingest;
pub mod questions;
pub mod recovery;
pub mod transitions;
pub mod validation;
pub mod worktree;

pub use error::LoopError;

/// Registry entry for a running per-issue driver task. `abort` tears the task
/// down on stop/cancel; `wake` nudges it to re-tick without polling.
pub struct DriverHandle {
    pub abort: AbortHandle,
    pub wake: Arc<Notify>,
}

/// The loop engineering engine. Cheaply shareable via `Arc`; all fields are
/// either `Arc`-backed handles or cloned connection refs.
pub struct LoopEngine {
    // Read by dispatch / driver / recovery / worktree.
    db: AppDatabase,
    manager: ConnectionManager,
    data_dir: PathBuf,
    emitter: EventEmitter,
    /// Process-internal single-instance guard: at most one driver task per
    /// issue. NOT the concurrency authority — that is the DB dispatch lease.
    drivers: Mutex<HashMap<i32, DriverHandle>>,
    /// Per-repo merge serialization: two issues that share a base repo must not
    /// run their `--no-ff` landings concurrently (they would race on the base
    /// branch ref and working tree). Keyed by repo path; entries are created on
    /// demand and never removed (bounded by the number of distinct repos).
    merge_locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl LoopEngine {
    pub fn new(
        db: AppDatabase,
        manager: ConnectionManager,
        data_dir: PathBuf,
        emitter: EventEmitter,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            manager,
            data_dir,
            emitter,
            drivers: Mutex::new(HashMap::new()),
            merge_locks: Mutex::new(HashMap::new()),
        })
    }

    /// The merge lock for `repo_path`, created on first use. Held across an
    /// issue's entire merge so concurrent merges into the same repo serialize.
    pub(crate) async fn repo_merge_lock(&self, repo_path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.merge_locks.lock().await;
        Arc::clone(
            locks
                .entry(repo_path.to_path_buf())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    /// Start the per-issue driver task (no-op if one is already registered —
    /// the registry is the in-process single-instance guard). The task ticks,
    /// then parks on its wake `Notify` until a completion or external nudge.
    pub async fn start_issue(self: &Arc<Self>, issue_id: i32) {
        let mut drivers = self.drivers.lock().await;
        if drivers.contains_key(&issue_id) {
            return;
        }
        let wake = Arc::new(Notify::new());
        let engine = Arc::clone(self);
        let wake_for_task = Arc::clone(&wake);
        let join = tokio::spawn(async move {
            driver::run_driver(engine, issue_id, wake_for_task).await;
        });
        drivers.insert(
            issue_id,
            DriverHandle {
                abort: join.abort_handle(),
                wake,
            },
        );
    }

    /// Wake a running driver to re-tick after an iteration settles or a human
    /// action lands. No-op when the issue has no driver. `notify_one` buffers a
    /// permit, so a wake that races ahead of the driver's `notified().await` is
    /// not lost.
    pub async fn wake(&self, issue_id: i32) {
        let drivers = self.drivers.lock().await;
        if let Some(handle) = drivers.get(&issue_id) {
            handle.wake.notify_one();
        }
    }

    /// Stop a running driver and drop its registry entry.
    pub async fn stop_issue(&self, issue_id: i32) {
        let mut drivers = self.drivers.lock().await;
        if let Some(handle) = drivers.remove(&issue_id) {
            handle.abort.abort();
        }
    }

    /// Remove a driver's registry entry. Called by the driver task itself when
    /// it exits cleanly (issue left `running`); idempotent with `stop_issue`.
    pub(crate) async fn deregister_driver(&self, issue_id: i32) {
        self.drivers.lock().await.remove(&issue_id);
    }

    /// On boot, reconcile interrupted iterations and restart a driver for every
    /// still-`running` issue. Idempotent — safe on every process start,
    /// including a clean boot with nothing in flight. Reconciliation (releasing
    /// stale leases + restoring worktrees) is pure DB+git and lives in
    /// [`recovery`]; this wrapper only restarts the drivers it identifies.
    pub async fn recover_on_boot(self: &Arc<Self>) {
        match recovery::reconcile_on_boot(&self.db).await {
            Ok(running_ids) => {
                for issue_id in running_ids {
                    self.start_issue(issue_id).await;
                }
            }
            Err(e) => eprintln!("[loop] recover_on_boot failed: {e}"),
        }
    }

    /// Subscribe to the in-process event bus synchronously and return the
    /// completion-watcher loop future; the caller spawns it with the
    /// mode-appropriate spawner (`tauri::async_runtime::spawn` from the desktop
    /// `setup` hook, which runs outside any tokio runtime, or `tokio::spawn`
    /// under the server runtime). This is the engine's completion-awareness: a
    /// separate, additive bus subscriber (it never touches the delegation
    /// lifecycle path), settling + waking loop iterations as their turns
    /// complete, reacting only to loop conversations.
    ///
    /// `subscribe()` runs here, before the future is returned, so a
    /// `TurnComplete` emitted between this call and the first poll is buffered
    /// by the broadcast channel rather than dropped (subscribe-before-spawn).
    pub fn completion_watcher_task(
        self: &Arc<Self>,
        bus: Arc<InternalEventBus>,
    ) -> impl Future<Output = ()> + Send + 'static {
        let engine = Arc::clone(self);
        let mut rx = bus.subscribe();
        async move {
            loop {
                match rx.recv().await {
                    Ok(envelope) => match &envelope.payload {
                        AcpEvent::TurnComplete { .. } => {
                            engine.on_turn_complete(&envelope.connection_id).await;
                        }
                        // A loop iteration's agent asked the operator a question:
                        // surface it as a `question` inbox card. Cleared on the
                        // matching QuestionResolved.
                        AcpEvent::QuestionRequest {
                            question_id,
                            questions,
                        } => {
                            engine
                                .on_question_request(
                                    &envelope.connection_id,
                                    question_id,
                                    questions,
                                )
                                .await;
                        }
                        AcpEvent::QuestionResolved { question_id } => {
                            engine.on_question_resolved(question_id).await;
                        }
                        _ => {}
                    },
                    // Fell behind the broadcast buffer — keep going; a missed
                    // event is reconciled by crash recovery (Task 1.7).
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    /// Settle the loop iteration backing a just-completed connection's turn,
    /// then wake its issue driver to advance the DAG. No-op for any connection
    /// that isn't a running loop iteration (e.g. ordinary or delegation turns).
    pub async fn on_turn_complete(self: &Arc<Self>, connection_id: &str) {
        // Resolve the conversation backing this connection (in-memory, same as
        // the delegation lifecycle path).
        let Some((state, _)) = self.manager.get_state_and_emitter(connection_id).await else {
            // Connection state already gone (e.g. teardown raced this event); the
            // driver's periodic reconcile will settle the iteration instead.
            eprintln!("[loop][turn] no connection state for {connection_id} (already torn down?)");
            return;
        };
        let conversation_id = state.read().await.conversation_id;
        let Some(cid) = conversation_id else {
            eprintln!("[loop][turn] connection {connection_id} has no conversation_id");
            return;
        };
        // DB-authoritative: is this conversation a running loop iteration?
        let iter = match loop_iteration::Entity::find()
            .filter(loop_iteration::Column::ConversationId.eq(cid))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .one(&self.db.conn)
            .await
        {
            Ok(Some(it)) => it,
            // Not a running loop iteration (ordinary/delegation turn, or already
            // settled by the reconcile) — nothing to do.
            Ok(None) => return,
            Err(e) => {
                eprintln!("[loop] on_turn_complete iteration lookup failed: {e}");
                return;
            }
        };
        if let Err(e) = self.settle_iteration(iter.id).await {
            eprintln!("[loop] settle iteration {} failed: {e}", iter.id);
        } else {
            eprintln!(
                "[loop][turn] settled iteration {} (issue {})",
                iter.id, iter.issue_id
            );
        }
        self.wake(iter.issue_id).await;
    }

    /// Run the §4.3 seven-step dispatch for a single frontier decision. Returns
    /// `Ok(None)` when the dispatch lease was already held (lost the race). The
    /// driver (Task 1.6) chooses the [`dispatch::DispatchInput`]; this just
    /// executes it against the live connection manager.
    pub async fn dispatch_iteration(
        &self,
        input: dispatch::DispatchInput,
    ) -> Result<Option<dispatch::DispatchHandle>, LoopError> {
        dispatch::dispatch_iteration(
            &self.db,
            &self.data_dir,
            &self.manager,
            self.emitter.clone(),
            input,
        )
        .await
    }

    /// §4.9 settlement for a finished iteration (token accounting + success CAS
    /// + no-progress signal).
    pub async fn settle_iteration(
        &self,
        iteration_id: i32,
    ) -> Result<dispatch::SettleOutcome, LoopError> {
        dispatch::settle_iteration(&self.db, &self.emitter, iteration_id).await
    }
}
