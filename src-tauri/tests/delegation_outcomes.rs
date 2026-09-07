//! PR1 integration: frozen COMPLETED delegation outcomes.
//!
//! Covers the v2 design §4.1 contract end to end against real SQLite:
//! * first-writer-wins success freezing (identical replay idempotent,
//!   different result conflicts, canceled/failed never frozen),
//! * frozen-first status projection surviving cache eviction, broker rebuild,
//!   and child-status drift (A01/A02),
//! * the upstream `resume_delegation` path staying intact for interrupted
//!   tasks (A33/A34) while a frozen snapshot refuses to re-resume a completed
//!   task (A35),
//! * complete/cancel races freezing only the success winner (A03), and
//! * storage failures diagnosing loudly without faking durability (A04).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use codeg_lib::acp::delegation::broker::{
    ConversationDepthLookup, DbChildStatusLookup, DbDelegationOutcomeStore, DelegationBroker,
    DelegationConfig, DelegationOutcomeStore, StatusWait,
};
use codeg_lib::acp::delegation::spawner::{
    mock::MockSpawner, ConnectionSpawner, ResumedSpawn, SpawnerError,
};
use codeg_lib::acp::delegation::types::{
    DelegationError, DelegationOutcome, DelegationRequest, DelegationSuccess,
    ResumeDelegationRequest, TaskStatus,
};
use codeg_lib::db::entities::conversation;
use codeg_lib::db::service::delegation_outcome_service::{
    self, DelegationOutcomeInsert, OutcomeWriteResult,
};
use codeg_lib::db::test_helpers::{fresh_disk_db, fresh_in_memory_db, seed_folder};
use codeg_lib::db::AppDatabase;
use codeg_lib::models::agent::AgentType;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, NotSet, QueryFilter, Set};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct NoopDepth;
#[async_trait::async_trait]
impl ConversationDepthLookup for NoopDepth {
    async fn parent_of(&self, _id: i32) -> Result<Option<i32>, DelegationError> {
        Ok(None)
    }
}

/// Production store wrapper that records write attempts and can inject storage
/// failures, so tests can assert BOTH the durable row and the failure path.
struct RecordingStore {
    inner: Arc<DbDelegationOutcomeStore>,
    writes: Mutex<Vec<String>>,
    fail_writes: AtomicBool,
}

impl RecordingStore {
    fn new(db: &AppDatabase) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(DbDelegationOutcomeStore {
                db: Arc::new(AppDatabase {
                    conn: db.conn.clone(),
                }),
            }),
            writes: Mutex::new(Vec::new()),
            fail_writes: AtomicBool::new(false),
        })
    }

    async fn write_count(&self) -> usize {
        self.writes.lock().await.len()
    }
}

#[async_trait::async_trait]
impl DelegationOutcomeStore for RecordingStore {
    async fn insert_once(
        &self,
        record: DelegationOutcomeInsert,
    ) -> Result<OutcomeWriteResult, String> {
        self.writes.lock().await.push(record.task_id.clone());
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err("injected storage failure".to_string());
        }
        self.inner.insert_once(record).await
    }

    async fn find_owned(
        &self,
        parent_conversation_id: i32,
        task_id: &str,
    ) -> Result<
        Option<codeg_lib::db::service::delegation_outcome_service::DelegationOutcomeRow>,
        String,
    > {
        self.inner.find_owned(parent_conversation_id, task_id).await
    }
}

/// Broker wired like production: real DB-backed status + outcome stores, mock
/// spawner, delegation enabled.
async fn harness(
    db: &AppDatabase,
    store: Arc<RecordingStore>,
) -> (Arc<MockSpawner>, DelegationBroker) {
    let mock = Arc::new(MockSpawner::new());
    let broker = DelegationBroker::new(
        mock.clone() as Arc<dyn ConnectionSpawner>,
        Arc::new(NoopDepth) as Arc<dyn ConversationDepthLookup>,
    )
    .with_status_lookup(Arc::new(DbChildStatusLookup {
        db: Arc::new(AppDatabase {
            conn: db.conn.clone(),
        }),
    }))
    .with_outcome_store(store);
    broker
        .set_config(DelegationConfig {
            enabled: true,
            ..DelegationConfig::default()
        })
        .await;
    (mock, broker)
}

