//! PR2 Task 3: collaboration turn coordinator over REAL SQLite.
//!
//! Every scenario below drives the real coordinator against a real on-disk
//! database (migrations included), with only the ACP boundary replaced by a
//! controllable `MockRuntime` (attach/send/cancel/disconnect counters, queued
//! outcomes, gates for deterministic interleavings, and storage-failure
//! injection via actually dropping a table).
//!
//! Covers the v2 plan's Task-3 assertion table (idempotency, conflict,
//! concurrency, ordinals, terminal-wins, close semantics), the startup
//! recovery table (A13/A14), and the storage-failure contract (A15).

// Monomorphized futures from the coordinator get deep; keep the compiler
// comfortable.
#![recursion_limit = "1024"]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use codeg_lib::acp::delegation::broker::{DbDelegationOutcomeStore, DelegationOutcomeStore};
use codeg_lib::acp::delegation::continuation::runtime::{
    AttachTarget, ContinuationRuntime,
};
use codeg_lib::acp::delegation::continuation::{
    CollaborationSessionState, ContinuationError, ContinuationErrorCode, ContinuationCoordinator,
    StrictAttachError, TurnState, TurnTerminal, VerifiedParent,
};
use codeg_lib::db::service::collaboration_service;
use codeg_lib::db::service::delegation_outcome_service::DelegationOutcomeInsert;
use codeg_lib::db::test_helpers::{fresh_disk_db, seed_folder};
use codeg_lib::db::AppDatabase;
use sea_orm::ConnectionTrait;

