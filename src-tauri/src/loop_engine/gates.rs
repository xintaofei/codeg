//! Write-pipeline stage gates (§4.5): implement → validate → review → finalize.
//!
//! M2.2 Task 2.1 lands **implement**, the first stage that changes code. It is
//! unlike the read stages in two ways:
//!
//! - **No submission.** The implement agent edits files in the worktree and
//!   calls no `loop_submit_*` tool (its briefing tool-contract says so). The
//!   engine measures progress by *checkpointing*: a non-empty diff that commits
//!   is success; an empty diff is no progress.
//! - **Serial per issue.** The per-issue task gate (`active_task_artifact_id`)
//!   lets only one task occupy the worktree at a time, so two tasks never race
//!   on the same tree. The gate is acquired when a task starts implementing and
//!   released only when it finishes review (M2.3) — so in M2.1+2.1 a task that
//!   implements holds the gate through to its (not-yet-built) validation.
//!
//! Idempotency across ticks keys on `iteration.attempt == task.attempt`: a
//! settled implement iteration is only checkpointed once, because a no-progress
//! checkpoint bumps the task's rework counter and the next dispatch carries the
//! new attempt.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveEnum, ColumnTrait, EntityTrait, QueryFilter};

use crate::db::entities::loop_artifact::{self, ArtifactKind, ArtifactStatus, ReviewVerdict};
use crate::db::entities::loop_inbox_item::InboxKind;
use crate::db::entities::loop_issue::{self, IssueStatus};
use crate::db::entities::loop_iteration::{self, IterationStatus, Stage};
use crate::db::service::{folder_service, loop_service};
use crate::db::AppDatabase;
use crate::models::loops::{IssueConfig, LoopArtifactRow, LoopDagView};
use crate::web::event_bridge::EventEmitter;

use crate::loop_engine::dispatch::{dispatch_iteration, DispatchInput, LoopAgentSpawner};
use crate::loop_engine::driver::resolve_agent;
use crate::loop_engine::error::LoopError;
use crate::loop_engine::transitions::{
    cas_issue_status, cas_iteration_status, release_task_gate, try_acquire_task_gate,
};
use crate::loop_engine::validation::{self, ValidationOutcome};
use crate::loop_engine::worktree;

/// Outcome of checkpointing + validating a settled implement iteration.
enum ImplementOutcome {
    /// Non-empty diff committed and validation passed (or none configured) → task
    /// promoted to `in_progress` (implemented, awaiting review).
    Advanced,
    /// Empty diff, or validation reported failures → rework counter bumped; the
    /// caller re-dispatches implement at the next attempt.
    NoProgress,
    /// The task was blocked — either validation could not run (missing tool /
    /// timeout) or a no-progress breaker tripped (max attempts / repeated
    /// failure). An inbox card is filed; the caller idles until a human
    /// intervenes.
    Blocked,
}

/// Drive the active task through the write pipeline (implement → validate →
/// review) for one tick. Returns `true` when it dispatched a new iteration (the
/// caller maps that to a `Dispatched` tick).
///
/// A no-op while no task exists yet (read stages still in flight), so the driver
/// can call it on every "read frontier empty" tick.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_active_task(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    dag: &LoopDagView,
    config: &IssueConfig,
    worktree_folder_id: i32,
) -> Result<bool, LoopError> {
    match issue.active_task_artifact_id {
        // A task already holds the gate → advance it.
        Some(active) => {
            advance_active_task(
                db,
                data_dir,
                spawner,
                emitter,
                issue,
                dag,
                config,
                worktree_folder_id,
                active,
            )
            .await
        }
        // Gate free → claim it for the next task awaiting implement and start.
        None => {
            let Some(task) = next_pending_task(dag) else {
                return Ok(false);
            };
            if try_acquire_task_gate(&db.conn, issue.id, task.id).await? {
                dispatch_implement(
                    db,
                    data_dir,
                    spawner,
                    emitter,
                    issue,
                    config,
                    worktree_folder_id,
                    task.id,
                    task.attempt,
                )
                .await
            } else {
                // Lost the gate race to a concurrent driver tick — try next time.
                Ok(false)
            }
        }
    }
}

/// The next task awaiting implement: the lowest-ordered `pending` task node.
fn next_pending_task(dag: &LoopDagView) -> Option<&LoopArtifactRow> {
    dag.artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::Task && a.status == ArtifactStatus::Pending)
        .min_by(|a, b| a.sort.cmp(&b.sort).then(a.id.cmp(&b.id)))
}

/// Route the gate-holding task to its write-pipeline stage by status: `pending`
/// implements, `in_progress` (implemented + validated) reviews, terminal idles.
#[allow(clippy::too_many_arguments)]
async fn advance_active_task(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    dag: &LoopDagView,
    config: &IssueConfig,
    worktree_folder_id: i32,
    active_task_id: i32,
) -> Result<bool, LoopError> {
    let Some(task) = dag.artifacts.iter().find(|a| a.id == active_task_id) else {
        // Gate points at a node not in this DAG — nothing to drive.
        return Ok(false);
    };
    match task.status {
        ArtifactStatus::Pending => {
            advance_implement(db, data_dir, spawner, emitter, issue, config, worktree_folder_id, task)
                .await
        }
        ArtifactStatus::InProgress => {
            drive_reviews(db, data_dir, spawner, emitter, issue, config, worktree_folder_id, task)
                .await
        }
        // Done (gate released on review pass), blocked, cancelled, etc. → idle.
        _ => Ok(false),
    }
}

