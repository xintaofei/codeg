//! The single funnel for loop state changes: compare-and-swap status
//! transitions and the durable dispatch leases (partial unique indexes enforce
//! one active finalize per issue, one active iteration per (target, stage)
//! excluding review, and N review slots per task). All concurrency safety
//! bottoms out here, not in the in-memory driver registry.

use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveEnum, ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    SqlErr,
};

use crate::db::entities::loop_artifact::{self, ArtifactStatus};
use crate::db::entities::loop_issue::{self, IssueStatus};
use crate::db::entities::loop_iteration::{self, IterationStatus, LaunchedBy, Stage};
use crate::loop_engine::error::LoopError;

/// A SQLite UNIQUE-constraint failure — a dispatch lease was already held by a
/// concurrent claimer. Classified through SeaORM's driver-typed `sql_err()`
/// (`SqlErr::UniqueConstraintViolation`) rather than matching the message text,
/// which silently breaks when a driver reworded its error.
fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
}

/// Single source of truth for legal status edges across the three loop state
/// machines (§2.8). Defense-in-depth: the `cas_*_status` helpers assert the
/// (expected → new) pair is a legal *edge* before issuing the conditional
/// UPDATE, so a stray CAS with a nonsense pair surfaces as
/// [`LoopError::IllegalTransition`] instead of silently corrupting the pipeline.
/// This is a *static* check on the pair, independent of the row's live value
/// (that race is the CAS miss → [`LoopError::Conflict`]). Bulk recovery
/// transitions (`recovery.rs` interrupting many rows at once) intentionally
/// bypass this — they are a documented mass `update_many`, not a per-row CAS.
pub(crate) fn is_legal_issue(from: IssueStatus, to: IssueStatus) -> bool {
    use IssueStatus::*;
    matches!(
        (from, to),
        (Pending, Running)
            | (Running, Paused)
            | (Paused, Running)
            | (Running, Blocked)
            | (Blocked, Running)
            | (Running, Done)
            | (Pending, Cancelled)
            | (Running, Cancelled)
            | (Paused, Cancelled)
            | (Blocked, Cancelled)
    )
}

pub(crate) fn is_legal_iteration(from: IterationStatus, to: IterationStatus) -> bool {
    use IterationStatus::*;
    matches!(
        (from, to),
        (Queued, Running)
            | (Running, Succeeded)
            | (Running, Failed)
            | (Queued, Failed)
            | (Queued, Interrupted)
            | (Running, Interrupted)
            | (Queued, Cancelled)
            | (Running, Cancelled)
    )
}

pub(crate) fn is_legal_artifact(from: ArtifactStatus, to: ArtifactStatus) -> bool {
    use ArtifactStatus::*;
    matches!(
        (from, to),
        (Pending, InProgress)
            | (InProgress, Done)
            | (AwaitingApproval, Done)
            | (Pending, Blocked)
            | (InProgress, Blocked)
            // Review-rejected retry sends an in-progress task back to pending so
            // it can be re-implemented at the next attempt (gates.rs).
            | (InProgress, Pending)
            | (AwaitingApproval, Superseded)
            | (AwaitingApproval, Cancelled)
            | (Done, Superseded)
            // Coverage loop-back supersedes still-`pending` plan tasks to replan
            // (driver.rs `maybe_coverage_loopback`).
            | (Pending, Superseded)
            | (Blocked, InProgress)
            | (Blocked, Pending)
            | (Pending, Cancelled)
            | (InProgress, Cancelled)
    )
}