fn delegation_request(parent_conversation_id: i32) -> DelegationRequest {
    DelegationRequest {
        parent_connection_id: "parent-conn".into(),
        parent_conversation_id,
        parent_tool_use_id: "pt-1".into(),
        agent_type: AgentType::ClaudeCode,
        task: "do x".into(),
        working_dir: None,
        requested_working_dir: None,
        external_handle: None,
    }
}

fn success(text: &str, child_conversation_id: i32) -> DelegationOutcome {
    DelegationOutcome::Ok(DelegationSuccess {
        text: text.to_string(),
        child_conversation_id,
        child_agent_type: AgentType::ClaudeCode,
        turn_count: 1,
        duration_ms: 5,
        token_usage: None,
    })
}

/// Insert a delegation child conversation row (status configurable) so the
/// resume path can resolve its DB context.
async fn seed_child_row(
    db: &AppDatabase,
    folder_id: i32,
    parent_conversation_id: i32,
    call_id: &str,
    status: conversation::ConversationStatus,
) -> i32 {
    let now = chrono::Utc::now();
    let active = conversation::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        title: Set(Some("delegation child".to_string())),
        title_locked: Set(false),
        agent_type: Set("claude_code".to_string()),
        status: Set(status),
        kind: Set(conversation::ConversationKind::Delegate),
        model: Set(None),
        git_branch: Set(None),
        external_id: Set(Some("ext-session-1".to_string())),
        parent_id: Set(Some(parent_conversation_id)),
        parent_tool_use_id: Set(Some("pt-1".to_string())),
        delegation_call_id: Set(Some(call_id.to_string())),
        message_count: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        pinned_at: Set(None),
        origin_cwd: Set(None),
    };
    active.insert(&db.conn).await.expect("seed child row").id
}

fn sample_insert(task_id: &str, text: &str, truncated: bool) -> DelegationOutcomeInsert {
    DelegationOutcomeInsert {
        task_id: task_id.to_string(),
        parent_conversation_id: 1,
        parent_tool_use_id: Some("pt-1".to_string()),
        child_conversation_id: Some(42),
        agent_type: "claude_code".to_string(),
        text: text.to_string(),
        duration_ms: 12,
        text_truncated: truncated,
        completed_at: chrono::Utc::now(),
        resume_binding_json: None,
    }
}

fn resume_request(task_id: &str) -> ResumeDelegationRequest {
    ResumeDelegationRequest {
        parent_connection_id: "parent-conn".into(),
        parent_conversation_id: 1,
        task_id: task_id.into(),
        reason: None,
        external_handle: None,
    }
}

// ---------------------------------------------------------------------------
// Service level
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcome_first_write_wins_conflicts_on_different_result() {
    let db = fresh_in_memory_db().await;

    let first = delegation_outcome_service::insert_once(
        &db.conn,
        sample_insert("task-1", "result-v1", false),
    )
    .await
    .expect("first insert");
    assert_eq!(first, OutcomeWriteResult::Inserted);

    // Byte-identical replay is idempotent.
    let replay = delegation_outcome_service::insert_once(
        &db.conn,
        sample_insert("task-1", "result-v1", false),
    )
    .await
    .expect("replay insert");
    assert_eq!(replay, OutcomeWriteResult::AlreadyIdentical);

    // A DIFFERENT success result never overwrites the winner.
    let conflict = delegation_outcome_service::insert_once(
        &db.conn,
        sample_insert("task-1", "result-v2", false),
    )
    .await
    .expect("conflicting insert");
    assert_eq!(conflict, OutcomeWriteResult::Conflict);

    let row = delegation_outcome_service::find_owned(&db.conn, 1, "task-1")
        .await
        .expect("find_owned")
        .expect("row exists");
    assert_eq!(row.text, "result-v1");
    assert_eq!(row.agent_type, "claude_code");
}

