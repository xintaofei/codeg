//! The single funnel for loop state changes: compare-and-swap status
//! transitions, the durable dispatch leases (partial unique indexes enforce
//! one active write-iteration per issue, one active iteration per (target,
//! stage) excluding review, and N review slots per task), and the per-issue
//! serial-task pipeline gate. All concurrency safety bottoms out here, not in
//! the in-memory driver registry.

use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveEnum, ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::db::entities::loop_artifact::{self, ArtifactStatus};
use crate::db::entities::loop_issue::{self, IssueStatus};
use crate::db::entities::loop_iteration::{self, IterationStatus, LaunchedBy, Stage};
use crate::loop_engine::error::LoopError;

/// A SQLite UNIQUE-constraint failure — i.e. a dispatch lease was already held
/// by a concurrent claimer. Matched on the message because sqlx surfaces it as
/// an opaque `DbErr`.
fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    e.to_string().to_lowercase().contains("unique")
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
    let res = loop_artifact::Entity::update_many()
        .col_expr(loop_artifact::Column::Status, Expr::value(new.to_value()))
        .col_expr(loop_artifact::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_artifact::Column::Id.eq(id))
        .filter(loop_artifact::Column::Status.eq(expected))
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
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
    let res = loop_iteration::Entity::update_many()
        .col_expr(loop_iteration::Column::Status, Expr::value(new.to_value()))
        .filter(loop_iteration::Column::Id.eq(id))
        .filter(loop_iteration::Column::Status.eq(expected))
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

/// Acquire the per-issue serial-task pipeline gate for `task_artifact_id`. Wins
/// (`true`) only when no task currently holds it (`active_task_artifact_id IS
/// NULL`). Keeps two tasks of one issue from sharing the worktree.
pub async fn try_acquire_task_gate(
    conn: &DatabaseConnection,
    issue_id: i32,
    task_artifact_id: i32,
) -> Result<bool, LoopError> {
    let res = loop_issue::Entity::update_many()
        .col_expr(
            loop_issue::Column::ActiveTaskArtifactId,
            Expr::value(task_artifact_id),
        )
        .filter(loop_issue::Column::Id.eq(issue_id))
        .filter(loop_issue::Column::ActiveTaskArtifactId.is_null())
        .exec(conn)
        .await?;
    Ok(res.rows_affected == 1)
}

/// Release the task gate, but only if it is still held by `task_artifact_id`.
pub async fn release_task_gate(
    conn: &DatabaseConnection,
    issue_id: i32,
    task_artifact_id: i32,
) -> Result<bool, LoopError> {
    let res = loop_issue::Entity::update_many()
        .col_expr(
            loop_issue::Column::ActiveTaskArtifactId,
            Expr::value(Option::<i32>::None),
        )
        .filter(loop_issue::Column::Id.eq(issue_id))
        .filter(loop_issue::Column::ActiveTaskArtifactId.eq(task_artifact_id))
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
            &IssueConfig::default(),
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
    async fn write_lease_blocks_second_implement_per_issue() {
        let (db, space_id, issue_id) = seed().await;
        let task = artifact::create_artifact(&db.conn, space_id, issue_id, ArtifactKind::Task, "T", ArtifactStatus::Pending, ActorKind::Agent, None).await.unwrap();
        let first = try_claim_iteration(&db.conn, claim(space_id, issue_id, Stage::Implement, Some(task.id), None, "tok-a")).await.unwrap();
        assert!(first.is_some());
        // Same issue, another implement → uniq_active_write blocks it.
        let second = try_claim_iteration(&db.conn, claim(space_id, issue_id, Stage::Implement, Some(task.id), None, "tok-b")).await.unwrap();
        assert!(second.is_none(), "second implement on the issue is leased out");
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
    async fn task_gate_serializes_then_releases() {
        let (db, _space, issue_id) = seed().await;
        assert!(try_acquire_task_gate(&db.conn, issue_id, 100).await.unwrap());
        // A different task cannot acquire while 100 holds the gate.
        assert!(!try_acquire_task_gate(&db.conn, issue_id, 200).await.unwrap());
        // Releasing with the wrong task is a no-op.
        assert!(!release_task_gate(&db.conn, issue_id, 200).await.unwrap());
        // Correct release frees the gate.
        assert!(release_task_gate(&db.conn, issue_id, 100).await.unwrap());
        assert!(try_acquire_task_gate(&db.conn, issue_id, 200).await.unwrap());
    }
}