/// CAS an issue's status: write `new` only if it currently equals `expected`.
/// Returns `true` on success, `false` on a miss (the caller maps that to
/// [`LoopError::Conflict`]).
pub async fn cas_issue_status(
    conn: &DatabaseConnection,
    id: i32,
    expected: IssueStatus,
    new: IssueStatus,
) -> Result<bool, LoopError> {
    // `IssueStatus` is `Clone` (not `Copy`, unlike its sibling enums), so clone
    // for the legality probe and keep the originals for the filter/update below.
    if !is_legal_issue(expected.clone(), new.clone()) {
        return Err(LoopError::IllegalTransition);
    }
    let res = loop_issue::Entity::update_many()
        .col_expr(loop_issue::Column::Status, Expr::value(new.to_value()))
        .col_expr(loop_issue::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_issue::Column::Id.eq(id))
        .filter(loop_issue::Column::Status.eq(expected))
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

/// CAS an artifact's status.
pub async fn cas_artifact_status(
    conn: &DatabaseConnection,
    id: i32,
    expected: ArtifactStatus,
    new: ArtifactStatus,
) -> Result<bool, LoopError> {
    if !is_legal_artifact(expected, new) {
        return Err(LoopError::IllegalTransition);
    }
    let res = loop_artifact::Entity::update_many()
        .col_expr(loop_artifact::Column::Status, Expr::value(new.to_value()))
        .col_expr(loop_artifact::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_artifact::Column::Id.eq(id))
        .filter(loop_artifact::Column::Status.eq(expected))
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

/// CAS an artifact's status from any of several legal predecessors to `to`
/// (§2.4). Each `(from, to)` pair must be legal ([`is_legal_artifact`]), else
/// [`LoopError::IllegalTransition`]. Used where a node is reached from more than
/// one state (e.g. blocking a task that may be `pending` or `in_progress`),
/// making the previously-blind write an explicit bounded CAS.
pub async fn cas_artifact_status_from(
    conn: &DatabaseConnection,
    id: i32,
    from: &[ArtifactStatus],
    to: ArtifactStatus,
) -> Result<bool, LoopError> {
    if from.iter().any(|f| !is_legal_artifact(*f, to)) {
        return Err(LoopError::IllegalTransition);
    }
    let res = loop_artifact::Entity::update_many()
        .col_expr(loop_artifact::Column::Status, Expr::value(to.to_value()))
        .col_expr(loop_artifact::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_artifact::Column::Id.eq(id))
        .filter(loop_artifact::Column::Status.is_in(from.iter().copied()))
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

/// Atomically mark a task `Done` AND freeze its integration commit in a single
/// CAS (`status='done', fan_in_commit=<sha> WHERE id=? AND status='in_progress'`).
/// Establishes the invariant **"a Done task always carries a non-null
/// fan_in_commit"** — there is no observable "Done but unfrozen" intermediate the
/// parallel fan-in could trip over. Returns whether the CAS applied (a miss means
/// the task was no longer `in_progress` — a stale snapshot, not an error).
pub async fn cas_task_done_with_freeze(
    conn: &DatabaseConnection,
    task_id: i32,
    fan_in_commit: &str,
) -> Result<bool, LoopError> {
    let res = loop_artifact::Entity::update_many()
        .col_expr(
            loop_artifact::Column::Status,
            Expr::value(ArtifactStatus::Done.to_value()),
        )
        .col_expr(
            loop_artifact::Column::FanInCommit,
            Expr::value(fan_in_commit.to_string()),
        )
        .col_expr(loop_artifact::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_artifact::Column::Id.eq(task_id))
        .filter(loop_artifact::Column::Status.eq(ArtifactStatus::InProgress))
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

/// Claim the parallel fan-in **session lock** by writing the manifest exactly
/// once: `UPDATE loop_issue SET fan_in_manifest=? WHERE id=? AND fan_in_manifest
/// IS NULL`. Returns whether this caller won (rows==1). A versioned, write-once
/// session token — distinct from the `uniq_active_finalize` agent lease (one
/// guards the integration *session*, the other a single in-flight agent).
pub async fn try_claim_fan_in(
    conn: &DatabaseConnection,
    issue_id: i32,
    manifest_json: &str,
) -> Result<bool, LoopError> {
    let res = loop_issue::Entity::update_many()
        .col_expr(
            loop_issue::Column::FanInManifest,
            Expr::value(manifest_json.to_string()),
        )
        .col_expr(loop_issue::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_issue::Column::Id.eq(issue_id))
        .filter(loop_issue::Column::FanInManifest.is_null())
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

/// Clear the fan-in session lock (`fan_in_manifest`/`fan_in_resolver_tip → NULL`)
/// once the session has landed or been abandoned, so a future re-trigger can claim
/// a fresh one. CAS-guarded on the exact manifest we owned (`WHERE
/// fan_in_manifest=?`): a stale driver replaying an old manifest must NOT erase a
/// newer session another driver has since claimed — a miss is benign (nothing to do).
pub async fn clear_fan_in(
    conn: &DatabaseConnection,
    issue_id: i32,
    expected_manifest: &str,
) -> Result<(), LoopError> {
    loop_issue::Entity::update_many()
        .col_expr(
            loop_issue::Column::FanInManifest,
            Expr::value(Option::<String>::None),
        )
        .col_expr(
            loop_issue::Column::FanInResolverTip,
            Expr::value(Option::<String>::None),
        )
        .col_expr(loop_issue::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_issue::Column::Id.eq(issue_id))
        .filter(loop_issue::Column::FanInManifest.eq(expected_manifest))
        .exec(conn)
        .await?;
    Ok(())
}

/// Record the integrate-worktree tip at which a fan-in conflict resolver is being
/// dispatched (see `loop_issue.fan_in_resolver_tip`). Idempotent overwrite within
/// a session; cleared by [`clear_fan_in`].
pub async fn set_fan_in_resolver_tip(
    conn: &DatabaseConnection,
    issue_id: i32,
    tip: &str,
) -> Result<(), LoopError> {
    loop_issue::Entity::update_many()
        .col_expr(
            loop_issue::Column::FanInResolverTip,
            Expr::value(tip.to_string()),
        )
        .col_expr(loop_issue::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_issue::Column::Id.eq(issue_id))
        .exec(conn)
        .await?;
    Ok(())
}

/// Inputs for a dispatch claim. `conversation_id` is intentionally absent — the
/// lease row is inserted first (conversation attached afterwards by the winner).
pub struct IterationClaim {
    pub space_id: i32,
    pub issue_id: i32,
    pub stage: Stage,
    pub target_artifact_id: Option<i32>,
    pub slot_no: Option<i32>,
    pub capability_token: String,
    pub attempt: i32,
}

/// Attempt to claim a dispatch lease by inserting a `queued` iteration row. The
/// partial unique indexes make this the atomic gate: a lost race surfaces as a
/// UNIQUE violation, returned here as `Ok(None)` (not an error) so the driver
/// simply skips. The winner gets `Ok(Some(row))`.
pub async fn try_claim_iteration(
    conn: &DatabaseConnection,
    claim: IterationClaim,
) -> Result<Option<loop_iteration::Model>, LoopError> {
    let now = Utc::now();
    let active = loop_iteration::ActiveModel {
        space_id: Set(claim.space_id),
        issue_id: Set(claim.issue_id),
        stage: Set(claim.stage),
        target_artifact_id: Set(claim.target_artifact_id),
        slot_no: Set(claim.slot_no),
        conversation_id: Set(None),
        capability_token: Set(claim.capability_token),
        status: Set(IterationStatus::Queued),
        launched_by: Set(LaunchedBy::Engine),
        attempt: Set(claim.attempt),
        tokens_used: Set(0),
        tokens_pending: Set(false),
        context_manifest: Set(None),
        created_at: Set(now),
        started_at: Set(None),
        ended_at: Set(None),
        ..Default::default()
    };
    match active.insert(conn).await {
        Ok(model) => Ok(Some(model)),
        Err(e) if is_unique_violation(&e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// CAS an iteration's status.
pub async fn cas_iteration_status(
    conn: &DatabaseConnection,
    id: i32,
    expected: IterationStatus,
    new: IterationStatus,
) -> Result<bool, LoopError> {
    if !is_legal_iteration(expected, new) {
        return Err(LoopError::IllegalTransition);
    }
    let res = loop_iteration::Entity::update_many()
        .col_expr(loop_iteration::Column::Status, Expr::value(new.to_value()))
        .filter(loop_iteration::Column::Id.eq(id))
        .filter(loop_iteration::Column::Status.eq(expected))
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

/// Fail an iteration from whichever active state it holds in a single UPDATE:
/// `status IN ('queued','running') → 'failed'`, stamping `ended_at`. Atomic
/// (§2.6) — replaces the old two sequential CAS that could wedge a row in
/// `running` if the process died between them. Returns whether a row changed.
pub async fn fail_iteration_active(conn: &DatabaseConnection, id: i32) -> Result<bool, LoopError> {
    let res = loop_iteration::Entity::update_many()
        .col_expr(
            loop_iteration::Column::Status,
            Expr::value(IterationStatus::Failed.to_value()),
        )
        .col_expr(loop_iteration::Column::EndedAt, Expr::value(Utc::now()))
        .filter(loop_iteration::Column::Id.eq(id))
        .filter(
            loop_iteration::Column::Status
                .is_in([IterationStatus::Queued, IterationStatus::Running]),
        )
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus};
    use crate::db::entities::loop_artifact_revision::ActorKind;
    use crate::db::entities::loop_issue::IssuePriority;
    use crate::db::service::loop_service::{artifact, issue, space};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::models::loops::IssueConfig;

    async fn seed() -> (crate::db::AppDatabase, i32, i32) {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/trans").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &db.conn,
            space.id,
            "I",
            "d",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        (db, space.id, issue.row.id)
    }

    fn claim(space_id: i32, issue_id: i32, stage: Stage, target: Option<i32>, slot: Option<i32>, token: &str) -> IterationClaim {
        IterationClaim {
            space_id,
            issue_id,
            stage,
            target_artifact_id: target,
            slot_no: slot,
            capability_token: token.to_string(),
            attempt: 0,
        }
    }

    #[tokio::test]
    async fn cas_issue_status_only_on_expected() {
        let (db, _space, issue_id) = seed().await;
        assert!(
            cas_issue_status(&db.conn, issue_id, IssueStatus::Pending, IssueStatus::Running)
                .await
                .unwrap()
        );
        // Now the row is Running; a Pending→Running CAS must miss.
        assert!(
            !cas_issue_status(&db.conn, issue_id, IssueStatus::Pending, IssueStatus::Running)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn node_lease_blocks_second_implement_per_task() {
        let (db, space_id, issue_id) = seed().await;
        let task = artifact::create_artifact(&db.conn, space_id, issue_id, ArtifactKind::Task, "T", ArtifactStatus::Pending, ActorKind::Agent, None).await.unwrap();
        let first = try_claim_iteration(&db.conn, claim(space_id, issue_id, Stage::Implement, Some(task.id), None, "tok-a")).await.unwrap();
        assert!(first.is_some());
        // Same task, another implement → uniq_active_node(target, stage) blocks it
        // (phase 2: per-task, not per-issue; different tasks now run concurrently).
        let second = try_claim_iteration(&db.conn, claim(space_id, issue_id, Stage::Implement, Some(task.id), None, "tok-b")).await.unwrap();
        assert!(second.is_none(), "second implement of the same task is leased out");
    }

    #[tokio::test]
    async fn review_slots_parallel_but_unique_per_slot() {
        let (db, space_id, issue_id) = seed().await;
        let task = artifact::create_artifact(&db.conn, space_id, issue_id, ArtifactKind::Task, "T", ArtifactStatus::Done, ActorKind::Agent, None).await.unwrap();
        // Two reviews of the same task on distinct slots both claim.
        let s0 = try_claim_iteration(&db.conn, claim(space_id, issue_id, Stage::Review, Some(task.id), Some(0), "r0")).await.unwrap();
        let s1 = try_claim_iteration(&db.conn, claim(space_id, issue_id, Stage::Review, Some(task.id), Some(1), "r1")).await.unwrap();
        assert!(s0.is_some() && s1.is_some(), "review slots run in parallel");
        // Same slot again → blocked.
        let dup = try_claim_iteration(&db.conn, claim(space_id, issue_id, Stage::Review, Some(task.id), Some(0), "r0b")).await.unwrap();
        assert!(dup.is_none(), "duplicate review slot is leased out");
    }

    #[tokio::test]
    async fn cas_task_done_with_freeze_sets_both_atomically() {
        let (db, space_id, issue_id) = seed().await;
        let task = artifact::create_artifact(
            &db.conn, space_id, issue_id, ArtifactKind::Task, "T",
            ArtifactStatus::InProgress, ActorKind::Agent, None,
        )
        .await
        .unwrap();

        // From InProgress: applies, setting status=Done AND fan_in_commit together.
        assert!(cas_task_done_with_freeze(&db.conn, task.id, "deadbeef")
            .await
            .unwrap());
        let row = loop_artifact::Entity::find_by_id(task.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, ArtifactStatus::Done);
        assert_eq!(
            row.fan_in_commit.as_deref(),
            Some("deadbeef"),
            "Done ⟹ frozen, no unfrozen window"
        );

        // A second call (now Done, not InProgress) is a CAS miss — no overwrite.
        assert!(!cas_task_done_with_freeze(&db.conn, task.id, "other")
            .await
            .unwrap());
        let row = loop_artifact::Entity::find_by_id(task.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.fan_in_commit.as_deref(), Some("deadbeef"));
    }

    #[tokio::test]
    async fn cas_task_done_with_freeze_misses_when_not_in_progress() {
        let (db, space_id, issue_id) = seed().await;
        // A Pending task is not yet eligible → CAS misses, no partial freeze.
        let task = artifact::create_artifact(
            &db.conn, space_id, issue_id, ArtifactKind::Task, "T",
            ArtifactStatus::Pending, ActorKind::Agent, None,
        )
        .await
        .unwrap();
        assert!(!cas_task_done_with_freeze(&db.conn, task.id, "abc")
            .await
            .unwrap());
        let row = loop_artifact::Entity::find_by_id(task.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, ArtifactStatus::Pending);
        assert!(row.fan_in_commit.is_none(), "no freeze on a CAS miss");
    }

    #[tokio::test]
    async fn duplicate_token_is_typed_unique_violation() {
        let (db, space_id, issue_id) = seed().await;
        let c = |t: &str| claim(space_id, issue_id, Stage::Triage, None, None, t);
        // First claim wins.
        assert!(try_claim_iteration(&db.conn, c("dup")).await.unwrap().is_some());
        // Second claim with the SAME capability_token hits uniq_loop_iteration_token
        // → classified as a lost race (Ok(None)), not an Err.
        let again = try_claim_iteration(&db.conn, c("dup")).await.unwrap();
        assert!(again.is_none(), "duplicate token is a typed unique violation → Ok(None)");
    }

    #[test]
    fn is_legal_covers_known_edges_and_rejects_nonsense() {
        use crate::db::entities::loop_artifact::ArtifactStatus as A;
        use crate::db::entities::loop_issue::IssueStatus as I;
        use crate::db::entities::loop_iteration::IterationStatus as It;
        // Representative legal edges.
        assert!(is_legal_issue(I::Pending, I::Running));
        assert!(is_legal_issue(I::Running, I::Done));
        assert!(is_legal_iteration(It::Queued, It::Running));
        assert!(is_legal_iteration(It::Running, It::Succeeded));
        assert!(is_legal_artifact(A::Pending, A::InProgress));
        assert!(is_legal_artifact(A::InProgress, A::Done));
        // Nonsense edges are illegal.
        assert!(!is_legal_issue(I::Done, I::Running));
        assert!(!is_legal_iteration(It::Succeeded, It::Running));
        assert!(!is_legal_artifact(A::Done, A::InProgress));
        // Identity is never a "transition".
        assert!(!is_legal_issue(I::Running, I::Running));
    }

    #[tokio::test]
    async fn cas_rejects_illegal_pair_before_touching_db() {
        let (db, _space, issue_id) = seed().await;
        // Done is terminal; Done→Running is not a legal edge → IllegalTransition,
        // independent of the row's current status.
        let err = cas_issue_status(&db.conn, issue_id, IssueStatus::Done, IssueStatus::Running)
            .await
            .unwrap_err();
        assert!(matches!(err, LoopError::IllegalTransition));
    }

    #[tokio::test]
    async fn fail_iteration_active_covers_queued_and_running_in_one_update() {
        let (db, space_id, issue_id) = seed().await;
        // queued lease
        let q = try_claim_iteration(&db.conn, claim(space_id, issue_id, Stage::Triage, None, None, "q"))
            .await
            .unwrap()
            .unwrap();
        assert!(fail_iteration_active(&db.conn, q.id).await.unwrap());
        // running lease
        let r = try_claim_iteration(&db.conn, claim(space_id, issue_id, Stage::Refine, Some(1), None, "r"))
            .await
            .unwrap()
            .unwrap();
        cas_iteration_status(&db.conn, r.id, IterationStatus::Queued, IterationStatus::Running)
            .await
            .unwrap();
        assert!(fail_iteration_active(&db.conn, r.id).await.unwrap());
        // already-terminal → no-op (false)
        assert!(!fail_iteration_active(&db.conn, r.id).await.unwrap());
    }

    #[tokio::test]
    async fn cas_artifact_status_from_blocks_pending_or_in_progress() {
        let (db, space_id, issue_id) = seed().await;
        let t = artifact::create_artifact(&db.conn, space_id, issue_id, ArtifactKind::Task, "T", ArtifactStatus::Pending, ActorKind::Agent, None)
            .await
            .unwrap();
        // pending → blocked via the multi-from set
        assert!(cas_artifact_status_from(&db.conn, t.id, &[ArtifactStatus::Pending, ArtifactStatus::InProgress], ArtifactStatus::Blocked)
            .await
            .unwrap());
        // already blocked → no-op
        assert!(!cas_artifact_status_from(&db.conn, t.id, &[ArtifactStatus::Pending, ArtifactStatus::InProgress], ArtifactStatus::Blocked)
            .await
            .unwrap());
    }

    /// Legality totality: the supersede edges the loop-backs rely on are legal, and
    /// representative illegal transitions are rejected (so a typo in a loop-back can
    /// never silently corrupt a node's lifecycle).
    #[test]
    fn is_legal_artifact_supersede_edges_and_rejections() {
        use ArtifactStatus::*;
        // Loop-back supersede edges (coverage / integration / design-reject).
        assert!(is_legal_artifact(Done, Superseded), "integration loop-back supersedes done tasks/result");
        assert!(is_legal_artifact(AwaitingApproval, Superseded), "design-reject supersedes the awaiting design");
        assert!(is_legal_artifact(Pending, Superseded), "coverage loop-back supersedes pending tasks");
        // Review-fail retry sends an in-progress task back to pending.
        assert!(is_legal_artifact(InProgress, Pending));
        // Rejections: a settled/implemented node can't regress or be superseded
        // through an undefined edge.
        assert!(!is_legal_artifact(Done, Pending), "a done task never reopens to pending");
        assert!(!is_legal_artifact(InProgress, Superseded), "an in-progress task isn't directly superseded");
        assert!(!is_legal_artifact(Done, InProgress), "a done task never reverts to in-progress");
        assert!(!is_legal_artifact(Blocked, Superseded), "a blocked node is resolved by retry/cancel, not supersede");
    }
}