#[tokio::test]
async fn outcome_parent_scoping_hides_foreign_rows() {
    let db = fresh_in_memory_db().await;
    delegation_outcome_service::insert_once(&db.conn, sample_insert("task-1", "result-v1", false))
        .await
        .expect("insert");

    // The owning parent reads it.
    assert!(
        delegation_outcome_service::find_owned(&db.conn, 1, "task-1")
            .await
            .expect("owned lookup")
            .is_some()
    );
    // A different parent gets the same answer as "never frozen" — no leak.
    assert!(
        delegation_outcome_service::find_owned(&db.conn, 2, "task-1")
            .await
            .expect("foreign lookup")
            .is_none()
    );
    // Unknown ids look identical to foreign ids.
    assert!(
        delegation_outcome_service::find_owned(&db.conn, 1, "nope")
            .await
            .expect("unknown lookup")
            .is_none()
    );
}

#[tokio::test]
async fn outcome_row_outlives_child_conversation_row() {
    let db = fresh_in_memory_db().await;
    let folder_id = seed_folder(&db, "/tmp/codeg-outcome-delete").await;
    let child_id = seed_child_row(
        &db,
        folder_id,
        1,
        "task-1",
        conversation::ConversationStatus::Completed,
    )
    .await;
    delegation_outcome_service::insert_once(&db.conn, sample_insert("task-1", "result-v1", false))
        .await
        .expect("insert");

    // The child row disappears (e.g. user deletes the sub-session).
    conversation::Entity::delete_by_id(child_id)
        .exec(&db.conn)
        .await
        .expect("delete child row");

    // The frozen result stays readable for the owning parent — history is not
    // destroyed by child-row deletion.
    let row = delegation_outcome_service::find_owned(&db.conn, 1, "task-1")
        .await
        .expect("lookup after delete")
        .expect("outcome survives child deletion");
    assert_eq!(row.text, "result-v1");
    assert_eq!(row.child_conversation_id, Some(42));
}