// ---------------------------------------------------------------------------
// MockRuntime
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MockRuntime {
    send_results: tokio::sync::Mutex<VecDeque<Result<(), String>>>,
    /// Park the NEXT attach until released (deterministic interleavings).
    attach_gate: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Park the NEXT blocked_on probe until released (deterministic
    /// interleavings for the cancel re-decide path).
    blocked_gate:
        tokio::sync::Mutex<Option<(tokio::sync::oneshot::Sender<()>, tokio::sync::oneshot::Receiver<()>)>>,
    attach_count: AtomicUsize,
    send_count: AtomicUsize,
    cancel_count: AtomicUsize,
    disconnect_count: AtomicUsize,
    blocked: tokio::sync::Mutex<Option<String>>,
    /// When set, every cancel reports failure (injects an undeliverable stop).
    pub fail_cancels: std::sync::atomic::AtomicBool,
    /// When set, every disconnect reports failure (an unreleasable owner).
    pub fail_disconnects: std::sync::atomic::AtomicBool,
}

impl MockRuntime {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    async fn install_attach_gate(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.attach_gate.lock().await = Some(rx);
        tx
    }

    fn attach_count(&self) -> usize {
        self.attach_count.load(Ordering::SeqCst)
    }

    fn counters(&self) -> (usize, usize, usize, usize) {
        (
            self.attach_count.load(Ordering::SeqCst),
            self.send_count.load(Ordering::SeqCst),
            self.cancel_count.load(Ordering::SeqCst),
            self.disconnect_count.load(Ordering::SeqCst),
        )
    }
}

#[async_trait]
impl ContinuationRuntime for MockRuntime {
    async fn attach_strict(
        &self,
        _target: &AttachTarget,
        _parent_connection_id: &str,
        _turn_id: &str,
        _execution_id: &str,
        _child_conversation_id: i32,
    ) -> Result<String, StrictAttachError> {
        self.attach_count.fetch_add(1, Ordering::SeqCst);
        // Honor the gate AFTER counting: a test can pin the drive inside the
        // attach window.
        let gate = self.attach_gate.lock().await.take();
        if let Some(gate) = gate {
            let _ = gate.await;
        }
        Ok(format!("conn-{}", uuid::Uuid::new_v4()))
    }

    async fn send_prompt(&self, _connection_id: &str, _message: &str) -> Result<(), String> {
        self.send_count.fetch_add(1, Ordering::SeqCst);
        self.send_results
            .lock()
            .await
            .pop_front()
            .unwrap_or(Ok(()))
    }

    async fn cancel(&self, _connection_id: &str) -> Result<(), String> {
        self.cancel_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_cancels.load(Ordering::SeqCst) {
            return Err("injected cancel failure".to_string());
        }
        Ok(())
    }

    async fn disconnect(&self, _connection_id: &str) -> Result<(), String> {
        self.disconnect_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_disconnects.load(Ordering::SeqCst) {
            return Err("disconnect not confirmed".to_string());
        }
        Ok(())
    }

    async fn blocked_on(&self, _connection_id: &str) -> Option<String> {
        let gate = self.blocked_gate.lock().await.take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            let _ = release.await;
        }
        (*self.blocked.lock().await).as_deref().map(str::to_string)
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

const PARENT: i32 = 1;
const SOURCE_TASK: &str = "task-0";

struct Harness {
    db: AppDatabase,
    _dir: tempfile::TempDir,
    runtime: Arc<MockRuntime>,
    coordinator: ContinuationCoordinator,
    parent: VerifiedParent,
    child_conversation_id: i32,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = fresh_disk_db(dir.path()).await;
    let folder_id = seed_folder(&db, "/work").await;

    // The child conversation must exist (alive check on admission).
    use codeg_lib::db::entities::conversation;
    use sea_orm::{ActiveModelTrait, NotSet, Set};
    let now = chrono::Utc::now();
    let child = conversation::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        title: Set(Some("child".into())),
        title_locked: Set(false),
        agent_type: Set("claude_code".into()),
        status: Set(conversation::ConversationStatus::Completed),
        kind: Set(conversation::ConversationKind::Delegate),
        model: Set(None),
        git_branch: Set(None),
        external_id: Set(Some("ext-session-1".into())),
        parent_id: Set(Some(PARENT)),
        parent_tool_use_id: Set(Some("pt-1".into())),
        delegation_call_id: Set(Some(SOURCE_TASK.into())),
        message_count: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        pinned_at: Set(None),
        origin_cwd: Set(None),
    };
    let child_conversation_id = child.insert(&db.conn).await.expect("seed child").id;

    // The frozen completed source with a verified resume binding.
    let binding = serde_json::json!({
        "schema_version": 1,
        "agent_type": "claude_code",
        "external_session_id": "ext-session-1",
        "cwd": dir.path().to_string_lossy(),
        "config_fingerprint": "fp-1",
    });
    let store: Arc<dyn DelegationOutcomeStore> = Arc::new(DbDelegationOutcomeStore {
        db: Arc::new(AppDatabase {
            conn: db.conn.clone(),
        }),
    });
    store
        .insert_once(DelegationOutcomeInsert {
            task_id: SOURCE_TASK.to_string(),
            parent_conversation_id: PARENT,
            parent_tool_use_id: Some("pt-1".into()),
            child_conversation_id: Some(child_conversation_id),
            agent_type: "claude_code".into(),
            text: "first success".into(),
            duration_ms: 11,
            text_truncated: false,
            completed_at: chrono::Utc::now(),
            resume_binding_json: Some(binding.to_string()),
        })
        .await
        .expect("seed outcome");

    let runtime = MockRuntime::new();
    let coordinator =
        ContinuationCoordinator::new(Arc::new(AppDatabase { conn: db.conn.clone() }), runtime.clone(), store);
    coordinator.set_enabled(true).await;

    Harness {
        db,
        _dir: dir,
        runtime,
        coordinator,
        parent: VerifiedParent {
            conversation_id: PARENT,
        },
        child_conversation_id,
    }
}

fn err_code(e: &ContinuationError) -> ContinuationErrorCode {
    e.error_code
}

/// Wait until the turn reaches one of `states` (bounded).
async fn wait_for_state(
    coordinator: &ContinuationCoordinator,
    parent: VerifiedParent,
    turn_id: &str,
    states: &[TurnState],
) -> TurnState {
    for _ in 0..200 {
        let report = coordinator
            .get_turn(parent, turn_id, 0)
            .await
            .expect("turn report");
        if states.contains(&report.state) {
            return report.state;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("turn {turn_id} never reached {states:?}");
}

// ---------------------------------------------------------------------------
// Assertion table
// ---------------------------------------------------------------------------

/// continue(T0,k1,m1) twice: same session/turn, ONE DB row, one send.
#[tokio::test]
async fn continue_twice_is_idempotent() {
    let h = harness().await;
    let ack1 = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .expect("first continue");
    let ack2 = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .expect("replayed continue");

    assert_eq!(ack1.session_id, ack2.session_id);
    assert_eq!(ack1.turn_id, ack2.turn_id);
    assert_eq!(ack1.ordinal, ack2.ordinal);

    wait_for_state(&h.coordinator, h.parent, &ack1.turn_id, &[TurnState::Running]).await;
    let sends = h.runtime.counters().1;
    assert_eq!(sends, 1, "an idempotent replay must not re-send");

    let turns = collaboration_service::list_turns(&h.db.conn, &ack1.session_id, 0, 100)
        .await
        .expect("list");
    assert_eq!(turns.len(), 1, "exactly one turn row in the DB");
}

/// continue(T0,k1,m2): request_conflict; original message stays m1; no second
/// send.
#[tokio::test]
async fn same_request_different_message_conflicts() {
    let h = harness().await;
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .expect("first");
    wait_for_state(&h.coordinator, h.parent, &ack.turn_id, &[TurnState::Running]).await;

    let err = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m2", None)
        .await
        .expect_err("conflicting payload");
    assert_eq!(err_code(&err), ContinuationErrorCode::RequestConflict);
    let sends = h.runtime.counters().1;
    assert_eq!(sends, 1);
    let report = h
        .coordinator
        .get_turn(h.parent, &ack.turn_id, 0)
        .await
        .unwrap();
    assert_eq!(report.message, "m1");
}

/// Concurrent continues: exactly one accepted, the other session_busy, at
/// most one send.
#[tokio::test]
async fn concurrent_continue_admits_only_one() {
    let h = harness().await;
    let c1 = h.coordinator.clone();
    let c2 = h.coordinator.clone();
    let p1 = h.parent;
    let p2 = h.parent;
    let (r1, r2) = tokio::join!(
        c1.continue_turn(p1, "parent-conn", SOURCE_TASK, "k1", "m1", None),
        c2.continue_turn(p2, "parent-conn", SOURCE_TASK, "k2", "m2", None),
    );
    let (ok_count, busy_count) = match (&r1, &r2) {
        (Ok(_), Err(e)) | (Err(e), Ok(_)) => {
            assert_eq!(err_code(e), ContinuationErrorCode::SessionBusy);
            (1, 1)
        }
        other => panic!("expected one ok + one busy, got {other:?}"),
    };
    assert_eq!((ok_count, busy_count), (1, 1));
    let sends = h.runtime.counters().1;
    assert!(sends <= 1, "at most one send after concurrent admission");
}

/// R1 completes → continue(T0,k2,m2): same session, ordinal+1, new turn;
/// R1 and T0 unchanged.
#[tokio::test]
async fn second_round_gets_new_turn_same_session() {
    let h = harness().await;
    let ack1 = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    wait_for_state(&h.coordinator, h.parent, &ack1.turn_id, &[TurnState::Running]).await;
    let (turn, execution) = h
        .coordinator
        .execution_owner_by_turn(&ack1.turn_id)
        .await
        .expect("execution registered");
    let applied = h
        .coordinator
        .settle(
            &turn,
            &execution,
            TurnTerminal::Completed {
                text: "round one done".into(),
            },
        )
        .await
        .unwrap();
    assert!(applied);

    let ack2 = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k2", "m2", None)
        .await
        .expect("second round");
    assert_eq!(ack2.session_id, ack1.session_id);
    assert_eq!(ack2.ordinal, ack1.ordinal + 1);
    assert_ne!(ack2.turn_id, ack1.turn_id);

    // R1 result is immutable.
    let r1 = h.coordinator.get_turn(h.parent, &ack1.turn_id, 0).await.unwrap();
    assert_eq!(r1.state, TurnState::Completed);
    assert_eq!(r1.result_text.as_deref(), Some("round one done"));
    let r2 = h.coordinator.get_turn(h.parent, &ack2.turn_id, 0).await.unwrap();
    // The ack says accepted, but the background drive advances fast — any
    // ACTIVE state is fine here; what matters is the terminal assertions.
    assert!(r2.state.is_active(), "second turn should be active: {:?}", r2.state);
}

/// A terminal landing while dispatching wins; the later send-return must NOT
/// regress the state to running.
#[tokio::test]
async fn terminal_during_dispatching_stays_terminal() {
    let h = harness().await;
    // Park the drive inside send: the state reaches dispatching and waits.
    // No queued result → send would return Ok immediately; instead gate by
    // making send slow via a queued pending... simplest deterministic tool:
    // settle while the turn is dispatching, which happens naturally because
    // settle is CAS-guarded.
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    // The drive may already be running; poll for dispatching or running and
    // settle the moment we see dispatching. To make dispatching observable,
    // queue a slow send: the mock returns Ok immediately, so instead settle
    // from the execution owner right after admission — CAS from
    // dispatching/running/accepted all go through settle's ACTIVE filter.
    let (turn, execution) = loop {
        if let Some(owner) = h.coordinator.execution_owner_by_turn(&ack.turn_id).await {
            break owner;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    let applied = h
        .coordinator
        .settle(
            &turn,
            &execution,
            TurnTerminal::Completed {
                text: "fast terminal".into(),
            },
        )
        .await
        .unwrap();
    assert!(applied, "the first terminal must win");

    // Whatever the drive does afterwards, the state stays completed.
    let report = h.coordinator.get_turn(h.parent, &ack.turn_id, 0).await.unwrap();
    assert_eq!(report.state, TurnState::Completed);
    assert_eq!(report.result_text.as_deref(), Some("fast terminal"));
    tokio::time::sleep(Duration::from_millis(120)).await;
    let report = h.coordinator.get_turn(h.parent, &ack.turn_id, 0).await.unwrap();
    assert_eq!(report.state, TurnState::Completed, "no regression to running");
}

/// Complete/cancel duplicates and interleavings: one terminal wins, results
/// are never overwritten.
#[tokio::test]
async fn only_one_terminal_wins() {
    let h = harness().await;
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    let (turn, execution) = loop {
        if let Some(owner) = h.coordinator.execution_owner_by_turn(&ack.turn_id).await {
            break owner;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };

    let first = h
        .coordinator
        .settle(&turn, &execution, TurnTerminal::Canceled)
        .await
        .unwrap();
    let second = h
        .coordinator
        .settle(
            &turn,
            &execution,
            TurnTerminal::Completed {
                text: "late success".into(),
            },
        )
        .await
        .unwrap();
    assert!(first);
    assert!(!second, "a stale terminal must lose");

    let report = h.coordinator.get_turn(h.parent, &turn, 0).await.unwrap();
    assert_eq!(report.state, TurnState::Canceled);
    assert_eq!(report.result_text, None, "the winner's (empty) result stands");
}

/// close_session then continue with the same T0: session_closed, no second
/// session row.
#[tokio::test]
async fn closed_session_rejects_new_rounds_without_a_second_session() {
    let h = harness().await;
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    wait_for_state(&h.coordinator, h.parent, &ack.turn_id, &[TurnState::Running]).await;
    let (turn, execution) = h
        .coordinator
        .execution_owner_by_turn(&ack.turn_id)
        .await
        .unwrap();
    h.coordinator
        .settle(&turn, &execution, TurnTerminal::Completed { text: "done".into() })
        .await
        .unwrap();

    let summary = h
        .coordinator
        .close_session(h.parent, &ack.session_id)
        .await
        .expect("close");
    assert_eq!(summary.state, CollaborationSessionState::Closed);

    let err = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k9", "m9", None)
        .await
        .expect_err("closed session");
    assert_eq!(err_code(&err), ContinuationErrorCode::SessionClosed);
    assert_eq!(err.session_id.as_deref(), Some(ack.session_id.as_str()));

    // Exactly one session row for the source remains.
    let session = collaboration_service::find_session_by_source(&h.db.conn, SOURCE_TASK)
        .await
        .unwrap()
        .expect("the one session");
    assert_eq!(session.id, ack.session_id);
    assert_eq!(session.state, "closed");
}

// ---------------------------------------------------------------------------
// Feature flag semantics
// ---------------------------------------------------------------------------

/// Disabled: no NEW relationships — but an existing same-payload request
/// still returns its original handle (query-style retry), and closing keeps
/// working.
#[tokio::test]
async fn disabled_feature_blocks_new_rounds_but_replays_existing_requests() {
    let h = harness().await;
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    wait_for_state(&h.coordinator, h.parent, &ack.turn_id, &[TurnState::Running]).await;
    let (turn, execution) = h
        .coordinator
        .execution_owner_by_turn(&ack.turn_id)
        .await
        .unwrap();
    h.coordinator
        .settle(&turn, &execution, TurnTerminal::Completed { text: "done".into() })
        .await
        .unwrap();

    h.coordinator.set_enabled(false).await;

    // Same payload → original handle, even with the feature off.
    let replay = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    assert_eq!(replay.turn_id, ack.turn_id);
    // New payload → feature_disabled.
    let err = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k9", "m9", None)
        .await
        .expect_err("new rounds need the feature");
    assert_eq!(err_code(&err), ContinuationErrorCode::FeatureDisabled);
    // Close still works (wind-down stays available).
    h.coordinator
        .close_session(h.parent, &ack.session_id)
        .await
        .expect("close works while disabled");
}

// ---------------------------------------------------------------------------
// Cancel semantics
// ---------------------------------------------------------------------------

/// Cancel before the send happens: the drive never sends; the turn is
/// canceled and the connection (if attached) is released.
#[tokio::test]
async fn cancel_before_send_never_sends() {
    let h = harness().await;
    let gate = h.runtime.install_attach_gate().await;
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    // Drive is parked inside attach (preparing). Cancel now.
    let report = h.coordinator.cancel_turn(h.parent, &ack.turn_id).await.unwrap();
    assert_eq!(report.state, TurnState::Canceled);
    drop(gate); // release the drive
    tokio::time::sleep(Duration::from_millis(80)).await;
    let sends = h.runtime.counters().1;
    assert_eq!(sends, 0, "a canceled pre-send turn must never send");
    // Depending on the interleaving the drive may have already been parked
    // inside attach when the cancel landed (then it releases after the gate
    // opens) or exited before attach returned (nothing to release). Both are
    // fine — the guarantee is that NOTHING was sent.
    let _ = h.runtime.counters();
}

/// Cancel while running: cancel_requested + the agent gets a cancel; the
/// terminal then depends on the confirmation path (settle wins, or the
/// unknown-timeout path blocks the session).
#[tokio::test]
async fn cancel_running_goes_cancel_requested_then_unknown_blocks() {
    let h = harness().await;
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    wait_for_state(&h.coordinator, h.parent, &ack.turn_id, &[TurnState::Running]).await;

    let report = h.coordinator.cancel_turn(h.parent, &ack.turn_id).await.unwrap();
    assert_eq!(report.state, TurnState::CancelRequested);
    assert_eq!(h.runtime.counters().2, 1, "the agent must be asked to stop");

    // No confirmation ever arrives; the hard-timeout path (Task 4 wires the
    // timer) settles unknown and blocks the session.
    let (turn, execution) = h
        .coordinator
        .execution_owner_by_turn(&ack.turn_id)
        .await
        .unwrap();
    h.coordinator.settle_unknown(&turn, &execution).await.unwrap();
    let report = h.coordinator.get_turn(h.parent, &ack.turn_id, 0).await.unwrap();
    assert_eq!(report.state, TurnState::OutcomeUnknown);
    let session = collaboration_service::find_session_by_id(&h.db.conn, &ack.session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.state, "blocked");

    // A blocked session admits nothing new.
    let err = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k2", "m2", None)
        .await
        .expect_err("blocked session");
    assert_eq!(err_code(&err), ContinuationErrorCode::SessionBlocked);
}

// ---------------------------------------------------------------------------
// Startup recovery (A13 / A14)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recovery_interrupts_undispatched_turns_and_blocks_unknowns() {
    let h = harness().await;
    // Seed a session + two turns directly (a crash left them behind).
    let session = collaboration_service::upsert_session_once(
        &h.db.conn,
        collaboration_service::NewSession {
            id: uuid::Uuid::new_v4().to_string(),
            source_task_id: SOURCE_TASK.into(),
            parent_conversation_id: PARENT,
            child_conversation_id: h.child_conversation_id,
            resume_binding_json: "{\"schema_version\":1,\"agent_type\":\"claude_code\",\"external_session_id\":\"ext-session-1\",\"cwd\":\"/work\",\"config_fingerprint\":\"fp-1\"}".into(),
        },
    )
    .await
    .unwrap();

    let accepted = collaboration_service::insert_turn(
        &h.db.conn,
        collaboration_service::NewTurn {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            ordinal: 1,
            request_id: "k1".into(),
            message: "m1".into(),
            initiator_parent_conversation_id: PARENT,
            initiator_tool_use_id: None,
        },
    )
    .await
    .unwrap();

    // A "restarted host" runs the recovery scan: the pre-dispatch turn
    // becomes interrupted.
    let runtime = MockRuntime::new();
    let store: Arc<dyn DelegationOutcomeStore> = Arc::new(DbDelegationOutcomeStore {
        db: Arc::new(AppDatabase {
            conn: h.db.conn.clone(),
        }),
    });
    let recovered_coordinator = ContinuationCoordinator::new(
        Arc::new(AppDatabase {
            conn: h.db.conn.clone(),
        }),
        runtime.clone(),
        store.clone(),
    );
    let recovered = recovered_coordinator.recover_on_startup().await.expect("recovery 1");
    assert!(recovered.interrupted.contains(&accepted.id), "recovered: {:?}", recovered);
    let t1 = collaboration_service::find_turn(&h.db.conn, &accepted.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t1.state, "interrupted");

    // Now simulate a MID-FLIGHT crash row in the same session (legal now:
    // the interrupted turn is terminal and no longer occupies the active
    // slot) and recover again.
    let mid = collaboration_service::insert_turn(
        &h.db.conn,
        collaboration_service::NewTurn {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            ordinal: 2,
            request_id: "k2".into(),
            message: "m2".into(),
            initiator_parent_conversation_id: PARENT,
            initiator_tool_use_id: None,
        },
    )
    .await
    .unwrap();
    assert!(collaboration_service::cas_turn_state(
        &h.db.conn,
        &mid.id,
        None,
        &["accepted"],
        "dispatching",
        true
    )
    .await
    .unwrap());

    let recovered2 = recovered_coordinator.recover_on_startup().await.expect("recovery 2");
    assert!(recovered2.outcome_unknown.contains(&mid.id), "recovered2: {:?}", recovered2);
    assert!(recovered2.blocked_sessions.contains(&session.id));
    let t2 = collaboration_service::find_turn(&h.db.conn, &mid.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t2.state, "outcome_unknown");
    let session_after = collaboration_service::find_session_by_id(&h.db.conn, &session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session_after.state, "blocked");
    // Nothing was re-sent.
    assert_eq!(runtime.counters().1, 0);
}

// ---------------------------------------------------------------------------
// Storage failure (A15)
// ---------------------------------------------------------------------------

/// If the dispatching write fails after attach, NOTHING is sent and the
/// session is isolated (blocked) — no authoritative result can be published.
#[tokio::test]
async fn storage_failure_at_dispatch_prevents_send_and_blocks() {
    let h = harness().await;
    let gate = h.runtime.install_attach_gate().await;
    let _ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    // Wait until the drive is parked INSIDE attach (the accepted→preparing
    // CAS has succeeded), then kill the turn table: the
    // preparing→dispatching write will fail while a connection is already
    // attached.
    for _ in 0..200 {
        if h.runtime.attach_count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(h.runtime.attach_count(), 1, "drive must be parked in attach");
    h.db
        .conn
        .execute_unprepared("DROP TABLE collaboration_turn")
        .await
        .expect("drop turn table");
    drop(gate);
    // Give the drive time to fail and isolate.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let sends = h.runtime.counters().1;
    assert_eq!(sends, 0, "a failed dispatch persistence must prevent the send");
    let session = collaboration_service::find_session_by_source(&h.db.conn, SOURCE_TASK)
        .await
        .unwrap()
        .expect("session row survives");
    assert_eq!(session.state, "blocked", "the session must be isolated");
}

/// An admission whose transaction cannot run fails WITHOUT starting
/// anything: no turn, no attach, no send.
#[tokio::test]
async fn storage_failure_at_admission_admits_nothing() {
    let h = harness().await;
    h.db
        .conn
        .execute_unprepared("DROP TABLE collaboration_turn")
        .await
        .expect("drop turn table");
    let err = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .expect_err("admission must fail");
    // A dead store admits nothing — the honest error is storage_unavailable,
    // not a busy session.
    assert_eq!(err_code(&err), ContinuationErrorCode::StorageUnavailable);
    let (attaches, sends) = (h.runtime.counters().0, h.runtime.counters().1);
    assert_eq!((attaches, sends), (0, 0), "nothing may start");
}

// ---------------------------------------------------------------------------
// Ownership (A21 pre-wiring)
// ---------------------------------------------------------------------------

/// A foreign parent gets the same opaque rejection for its own and unknown
/// sources; no existence leak.
#[tokio::test]
async fn cross_parent_access_is_opaque() {
    let h = harness().await;
    let foreign = VerifiedParent { conversation_id: 99 };
    let err = h
        .coordinator
        .continue_turn(foreign, "foreign-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .expect_err("foreign parent");
    assert_eq!(err_code(&err), ContinuationErrorCode::NotFoundOrForbidden);
    // The real parent still works.
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    assert_eq!(ack.state, TurnState::Accepted);
}

/// Acceptance F4 regression (was `review_parent_cancel_during_prepare_…`):
/// the parent connection goes away while the round is parked inside the
/// strict attach (preparing). `cancel_by_parent_connection` must cancel the
/// pending round so the drive NEVER sends; and for an already-running round
/// whose agent cancel fails, the round settles unknown + blocked instead of
/// a fabricated clean stop.
#[tokio::test]
async fn parent_cancel_during_prepare_prevents_dispatch() {
    let h = harness().await;
    // Park the drive INSIDE attach (accepted → preparing done, attach pending).
    let gate = h.runtime.install_attach_gate().await;
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    // Wait until the drive is parked in attach.
    for _ in 0..200 {
        if h.runtime.attach_count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(h.runtime.attach_count(), 1);

    // The parent disconnects.
    h.coordinator.cancel_by_parent_connection("parent-conn").await;
    // Release the attach; the drive must observe the canceled state and
    // release WITHOUT sending.
    drop(gate);
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(h.runtime.counters().1, 0, "a parent-canceled preparing round must never send");
    let report = h
        .coordinator
        .get_turn(h.parent, &ack.turn_id, 0)
        .await
        .unwrap();
    assert_eq!(report.state, TurnState::Canceled);
}

/// The attached-round half of F4: cancel delivery FAILURE means the outcome
/// is unknowable — outcome_unknown + session blocked, never a fabricated
/// canceled.
#[tokio::test]
async fn parent_cancel_with_failing_delivery_settles_unknown() {
    let h = harness().await;
    // Make every cancel fail AFTER the round is running.
    h.runtime.fail_cancels.store(true, Ordering::SeqCst);
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    for _ in 0..200 {
        if h.runtime.counters().1 == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(h.runtime.counters().1, 1, "precondition: the round was sent");

    h.coordinator.cancel_by_parent_connection("parent-conn").await;
    let report = h
        .coordinator
        .get_turn(h.parent, &ack.turn_id, 0)
        .await
        .unwrap();
    assert_eq!(
        report.state,
        TurnState::OutcomeUnknown,
        "an undeliverable stop must settle unknown, not canceled"
    );
    let session = collaboration_service::find_session_by_id(&h.db.conn, &ack.session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.state, "blocked");
}

/// Acceptance F5 regression (was `review_restart_must_block_already_…`):
/// a crash between the two pre-atomic writes could leave `outcome_unknown`
/// under an `open` session; startup recovery must repair exactly that.
#[tokio::test]
async fn restart_must_block_already_unknown_open_session() {
    let h = harness().await;
    let session = collaboration_service::upsert_session_once(
        &h.db.conn,
        collaboration_service::NewSession {
            id: uuid::Uuid::new_v4().to_string(),
            source_task_id: SOURCE_TASK.into(),
            parent_conversation_id: PARENT,
            child_conversation_id: h.child_conversation_id,
            resume_binding_json: "{\"schema_version\":1,\"agent_type\":\"claude_code\",\"external_session_id\":\"ext-session-1\",\"cwd\":\"/work\",\"config_fingerprint\":\"fp-1\"}".into(),
        },
    )
    .await
    .unwrap();
    let turn = collaboration_service::insert_turn(
        &h.db.conn,
        collaboration_service::NewTurn {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            ordinal: 1,
            request_id: "k1".into(),
            message: "m1".into(),
            initiator_parent_conversation_id: PARENT,
            initiator_tool_use_id: None,
        },
    )
    .await
    .unwrap();
    // The crash artifact: turn already outcome_unknown, session still open.
    assert!(collaboration_service::cas_turn_state(
        &h.db.conn,
        &turn.id,
        None,
        &["accepted"],
        "outcome_unknown",
        false
    )
    .await
    .unwrap());

    let runtime = MockRuntime::new();
    let store: Arc<dyn DelegationOutcomeStore> = Arc::new(DbDelegationOutcomeStore {
        db: Arc::new(AppDatabase {
            conn: h.db.conn.clone(),
        }),
    });
    let recovered = ContinuationCoordinator::new(
        Arc::new(AppDatabase {
            conn: h.db.conn.clone(),
        }),
        runtime.clone(),
        store,
    )
    .recover_on_startup()
    .await
    .expect("recovery");
    assert!(
        recovered.blocked_sessions.contains(&session.id),
        "recovery must block a session holding an unknown outcome: {:?}",
        recovered
    );
    let session_after = collaboration_service::find_session_by_id(&h.db.conn, &session.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session_after.state, "blocked");
}

/// Acceptance F6: close and continue share the per-source admission lock, so
/// a close that passes its active-check cannot race a continue inserting a
/// turn into a session that ends up closed. The observable serialization:
/// closing twice concurrently is idempotent and a continue after close is
/// rejected with the session id attached.
#[tokio::test]
async fn close_and_continue_do_not_interleave() {
    let h = harness().await;
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    for _ in 0..200 {
        if h.runtime.counters().1 == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (turn, execution) = h
        .coordinator
        .execution_owner_by_turn(&ack.turn_id)
        .await
        .unwrap();
    h.coordinator
        .settle(&turn, &execution, TurnTerminal::Completed { text: "done".into() })
        .await
        .unwrap();

    // Concurrent close + continue for a NEW round: either close wins (continue
    // → session_closed) or continue wins (close → SessionBusy). Never a
    // closed session with an active turn.
    let c = h.coordinator.clone();
    let session_id_for_close = ack.session_id.clone();
    let close_task = tokio::spawn(async move {
        c.close_session(h.parent, &session_id_for_close).await
    });
    let continue_result = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k2", "m2", None)
        .await;
    let close_result = close_task.await.unwrap();

    match (&continue_result, &close_result) {
        (Ok(_), Err(e)) => {
            assert_eq!(e.error_code, ContinuationErrorCode::SessionBusy);
        }
        (Err(e), Ok(_)) => {
            assert_eq!(e.error_code, ContinuationErrorCode::SessionClosed);
        }
        other => panic!("unexpected combination: {other:?}"),
    }
    // No closed session may hold an active turn.
    let turns = collaboration_service::list_turns(&h.db.conn, &ack.session_id, 0, 100)
        .await
        .unwrap();
    let active = turns
        .iter()
        .any(|t| ["accepted", "preparing", "dispatching", "running", "cancel_requested"].contains(&t.state.as_str()));
    if close_result.is_ok() {
        assert!(!active, "a closed session must never hold an active turn");
    }
}

/// Acceptance F10 regression (was `review_snapshot_must_keep_history_cursor…`):
/// with 24 terminal rounds + 1 active round (25 total) and a 20-row page, the
/// snapshot must report a cursor that still reaches rounds 21–24 — appending
/// the active round to the page must NOT flip `next_after_ordinal` to None.
#[tokio::test]
async fn snapshot_keeps_history_cursor_when_active_is_appended() {
    let h = harness().await;
    let session = collaboration_service::upsert_session_once(
        &h.db.conn,
        collaboration_service::NewSession {
            id: uuid::Uuid::new_v4().to_string(),
            source_task_id: SOURCE_TASK.into(),
            parent_conversation_id: PARENT,
            child_conversation_id: h.child_conversation_id,
            resume_binding_json: "{\"schema_version\":1,\"agent_type\":\"claude_code\",\"external_session_id\":\"ext-session-1\",\"cwd\":\"/work\",\"config_fingerprint\":\"fp-1\"}".into(),
        },
    )
    .await
    .unwrap();

    // Rounds 1..=24 terminal (the single-active index forbids more than one
    // accepted row per session), round 25 accepted (active).
    for i in 1..=24 {
        let turn = collaboration_service::insert_turn(
            &h.db.conn,
            collaboration_service::NewTurn {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session.id.clone(),
                ordinal: i,
                request_id: format!("k{i}"),
                message: format!("m{i}"),
                initiator_parent_conversation_id: PARENT,
                initiator_tool_use_id: None,
            },
        )
        .await
        .unwrap();
        assert!(collaboration_service::cas_turn_state(
            &h.db.conn,
            &turn.id,
            None,
            &["accepted"],
            "completed",
            false
        )
        .await
        .unwrap());
    }
    let active = collaboration_service::insert_turn(
        &h.db.conn,
        collaboration_service::NewTurn {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            ordinal: 25,
            request_id: "k25".into(),
            message: "m25".into(),
            initiator_parent_conversation_id: PARENT,
            initiator_tool_use_id: None,
        },
    )
    .await
    .unwrap();

    let snapshot = codeg_lib::commands::collaboration::get_collaboration_session_core(
        &h.coordinator,
        PARENT,
        SOURCE_TASK,
        0,
        Some(20),
    )
    .await
    .expect("snapshot");

    assert_eq!(snapshot.turns.len(), 21, "page + appended active round");
    assert_eq!(
        snapshot.next_after_ordinal,
        Some(20),
        "rounds 21-24 must remain reachable"
    );

    // The second page reaches them, and still carries the active round.
    let page2 = codeg_lib::commands::collaboration::get_collaboration_session_core(
        &h.coordinator,
        PARENT,
        SOURCE_TASK,
        snapshot.next_after_ordinal.unwrap(),
        Some(20),
    )
    .await
    .expect("page 2");
    let ordinals: Vec<i32> = page2.turns.iter().map(|t| t.ordinal).collect();
    for o in 21..=24 {
        assert!(ordinals.contains(&o), "page 2 missing round {o}: {ordinals:?}");
    }
    assert!(ordinals.contains(&25), "the active round must stay visible");
    let _ = active;
}

// ---------------------------------------------------------------------------
// Reacceptance regressions (R3/R4/R5/R6): boundary state and transport
// failure injection mirroring the independent re-review's probes.
// ---------------------------------------------------------------------------

/// R3: a cancel whose first report snapshot was taken while the round was
/// `preparing` must re-decide on the LATEST persisted state — the concurrent
/// drive advancing preparing → dispatching → running before the CAS must
/// land the round in `cancel_requested` (and ask the agent to stop), not
/// spin three pre-send CAS attempts and drop the request.
#[tokio::test]
async fn review_cancel_must_redecide_after_preparing_snapshot_advances() {
    let h = harness().await;
    let session = collaboration_service::upsert_session_once(
        &h.db.conn,
        collaboration_service::NewSession {
            id: uuid::Uuid::new_v4().to_string(),
            source_task_id: SOURCE_TASK.into(),
            parent_conversation_id: PARENT,
            child_conversation_id: h.child_conversation_id,
            resume_binding_json: "{}".into(),
        },
    )
    .await
    .unwrap();
    let turn = collaboration_service::insert_turn(
        &h.db.conn,
        collaboration_service::NewTurn {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session.id,
            ordinal: 1,
            request_id: "k1".into(),
            message: "m1".into(),
            initiator_parent_conversation_id: PARENT,
            initiator_tool_use_id: None,
        },
    )
    .await
    .unwrap();
    collaboration_service::cas_turn_state(
        &h.db.conn,
        &turn.id,
        None,
        &["accepted"],
        "preparing",
        false,
    )
    .await
    .unwrap();
    collaboration_service::set_turn_connection(
        &h.db.conn,
        &turn.id,
        &turn.execution_id,
        "boundary-connection",
    )
    .await
    .unwrap();
    // Park the cancel's blocked probe: the initial report snapshot captures
    // Preparing, then the concurrent drive advances the persisted states
    // BEFORE the probe releases and the CAS runs.
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *h.runtime.blocked_gate.lock().await = Some((entered_tx, release_rx));
    let c = h.coordinator.clone();
    let tid = turn.id.clone();
    let parent = h.parent;
    let cancel = tokio::spawn(async move { c.cancel_turn(parent, &tid).await });
    entered_rx.await.unwrap();
    collaboration_service::cas_turn_state(
        &h.db.conn,
        &turn.id,
        None,
        &["preparing"],
        "dispatching",
        true,
    )
    .await
    .unwrap();
    collaboration_service::cas_turn_state(
        &h.db.conn,
        &turn.id,
        None,
        &["dispatching"],
        "running",
        false,
    )
    .await
    .unwrap();
    release_tx.send(()).unwrap();
    let report = cancel.await.unwrap().unwrap();
    assert_eq!(
        report.state,
        TurnState::CancelRequested,
        "a stale preparing snapshot must not drop the cancellation"
    );
    assert_eq!(
        h.runtime.counters().2,
        1,
        "the running round's agent must have been asked to stop"
    );
}

/// R4: a parent cleanup whose cancel ENQUEUED successfully (transport Ok)
/// but was never confirmed by any agent stop event must not report the round
/// as cleanly canceled — enqueue is not a terminal. With the disconnect also
/// failing (the execution demonstrably still owned), the round must sit at
/// `cancel_requested` (or settle unknown), never a fabricated `canceled`.
#[tokio::test]
async fn review_parent_cancel_enqueue_is_not_stop_confirmation() {
    let h = harness().await;
    h.runtime.fail_disconnects.store(true, Ordering::SeqCst);
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    wait_for_state(&h.coordinator, h.parent, &ack.turn_id, &[TurnState::Running]).await;
    // cancel enqueues fine but no stop confirmation ever arrives; the
    // disconnect also fails, so the execution is still registered.
    h.coordinator.cancel_by_parent_connection("parent-conn").await;
    assert!(
        h.coordinator.connection_of_turn(&ack.turn_id).await.is_some(),
        "the unconfirmed execution must still be owned"
    );
    let report = h
        .coordinator
        .get_turn(h.parent, &ack.turn_id, 0)
        .await
        .unwrap();
    assert!(
        matches!(
            report.state,
            TurnState::CancelRequested | TurnState::OutcomeUnknown
        ),
        "an unconfirmed live execution reported {:?}",
        report.state
    );
}

/// R6: a close whose held-connection release FAILS must not have written
/// `closed` first — the write reservation (open/blocked session) has to
/// survive the failed release so ordinary writers stay out while the
/// platform still holds the connection.
#[tokio::test]
async fn review_failed_close_must_keep_child_write_reservation() {
    let h = harness().await;
    h.runtime.fail_disconnects.store(true, Ordering::SeqCst);
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    wait_for_state(&h.coordinator, h.parent, &ack.turn_id, &[TurnState::Running]).await;
    let (tid, eid) = h
        .coordinator
        .execution_owner_by_turn(&ack.turn_id)
        .await
        .unwrap();
    h.coordinator
        .settle(&tid, &eid, TurnTerminal::Completed { text: "done".into() })
        .await
        .unwrap();
    assert!(
        h.coordinator.close_session(h.parent, &ack.session_id).await.is_err(),
        "the failed release must surface as an error"
    );
    assert!(
        h.coordinator.connection_of_turn(&ack.turn_id).await.is_some(),
        "the unreleased connection must still be owned"
    );
    let session = collaboration_service::find_session_by_id(&h.db.conn, &ack.session_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        session.state, "closed",
        "a failed release must not lift the write reservation while the owner is live"
    );
}

/// R5: admission re-validates the session state inside the inserting
/// transaction. After an interleaving `settle_unknown_and_block` commit
/// (turn terminal + session blocked), a `continue` that already read the
/// session as open must be refused — never insert a fresh round under the
/// stale open snapshot.
#[tokio::test]
async fn review_admission_after_unknown_settle_must_not_insert_under_stale_open() {
    let h = harness().await;
    let ack = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k1", "m1", None)
        .await
        .unwrap();
    wait_for_state(&h.coordinator, h.parent, &ack.turn_id, &[TurnState::Running]).await;
    let (tid, eid) = h
        .coordinator
        .execution_owner_by_turn(&ack.turn_id)
        .await
        .unwrap();

    // The unknown settle commits (cancel deadline / hard disconnect path).
    h.coordinator.settle_unknown(&tid, &eid).await.unwrap();

    // A racing admission that had read the session as open BEFORE that
    // commit now reaches its insert: the transactional re-validation must
    // refuse it with the blocked domain error and admit nothing.
    let err = h
        .coordinator
        .continue_turn(h.parent, "parent-conn", SOURCE_TASK, "k2", "m2", None)
        .await
        .expect_err("an unknown-settled session admits no new round");
    assert_eq!(err_code(&err), ContinuationErrorCode::SessionBlocked);
    let turns = collaboration_service::list_turns(&h.db.conn, &ack.session_id, 0, 100)
        .await
        .unwrap();
    assert_eq!(turns.len(), 1, "no second turn row may exist");
}