/// Advance a `pending` task's implement: wait while its iteration is in flight,
/// checkpoint + validate once settled, or (re)dispatch when nothing is live.
#[allow(clippy::too_many_arguments)]
async fn advance_implement(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_folder_id: i32,
    task: &LoopArtifactRow,
) -> Result<bool, LoopError> {
    let impls = implement_iterations(db, issue.id, task.id).await?;
    if impls
        .iter()
        .any(|it| matches!(it.status, IterationStatus::Queued | IterationStatus::Running))
    {
        // Implement in flight — wait for its completion to wake us.
        return Ok(false);
    }

    // A succeeded implement at the current attempt is awaiting its checkpoint +
    // validation.
    let settled = impls
        .iter()
        .find(|it| it.status == IterationStatus::Succeeded && it.attempt == task.attempt);
    if let Some(settled) = settled {
        match finish_implement(db, issue, config, worktree_folder_id, task, settled.id).await? {
            // Advanced (validated) or Blocked (validation can't run) both idle —
            // review or a human takes over next.
            ImplementOutcome::Advanced | ImplementOutcome::Blocked => Ok(false),
            ImplementOutcome::NoProgress => {
                // The rework counter was bumped; retry implement at the new attempt.
                dispatch_implement(
                    db,
                    data_dir,
                    spawner,
                    emitter,
                    issue,
                    config,
                    worktree_folder_id,
                    task.id,
                    task.attempt + 1,
                )
                .await
            }
        }
    } else {
        // Gate held but nothing live or freshly settled (just acquired, or a
        // prior attempt already processed) → (re)dispatch implement.
        dispatch_implement(
            db,
            data_dir,
            spawner,
            emitter,
            issue,
            config,
            worktree_folder_id,
            task.id,
            task.attempt,
        )
        .await
    }
}

/// Checkpoint, then validate, a settled implement iteration. An empty diff is
/// discarded as no progress; a committed diff is handed to validation, whose
/// outcome decides advance / rework / block.
async fn finish_implement(
    db: &AppDatabase,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_folder_id: i32,
    task: &LoopArtifactRow,
    iteration_id: i32,
) -> Result<ImplementOutcome, LoopError> {
    let conn = &db.conn;
    let folder = folder_service::get_folder_by_id(conn, worktree_folder_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("worktree folder {worktree_folder_id}")))?;
    let worktree_path = Path::new(&folder.path);

    let message = format!("loop: implement #{} (issue #{})", task.id, issue.seq_no);
    match worktree::checkpoint(worktree_path, &message).await? {
        Some(_sha) => {
            validate_after_implement(db, issue, config, worktree_path, task, iteration_id).await
        }
        None => {
            // No diff to accept. Discard any stray uncommitted state and record a
            // no-progress rework; the breaker decides retry vs. block.
            worktree::reset_to_head(worktree_path).await?;
            match record_rework(db, issue, config, task, Some(iteration_id), "empty_diff:implement")
                .await?
            {
                ReworkOutcome::Retry => Ok(ImplementOutcome::NoProgress),
                ReworkOutcome::Blocked => Ok(ImplementOutcome::Blocked),
            }
        }
    }
}

/// Run the issue's `validation_commands` against the freshly committed checkpoint
/// and map the result onto an [`ImplementOutcome`]:
///
/// - no commands configured → straight to `in_progress` (nothing to check);
/// - passed → `in_progress` (implemented, awaiting review);
/// - failed → rework (bump attempt; the recorded output feeds the next briefing);
/// - unrunnable → block the task + file a `blocked` inbox card.
///
/// The worktree is reset to HEAD afterward so build artifacts the commands
/// produced don't leak into the next attempt — the checkpoint commit stays, as
/// `reset_to_head` only clears uncommitted side-effects.
async fn validate_after_implement(
    db: &AppDatabase,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_path: &Path,
    task: &LoopArtifactRow,
    iteration_id: i32,
) -> Result<ImplementOutcome, LoopError> {
    let commands = &config.validation_commands;
    if commands.is_empty() {
        set_task_status(db, task.id, ArtifactStatus::InProgress).await?;
        return Ok(ImplementOutcome::Advanced);
    }

    let timeout = config.iteration_timeout_secs.map(Duration::from_secs);
    let report = validation::run_validation(worktree_path, commands, timeout).await?;
    worktree::reset_to_head(worktree_path).await?;
    loop_service::validation::record_validation_run(
        &db.conn,
        issue.space_id,
        issue.id,
        task.id,
        Some(iteration_id),
        commands,
        &report.exit_codes,
        &report.output,
        report.passed(),
    )
    .await?;

    match report.outcome {
        ValidationOutcome::Passed => {
            set_task_status(db, task.id, ArtifactStatus::InProgress).await?;
            Ok(ImplementOutcome::Advanced)
        }
        ValidationOutcome::Failed => {
            // Fingerprint the failure so the breaker can tell "the same failure
            // again" from a genuinely new one.
            let sig = format!(
                "validation_failed:{}",
                sig_hash(&format!("{:?}\n{}", report.exit_codes, report.output))
            );
            match record_rework(db, issue, config, task, Some(iteration_id), &sig).await? {
                ReworkOutcome::Retry => Ok(ImplementOutcome::NoProgress),
                ReworkOutcome::Blocked => Ok(ImplementOutcome::Blocked),
            }
        }
        ValidationOutcome::Unrunnable => {
            set_task_status(db, task.id, ArtifactStatus::Blocked).await?;
            loop_service::inbox::upsert_inbox(
                &db.conn,
                issue.space_id,
                issue.id,
                Some(iteration_id),
                InboxKind::Blocked,
                &format!("validation_blocked:{}", task.id),
                serde_json::json!({
                    "task_artifact_id": task.id,
                    "reason": "validation_unrunnable",
                    "commands": commands,
                    "exit_codes": report.exit_codes,
                }),
            )
            .await?;
            Ok(ImplementOutcome::Blocked)
        }
    }
}