// ---------------------------------------------------------------------------
// Broker level: completed-only freezing + frozen-first queries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn broker_freezes_success_but_never_canceled_or_failed() {
    let db = fresh_in_memory_db().await;
    let store = RecordingStore::new(&db);
    let (mock, broker) = harness(&db, store.clone()).await;

    // T1: completes successfully → frozen.
    mock.queue_spawn(Ok("child-conn-1".into())).await;
    mock.queue_send(Ok(42)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let t1 = ack.task_id.expect("task id");
    assert_eq!(ack.status, TaskStatus::Running);
    broker.complete_call(&t1, success("result-v1", 42)).await;
    assert_eq!(store.write_count().await, 1);
    assert!(
        delegation_outcome_service::find_owned(&db.conn, 1, &t1)
            .await
            .expect("lookup")
            .is_some(),
        "success result must be frozen"
    );

    // T2: fails (child refusal) → NOT frozen; upstream resume stays available.
    mock.queue_spawn(Ok("child-conn-2".into())).await;
    mock.queue_send(Ok(43)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let t2 = ack.task_id.expect("task id");
    broker
        .complete_call(
            &t2,
            DelegationOutcome::from_err(DelegationError::ChildRefusal, Some(43)),
        )
        .await;
    assert_eq!(store.write_count().await, 1, "failed outcome must not write");
    assert!(
        delegation_outcome_service::find_owned(&db.conn, 1, &t2)
            .await
            .expect("lookup")
            .is_none()
    );

    // T3: canceled → NOT frozen.
    mock.queue_spawn(Ok("child-conn-3".into())).await;
    mock.queue_send(Ok(44)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let t3 = ack.task_id.expect("task id");
    let canceled = broker.cancel_task_by_id("parent-conn", Some(1), &t3).await;
    assert_eq!(canceled.status, TaskStatus::Canceled);
    assert_eq!(store.write_count().await, 1, "canceled outcome must not write");
    assert!(
        delegation_outcome_service::find_owned(&db.conn, 1, &t3)
            .await
            .expect("lookup")
            .is_none()
    );

    // A second terminal for the already-completed T1 is dropped by the
    // first-writer-wins bookkeeping — still exactly one write.
    broker.complete_call(&t1, success("late duplicate", 42)).await;
    assert_eq!(store.write_count().await, 1);
}

/// A01 + A02: the frozen result survives cache eviction, broker rebuild, and
/// child-status drift; the mutable fallback never shadows it.
#[tokio::test]
async fn frozen_result_survives_rebuild_and_child_status_drift() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = fresh_disk_db(tmp.path()).await;
    let folder_id = seed_folder(&db, "/tmp/codeg-frozen-drift").await;
    seed_child_row(
        &db,
        folder_id,
        1,
        "task-1",
        conversation::ConversationStatus::Completed,
    )
    .await;
    let store = RecordingStore::new(&db);
    let (mock, broker) = harness(&db, store.clone()).await;

    mock.queue_spawn(Ok("child-conn-1".into())).await;
    mock.queue_send(Ok(42)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let task_id = ack.task_id.expect("task id");
    // Production's `send_prompt_linked_for_delegation` persists the broker
    // call id onto the child row; the mock spawner doesn't, so write the link
    // here to keep the child row consistent with the live task.
    let child = conversation::Entity::find()
        .filter(conversation::Column::DelegationCallId.eq("task-1"))
        .one(&db.conn)
        .await
        .expect("find child")
        .expect("child row");
    let mut link: conversation::ActiveModel = child.into();
    link.delegation_call_id = Set(Some(task_id.clone()));
    link.update(&db.conn).await.expect("link call id");
    broker
        .complete_call(&task_id, success("result-v1-中文", 42))
        .await;

    // Rebuild the broker (restart simulation): memory is empty, the store is
    // the only source for this task's result.
    let (_mock2, broker2) = harness(&db, store.clone()).await;
    let report = broker2
        .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
        .await;
    assert_eq!(report.status, TaskStatus::Completed);
    assert_eq!(report.text.as_deref(), Some("result-v1-中文"));

    // Now drift the child row's mutable status — the frozen answer must not
    // change (no regression to the conversation-derived fallback).
    let child = conversation::Entity::find()
        .filter(conversation::Column::DelegationCallId.eq(task_id.clone()))
        .one(&db.conn)
        .await
        .expect("find child")
        .expect("child row");
    let mut drift: conversation::ActiveModel = child.into();
    drift.status = Set(conversation::ConversationStatus::InProgress);
    drift.update(&db.conn).await.expect("drift status");

    let (_mock3, broker3) = harness(&db, store.clone()).await;
    let report = broker3
        .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
        .await;
    assert_eq!(report.status, TaskStatus::Completed);
    assert_eq!(report.text.as_deref(), Some("result-v1-中文"));
    // Duration is broker-measured (near-zero in the setup-window path here),
    // so only its presence is asserted.
    assert!(report.duration_ms.is_some());
    assert_eq!(report.agent_type, Some(AgentType::ClaudeCode));
}

// ---------------------------------------------------------------------------
// Resume interplay: interrupted tasks keep resuming, frozen ones do not
// ---------------------------------------------------------------------------

/// A33: canceled → resume (same id) → running → completed. The canceled phase
/// leaves NO outcome row, the resumed run freezes exactly one success result,
/// and a rebuilt broker still reports the frozen text.
#[tokio::test]
async fn resume_flow_freezes_only_the_eventual_success() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = fresh_disk_db(tmp.path()).await;
    let folder_id = seed_folder(&db, "/work").await;
    // A previous run left the task canceled (row `cancelled`), with the
    // external session id recorded.
    seed_child_row(
        &db,
        folder_id,
        1,
        "task-1",
        conversation::ConversationStatus::Cancelled,
    )
    .await;
    let store = RecordingStore::new(&db);
    let (mock, broker) = harness(&db, store.clone()).await;

    // Resume under the SAME task id.
    mock.queue_resume_spawn(Ok(ResumedSpawn::fresh("child-conn-2")))
        .await;
    mock.queue_resume_send(Ok(())).await;
    let ack = broker
        .resume_delegation(resume_request("task-1"))
        .await;
    assert_eq!(ack.status, TaskStatus::Running);
    assert_eq!(ack.task_id.as_deref(), Some("task-1"));
    assert_eq!(
        mock.resume_spawn_args.lock().await[0].external_session_id,
        "ext-session-1"
    );

    // Still nothing frozen while the resumed run is in flight.
    assert_eq!(store.write_count().await, 0);

    // The resumed execution completes → exactly one success snapshot.
    broker.complete_call("task-1", success("finished after resume", 42)).await;
    assert_eq!(store.write_count().await, 1);
    let row = delegation_outcome_service::find_owned(&db.conn, 1, "task-1")
        .await
        .expect("lookup")
        .expect("frozen after resumed success");
    assert_eq!(row.text, "finished after resume");

    // A rebuilt broker answers the frozen result (no live cache).
    let (_mock2, broker2) = harness(&db, store.clone()).await;
    let report = broker2
        .get_task_status("parent-conn", Some(1), "task-1", StatusWait::Immediate)
        .await;
    assert_eq!(report.status, TaskStatus::Completed);
    assert_eq!(report.text.as_deref(), Some("finished after resume"));
}

/// A35: once the success snapshot exists, the OLD resume tool must refuse even
/// when the cache was evicted and the child row's status later drifted — the
/// frozen store, not the mutable fallback, decides.
#[tokio::test]
async fn frozen_snapshot_refuses_old_resume_after_status_drift() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = fresh_disk_db(tmp.path()).await;
    let folder_id = seed_folder(&db, "/work").await;
    // Drift: the child row says `in_progress` (e.g. user opened it / stale
    // crash row) and the cache is empty (fresh broker). A naive DB-fallback
    // gate would treat this as resumable.
    seed_child_row(
        &db,
        folder_id,
        1,
        "task-1",
        conversation::ConversationStatus::InProgress,
    )
    .await;
    delegation_outcome_service::insert_once(
        &db.conn,
        DelegationOutcomeInsert {
            task_id: "task-1".into(),
            parent_conversation_id: 1,
            parent_tool_use_id: Some("pt-1".into()),
            child_conversation_id: Some(42),
            agent_type: "claude_code".into(),
            text: "original success".into(),
            duration_ms: 7,
            text_truncated: false,
            completed_at: chrono::Utc::now(),
            resume_binding_json: None,
        },
    )
    .await
    .expect("seed frozen result");

    let store = RecordingStore::new(&db);
    let (mock, broker) = harness(&db, store.clone()).await;
    let report = broker.resume_delegation(resume_request("task-1")).await;
    assert_eq!(report.error_code.as_deref(), Some("not_resumable"));
    assert!(
        report.message.unwrap().contains("already completed"),
        "refusal must name the completed task"
    );
    assert!(
        mock.resume_spawn_args.lock().await.is_empty(),
        "a frozen task must never re-spawn"
    );
    // The original result stays queryable.
    let status = broker
        .get_task_status("parent-conn", Some(1), "task-1", StatusWait::Immediate)
        .await;
    assert_eq!(status.status, TaskStatus::Completed);
    assert_eq!(status.text.as_deref(), Some("original success"));
}

/// A34 (partial): a canceled task WITHOUT a frozen snapshot keeps its upstream
/// resume path — spawn failure leaves the canceled record intact and no row.
#[tokio::test]
async fn resume_spawn_failure_keeps_task_resumable_without_snapshot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = fresh_disk_db(tmp.path()).await;
    let folder_id = seed_folder(&db, "/work").await;
    seed_child_row(
        &db,
        folder_id,
        1,
        "task-1",
        conversation::ConversationStatus::Cancelled,
    )
    .await;
    let store = RecordingStore::new(&db);
    let (mock, broker) = harness(&db, store.clone()).await;

    // First resume attempt: spawn fails.
    mock.queue_resume_spawn(Err(SpawnerError::Spawn("agent not installed".into())))
        .await;
    let report = broker.resume_delegation(resume_request("task-1")).await;
    assert_eq!(report.status, TaskStatus::Failed);
    assert_eq!(report.error_code.as_deref(), Some("spawn_failed"));
    assert_eq!(store.write_count().await, 0, "no frozen row on failed resume");
    assert!(
        delegation_outcome_service::find_owned(&db.conn, 1, "task-1")
            .await
            .expect("lookup")
            .is_none()
    );

    // A second, healthy resume is still allowed — the interruption keeps its
    // retry eligibility.
    mock.queue_resume_spawn(Ok(ResumedSpawn::fresh("child-conn-2")))
        .await;
    mock.queue_resume_send(Ok(())).await;
    let ack = broker.resume_delegation(resume_request("task-1")).await;
    assert_eq!(ack.status, TaskStatus::Running);
}