/// Dispatch an implement iteration for `task_id` at `attempt`. Returns `true`
/// when a new iteration was actually launched (the lease was free).
#[allow(clippy::too_many_arguments)]
async fn dispatch_implement(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_folder_id: i32,
    task_id: i32,
    attempt: i32,
) -> Result<bool, LoopError> {
    let handle = dispatch_iteration(
        db,
        data_dir,
        spawner,
        emitter.clone(),
        DispatchInput {
            space_id: issue.space_id,
            issue_id: issue.id,
            stage: Stage::Implement,
            target_artifact_id: Some(task_id),
            slot_no: None,
            attempt,
            agent_type: resolve_agent(config, Stage::Implement),
            worktree_folder_id,
        },
    )
    .await?;
    Ok(handle.is_some())
}

async fn implement_iterations(
    db: &AppDatabase,
    issue_id: i32,
    task_id: i32,
) -> Result<Vec<loop_iteration::Model>, LoopError> {
    Ok(loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .filter(loop_iteration::Column::Stage.eq(Stage::Implement))
        .filter(loop_iteration::Column::TargetArtifactId.eq(task_id))
        .all(&db.conn)
        .await?)
}

async fn set_task_status(
    db: &AppDatabase,
    task_id: i32,
    status: ArtifactStatus,
) -> Result<(), LoopError> {
    loop_artifact::Entity::update_many()
        .col_expr(loop_artifact::Column::Status, Expr::value(status.to_value()))
        .col_expr(loop_artifact::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_artifact::Column::Id.eq(task_id))
        .exec(&db.conn)
        .await?;
    Ok(())
}

async fn bump_rework(db: &AppDatabase, task_id: i32, sig: &str) -> Result<(), LoopError> {
    loop_artifact::Entity::update_many()
        .col_expr(
            loop_artifact::Column::Attempt,
            Expr::col(loop_artifact::Column::Attempt).add(1),
        )
        .col_expr(
            loop_artifact::Column::LastFailureSig,
            Expr::value(sig.to_string()),
        )
        .col_expr(loop_artifact::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(loop_artifact::Column::Id.eq(task_id))
        .exec(&db.conn)
        .await?;
    Ok(())
}

// ---- Circuit breakers (§4.10): no-progress + max-attempts ----

/// Whether a recorded rework should retry or has tripped a breaker.
enum ReworkOutcome {
    /// The rework counter advanced; the caller may re-dispatch at the new attempt.
    Retry,
    /// A breaker tripped — the task + issue are now `blocked` and an inbox card is
    /// filed. The caller must not re-dispatch.
    Blocked,
}

/// Record one failed attempt against `task` and evaluate the no-progress
/// breakers. Bumps the rework counter + failure signature, then blocks (task +
/// issue → `blocked`, inbox card) when either:
///
/// - the task has exhausted `max_attempts` (`attempt >= max_attempts` after the
///   bump; `0` = unlimited), or
/// - this failure repeats the immediately-preceding signature — the agent is
///   producing the identical failure, so further attempts won't help.
///
/// Returns [`ReworkOutcome::Retry`] otherwise.
async fn record_rework(
    db: &AppDatabase,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    task: &LoopArtifactRow,
    iteration_id: Option<i32>,
    sig: &str,
) -> Result<ReworkOutcome, LoopError> {
    let prev_sig = loop_artifact::Entity::find_by_id(task.id)
        .one(&db.conn)
        .await?
        .and_then(|m| m.last_failure_sig);
    let repeated = prev_sig.as_deref() == Some(sig);

    bump_rework(db, task.id, sig).await?;
    let attempt = task.attempt + 1;

    let max = config.max_attempts as i32;
    let exhausted = max > 0 && attempt >= max;
    if exhausted || repeated {
        let reason = if exhausted {
            "max_attempts"
        } else {
            "repeated_failure"
        };
        mark_blocked(db, issue, task.id, iteration_id, reason, sig, attempt).await?;
        Ok(ReworkOutcome::Blocked)
    } else {
        Ok(ReworkOutcome::Retry)
    }
}

/// Block a stalled node: set the task `blocked`, CAS the issue `running →
/// blocked` (so the driver stops on its next tick), and file a `blocked` inbox
/// card keyed on the task (deduped on `no_progress:{task}`). A human resolves it
/// via the inbox (M2.2 Task 2.7).
#[allow(clippy::too_many_arguments)]
async fn mark_blocked(
    db: &AppDatabase,
    issue: &loop_issue::Model,
    task_id: i32,
    iteration_id: Option<i32>,
    reason: &str,
    sig: &str,
    attempt: i32,
) -> Result<(), LoopError> {
    set_task_status(db, task_id, ArtifactStatus::Blocked).await?;
    cas_issue_status(&db.conn, issue.id, IssueStatus::Running, IssueStatus::Blocked).await?;
    loop_service::inbox::upsert_inbox(
        &db.conn,
        issue.space_id,
        issue.id,
        iteration_id,
        InboxKind::Blocked,
        &format!("no_progress:{task_id}"),
        serde_json::json!({
            "task_artifact_id": task_id,
            "reason": reason,
            "failure_sig": sig,
            "attempt": attempt,
        }),
    )
    .await?;
    Ok(())
}

/// Stable 64-bit FNV-1a fingerprint (hex) of a failure's specifics, so the
/// repeated-failure breaker can compare "same failure" without storing the full
/// output in the signature column.
fn sig_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

// ---- Review stage (§4.7) ----

/// The outcome of a review round under the configured pass rule.
enum ReviewDecision {
    /// Enough passes to accept the implementation — task is done.
    Pass,
    /// A reviewer rejected (or a passing quorum is no longer reachable) → rework.
    Fail,
    /// Not enough verdicts in yet — dispatch / await more reviewers.
    Undecided,
}

/// Drive an `in_progress` (implemented + validated) task through its review
/// round: ensure `reviewer_count` review slots run, aggregate their verdicts,
/// then accept (task `done` + release the task gate) or reject (rework + cancel
/// the remaining reviewers). Returns `true` only when it dispatched a reviewer.
#[allow(clippy::too_many_arguments)]
async fn drive_reviews(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_folder_id: i32,
    task: &LoopArtifactRow,
) -> Result<bool, LoopError> {
    let reviewers = config.reviewer_count.max(1) as i32;
    let iters = review_iterations(db, issue.id, task.id, task.attempt).await?;
    let verdicts = review_verdicts(db, &iters).await?;

    // Resolve each slot to a verdict (decided), in-flight, or needing dispatch.
    let mut decided: Vec<ReviewVerdict> = Vec::new();
    let mut missing_slots: Vec<i32> = Vec::new();
    for slot in 0..reviewers {
        let slot_iters: Vec<&loop_iteration::Model> =
            iters.iter().filter(|it| it.slot_no == Some(slot)).collect();
        if let Some(v) = slot_iters.iter().find_map(|it| verdicts.get(&it.id).copied()) {
            decided.push(v);
        } else if !slot_iters
            .iter()
            .any(|it| matches!(it.status, IterationStatus::Queued | IterationStatus::Running))
        {
            // No iteration, or only terminal ones without a verdict → (re)dispatch.
            missing_slots.push(slot);
        }
    }

    match aggregate(&config.review_pass_rule, reviewers, &decided) {
        ReviewDecision::Pass => {
            cancel_active_reviews(db, &iters).await?;
            set_task_status(db, task.id, ArtifactStatus::Done).await?;
            release_task_gate(&db.conn, issue.id, task.id).await?;
            Ok(false)
        }
        ReviewDecision::Fail => {
            cancel_active_reviews(db, &iters).await?;
            // Defensive: clear any reviewer side-effects before re-implementing.
            let folder = folder_service::get_folder_by_id(&db.conn, worktree_folder_id)
                .await?
                .ok_or_else(|| {
                    LoopError::NotFound(format!("worktree folder {worktree_folder_id}"))
                })?;
            worktree::reset_to_head(Path::new(&folder.path)).await?;
            // Fingerprint the rejecting findings so the breaker can tell "the same
            // objection again" from a genuinely new one.
            let findings =
                loop_service::artifact::latest_failed_review_findings(&db.conn, task.id).await?;
            let sig = format!("review_rejected:{}", sig_hash(&findings.join("\n---\n")));
            match record_rework(db, issue, config, task, None, &sig).await? {
                // Back to awaiting implement at the new attempt.
                ReworkOutcome::Retry => set_task_status(db, task.id, ArtifactStatus::Pending).await?,
                // Breaker tripped: `mark_blocked` already set the task + issue
                // blocked; leave them as-is.
                ReworkOutcome::Blocked => {}
            }
            Ok(false)
        }
        ReviewDecision::Undecided => {
            let mut dispatched = false;
            for slot in missing_slots {
                if dispatch_review(
                    db,
                    data_dir,
                    spawner,
                    emitter,
                    issue,
                    config,
                    worktree_folder_id,
                    task.id,
                    slot,
                    task.attempt,
                )
                .await?
                {
                    dispatched = true;
                }
            }
            Ok(dispatched)
        }
    }
}

/// Aggregate review verdicts under the pass rule. `unanimous` fails fast on any
/// fail and accepts only when all `n` slots pass; `majority` accepts on
/// `pass*2 > n` and rejects once a passing majority is unreachable (an even
/// split rejects).
fn aggregate(rule: &str, n: i32, verdicts: &[ReviewVerdict]) -> ReviewDecision {
    let pass = verdicts
        .iter()
        .filter(|v| matches!(v, ReviewVerdict::Pass))
        .count() as i32;
    let fail = verdicts.len() as i32 - pass;
    if rule == "majority" {
        if pass * 2 > n {
            ReviewDecision::Pass
        } else if fail * 2 >= n {
            ReviewDecision::Fail
        } else {
            ReviewDecision::Undecided
        }
    } else if fail >= 1 {
        // "unanimous" (default): any fail rejects; all-pass accepts.
        ReviewDecision::Fail
    } else if pass >= n {
        ReviewDecision::Pass
    } else {
        ReviewDecision::Undecided
    }
}

/// Dispatch one review slot. Returns `true` when a new iteration launched (the
/// review-slot lease was free).
#[allow(clippy::too_many_arguments)]
async fn dispatch_review(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_folder_id: i32,
    task_id: i32,
    slot: i32,
    attempt: i32,
) -> Result<bool, LoopError> {
    let handle = dispatch_iteration(
        db,
        data_dir,
        spawner,
        emitter.clone(),
        DispatchInput {
            space_id: issue.space_id,
            issue_id: issue.id,
            stage: Stage::Review,
            target_artifact_id: Some(task_id),
            slot_no: Some(slot),
            attempt,
            agent_type: resolve_agent(config, Stage::Review),
            worktree_folder_id,
        },
    )
    .await?;
    Ok(handle.is_some())
}

/// Invalidate any still-active reviewers (a decision was reached without them).
/// CAS to `cancelled` voids the capability token — `ingest` rejects a submit
/// from a non-running iteration — so a late verdict can't change the outcome.
/// Killing the agent process is Task 2.6.
async fn cancel_active_reviews(
    db: &AppDatabase,
    iters: &[loop_iteration::Model],
) -> Result<(), LoopError> {
    for it in iters {
        if matches!(it.status, IterationStatus::Queued | IterationStatus::Running)
            && cas_iteration_status(&db.conn, it.id, it.status, IterationStatus::Cancelled).await?
        {
            loop_iteration::Entity::update_many()
                .col_expr(loop_iteration::Column::EndedAt, Expr::value(Utc::now()))
                .filter(loop_iteration::Column::Id.eq(it.id))
                .exec(&db.conn)
                .await?;
        }
    }
    Ok(())
}

async fn review_iterations(
    db: &AppDatabase,
    issue_id: i32,
    task_id: i32,
    attempt: i32,
) -> Result<Vec<loop_iteration::Model>, LoopError> {
    Ok(loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .filter(loop_iteration::Column::Stage.eq(Stage::Review))
        .filter(loop_iteration::Column::TargetArtifactId.eq(task_id))
        .filter(loop_iteration::Column::Attempt.eq(attempt))
        .all(&db.conn)
        .await?)
}

/// Map each succeeded review iteration to the verdict of the review artifact it
/// produced.
async fn review_verdicts(
    db: &AppDatabase,
    iters: &[loop_iteration::Model],
) -> Result<HashMap<i32, ReviewVerdict>, LoopError> {
    let succeeded: Vec<i32> = iters
        .iter()
        .filter(|it| it.status == IterationStatus::Succeeded)
        .map(|it| it.id)
        .collect();
    if succeeded.is_empty() {
        return Ok(HashMap::new());
    }
    let mut map = HashMap::new();
    for art in loop_artifact::Entity::find()
        .filter(loop_artifact::Column::Kind.eq(ArtifactKind::Review))
        .filter(loop_artifact::Column::ProducedByIterationId.is_in(succeeded))
        .all(&db.conn)
        .await?
    {
        if let (Some(iter_id), Some(verdict)) = (art.produced_by_iteration_id, art.verdict) {
            map.insert(iter_id, verdict);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::error::AcpError;
    use crate::db::entities::loop_artifact_revision::ActorKind;
    use crate::db::entities::loop_issue::{IssuePriority, IssueStatus};
    use crate::db::entities::loop_link::LinkKind;
    use crate::db::service::loop_service::{artifact, issue, link, space};
    use crate::db::test_helpers::{fresh_disk_db, seed_folder};
    use crate::loop_engine::dispatch::settle_iteration;
    use crate::models::agent::AgentType;
    use async_trait::async_trait;
    use std::path::{Path, PathBuf};
    use std::process::Command as StdCommand;

    /// Minimal spawner: the "agent" is simulated by the test mutating the
    /// worktree directly, so the stub only needs to hand back a connection id.
    struct StubSpawner;

    #[async_trait]
    impl LoopAgentSpawner for StubSpawner {
        async fn spawn_loop_agent(
            &self,
            _db: &AppDatabase,
            _data_dir: &Path,
            _agent_type: AgentType,
            _working_dir: String,
            _emitter: EventEmitter,
            _capability_token: String,
        ) -> Result<String, AcpError> {
            Ok("loop-conn".to_string())
        }
        async fn send_loop_prompt(
            &self,
            _db: &AppDatabase,
            _conn_id: &str,
            _text: String,
            _folder_id: i32,
            _conversation_id: i32,
        ) -> Result<(), AcpError> {
            Ok(())
        }
        async fn disconnect_loop_agent(&self, _conn_id: &str) {}
    }

    fn git(dir: &Path, args: &[&str]) {
        let st = StdCommand::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("spawn git");
        assert!(st.success(), "git {args:?} failed");
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "t@example.com"]);
        git(dir, &["config", "user.name", "tester"]);
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    struct Harness {
        db: AppDatabase,
        data: tempfile::TempDir,
        _repo: tempfile::TempDir,
        issue_id: i32,
        space_id: i32,
        worktree_folder_id: i32,
        worktree_path: PathBuf,
    }

    /// Real git repo + worktree + a running issue. Returns a harness whose
    /// tempdirs stay alive for the test's duration.
    async fn setup() -> Harness {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let data = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(data.path()).await;
        let folder_id = seed_folder(&db, &repo.path().to_string_lossy()).await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &db.conn,
            space.id,
            "Build",
            "do the thing",
            IssuePriority::Medium,
            &IssueConfig::default(),
        )
        .await
        .unwrap();
        let ctx = worktree::ensure_worktree(&db.conn, data.path(), issue.row.id)
            .await
            .unwrap();
        // Mark the issue running (trigger would do this).
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Status,
                Expr::value(IssueStatus::Running.to_value()),
            )
            .filter(loop_issue::Column::Id.eq(issue.row.id))
            .exec(&db.conn)
            .await
            .unwrap();
        Harness {
            db,
            data,
            _repo: repo,
            issue_id: issue.row.id,
            space_id: space.id,
            worktree_folder_id: ctx.worktree_folder_id,
            worktree_path: ctx.worktree_path,
        }
    }

    /// Mint a pending task node linked to the issue root (as the plan stage does).
    async fn add_task(h: &Harness, title: &str) -> i32 {
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        let root = dag
            .artifacts
            .iter()
            .find(|a| a.kind == ArtifactKind::Issue)
            .unwrap()
            .id;
        let task = artifact::create_artifact(
            &h.db.conn,
            h.space_id,
            h.issue_id,
            ArtifactKind::Task,
            title,
            ArtifactStatus::Pending,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        link::create_link(&h.db.conn, h.space_id, task.id, root, LinkKind::DerivesFrom)
            .await
            .unwrap();
        task.id
    }

    async fn load_issue(h: &Harness) -> loop_issue::Model {
        loop_issue::Entity::find_by_id(h.issue_id)
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap()
    }

    async fn drive(h: &Harness) -> bool {
        let issue = load_issue(h).await;
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        drive_active_task(
            &h.db,
            h.data.path(),
            &StubSpawner,
            &EventEmitter::Noop,
            &issue,
            &dag,
            &IssueConfig::default(),
            h.worktree_folder_id,
        )
        .await
        .unwrap()
    }

    async fn running_implement_id(h: &Harness) -> i32 {
        loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Implement))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .one(&h.db.conn)
            .await
            .unwrap()
            .expect("a running implement iteration")
            .id
    }

    async fn task_node(h: &Harness, id: i32) -> LoopArtifactRow {
        artifact::list_dag(&h.db.conn, h.issue_id)
            .await
            .unwrap()
            .artifacts
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
    }

    /// The raw artifact row — for fields the DAG DTO omits (e.g. last_failure_sig).
    async fn task_model(h: &Harness, id: i32) -> loop_artifact::Model {
        loop_artifact::Entity::find_by_id(id)
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn gate_serializes_implement_to_one_task() {
        let h = setup().await;
        let t1 = add_task(&h, "Task 1").await;
        let t2 = add_task(&h, "Task 2").await;

        // First tick claims the gate for the lowest-ordered task and dispatches.
        assert!(drive(&h).await, "first tick dispatches an implement");
        let issue = load_issue(&h).await;
        assert_eq!(issue.active_task_artifact_id, Some(t1), "gate held by task 1");

        // A second tick (no completion yet) must not start the other task.
        assert!(!drive(&h).await, "no second implement while the gate is held");
        let iters = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Implement))
            .all(&h.db.conn)
            .await
            .unwrap();
        assert_eq!(iters.len(), 1, "exactly one implement iteration");
        assert_eq!(iters[0].target_artifact_id, Some(t1));
        // Task 2 never got an iteration.
        assert!(implement_iterations(&h.db, h.issue_id, t2)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn implement_success_checkpoints_and_advances() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;

        // Tick 1: dispatch implement for the task.
        assert!(drive(&h).await);
        let iter_id = running_implement_id(&h).await;

        // The agent makes a change in the worktree (non-empty diff), then the
        // turn settles.
        std::fs::write(h.worktree_path.join("feature.txt"), "new code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();
        // Settlement must not bump the task's rework counter for implement.
        assert_eq!(task_node(&h, task).await.attempt, 0);

        // Tick 2: checkpoint the diff → commit + promote the task.
        assert!(!drive(&h).await, "checkpoint/advance is not a dispatch");
        let node = task_node(&h, task).await;
        assert_eq!(node.status, ArtifactStatus::InProgress, "task implemented");
        assert_eq!(node.attempt, 0, "successful implement does not bump attempt");

        // The change was committed onto the issue branch (HEAD advanced) and the
        // tree is clean.
        let log = StdCommand::new("git")
            .args(["log", "--oneline"])
            .current_dir(&h.worktree_path)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(log.contains("implement"), "checkpoint commit present:\n{log}");
        let status = StdCommand::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&h.worktree_path)
            .output()
            .unwrap();
        assert!(status.stdout.is_empty(), "worktree clean after checkpoint");

        // Gate is still held by the task (released only after review, M2.3).
        assert_eq!(load_issue(&h).await.active_task_artifact_id, Some(task));
    }

    #[tokio::test]
    async fn implement_empty_diff_counts_no_progress() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;

        assert!(drive(&h).await);
        let iter_id = running_implement_id(&h).await;
        // Agent produced no change. Settle, then drive: empty diff → no progress.
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // Tick 2: checkpoint finds nothing → rework bump + retry dispatch.
        assert!(drive(&h).await, "no-progress retries implement");
        let node = task_node(&h, task).await;
        assert_eq!(node.attempt, 1, "rework counter bumped");
        assert_eq!(node.status, ArtifactStatus::Pending, "still awaiting implement");
        assert_eq!(
            task_model(&h, task).await.last_failure_sig.as_deref(),
            Some("empty_diff:implement")
        );
        // The retry is a fresh implement iteration at the new attempt.
        let running = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Implement))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(running.attempt, 1);
    }

    // ---- Task 2.2: deterministic validation after implement ----

    fn config_with_validation(cmds: &[&str]) -> IssueConfig {
        IssueConfig {
            validation_commands: cmds.iter().map(|s| s.to_string()).collect(),
            ..IssueConfig::default()
        }
    }

    async fn drive_with(h: &Harness, config: &IssueConfig) -> bool {
        let issue = load_issue(h).await;
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        drive_active_task(
            &h.db,
            h.data.path(),
            &StubSpawner,
            &EventEmitter::Noop,
            &issue,
            &dag,
            config,
            h.worktree_folder_id,
        )
        .await
        .unwrap()
    }

    /// Implement → checkpoint → validation passes → task implemented (in_progress).
    #[cfg(unix)]
    #[tokio::test]
    async fn implement_passing_validation_advances() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_with_validation(&["true"]);

        assert!(drive_with(&h, &cfg).await, "tick 1 dispatches implement");
        let iter_id = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // Tick 2: checkpoint + validation(pass) → advance (not a dispatch).
        assert!(!drive_with(&h, &cfg).await);
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::InProgress);
        let runs = loop_service::validation::list_for_task(&h.db.conn, task)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1, "one validation run recorded");
        assert!(runs[0].passed, "run passed");
    }

    /// Implement → checkpoint → validation fails → rework (attempt++, re-dispatch).
    #[cfg(unix)]
    #[tokio::test]
    async fn implement_failing_validation_reworks() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_with_validation(&["false"]);

        assert!(drive_with(&h, &cfg).await);
        let iter_id = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // Tick 2: checkpoint + validation(fail) → rework + re-dispatch implement.
        assert!(drive_with(&h, &cfg).await, "validation failure retries implement");
        let node = task_node(&h, task).await;
        assert_eq!(node.attempt, 1, "rework counter bumped");
        assert_eq!(
            node.status,
            ArtifactStatus::Pending,
            "back to awaiting implement"
        );
        assert!(
            task_model(&h, task)
                .await
                .last_failure_sig
                .as_deref()
                .unwrap()
                .starts_with("validation_failed:"),
            "failure signature records a validation failure"
        );
        let runs = loop_service::validation::list_for_task(&h.db.conn, task)
            .await
            .unwrap();
        assert!(!runs[0].passed, "failing run recorded");
        // The retry is a fresh implement iteration at the new attempt.
        let running = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Implement))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(running.attempt, 1);
    }

    /// Implement → checkpoint → validation can't run (missing tool) → task blocked
    /// + inbox card; no rework (not the agent's fault), no further dispatch.
    #[cfg(unix)]
    #[tokio::test]
    async fn implement_unrunnable_validation_blocks() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_with_validation(&["codeg-no-such-tool-xyzzy"]);

        assert!(drive_with(&h, &cfg).await);
        let iter_id = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // Tick 2: checkpoint + validation(unrunnable) → block (not a dispatch).
        assert!(
            !drive_with(&h, &cfg).await,
            "unrunnable validation does not retry"
        );
        let node = task_node(&h, task).await;
        assert_eq!(node.status, ArtifactStatus::Blocked);
        assert_eq!(node.attempt, 0, "config error does not consume a rework");
        // A blocked inbox card was filed for the task.
        let inbox = loop_service::inbox::list_inbox(&h.db.conn, h.space_id, None)
            .await
            .unwrap();
        assert!(
            inbox.iter().any(|i| i.kind == InboxKind::Blocked
                && i.subject_key == format!("validation_blocked:{task}")),
            "blocked inbox card filed"
        );
        // The gate is still held by the task; no new implement was dispatched.
        assert_eq!(load_issue(&h).await.active_task_artifact_id, Some(task));
    }

    // ---- Task 2.3: review stage ----

    fn config_reviewers(n: u32, rule: &str) -> IssueConfig {
        IssueConfig {
            reviewer_count: n,
            review_pass_rule: rule.to_string(),
            ..IssueConfig::default()
        }
    }

    /// Drive a fresh task from pending to `in_progress` (implemented + validated)
    /// so review tests can start at the review stage.
    async fn implement_to_in_progress(h: &Harness, cfg: &IssueConfig, marker: &str) {
        assert!(drive_with(h, cfg).await, "dispatch implement");
        let iter_id = running_implement_id(h).await;
        std::fs::write(h.worktree_path.join(marker), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();
        assert!(!drive_with(h, cfg).await, "checkpoint + validate → in_progress");
    }

    async fn running_review(h: &Harness) -> i32 {
        loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Review))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .one(&h.db.conn)
            .await
            .unwrap()
            .expect("a running review iteration")
            .id
    }

    async fn review_iters_of(h: &Harness, task: i32) -> Vec<loop_iteration::Model> {
        let mut v = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Review))
            .filter(loop_iteration::Column::TargetArtifactId.eq(task))
            .all(&h.db.conn)
            .await
            .unwrap();
        v.sort_by_key(|it| it.slot_no);
        v
    }

    /// A reviewer submits its verdict through the real ingest path (token →
    /// running iteration → review artifact + verdict + link).
    async fn submit_verdict(h: &Harness, review_iter_id: i32, verdict: &str, findings: &str) {
        let it = loop_iteration::Entity::find_by_id(review_iter_id)
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap();
        crate::loop_engine::ingest::ingest(
            &h.db.conn,
            &it.capability_token,
            "loop_submit_review",
            &serde_json::json!({ "verdict": verdict, "findings": findings }),
        )
        .await
        .unwrap();
    }

    /// Review passes → task done + the task gate is released for the next task.
    #[tokio::test]
    async fn review_pass_marks_done_and_releases_gate() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(1, "unanimous");
        implement_to_in_progress(&h, &cfg, "feature.txt").await;
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::InProgress);

        // Dispatch the reviewer, who passes.
        assert!(drive_with(&h, &cfg).await, "dispatches a reviewer");
        let review = running_review(&h).await;
        submit_verdict(&h, review, "pass", "looks good").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review)
            .await
            .unwrap();

        // Aggregate → pass → task done + gate released.
        assert!(!drive_with(&h, &cfg).await);
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::Done);
        assert_eq!(
            load_issue(&h).await.active_task_artifact_id,
            None,
            "task gate released for the next task"
        );
    }

    /// Review fails → rework (task pending, attempt++, findings recorded), gate
    /// still held; the findings surface for the next implement briefing.
    #[tokio::test]
    async fn review_fail_reworks_with_findings() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(1, "unanimous");
        implement_to_in_progress(&h, &cfg, "feature.txt").await;

        assert!(drive_with(&h, &cfg).await);
        let review = running_review(&h).await;
        submit_verdict(&h, review, "fail", "missing error handling").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review)
            .await
            .unwrap();

        // Aggregate → fail → rework.
        assert!(!drive_with(&h, &cfg).await, "review fail reworks, not a dispatch");
        let node = task_node(&h, task).await;
        assert_eq!(node.status, ArtifactStatus::Pending);
        assert_eq!(node.attempt, 1);
        assert!(
            task_model(&h, task)
                .await
                .last_failure_sig
                .as_deref()
                .unwrap()
                .starts_with("review_rejected:"),
            "failure signature records a review rejection"
        );
        assert_eq!(
            load_issue(&h).await.active_task_artifact_id,
            Some(task),
            "gate held across rework"
        );
        let findings = loop_service::artifact::latest_failed_review_findings(&h.db.conn, task)
            .await
            .unwrap();
        assert_eq!(findings, vec!["missing error handling".to_string()]);
    }

    /// Unanimous rule: one fail rejects immediately and cancels the still-running
    /// reviewers (their late verdicts can no longer change the outcome).
    #[tokio::test]
    async fn unanimous_fail_fast_cancels_other_reviewers() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(2, "unanimous");
        implement_to_in_progress(&h, &cfg, "feature.txt").await;

        // Dispatch both review slots.
        assert!(drive_with(&h, &cfg).await, "dispatches reviewers");
        let reviews = review_iters_of(&h, task).await;
        assert_eq!(reviews.len(), 2, "two review slots");

        // Slot 0 fails; slot 1 is still running → unanimous fail-fast.
        submit_verdict(&h, reviews[0].id, "fail", "regression").await;
        settle_iteration(&h.db, &EventEmitter::Noop, reviews[0].id)
            .await
            .unwrap();

        assert!(!drive_with(&h, &cfg).await, "fail-fast reworks");
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::Pending);
        let slot1 = loop_iteration::Entity::find_by_id(reviews[1].id)
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            slot1.status,
            IterationStatus::Cancelled,
            "the other reviewer was cancelled"
        );
    }

    // ---- Task 2.4: circuit breakers ----

    /// `max_attempts` exhausted → the task and its issue are blocked and a card
    /// is filed. With `max_attempts = 1` the first failure trips it immediately.
    #[cfg(unix)]
    #[tokio::test]
    async fn breaker_max_attempts_blocks_task_and_issue() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = IssueConfig {
            max_attempts: 1,
            ..config_with_validation(&["false"])
        };

        assert!(drive_with(&h, &cfg).await, "tick 1 dispatches implement");
        let iter_id = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // Tick 2: validation fails at attempt 0 → bump→1 ≥ max(1) → block.
        assert!(!drive_with(&h, &cfg).await, "a breaker block is not a dispatch");
        let node = task_node(&h, task).await;
        assert_eq!(node.status, ArtifactStatus::Blocked, "task blocked");
        assert_eq!(node.attempt, 1);
        assert_eq!(load_issue(&h).await.status, IssueStatus::Blocked, "issue blocked");
        let inbox = loop_service::inbox::list_inbox(&h.db.conn, h.space_id, None)
            .await
            .unwrap();
        assert!(
            inbox
                .iter()
                .any(|i| i.kind == InboxKind::Blocked
                    && i.subject_key == format!("no_progress:{task}")),
            "no-progress inbox card filed"
        );
    }

    /// Two consecutive identical failures trip the repeated-failure breaker even
    /// though `max_attempts` (default 6) is far from exhausted.
    #[cfg(unix)]
    #[tokio::test]
    async fn breaker_repeated_failure_blocks() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_with_validation(&["false"]); // default max_attempts = 6

        // Attempt 0: implement → validation fails → retry (not yet blocked).
        assert!(drive_with(&h, &cfg).await);
        let iter0 = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter0)
            .await
            .unwrap();
        assert!(drive_with(&h, &cfg).await, "attempt 0 failure retries");
        assert_eq!(task_node(&h, task).await.attempt, 1);
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::Pending);

        // Attempt 1: an identical validation failure → repeated-failure breaker.
        let iter1 = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code2\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter1)
            .await
            .unwrap();
        assert!(!drive_with(&h, &cfg).await, "the repeat trips the breaker");

        let node = task_node(&h, task).await;
        assert_eq!(node.status, ArtifactStatus::Blocked, "task blocked on repeat");
        assert_eq!(node.attempt, 2);
        assert_eq!(load_issue(&h).await.status, IssueStatus::Blocked);
    }

    /// Budget breaker: once accumulated `token_used` crosses `token_budget`,
    /// settling any iteration pauses the issue (`pause_reason = budget`) and
    /// files a `budget_exhausted` card.
    #[tokio::test]
    async fn breaker_budget_pause_on_exhaustion() {
        let h = setup().await;
        let _task = add_task(&h, "Task 1").await;

        // Pre-seed accumulated usage already over a tight budget.
        loop_issue::Entity::update_many()
            .col_expr(loop_issue::Column::TokenUsed, Expr::value(1000_i64))
            .col_expr(loop_issue::Column::TokenBudget, Expr::value(500_i64))
            .filter(loop_issue::Column::Id.eq(h.issue_id))
            .exec(&h.db.conn)
            .await
            .unwrap();

        // Dispatch + settle any iteration; settlement evaluates the budget breaker.
        assert!(drive(&h).await, "dispatch implement");
        let iter_id = running_implement_id(&h).await;
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        let issue = load_issue(&h).await;
        assert_eq!(issue.status, IssueStatus::Paused, "issue paused on budget");
        assert_eq!(issue.pause_reason, Some(loop_issue::PauseReason::Budget));
        let inbox = loop_service::inbox::list_inbox(&h.db.conn, h.space_id, None)
            .await
            .unwrap();
        assert!(
            inbox
                .iter()
                .any(|i| i.kind == InboxKind::BudgetExhausted
                    && i.subject_key == format!("budget:{}", h.issue_id)),
            "budget_exhausted card filed"
        );
    }
}