/// A04: a storage failure must not fake durability. The in-memory one-shot
/// behavior (report + teardown disconnect) completes normally, the failure is
/// reported by the store, and no row is silently pretended into existence.
#[tokio::test]
async fn storage_failure_preserves_legacy_behavior_and_diagnostics() {
    let db = fresh_in_memory_db().await;
    let store = RecordingStore::new(&db);
    store.fail_writes.store(true, Ordering::SeqCst);
    let (mock, broker) = harness(&db, store.clone()).await;

    mock.queue_spawn(Ok("child-conn-1".into())).await;
    mock.queue_send(Ok(42)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let task_id = ack.task_id.expect("task id");

    // Terminal resolution still reports Completed (legacy in-memory answer).
    broker.complete_call(&task_id, success("lost result", 42)).await;
    assert_eq!(store.write_count().await, 1, "write was attempted");

    // Teardown still happened — the one-shot semantics are not blocked on the
    // failed persistence.
    assert_eq!(
        mock.disconnects.lock().await.as_slice(),
        &["child-conn-1".to_string()]
    );

    // The in-memory cache still answers for the LIVE broker...
    let report = broker
        .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
        .await;
    assert_eq!(report.status, TaskStatus::Completed);

    // ...but a rebuilt broker falls back to the legacy DB path (canceled/
    // unknown) — the result was NOT durably frozen, and the source is not
    // continuable. Honest, not silent.
    let (_mock2, broker2) = harness(&db, store.clone()).await;
    let report = broker2
        .get_task_status("parent-conn", Some(1), &task_id, StatusWait::Immediate)
        .await;
    assert_ne!(report.status, TaskStatus::Completed);
}

/// A03 (sequenced): in the complete-vs-cancel race, only the SUCCESS winner
/// freezes a row; when cancel wins, nothing is written and the upstream path
/// keeps the task resumable.
#[tokio::test]
async fn complete_cancel_race_freezes_only_success_winner() {
    let db = fresh_in_memory_db().await;
    let store = RecordingStore::new(&db);

    // Cancel wins for T1 → no row.
    let (mock, broker) = harness(&db, store.clone()).await;
    mock.queue_spawn(Ok("child-conn-1".into())).await;
    mock.queue_send(Ok(42)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let t1 = ack.task_id.expect("task id");
    broker.cancel_task_by_id("parent-conn", Some(1), &t1).await;
    broker.complete_call(&t1, success("too late", 42)).await;
    assert_eq!(
        store.write_count().await,
        0,
        "a cancel that beat the completion must not freeze anything"
    );

    // Success wins for T2 → exactly one row; the later cancel attempt finds
    // the terminal record and does not write again.
    mock.queue_spawn(Ok("child-conn-2".into())).await;
    mock.queue_send(Ok(43)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let t2 = ack.task_id.expect("task id");
    broker.complete_call(&t2, success("winner", 43)).await;
    let canceled = broker.cancel_task_by_id("parent-conn", Some(1), &t2).await;
    assert_eq!(canceled.status, TaskStatus::Completed);
    assert_eq!(canceled.text.as_deref(), Some("winner"));
    assert_eq!(store.write_count().await, 1);
}

/// A04 (UTF-8 cap): an oversized multi-byte result is frozen UTF-8-safe, within
/// the cap, flagged truncated; a short result passes through unflagged.
#[tokio::test]
async fn oversized_chinese_result_is_frozen_truncated_and_valid_utf8() {
    let db = fresh_in_memory_db().await;
    let store = RecordingStore::new(&db);
    let (mock, broker) = harness(&db, store.clone()).await;

    // '好' is 3 bytes in UTF-8; 200_000 chars = 600_000 bytes > 256 KiB cap.
    let huge = "好".repeat(200_000);
    mock.queue_spawn(Ok("child-conn-1".into())).await;
    mock.queue_send(Ok(42)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let t1 = ack.task_id.expect("task id");
    broker.complete_call(&t1, success(&huge, 42)).await;

    let row = delegation_outcome_service::find_owned(&db.conn, 1, &t1)
        .await
        .expect("lookup")
        .expect("frozen");
    assert!(row.text_truncated, "oversized result must be flagged");
    assert!(row.text.ends_with('…'), "truncation appends the ellipsis");
    assert!(row.text.is_char_boundary(row.text.len()));
    assert!(row.text.len() <= 256 * 1024, "frozen text must fit the cap");
    // The stored text is a byte-prefix of the original plus the ellipsis.
    let without_ellipsis = row.text.trim_end_matches('…');
    assert!(huge.starts_with(without_ellipsis));
    // The stored text IS the report text — the frozen report matches.
    let (_mock2, broker2) = harness(&db, store.clone()).await;
    let report = broker2
        .get_task_status("parent-conn", Some(1), &t1, StatusWait::Immediate)
        .await;
    assert_eq!(report.text.as_deref(), Some(row.text.as_str()));

    // A short result stays byte-exact and unflagged.
    mock.queue_spawn(Ok("child-conn-2".into())).await;
    mock.queue_send(Ok(43)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let t2 = ack.task_id.expect("task id");
    broker.complete_call(&t2, success("short 结果", 43)).await;
    let row = delegation_outcome_service::find_owned(&db.conn, 1, &t2)
        .await
        .expect("lookup")
        .expect("frozen");
    assert!(!row.text_truncated);
    assert_eq!(row.text, "short 结果");
}

/// Acceptance F2 regression (was `review_frozen_result_must_use_persistent_…`):
/// the same parent CONVERSATION reconnects on a NEW parent connection while a
/// stale cache entry from the OLD connection still sits in the completed map.
/// The persistent-scope resolution must win — the frozen result stays readable
/// and a foreign parent still sees nothing.
#[tokio::test]
async fn frozen_result_readable_after_reconnect_despite_stale_cache_entry() {
    let db = fresh_in_memory_db().await;
    let store = RecordingStore::new(&db);
    let (mock, broker) = harness(&db, store.clone()).await;

    // T0 completes on the OLD parent connection.
    mock.queue_spawn(Ok("child-conn-1".into())).await;
    mock.queue_send(Ok(42)).await;
    let ack = broker.start_delegation(delegation_request(1)).await;
    let task_id = ack.task_id.expect("task id");
    broker.complete_call(&task_id, success("frozen text", 42)).await;
    assert!(
        delegation_outcome_service::find_owned(&db.conn, 1, &task_id)
            .await
            .expect("lookup")
            .is_some()
    );

    // Rebuild the broker but poison the cache with a STALE entry attributed
    // to the OLD parent connection while the caller presents the NEW one
    // (same parent conversation, reconnect window before teardown cleanup).
    let (_mock2, broker2) = harness(&db, store.clone()).await;
    broker2
        .seed_completed_for_test("OLD-PARENT-CONN", &task_id)
        .await;
    let report = broker2
        .get_task_status(
            "NEW-PARENT-CONN",
            Some(1),
            &task_id,
            StatusWait::Immediate,
        )
        .await;
    assert_eq!(report.status, TaskStatus::Completed);
    assert_eq!(report.text.as_deref(), Some("frozen text"));

    // A genuinely foreign parent still sees nothing (no existence leak).
    let foreign = broker2
        .get_task_status(
            "NEW-PARENT-CONN",
            Some(999),
            &task_id,
            StatusWait::Immediate,
        )
        .await;
    assert_eq!(foreign.status, TaskStatus::Unknown);
}
