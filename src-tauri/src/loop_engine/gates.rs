//! Write-pipeline stage gates (§4.5): implement → validate → review → finalize.
//!
//! **implement** is the first stage that changes code. It is unlike the read
//! stages in two ways:
//!
//! - **No submission.** The implement agent edits files in the worktree and
//!   calls no `loop_submit_*` tool (its briefing tool-contract says so). The
//!   engine measures progress by *checkpointing*: a non-empty diff that commits
//!   is success; an empty diff is no progress.
//! - **Per-task isolation, concurrent across tasks.** There is no per-issue
//!   write gate. A `parallel` issue drives every ready/in-review task at once,
//!   each in its **own** worktree (so two tasks never race on one tree); the
//!   `(issue, target)` / review-slot dispatch leases keep a repeated tick from
//!   double-dispatching a task. A `serial` (or not-yet-decided) issue shares the
//!   issue worktree, so it drives exactly one task at a time — a serial chain
//!   yields ≤1 ready task anyway.
//!
//! Idempotency across ticks keys on `iteration.attempt == task.attempt`: a
//! settled implement iteration is only checkpointed once, because a no-progress
//! checkpoint bumps the task's rework counter and the next dispatch carries the
//! new attempt.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use chrono::Utc;
use regex::Regex;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::db::entities::loop_artifact::{self, ArtifactKind, ArtifactStatus, ReviewVerdict};
use crate::db::entities::loop_criterion_check::CheckVerdict;
use crate::db::entities::loop_gate_decision::GateOutcome;
use crate::db::entities::loop_inbox_item::InboxKind;
use crate::db::entities::loop_issue::{self, IssueStatus};
use crate::db::entities::loop_iteration::{self, IterationOutcome, IterationStatus, Stage};
use crate::db::entities::loop_link::LinkKind;
use crate::db::service::{folder_service, loop_service};
use crate::db::AppDatabase;
use crate::models::loops::{
    IssueConfig, LoopArtifactRow, LoopCriterionCheckRow, LoopDagView, ReviewPassRule, ReviewerSpec,
};
use crate::web::event_bridge::EventEmitter;

use crate::loop_engine::dispatch::{
    dispatch_iteration, emit_changed, over_budget, pause_for_budget, DispatchInput,
    LoopAgentSpawner,
};
use crate::loop_engine::driver::resolve_agent_spec;
use crate::loop_engine::error::LoopError;
use crate::loop_engine::transitions::{
    self, cas_artifact_status, cas_issue_status, cas_iteration_status,
    cas_task_done_with_contribution, TaskContribution,
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

/// Result of one write-pipeline gate step — the driver uses it to decide whether
/// to re-tick immediately or park.
///
/// * `Dispatched` — a new iteration was launched (now in flight). Park; its
///   settlement wakes the driver.
/// * `Advanced` — the engine's **durable** state moved forward (task promoted /
///   task gate released / rework counter bumped / issue blocked) but **nothing**
///   is in flight. The next tick must re-read state to dispatch the follow-on
///   step, or observe the issue leaving `running` and stop. The driver therefore
///   re-ticks immediately; otherwise it would park on the no-timeout wake and
///   wedge. **Invariant: returning `Advanced` requires a real durable change** —
///   otherwise a stale snapshot would re-enter the same arm and hot-spin.
/// * `Idle` — nothing to do: an iteration is still in flight (await its wake), a
///   human gate is open, or there is no pending work. Park.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepOutcome {
    Dispatched,
    Advanced,
    Idle,
}

impl StepOutcome {
    /// Lift a raw "did it dispatch?" bool into a step outcome.
    fn from_dispatched(dispatched: bool) -> Self {
        if dispatched {
            StepOutcome::Dispatched
        } else {
            StepOutcome::Idle
        }
    }

    /// Combine the outcomes of the several tasks driven in one tick. Priority:
    /// `Advanced` > `Dispatched` > `Idle`. Any durable change (a task promoted /
    /// done / blocked, a rework bump) forces a re-tick so the driver re-reads the
    /// now-changed frontier (a done task may unblock a dependent or open finalize;
    /// a blocked issue must stop). Else, if anything launched, park awaiting its
    /// settlement; else idle.
    fn merge(self, other: StepOutcome) -> StepOutcome {
        use StepOutcome::*;
        match (self, other) {
            (Advanced, _) | (_, Advanced) => Advanced,
            (Dispatched, _) | (_, Dispatched) => Dispatched,
            _ => Idle,
        }
    }
}

/// Consecutive per-task infrastructure failures (e.g. worktree creation) tolerated
/// before the task + issue are blocked. NOT a business cap — a pure safety net so a
/// genuinely broken environment surfaces as a `blocked` card instead of an infinite
/// retry/log loop. The count is driver-memory, per task, reset on any success and
/// pruned when the task leaves the drivable set, so unrelated transient failures
/// never accumulate into a false block.
const INFRA_RETRY_MAX: u32 = 5;

/// Drive the issue's tasks through the write pipeline (implement → validate →
/// review) for one tick. See [`StepOutcome`] for how the driver reacts to the
/// return value.
///
/// No per-issue write gate: a `parallel` issue fans out over **every** drivable
/// task (each in its own worktree, dispatch idempotent via the `(issue, target)`
/// / review-slot leases); a `serial`/undecided issue drives exactly one task at a
/// time (sharing the issue worktree). A no-op while no task exists yet (read
/// stages still in flight), so the driver can call it on every "read frontier
/// empty" tick.
///
/// `infra_retries` is the driver's per-task infrastructure-failure counter (keyed
/// by task id). A worktree-ensure failure increments it and skips that task —
/// siblings still run — and `run_driver` keeps re-ticking (it arms a timer while
/// the map is non-empty) until the worktree succeeds or [`INFRA_RETRY_MAX`] trips.
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
    infra_retries: &mut HashMap<i32, u32>,
) -> Result<StepOutcome, LoopError> {
    // Budget pre-check (dispatch-time half of the double-check): refuse to start
    // new task work once the issue has reached its budget. Parallel fan-out can
    // otherwise launch several writes before any settles and trips the settle-time
    // breaker. In-flight work still settles (and may mildly overspend — budget is
    // not reserved); this only stops NEW dispatch and pauses so the driver halts
    // next tick.
    if over_budget(issue) {
        if pause_for_budget(&db.conn, issue, None).await? {
            emit_changed(emitter, issue.space_id, issue.id, issue.id, "budget");
        }
        return Ok(StepOutcome::Advanced);
    }

    // The tasks that can make progress this tick: those mid-review
    // (`in_progress`) and those whose every dependency is `Done` (ready pending).
    // `in_progress` first so a serial issue continues an in-flight task's review
    // before starting a fresh one.
    let mut drivable: Vec<i32> = dag
        .artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::Task && a.status == ArtifactStatus::InProgress)
        .map(|a| a.id)
        .collect();
    drivable.extend(ready_tasks(dag).into_iter().map(|t| t.id));

    if drivable.is_empty() {
        // Nothing drivable. If a pending task is wedged behind a Blocked /
        // Cancelled dependency that can never become Done, block the issue
        // (retry-reachable) instead of parking silently.
        return detect_dead_dependency(db, emitter, issue, dag).await;
    }

    // Parallel issues fan out — each drivable task runs in its OWN worktree, so
    // concurrent dispatch is safe. Serial / undecided issues would share the issue
    // worktree, so drive exactly one task (the safety floor for the not-yet-decided
    // case; a serial chain yields ≤1 ready task regardless).
    if issue.execution_mode.as_deref() != Some("parallel") {
        drivable.truncate(1);
    }

    let drivable_set: std::collections::HashSet<i32> = drivable.iter().copied().collect();
    let mut outcome = StepOutcome::Idle;
    for task_id in &drivable {
        let task_id = *task_id;
        let wt = match task_worktree_folder(db, data_dir, issue, task_id, worktree_folder_id).await {
            Ok(wt) => wt,
            Err(e) => {
                // Infra failure (e.g. worktree creation). Don't abort the tick or
                // starve sibling tasks — count it and, after a bounded run of
                // consecutive failures, block the task + issue (a real, persistent
                // environment fault). Otherwise skip it this tick; `run_driver`
                // re-ticks (it arms a timer while `infra_retries` is non-empty).
                let n = infra_retries.entry(task_id).or_insert(0);
                *n += 1;
                tracing::warn!(
                    issue_id = issue.id,
                    task_id,
                    attempt = *n,
                    error = %e,
                    "drive: task worktree ensure failed"
                );
                if *n >= INFRA_RETRY_MAX {
                    infra_retries.remove(&task_id);
                    block_task_infra(db, emitter, issue, task_id).await?;
                    outcome = outcome.merge(StepOutcome::Advanced);
                }
                continue;
            }
        };
        // Worktree is available → clear any prior failure streak for this task.
        infra_retries.remove(&task_id);
        let step =
            advance_active_task(db, data_dir, spawner, emitter, issue, dag, config, wt, task_id)
                .await?;
        outcome = outcome.merge(step);
    }
    // Drop failure counts for tasks no longer drivable (reached a terminal state),
    // so an unrelated transient failure can never accumulate into a false block.
    infra_retries.retain(|tid, _| drivable_set.contains(tid));
    Ok(outcome)
}

/// The worktree folder a task's write-pipeline iterations (implement / review /
/// checkpoint / validation) run in. Parallel-mode issues give each task its own
/// worktree — ensured idempotently here so two concurrently-driven tasks never
/// share a tree; serial-mode issues share the issue worktree. The ensure can fail
/// (a transient infra error); the caller treats that as a bounded-retry skip
/// rather than aborting the whole tick.
async fn task_worktree_folder(
    db: &AppDatabase,
    data_dir: &Path,
    issue: &loop_issue::Model,
    task_id: i32,
    issue_worktree_folder_id: i32,
) -> Result<i32, LoopError> {
    if issue.execution_mode.as_deref() == Some("parallel") {
        let ctx = worktree::ensure_task_worktree(&db.conn, data_dir, issue.id, task_id).await?;
        Ok(ctx.worktree_folder_id)
    } else {
        Ok(issue_worktree_folder_id)
    }
}

/// Whether `task` (pending) transitively depends on a `Blocked` or `Cancelled`
/// task — a predecessor that can never become `Done`, so the task can never
/// start. Walks the `DependsOn` closure (from = successor, to = predecessor);
/// the submit-time acyclicity guard bounds the walk.
fn has_dead_dependency(dag: &LoopDagView, task_id: i32) -> bool {
    let mut stack = vec![task_id];
    let mut seen = std::collections::HashSet::new();
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        for l in dag
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::DependsOn && l.from_artifact_id == cur)
        {
            match dag.artifacts.iter().find(|a| a.id == l.to_artifact_id) {
                Some(p)
                    if matches!(
                        p.status,
                        ArtifactStatus::Blocked | ArtifactStatus::Cancelled
                    ) =>
                {
                    return true;
                }
                Some(_) => stack.push(l.to_artifact_id),
                None => {}
            }
        }
    }
    false
}

/// Whether the issue has any queued/running iteration.
async fn issue_has_inflight(db: &AppDatabase, issue_id: i32) -> Result<bool, LoopError> {
    Ok(loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .filter(
            loop_iteration::Column::Status
                .is_in([IterationStatus::Queued, IterationStatus::Running]),
        )
        .one(&db.conn)
        .await?
        .is_some())
}

/// Called when the gate is free and no task is ready. If a pending task is wedged
/// behind a `Blocked`/`Cancelled` dependency and nothing is in flight, the issue
/// can never progress on its own — block it (retry-reachable) with an inbox card
/// rather than parking silently. Otherwise idle (all done → finalize handles it;
/// or work is still in flight that may yet open the frontier).
async fn detect_dead_dependency(
    db: &AppDatabase,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    dag: &LoopDagView,
) -> Result<StepOutcome, LoopError> {
    let pending: Vec<&LoopArtifactRow> = dag
        .artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::Task && a.status == ArtifactStatus::Pending)
        .collect();
    if pending.is_empty() {
        return Ok(StepOutcome::Idle); // nothing pending → not a dead end (finalize path)
    }
    if issue_has_inflight(db, issue.id).await? {
        return Ok(StepOutcome::Idle); // in-flight work may yet open the frontier
    }
    if pending.iter().any(|t| has_dead_dependency(dag, t.id)) {
        if cas_issue_status(&db.conn, issue.id, IssueStatus::Running, IssueStatus::Blocked).await? {
            loop_service::inbox::upsert_inbox(
                &db.conn,
                issue.space_id,
                issue.id,
                None,
                InboxKind::Blocked,
                &format!("dependency_unsatisfiable:{}", issue.id),
                serde_json::json!({ "v": 1, "reason": "dependency_unsatisfiable" }),
            )
            .await?;
            emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
        }
        // Issue now blocked → re-tick so the driver observes it and stops.
        return Ok(StepOutcome::Advanced);
    }
    Ok(StepOutcome::Idle)
}

/// Tasks whose every `DependsOn` predecessor is `Done` — the dependency-aware
/// ready frontier. Edge contract: a `DependsOn` link is `from = successor`,
/// `to = predecessor`, so a task is ready when all links whose `from` is the task
/// point to `Done` tasks. Deterministic order by `(sort, id)` so downstream
/// dispatch/topology is stable. A root task (no `DependsOn` edges) is ready as
/// soon as it is `pending`. (Serial/single-chain issues yield ≤1 ready task, so
/// taking the first preserves today's behavior; phase 2 dispatches the whole set.)
fn ready_tasks(dag: &LoopDagView) -> Vec<&LoopArtifactRow> {
    let done: std::collections::HashSet<i32> = dag
        .artifacts
        .iter()
        .filter(|a| a.status == ArtifactStatus::Done)
        .map(|a| a.id)
        .collect();
    let mut out: Vec<&LoopArtifactRow> = dag
        .artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::Task && a.status == ArtifactStatus::Pending)
        .filter(|t| {
            dag.links
                .iter()
                .filter(|l| l.kind == LinkKind::DependsOn && l.from_artifact_id == t.id)
                .all(|l| done.contains(&l.to_artifact_id))
        })
        .collect();
    out.sort_by(|a, b| a.sort.cmp(&b.sort).then(a.id.cmp(&b.id)));
    out
}

/// Route one drivable task to its write-pipeline stage by status: `pending`
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
) -> Result<StepOutcome, LoopError> {
    let Some(task) = dag.artifacts.iter().find(|a| a.id == active_task_id) else {
        // Gate points at a node not in this DAG — nothing to drive.
        return Ok(StepOutcome::Idle);
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
        // Done (gate released on review pass), blocked (awaiting a human retry),
        // cancelled, etc. → idle.
        _ => Ok(StepOutcome::Idle),
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
) -> Result<StepOutcome, LoopError> {
    let impls = implement_iterations(db, issue.id, task.id).await?;
    if impls
        .iter()
        .any(|it| matches!(it.status, IterationStatus::Queued | IterationStatus::Running))
    {
        // Implement in flight — wait for its completion to wake us.
        return Ok(StepOutcome::Idle);
    }

    // A succeeded implement at the current attempt is awaiting its checkpoint +
    // validation.
    let settled = impls
        .iter()
        .find(|it| it.status == IterationStatus::Succeeded && it.attempt == task.attempt);
    if let Some(settled) = settled {
        match finish_implement(db, emitter, issue, config, worktree_folder_id, task, settled.id)
            .await?
        {
            // Promoted to in_progress → re-tick to dispatch review.
            ImplementOutcome::Advanced => Ok(StepOutcome::Advanced),
            // Task (and possibly the issue) was blocked → re-tick: a blocked issue
            // stops + deregisters the driver (so a human retry's respawn takes
            // effect); a task-only block (issue still running) lands on
            // `advance_active_task`'s idle arm and parks awaiting a human.
            ImplementOutcome::Blocked => Ok(StepOutcome::Advanced),
            ImplementOutcome::NoProgress => {
                // The rework counter was bumped (durable progress); retry implement
                // at the new attempt. If the write lease was momentarily busy and
                // nothing launched, still Advanced so the next tick re-attempts (it
                // lands on the in-flight idle arm if a retry is by then running).
                let dispatched = dispatch_implement(
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
                .await?;
                Ok(if dispatched {
                    StepOutcome::Dispatched
                } else {
                    StepOutcome::Advanced
                })
            }
        }
    } else {
        // Gate held but nothing live or freshly settled (just acquired, or a
        // prior attempt already processed) → (re)dispatch implement.
        let dispatched = dispatch_implement(
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
        .await?;
        Ok(StepOutcome::from_dispatched(dispatched))
    }
}

/// Checkpoint, then validate, a settled implement iteration. An empty diff is
/// discarded as no progress; a committed diff is handed to validation, whose
/// outcome decides advance / rework / block.
async fn finish_implement(
    db: &AppDatabase,
    emitter: &EventEmitter,
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
            validate_after_implement(db, emitter, issue, config, worktree_path, task, iteration_id)
                .await
        }
        None => {
            // No diff to accept. Discard any stray uncommitted state either way.
            worktree::reset_to_head(worktree_path).await?;
            // D12: did the agent explicitly declare the task already satisfied
            // (loop_task_complete)? If so this empty result is intentional — route
            // to review (the review gate still verifies the criteria against HEAD),
            // NOT a no-progress rework.
            let declared = loop_service::iteration::get_iteration(&db.conn, iteration_id)
                .await?
                .and_then(|it| it.agent_completion_reason)
                .filter(|r| !r.trim().is_empty());
            if declared.is_some() {
                set_task_status_cas(
                    db,
                    task.id,
                    ArtifactStatus::Pending,
                    ArtifactStatus::InProgress,
                )
                .await?;
                loop_service::iteration::set_iteration_outcome(
                    &db.conn,
                    iteration_id,
                    IterationOutcome::DeclaredComplete,
                )
                .await?;
                Ok(ImplementOutcome::Advanced)
            } else {
                // D11: a genuine empty diff is no progress; record it and let the
                // breaker decide retry vs. block.
                loop_service::iteration::set_iteration_outcome(
                    &db.conn,
                    iteration_id,
                    IterationOutcome::EmptyDiff,
                )
                .await?;
                match record_rework(
                    db,
                    emitter,
                    issue,
                    config,
                    task,
                    Some(iteration_id),
                    "empty_diff:implement",
                )
                .await?
                {
                    ReworkOutcome::Retry => Ok(ImplementOutcome::NoProgress),
                    ReworkOutcome::Blocked => Ok(ImplementOutcome::Blocked),
                }
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
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_path: &Path,
    task: &LoopArtifactRow,
    iteration_id: i32,
) -> Result<ImplementOutcome, LoopError> {
    let commands = &config.validation_commands;
    if commands.is_empty() {
        set_task_status_cas(db, task.id, ArtifactStatus::Pending, ArtifactStatus::InProgress).await?;
        // No commands to run → the committed implement is the outcome (D11).
        loop_service::iteration::set_iteration_outcome(
            &db.conn,
            iteration_id,
            IterationOutcome::Succeeded,
        )
        .await?;
        // D14: a committed checkpoint advancing to review is real forward progress.
        transitions::clear_oscillation(&db.conn, task.id).await?;
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
            set_task_status_cas(db, task.id, ArtifactStatus::Pending, ArtifactStatus::InProgress).await?;
            // Validation passed → the implement iteration succeeded (D11).
            loop_service::iteration::set_iteration_outcome(
                &db.conn,
                iteration_id,
                IterationOutcome::Succeeded,
            )
            .await?;
            // D14: passing validation is real forward progress on the task.
            transitions::clear_oscillation(&db.conn, task.id).await?;
            Ok(ImplementOutcome::Advanced)
        }
        ValidationOutcome::Failed => {
            // Fingerprint the failure so the breaker can tell "the same failure
            // again" from a genuinely new one.
            let sig = format!(
                "validation_failed:{}",
                sig_hash(&format!(
                    "{:?}\n{}",
                    report.exit_codes,
                    normalize_failure_output(&report.output)
                ))
            );
            loop_service::iteration::set_iteration_outcome(
                &db.conn,
                iteration_id,
                IterationOutcome::ValidationFailed,
            )
            .await?;
            match record_rework(db, emitter, issue, config, task, Some(iteration_id), &sig).await? {
                ReworkOutcome::Retry => Ok(ImplementOutcome::NoProgress),
                ReworkOutcome::Blocked => Ok(ImplementOutcome::Blocked),
            }
        }
        ValidationOutcome::Unrunnable => {
            set_task_status_cas(db, task.id, ArtifactStatus::Pending, ArtifactStatus::Blocked)
                .await?;
            // The checkpoint ran but validation couldn't execute — the implement
            // did not pass its gate, so record it (keeps the NULL invariant: a
            // settled+checkpointed implement always has an outcome).
            loop_service::iteration::set_iteration_outcome(
                &db.conn,
                iteration_id,
                IterationOutcome::ValidationFailed,
            )
            .await?;
            // Block the issue too (consistent with the no-progress breaker's
            // `mark_blocked`), so the human `retry` escape hatch — which requires a
            // `blocked` issue — can reach this stall and re-arm the task. Without
            // this the issue would sit `running` with a blocked task: the driver
            // parks and `retry_issue` rejects it as not-blocked, an unrecoverable
            // dead end.
            cas_issue_status(&db.conn, issue.id, IssueStatus::Running, IssueStatus::Blocked)
                .await?;
            let upsert = loop_service::inbox::upsert_inbox(
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
            if upsert.changed() {
                emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
            }
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
    let spec = resolve_agent_spec(config, Stage::Implement);
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
            agent_type: spec.agent,
            mode_id: spec.mode_id,
            config_values: spec.config_values,
            worktree_folder_id,
        },
    )
    .await?;
    Ok(handle.is_some())
}

/// All implement iterations for **one task** — keyed by `(issue, target)`, never
/// "the issue's single write". With several tasks implementing concurrently
/// (phase 2 dropped the per-issue write lease), this still resolves exactly this
/// task's iterations. (No in-flight-write lookup assumes a per-issue singleton —
/// they key on `(issue, target)` here / `(issue, finalize)` for the issue-level
/// finalize, or iterate all in-flight rows.)
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

/// CAS a task artifact's status `from → to` — the artifact analogue of the
/// `cas_*_status` discipline used for issues and iterations. Returns whether the
/// transition applied. A single per-issue driver advances its tasks (they fan
/// out within a tick but are stepped one at a time), so a `false` here means the
/// expected `from` was wrong — a logic bug — and is logged rather than silently
/// swallowed.
async fn set_task_status_cas(
    db: &AppDatabase,
    task_id: i32,
    from: ArtifactStatus,
    to: ArtifactStatus,
) -> Result<bool, LoopError> {
    let applied = cas_artifact_status(&db.conn, task_id, from, to).await?;
    if !applied {
        tracing::warn!(
            task_id,
            from = ?from,
            to = ?to,
            "task status CAS did not apply (unexpected current status)"
        );
    }
    Ok(applied)
}

/// Read the task's accepted tip — HEAD of the worktree it ran in (the task branch
/// in parallel mode, the issue branch in serial mode) — and atomically mark the
/// task `Done` with its contribution kind (D12). In parallel mode the kind is
/// `NoOp` when HEAD == the pinned integration base (the agent declared the task
/// already satisfied; no commit) and `Delta` otherwise (freezing HEAD as
/// `fan_in_commit`); serial tasks always record `Delta` (the column is unused for
/// serial fan-in). The single CAS guarantees no "Done but unfrozen" window the
/// fan-in could observe. On Done, clears the task's oscillation epoch (D14).
/// Returns whether the CAS applied.
async fn freeze_and_done(
    db: &AppDatabase,
    issue: &loop_issue::Model,
    worktree_folder_id: i32,
    task_id: i32,
) -> Result<bool, LoopError> {
    let folder = folder_service::get_folder_by_id(&db.conn, worktree_folder_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("worktree folder {worktree_folder_id}")))?;
    let head = worktree::head_commit(Path::new(&folder.path)).await?;
    // Base stability holds because the parallel fan-in runs strictly after ALL
    // tasks are Done (the run_finalize gate): at this task's done the issue tip is
    // unmoved and every predecessor's frozen commit is fixed, so re-resolving the
    // base here matches what `ensure_task_worktree` branched from.
    let contribution = if issue.execution_mode.as_deref() == Some("parallel") {
        let base = worktree::task_base_oid(&db.conn, issue, task_id).await?;
        if head == base {
            TaskContribution::NoOp
        } else {
            TaskContribution::Delta(head)
        }
    } else {
        TaskContribution::Delta(head)
    };
    let applied = cas_task_done_with_contribution(&db.conn, task_id, contribution).await?;
    if applied {
        // D14: reaching Done is real forward progress — clear the oscillation epoch.
        transitions::clear_oscillation(&db.conn, task_id).await?;
    }
    Ok(applied)
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
    emitter: &EventEmitter,
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
        mark_blocked(db, emitter, issue, config, task.id, iteration_id, reason, sig, attempt)
            .await?;
        Ok(ReworkOutcome::Blocked)
    } else {
        Ok(ReworkOutcome::Retry)
    }
}

/// Block a stalled node: set the task `blocked`, CAS the issue `running →
/// blocked` (so the driver stops on its next tick), and file a `blocked` inbox
/// card keyed on the task. D14: when the same failure recurs across enough block
/// epochs (`oscillation_limit`), promote the ordinary `no_progress` card to an
/// `oscillation` card — a deterministic failure a plain retry can't fix, needing
/// an explicit human exit. A human resolves it via the inbox.
#[allow(clippy::too_many_arguments)]
async fn mark_blocked(
    db: &AppDatabase,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    task_id: i32,
    iteration_id: Option<i32>,
    reason: &str,
    sig: &str,
    attempt: i32,
) -> Result<(), LoopError> {
    // Whether THIS call actually blocked the node — a genuine new block epoch. An
    // idempotent replay (task already blocked) must NOT inflate the oscillation
    // count. The issue CAS is independent (it may miss when a sibling already
    // blocked the issue) and must NOT gate the task's own epoch.
    let blocked_now = crate::loop_engine::transitions::cas_artifact_status_from(
        &db.conn,
        task_id,
        &[ArtifactStatus::Pending, ArtifactStatus::InProgress],
        ArtifactStatus::Blocked,
    )
    .await?;
    cas_issue_status(&db.conn, issue.id, IssueStatus::Running, IssueStatus::Blocked).await?;

    // D14: step the epoch only on a genuine new block; on a replay read the
    // existing count without stepping.
    let limit = config.oscillation_limit as i32;
    let osc = if blocked_now {
        transitions::step_oscillation(&db.conn, task_id, sig).await?
    } else {
        loop_artifact::Entity::find_by_id(task_id)
            .one(&db.conn)
            .await?
            .map(|m| m.oscillation_count)
            .unwrap_or(0)
    };

    if limit > 0 && osc >= limit {
        // Deterministic failure → promote. Upsert the oscillation card FIRST so a
        // crash before the resolve still leaves an actionable card (never zero
        // cards); a stale `no_progress` card briefly coexisting is harmless — retry
        // exclusion keys on the artifact's `oscillation_count`, not on the card.
        let upsert = loop_service::inbox::upsert_inbox(
            &db.conn,
            issue.space_id,
            issue.id,
            iteration_id,
            InboxKind::Blocked,
            &format!("oscillation:{task_id}"),
            serde_json::json!({
                "task_artifact_id": task_id,
                "reason": "oscillation",
                "failure_sig": sig,
                "count": osc,
                "attempt": attempt,
            }),
        )
        .await?;
        if upsert.changed() {
            emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
        }
        // Supersede the ordinary task-level blocker cards (NOT `oscillation:`, which
        // clears only via override / force-complete).
        loop_service::inbox::resolve_task_blocker_cards(
            &db.conn,
            issue.id,
            task_id,
            &["no_progress", "validation_blocked", "infra_failure"],
            serde_json::json!({ "action": "superseded_by_oscillation" }),
        )
        .await?;
        return Ok(());
    }

    // Below the limit (or limit=0, breaker off): ordinary retryable no_progress
    // card. Surface it live (D6): emit on a new/changed card, stay silent on a
    // no-op recurrence so a parked breaker does not spam every tick.
    let upsert = loop_service::inbox::upsert_inbox(
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
    if upsert.changed() {
        emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
    }
    Ok(())
}

/// Block a task whose worktree could not be provisioned after
/// [`INFRA_RETRY_MAX`] consecutive attempts: set the task `blocked`, CAS the issue
/// `running → blocked` (driver stops next tick), and file an `infra_failure:{task}`
/// card for a human. Distinct subject from the no-progress breaker — this is an
/// environment fault (disk, git), not a stuck agent.
async fn block_task_infra(
    db: &AppDatabase,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    task_id: i32,
) -> Result<(), LoopError> {
    crate::loop_engine::transitions::cas_artifact_status_from(
        &db.conn,
        task_id,
        &[ArtifactStatus::Pending, ArtifactStatus::InProgress],
        ArtifactStatus::Blocked,
    )
    .await?;
    cas_issue_status(&db.conn, issue.id, IssueStatus::Running, IssueStatus::Blocked).await?;
    loop_service::inbox::upsert_inbox(
        &db.conn,
        issue.space_id,
        issue.id,
        None,
        InboxKind::Blocked,
        &format!("infra_failure:{task_id}"),
        serde_json::json!({
            "task_artifact_id": task_id,
            "reason": "worktree_unavailable",
        }),
    )
    .await?;
    emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
    Ok(())
}

/// ISO-8601 timestamps (`2026-06-18T12:34:56.789Z`, with or without fractional
/// seconds / timezone) — differ run-to-run for the same failure.
static TS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?").unwrap()
});
/// Absolute / temp / worktree paths (each run gets a fresh temp dir, so the path
/// segment that follows `tmp`/`temp`/`loop-worktrees`/`var/folders` is volatile).
static TMP_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(/[\w.\-]+)*/(tmp|temp|loop-worktrees|var/folders)/[\w./\-]+").unwrap()
});
/// Full-length git object ids (40 hex = SHA-1, 64 hex = SHA-256). Deliberately
/// NOT 7–39 char hex: those collide with real, distinguishing failure content —
/// asserted addresses, expected/got hashes, generated ids — that must stay
/// distinct so two genuinely different failures keep different signatures.
static OID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[0-9a-f]{40}\b|\b[0-9a-f]{64}\b").unwrap());
/// Elapsed-time tokens (`12ms`, `1.3s`, `400µs`, `7ns`).
static DUR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d+(\.\d+)?(ms|s|µs|ns)\b").unwrap());

/// Strip volatile substrings from validation output before fingerprinting, so the
/// oscillation breaker (D14) can recognise "the same failure again" across
/// retries. Removes ISO timestamps, absolute/temp/worktree paths, full-length git
/// oids, and durations — the parts that differ run-to-run for an otherwise
/// identical failure — while leaving the failure's distinguishing content (error
/// messages, assertion expected/got values, short hex) untouched, so genuinely
/// different failures still hash differently. Operates line-by-line and trims
/// trailing whitespace (another run-to-run wobble).
fn normalize_failure_output(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        // Order matters: paths before oids so a temp dir's hex tail goes with the
        // path rather than being independently rewritten.
        let l = TS_RE.replace_all(line, "<ts>");
        let l = TMP_PATH_RE.replace_all(&l, "<path>");
        let l = OID_RE.replace_all(&l, "<oid>");
        let l = DUR_RE.replace_all(&l, "<dur>");
        out.push_str(l.trim_end());
        out.push('\n');
    }
    out
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
/// then accept (task `done`, freezing its integration commit) or reject (rework +
/// cancel the remaining reviewers). See [`StepOutcome`] for the return semantics.
/// The gate-decision stage label for a task review (D5: integration review uses
/// `finalize`). Stored in `loop_gate_decision.stage` and used as the replay key.
const REVIEW_STAGE: &str = "review";

/// The gate-decision stage label for the integration review (target = result).
/// `merge_issue` reads it; `count_fail(issue, FINALIZE_GATE_STAGE)` bounds the
/// integration loop-back.
pub(crate) const FINALIZE_GATE_STAGE: &str = "finalize";

/// The issue's live (non-superseded/cancelled) result artifact, if any — the one
/// `uniq_result_per_issue` admits. The integration gate and merge gate both reason
/// about THIS result, never a superseded one a prior loop-back left behind.
pub(crate) fn live_result(dag: &LoopDagView) -> Option<&LoopArtifactRow> {
    dag.artifacts.iter().find(|a| {
        a.kind == ArtifactKind::Result
            && !matches!(a.status, ArtifactStatus::Superseded | ArtifactStatus::Cancelled)
    })
}

/// Whether the issue's live result has passed integration (a recorded
/// `gate_decision(result, finalize, attempt) == Pass`). The merge gate's
/// precondition (D6) and the driver's auto-merge trigger both consult this, so a
/// result can land only after the whole-issue closure is verified.
pub(crate) async fn integration_passed(
    conn: &sea_orm::DatabaseConnection,
    dag: &LoopDagView,
) -> Result<bool, LoopError> {
    let Some(result) = live_result(dag) else {
        return Ok(false);
    };
    Ok(loop_service::gate_decision::outcome_for(
        conn,
        result.id,
        FINALIZE_GATE_STAGE,
        result.attempt,
    )
    .await?
        == Some(GateOutcome::Pass))
}

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
) -> Result<StepOutcome, LoopError> {
    let reviewer_specs = config.effective_reviewers();
    let reviewers = reviewer_specs.len() as i32;
    let iters = review_iterations(db, issue.id, task.id, task.attempt).await?;

    // Replay-safe pivot (D4): a decision already recorded for (task, review,
    // attempt) drives the side-effects idempotently — so a crash after recording
    // but before the freeze/rework completed is finished by this tick from the
    // recorded outcome (never recomputed). The key advances with the attempt, so a
    // completed rework (attempt bumped) is never re-entered here.
    if let Some(outcome) =
        loop_service::gate_decision::outcome_for(&db.conn, task.id, REVIEW_STAGE, task.attempt)
            .await?
    {
        return drive_review_outcome(
            db, spawner, emitter, issue, config, worktree_folder_id, task, &iters, outcome,
        )
        .await;
    }

    // Slot accounting: a slot is "decided" once it has a submitted (succeeded)
    // review; "missing" when it has no active and no submitted iteration. The
    // display verdict isn't the gate input — it only tells us WHICH iterations
    // submitted, so we know whose checks to aggregate and which slots to dispatch.
    let verdicts = review_verdicts(db, &iters).await?;
    let decided_iter_ids: Vec<i32> = verdicts.keys().copied().collect();
    let mut missing_slots: Vec<i32> = Vec::new();
    for slot in 0..reviewers {
        let slot_iters: Vec<&loop_iteration::Model> =
            iters.iter().filter(|it| it.slot_no == Some(slot)).collect();
        let decided = slot_iters.iter().any(|it| verdicts.contains_key(&it.id));
        let in_flight = slot_iters
            .iter()
            .any(|it| matches!(it.status, IterationStatus::Queued | IterationStatus::Running));
        if !decided && !in_flight {
            missing_slots.push(slot);
        }
    }

    // Canonical per-criterion decision over the submitted checks (D8) — NOT an
    // aggregation of per-reviewer verdicts.
    let injected_ids = injected_criterion_ids(&iters);
    let checks =
        loop_service::criterion_check::for_scope_iterations(&db.conn, task.id, &decided_iter_ids)
            .await?;
    let outcome = aggregate_checks(config.review_pass_rule, reviewers, &checks, &injected_ids);

    match outcome {
        GateOutcome::Pass | GateOutcome::Fail => {
            // Record the immutable decision FIRST (the durable pivot), THEN drive
            // side-effects. A divergent recompute at the same key (different
            // inputs) is a Conflict → re-tick against fresh state, never overwrite.
            let policy = review_policy_json(config);
            match loop_service::gate_decision::record_decision(
                &db.conn,
                issue.space_id,
                issue.id,
                task.id,
                REVIEW_STAGE,
                task.attempt,
                &checks,
                &injected_ids,
                &policy,
                outcome,
            )
            .await?
            {
                loop_service::gate_decision::RecordedDecision::Settled(_) => {}
                loop_service::gate_decision::RecordedDecision::Conflict(_) => {
                    return Err(LoopError::Conflict)
                }
            }
            drive_review_outcome(
                db, spawner, emitter, issue, config, worktree_folder_id, task, &iters, outcome,
            )
            .await
        }
        GateOutcome::Undecided => {
            let mut dispatched = false;
            for slot in missing_slots {
                if dispatch_review(
                    db,
                    data_dir,
                    spawner,
                    emitter,
                    issue,
                    worktree_folder_id,
                    task.id,
                    slot,
                    task.attempt,
                    &reviewer_specs[slot as usize],
                )
                .await?
                {
                    dispatched = true;
                }
            }
            Ok(StepOutcome::from_dispatched(dispatched))
        }
    }
}

/// Drive a settled review decision's side-effects (shared by the fresh decision
/// and the replay pivot). Pass → freeze the accepted tip + mark Done; Fail →
/// cancel remaining reviewers, reset the tree, and rework (retry or breaker).
/// Idempotent: Pass is a CAS (InProgress→Done); Fail is only reached while the
/// task is still InProgress at the deciding attempt (a completed rework bumped the
/// attempt, so the decision key no longer resolves here).
#[allow(clippy::too_many_arguments)]
async fn drive_review_outcome(
    db: &AppDatabase,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_folder_id: i32,
    task: &LoopArtifactRow,
    iters: &[loop_iteration::Model],
    outcome: GateOutcome,
) -> Result<StepOutcome, LoopError> {
    match outcome {
        GateOutcome::Pass => {
            cancel_active_reviews(db, spawner, iters).await?;
            if freeze_and_done(db, issue, worktree_folder_id, task.id).await? {
                Ok(StepOutcome::Advanced)
            } else {
                Ok(StepOutcome::Idle)
            }
        }
        GateOutcome::Fail => {
            cancel_active_reviews(db, spawner, iters).await?;
            // D12: review rejected — drop any declared-completion claim for this
            // task so the next empty implement attempt is treated as genuine
            // no-progress, not silently routed back to review on a stale claim.
            loop_service::iteration::clear_declared_completion(&db.conn, issue.id, task.id).await?;
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
            match record_rework(db, emitter, issue, config, task, None, &sig).await? {
                ReworkOutcome::Retry => {
                    set_task_status_cas(
                        db,
                        task.id,
                        ArtifactStatus::InProgress,
                        ArtifactStatus::Pending,
                    )
                    .await?;
                }
                ReworkOutcome::Blocked => {}
            }
            Ok(StepOutcome::Advanced)
        }
        // Undecided is never recorded as a decision; defensive no-op.
        GateOutcome::Undecided => Ok(StepOutcome::Idle),
    }
}

/// The injected criterion-id set for a review round = the union of the criterion
/// ids in the dispatched iterations' persisted manifests (D10). Every slot at one
/// attempt was shown the same frozen manifest, so this is the canonical "what must
/// be checked" set the gate aggregates against.
fn injected_criterion_ids(iters: &[loop_iteration::Model]) -> Vec<i32> {
    let mut set: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    for it in iters {
        if let Some(raw) = it.context_manifest.as_deref() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
                if let Some(obj) = v.get("criteria").and_then(|c| c.as_object()) {
                    for val in obj.values() {
                        if let Some(id) = val.as_i64() {
                            set.insert(id as i32);
                        }
                    }
                }
            }
        }
    }
    set.into_iter().collect()
}

/// The gate's policy fingerprint, recorded with the decision so a config change
/// (rule / reviewer count) is detectable as a different decision input.
fn review_policy_json(config: &IssueConfig) -> String {
    let rule = match config.review_pass_rule {
        ReviewPassRule::Unanimous => "unanimous",
        ReviewPassRule::Majority => "majority",
    };
    serde_json::json!({
        "rule": rule,
        "reviewers": config.effective_reviewers().len(),
        "v": 1,
    })
    .to_string()
}

/// Aggregate review verdicts under the pass rule. `unanimous` fails fast on any
/// fail and accepts only when all `n` slots pass; `majority` accepts on
/// `pass*2 > n` and rejects once a passing majority is unreachable (an even
/// split rejects).
fn aggregate(rule: ReviewPassRule, n: i32, verdicts: &[ReviewVerdict]) -> ReviewDecision {
    let pass = verdicts
        .iter()
        .filter(|v| matches!(v, ReviewVerdict::Pass))
        .count() as i32;
    let fail = verdicts.len() as i32 - pass;
    if rule == ReviewPassRule::Majority {
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

/// Per-criterion aggregation (D8) — the canonical gate decision. Group the
/// submitted checks by criterion, apply the SAME quorum as [`aggregate`] to each
/// criterion's reviewer checks, then: the gate is `Fail` iff ANY injected
/// criterion failed, `Pass` iff EVERY injected criterion passed, else `Undecided`
/// (more reviewer checks needed). This is NOT the same as aggregating per-reviewer
/// verdicts — under Majority the two can diverge (see the counterexample test):
/// reviewers can split such that no reviewer is in the majority yet every
/// criterion individually clears quorum. An empty injected set is `Undecided`
/// (no criteria dispatched yet, or a degenerate task) — never a vacuous pass.
fn aggregate_checks(
    rule: ReviewPassRule,
    n: i32,
    checks: &[LoopCriterionCheckRow],
    injected_ids: &[i32],
) -> GateOutcome {
    if injected_ids.is_empty() {
        return GateOutcome::Undecided;
    }
    let mut any_fail = false;
    let mut all_pass = true;
    for &cid in injected_ids {
        let verdicts: Vec<ReviewVerdict> = checks
            .iter()
            .filter(|c| c.criterion_id == cid)
            .map(|c| match c.verdict {
                CheckVerdict::Pass => ReviewVerdict::Pass,
                CheckVerdict::Fail => ReviewVerdict::Fail,
            })
            .collect();
        match aggregate(rule, n, &verdicts) {
            ReviewDecision::Fail => any_fail = true,
            ReviewDecision::Pass => {}
            ReviewDecision::Undecided => all_pass = false,
        }
    }
    if any_fail {
        GateOutcome::Fail
    } else if all_pass {
        GateOutcome::Pass
    } else {
        GateOutcome::Undecided
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
    worktree_folder_id: i32,
    task_id: i32,
    slot: i32,
    attempt: i32,
    spec: &ReviewerSpec,
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
            agent_type: spec.agent,
            mode_id: spec.mode_id.clone(),
            config_values: spec.config_values.clone(),
            worktree_folder_id,
        },
    )
    .await?;
    Ok(handle.is_some())
}

/// Invalidate any still-active reviewers (a decision was reached without them).
/// CAS to `cancelled` voids the capability token — `ingest` rejects a submit from
/// a non-running iteration — so a late verdict can't change the outcome. It then
/// reaps the reviewer's agent *process*: voiding the token only blocks a late
/// submit, but the process itself could keep mutating the shared worktree right up
/// until it's disconnected (and the caller resets the tree immediately after).
/// Best-effort kill — a reviewer whose connection already exited just isn't found.
async fn cancel_active_reviews(
    db: &AppDatabase,
    spawner: &dyn LoopAgentSpawner,
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
            // D11: a cancelled losing reviewer is `abandoned`. Only after the CAS
            // wins, and write-once anyway, so a stale call can never clobber a real
            // outcome (Codex r2 C2).
            loop_service::iteration::set_iteration_outcome(
                &db.conn,
                it.id,
                IterationOutcome::Abandoned,
            )
            .await?;
            if let Some(conv_id) = it.conversation_id {
                if let Some(conn_id) = spawner.find_loop_connection(conv_id).await {
                    spawner.disconnect_loop_agent(&conn_id).await;
                }
            }
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

// ---- Finalize stage (§4.6): produce the result artifact ----

/// Finalize the issue once the write pipeline is fully drained (every task `done`,
/// gate free): assert the worktree is clean (all checkpoints committed), dispatch
/// a finalize iteration — which submits the `result` artifact via ingest, fanning
/// `results_from` edges to each task — and commit any finalize worktree changes as
/// the final checkpoint. A dirty tree blocks the issue (a structural fault a human
/// must resolve). A no-op until the pipeline is drained. See [`StepOutcome`] for
/// the return semantics (a dirty-tree block reports `Advanced` so the driver
/// re-ticks and stops).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_finalize(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    dag: &LoopDagView,
    config: &IssueConfig,
    worktree_folder_id: i32,
) -> Result<StepOutcome, LoopError> {
    // Only finalize once every LIVE task is done and no task holds the gate.
    // Excludes superseded/cancelled tasks: an integration (or coverage) loop-back
    // supersedes the prior plan's tasks, and a stale `Superseded` task must not
    // wedge the `all done` precondition forever (the new plan's tasks are what
    // must complete).
    let tasks: Vec<&LoopArtifactRow> = dag
        .artifacts
        .iter()
        .filter(|a| {
            a.kind == ArtifactKind::Task
                && !matches!(a.status, ArtifactStatus::Superseded | ArtifactStatus::Cancelled)
        })
        .collect();
    if tasks.is_empty() || !tasks.iter().all(|t| t.status == ArtifactStatus::Done) {
        return Ok(StepOutcome::Idle);
    }
    // The issue must be fully quiescent before finalizing — every task `Done` is
    // not enough on its own: a losing review slot (or any stray write) could still
    // be settling. With no per-issue write gate, "no in-flight iteration of any
    // stage" is the precondition. (The parallel fan-in path below re-checks this
    // internally so a conflict resolver it dispatches can still settle.)
    if issue_has_inflight(db, issue.id).await? {
        return Ok(StepOutcome::Idle);
    }

    // Parallel issues integrate their per-task branches via the result-stage
    // fan-in (engine-synthesized result), not an agent-submitted finalize. Only
    // once the result exists do they rejoin the shared "result ready → merge gate"
    // tail below.
    if issue.execution_mode.as_deref() == Some("parallel") && live_result(dag).is_none() {
        return crate::loop_engine::fan_in::run_parallel_finalize(
            db, data_dir, spawner, emitter, issue, dag, config, worktree_folder_id,
        )
        .await;
    }

    let fins = finalize_iterations(db, issue.id).await?;
    if fins
        .iter()
        .any(|it| matches!(it.status, IterationStatus::Queued | IterationStatus::Running))
    {
        // Finalize in flight — wait for its completion.
        return Ok(StepOutcome::Idle);
    }

    let folder = folder_service::get_folder_by_id(&db.conn, worktree_folder_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("worktree folder {worktree_folder_id}")))?;
    let worktree_path = Path::new(&folder.path);

    // Result already produced → commit any finalize worktree changes as the final
    // checkpoint, then run the INTEGRATION gate (§3.6) before the merge gate. The
    // checkpoint commits FIRST so integration reviewers read the committed combined
    // tree; the gate verifies the whole-issue closure against the assembled result.
    if let Some(result) = live_result(dag) {
        let message = format!("loop: finalize (issue #{})", issue.seq_no);
        worktree::checkpoint(worktree_path, &message).await?;

        return match drive_integration_review(
            db,
            data_dir,
            spawner,
            emitter,
            issue,
            config,
            worktree_folder_id,
            result,
        )
        .await?
        {
            // Whole-issue closure verified → open the merge gate. With a human gate
            // (auto_merge off) keep the approval card filed; auto_merge lands via
            // the driver, which only triggers once `integration_passed` holds.
            IntegrationGate::Pass => {
                if !config.auto_merge {
                    // First filing emits (the merge gate now needs a human); the
                    // per-tick recurrence has an identical `{gate}` payload → merge
                    // is a no-op → Unchanged → no event (no per-tick spam).
                    let upsert = loop_service::inbox::upsert_inbox(
                        &db.conn,
                        issue.space_id,
                        issue.id,
                        None,
                        InboxKind::Approval,
                        &format!("merge:{}", issue.id),
                        serde_json::json!({ "v": 1, "gate": "merge" }),
                    )
                    .await?;
                    if upsert.changed() {
                        emit_changed(emitter, issue.space_id, issue.id, issue.id, "approval");
                    }
                }
                Ok(StepOutcome::Idle)
            }
            // The assembled result fails a requirement/obligation → bounded loop-back.
            IntegrationGate::Fail => {
                maybe_integration_loopback(
                    db, emitter, issue, config, dag, result, worktree_folder_id,
                )
                .await
            }
            // Nothing to verify (empty closure) → issue already blocked; re-tick to stop.
            IntegrationGate::Blocked => Ok(StepOutcome::Advanced),
            // More integration reviewer checks needed.
            IntegrationGate::Pending(dispatched) => Ok(StepOutcome::from_dispatched(dispatched)),
        };
    }

    // No result yet. Assert the tree is clean (every task's checkpoint committed,
    // no stray state) before launching finalize; a dirty tree is a structural
    // fault a human must resolve, not something an agent should build a result on.
    if !worktree::is_clean(worktree_path).await? {
        cas_issue_status(&db.conn, issue.id, IssueStatus::Running, IssueStatus::Blocked).await?;
        let upsert = loop_service::inbox::upsert_inbox(
            &db.conn,
            issue.space_id,
            issue.id,
            None,
            InboxKind::Blocked,
            &format!("finalize_dirty:{}", issue.id),
            serde_json::json!({ "reason": "worktree_dirty_before_finalize" }),
        )
        .await?;
        if upsert.changed() {
            emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
        }
        // Issue is now blocked → re-tick so the driver observes it and stops (a
        // human retry then respawns the driver).
        return Ok(StepOutcome::Advanced);
    }

    let dispatched =
        dispatch_finalize(db, data_dir, spawner, emitter, issue, config, worktree_folder_id).await?;
    Ok(StepOutcome::from_dispatched(dispatched))
}

/// Outcome of the integration gate (target = the assembled result).
enum IntegrationGate {
    /// Whole-issue closure verified → caller opens the merge gate.
    Pass,
    /// A requirement / obligation is unmet by the assembled result → caller loops back.
    Fail,
    /// Nothing to verify (empty closure, D11) → issue blocked + inbox(unverifiable).
    Blocked,
    /// More integration reviewer checks needed (`true` = a slot was dispatched).
    Pending(bool),
}

/// The integration gate (§3.6): the result-targeted analogue of [`drive_reviews`].
/// Same per-criterion machinery (D2/D3/D8) — reviewers run as `Stage::Review` slots
/// on the result, each submitting one check per injected `integration_ordinals`
/// handle — but the decision is recorded under `FINALIZE_GATE_STAGE` and drives no
/// freeze/rework: the caller acts on the returned gate (merge / loop-back). A
/// recorded decision is the replay pivot. An empty closure blocks the issue (D11).
#[allow(clippy::too_many_arguments)]
async fn drive_integration_review(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_folder_id: i32,
    result: &LoopArtifactRow,
) -> Result<IntegrationGate, LoopError> {
    // Replay-safe pivot (D4): a recorded finalize decision drives the outcome.
    if let Some(outcome) =
        loop_service::gate_decision::outcome_for(&db.conn, result.id, FINALIZE_GATE_STAGE, result.attempt)
            .await?
    {
        return Ok(match outcome {
            GateOutcome::Pass => IntegrationGate::Pass,
            GateOutcome::Fail => IntegrationGate::Fail,
            GateOutcome::Undecided => IntegrationGate::Pending(false),
        });
    }

    // Empty-closure guard (D11): nothing to verify → block, never a vacuous pass.
    let ordinals =
        loop_service::criterion_ordinals::integration_ordinals(&db.conn, issue.id).await?;
    if ordinals.is_empty() {
        cas_issue_status(&db.conn, issue.id, IssueStatus::Running, IssueStatus::Blocked).await?;
        loop_service::inbox::upsert_inbox(
            &db.conn,
            issue.space_id,
            issue.id,
            None,
            InboxKind::Blocked,
            &format!("unverifiable:{}", issue.id),
            serde_json::json!({ "v": 1, "reason": "no_integration_criteria" }),
        )
        .await?;
        emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
        return Ok(IntegrationGate::Blocked);
    }

    let reviewer_specs = config.effective_reviewers();
    let reviewers = reviewer_specs.len() as i32;
    let iters = review_iterations(db, issue.id, result.id, result.attempt).await?;
    let verdicts = review_verdicts(db, &iters).await?;
    let decided_iter_ids: Vec<i32> = verdicts.keys().copied().collect();
    let mut missing_slots: Vec<i32> = Vec::new();
    for slot in 0..reviewers {
        let slot_iters: Vec<&loop_iteration::Model> =
            iters.iter().filter(|it| it.slot_no == Some(slot)).collect();
        let decided = slot_iters.iter().any(|it| verdicts.contains_key(&it.id));
        let in_flight = slot_iters
            .iter()
            .any(|it| matches!(it.status, IterationStatus::Queued | IterationStatus::Running));
        if !decided && !in_flight {
            missing_slots.push(slot);
        }
    }

    let injected_ids = injected_criterion_ids(&iters);
    let checks =
        loop_service::criterion_check::for_scope_iterations(&db.conn, result.id, &decided_iter_ids)
            .await?;
    let outcome = aggregate_checks(config.review_pass_rule, reviewers, &checks, &injected_ids);

    match outcome {
        GateOutcome::Pass | GateOutcome::Fail => {
            let policy = review_policy_json(config);
            match loop_service::gate_decision::record_decision(
                &db.conn,
                issue.space_id,
                issue.id,
                result.id,
                FINALIZE_GATE_STAGE,
                result.attempt,
                &checks,
                &injected_ids,
                &policy,
                outcome,
            )
            .await?
            {
                loop_service::gate_decision::RecordedDecision::Settled(_) => {}
                loop_service::gate_decision::RecordedDecision::Conflict(_) => {
                    return Err(LoopError::Conflict)
                }
            }
            Ok(if outcome == GateOutcome::Pass {
                IntegrationGate::Pass
            } else {
                IntegrationGate::Fail
            })
        }
        GateOutcome::Undecided => {
            let mut dispatched = false;
            for slot in missing_slots {
                if dispatch_review(
                    db,
                    data_dir,
                    spawner,
                    emitter,
                    issue,
                    worktree_folder_id,
                    result.id,
                    slot,
                    result.attempt,
                    &reviewer_specs[slot as usize],
                )
                .await?
                {
                    dispatched = true;
                }
            }
            Ok(IntegrationGate::Pending(dispatched))
        }
    }
}

/// Integration failure → bounded loop-back to plan (D7). Supersede the live result
/// (frees `uniq_result_per_issue`) and the live tasks so the read frontier re-emits
/// Plan next tick; the committed task work stays in the worktree (integration
/// faults are usually missing glue — a full reset would discard all task work and
/// likely re-fail). Bounded by the count of failed finalize decisions vs
/// `max_attempts` (0 = unlimited); exhaustion → block + inbox(integration_gap).
/// Parallel issues also restore their per-task worktrees from the integrated HEAD
/// so the new plan's tasks branch cleanly. Returns `Advanced` (re-tick).
#[allow(clippy::too_many_arguments)]
async fn maybe_integration_loopback(
    db: &AppDatabase,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    dag: &LoopDagView,
    result: &LoopArtifactRow,
    worktree_folder_id: i32,
) -> Result<StepOutcome, LoopError> {
    let conn = &db.conn;
    let fails = loop_service::gate_decision::count_fail(conn, issue.id, FINALIZE_GATE_STAGE).await?;
    let exhausted = config.max_attempts != 0 && fails >= config.max_attempts;
    if exhausted {
        cas_issue_status(conn, issue.id, IssueStatus::Running, IssueStatus::Blocked).await?;
        loop_service::inbox::upsert_inbox(
            conn,
            issue.space_id,
            issue.id,
            None,
            InboxKind::Blocked,
            &format!("integration_gap:{}", issue.id),
            serde_json::json!({ "v": 1, "reason": "integration_gap_exhausted", "fails": fails }),
        )
        .await?;
        emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
        return Ok(StepOutcome::Advanced);
    }

    // Supersede the live result, then the live tasks, so the next tick's read
    // frontier sees no live tasks and re-emits Plan (whose briefing carries the
    // failing requirement feedback via the standard channels).
    crate::loop_engine::transitions::cas_artifact_status_from(
        conn,
        result.id,
        &[ArtifactStatus::Done, ArtifactStatus::AwaitingApproval],
        ArtifactStatus::Superseded,
    )
    .await?;
    // Every live task is Done here (the finalize precondition), and only
    // (Done, Superseded) is a legal supersede edge for an implemented task.
    for t in dag.artifacts.iter().filter(|a| {
        a.kind == ArtifactKind::Task
            && !matches!(a.status, ArtifactStatus::Superseded | ArtifactStatus::Cancelled)
    }) {
        crate::loop_engine::transitions::cas_artifact_status_from(
            conn,
            t.id,
            &[ArtifactStatus::Done],
            ArtifactStatus::Superseded,
        )
        .await?;
    }

    // Parallel: restore the per-task worktrees from the integrated HEAD before the
    // re-plan creates fresh tasks (serial shares the issue worktree, nothing to do).
    if issue.execution_mode.as_deref() == Some("parallel") {
        if let Some(issue_wt) =
            folder_service::get_folder_by_id(conn, worktree_folder_id).await?
        {
            if let Some(space) = loop_service::space::get_space(conn, issue.space_id).await? {
                if let Some(repo) = folder_service::get_folder_by_id(conn, space.folder_id).await? {
                    worktree::reset_issue_subtree(
                        Path::new(&repo.path),
                        Path::new(&issue_wt.path),
                    )
                    .await?;
                }
            }
        }
    }

    tracing::info!(
        issue_id = issue.id,
        fails,
        result_id = result.id,
        "integration gap: superseding result + tasks and replanning"
    );
    emit_changed(emitter, issue.space_id, issue.id, issue.id, "issue");
    Ok(StepOutcome::Advanced)
}

/// Dispatch the finalize iteration (issue-level: `target = None`; the
/// `uniq_active_finalize` lease admits one finalize per issue).
async fn dispatch_finalize(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_folder_id: i32,
) -> Result<bool, LoopError> {
    let spec = resolve_agent_spec(config, Stage::Finalize);
    let handle = dispatch_iteration(
        db,
        data_dir,
        spawner,
        emitter.clone(),
        DispatchInput {
            space_id: issue.space_id,
            issue_id: issue.id,
            stage: Stage::Finalize,
            target_artifact_id: None,
            slot_no: None,
            attempt: 0,
            agent_type: spec.agent,
            mode_id: spec.mode_id,
            config_values: spec.config_values,
            worktree_folder_id,
        },
    )
    .await?;
    Ok(handle.is_some())
}

/// All finalize iterations for the issue — keyed by `(issue, finalize)`. Finalize
/// is issue-level (`target = None`) and stays singular under the parallel model
/// (`uniq_active_finalize` admits one), so no target key is needed.
async fn finalize_iterations(
    db: &AppDatabase,
    issue_id: i32,
) -> Result<Vec<loop_iteration::Model>, LoopError> {
    Ok(loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .filter(loop_iteration::Column::Stage.eq(Stage::Finalize))
        .all(&db.conn)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::error::AcpError;
    use crate::models::loops::{ReviewerEntry, ReviewerInherit};
    use sea_orm::ActiveEnum; // for `*.to_value()` in the test helpers below
    use crate::db::entities::loop_artifact_revision::ActorKind;
    use crate::db::entities::loop_issue::{IssuePriority, IssueStatus};
    use crate::db::service::loop_service::{artifact, issue, link, space};
    use crate::models::loops::LoopLinkRow;
    use crate::db::test_helpers::{fresh_disk_db, seed_folder};
    use crate::loop_engine::dispatch::settle_iteration;
    use crate::models::agent::AgentType;
    use async_trait::async_trait;
    use std::path::{Path, PathBuf};
    use std::process::Command as StdCommand;

    /// Build the validation signature the way `validate_after_implement` does, so
    /// the tests exercise the real fingerprint pipeline (normalize → hash).
    fn validation_sig(exit_codes: &[i32], output: &str) -> String {
        format!(
            "validation_failed:{}",
            sig_hash(&format!(
                "{exit_codes:?}\n{}",
                normalize_failure_output(output)
            ))
        )
    }

    #[test]
    fn normalize_collapses_volatile_substrings_to_one_sig() {
        // Two runs of the SAME failure: only timestamp, temp/worktree path, full
        // git oid, and duration differ. They must fingerprint identically (D14 —
        // otherwise the oscillation breaker never recognises a repeat).
        let run_a = "\
2026-06-18T12:34:56.789Z FAIL tests::auth at /var/folders/xy/abc123/loop-worktrees/issue-1/src/auth.rs
assertion failed: expected Ok got Err at commit 0123456789abcdef0123456789abcdef01234567
test result: FAILED. 1 failed in 12.4s";
        let run_b = "\
2026-06-19T01:02:03Z FAIL tests::auth at /var/folders/zz/def999/loop-worktrees/issue-1/src/auth.rs
assertion failed: expected Ok got Err at commit fedcba9876543210fedcba9876543210fedcba98
test result: FAILED. 1 failed in 0.9s";
        assert_eq!(
            validation_sig(&[1], run_a),
            validation_sig(&[1], run_b),
            "same failure with only volatile parts differing must share a sig"
        );
    }

    #[test]
    fn normalize_keeps_genuinely_different_failures_distinct() {
        let base = "assertion failed at tests::math\nexpected 0xdeadbeef got 0xcafef00d";
        // Different exit code → different sig.
        assert_ne!(validation_sig(&[1], base), validation_sig(&[2], base));
        // Different assertion expected/got values (short hex must NOT be stripped).
        let other_values = "assertion failed at tests::math\nexpected 0x12345678 got 0x000000ff";
        assert_ne!(validation_sig(&[1], base), validation_sig(&[1], other_values));
        // Different error message.
        let other_msg = "panic: index out of bounds\nexpected 0xdeadbeef got 0xcafef00d";
        assert_ne!(validation_sig(&[1], base), validation_sig(&[1], other_msg));
        // Different test name.
        let other_test = "assertion failed at tests::geometry\nexpected 0xdeadbeef got 0xcafef00d";
        assert_ne!(validation_sig(&[1], base), validation_sig(&[1], other_test));
    }

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
            _preferred_mode_id: Option<String>,
            _preferred_config_values: std::collections::BTreeMap<String, String>,
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
        async fn find_loop_connection(&self, _conversation_id: i32) -> Option<String> {
            None
        }
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
            Some(&IssueConfig::default()),
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
        // A real plan attaches the task's own acceptance criterion; the review
        // gate injects it as the `T1` check handle.
        artifact::add_criterion(
            &h.db.conn,
            task.id,
            crate::db::entities::loop_criterion::CriterionKind::Acceptance,
            "the task is implemented correctly",
        )
        .await
        .unwrap();
        link::create_link(&h.db.conn, h.space_id, task.id, root, LinkKind::DerivesFrom, None)
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

    async fn drive(h: &Harness) -> StepOutcome {
        drive_tracking(h, &IssueConfig::default(), &mut HashMap::new()).await
    }

    /// Drive with an explicit infra-retry counter that persists across calls (for
    /// the bounded-retry test); the convenience `drive`/`drive_with` discard it.
    async fn drive_tracking(
        h: &Harness,
        config: &IssueConfig,
        infra_retries: &mut HashMap<i32, u32>,
    ) -> StepOutcome {
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
            infra_retries,
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

    /// Pure `Task` row for the dependency-frontier tests (`ready_tasks`).
    fn ready_task_row(id: i32, status: ArtifactStatus) -> LoopArtifactRow {
        LoopArtifactRow {
            id,
            issue_id: 1,
            issue_seq: 1,
            kind: ArtifactKind::Task,
            title: format!("T{id}"),
            status,
            origin: ActorKind::Agent,
            produced_by_iteration_id: None,
            verdict: None,
            attempt: 0,
            contribution_kind: loop_artifact::ContributionKind::Delta,
            sort: id,
            updated_at: Utc::now(),
        }
    }

    /// Build a task-only DAG. `edges` are `(successor, predecessor)` pairs — the
    /// `DependsOn` direction (from = successor, to = predecessor).
    fn depends_dag(tasks: &[(i32, ArtifactStatus)], edges: &[(i32, i32)]) -> LoopDagView {
        LoopDagView {
            artifacts: tasks.iter().map(|&(id, st)| ready_task_row(id, st)).collect(),
            links: edges
                .iter()
                .enumerate()
                .map(|(i, &(succ, pred))| LoopLinkRow {
                    id: i as i32 + 1,
                    from_artifact_id: succ,
                    to_artifact_id: pred,
                    kind: LinkKind::DependsOn,
                    source_revision_id: None,
                })
                .collect(),
            coverage: Vec::new(),
            criterion_checks: Vec::new(),
            gate_decisions: Vec::new(),
            live_iterations: Vec::new(),
            artifact_iteration_refs: Vec::new(),
        }
    }

    #[test]
    fn ready_tasks_chain() {
        use ArtifactStatus::{Done, Pending};
        // A→B→C, all pending: only the root A is ready.
        let dag = depends_dag(&[(1, Pending), (2, Pending), (3, Pending)], &[(2, 1), (3, 2)]);
        assert_eq!(
            ready_tasks(&dag).iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![1]
        );
        // A done → B becomes ready; C is still blocked behind B.
        let dag = depends_dag(&[(1, Done), (2, Pending), (3, Pending)], &[(2, 1), (3, 2)]);
        assert_eq!(
            ready_tasks(&dag).iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn ready_tasks_fanout() {
        use ArtifactStatus::{Done, Pending};
        // A→B, A→C. While A is pending, neither successor is ready.
        let dag = depends_dag(&[(1, Pending), (2, Pending), (3, Pending)], &[(2, 1), (3, 1)]);
        assert_eq!(
            ready_tasks(&dag).iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![1]
        );
        // A done → B and C are BOTH ready at once (true parallelism).
        let dag = depends_dag(&[(1, Done), (2, Pending), (3, Pending)], &[(2, 1), (3, 1)]);
        assert_eq!(
            ready_tasks(&dag).iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn ready_tasks_edge_direction_contract() {
        use ArtifactStatus::{Done, Pending};
        // B depends_on A ⇒ edge (from=B, to=A). B's readiness is gated on A being
        // Done, never the reverse.
        let blocked = depends_dag(&[(1, Pending), (2, Pending)], &[(2, 1)]);
        assert_eq!(
            ready_tasks(&blocked).iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![1]
        );
        let unblocked = depends_dag(&[(1, Done), (2, Pending)], &[(2, 1)]);
        assert_eq!(
            ready_tasks(&unblocked).iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[tokio::test]
    async fn blocked_task_does_not_strand_other_ready_tasks() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        let a = add_task(&h, "A").await;
        let b = add_task(&h, "B").await;
        // A is blocked; B is independent and ready. With no per-issue write gate, a
        // blocked task must not strand B — the drive still dispatches B's implement.
        cas_artifact_status(&h.db.conn, a, ArtifactStatus::Pending, ArtifactStatus::Blocked)
            .await
            .unwrap();
        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
        assert_eq!(
            implement_iterations(&h.db, h.issue_id, b).await.unwrap().len(),
            1,
            "ready task B dispatched despite blocked A"
        );
        assert!(
            implement_iterations(&h.db, h.issue_id, a)
                .await
                .unwrap()
                .is_empty(),
            "blocked A never dispatched"
        );
    }

    #[tokio::test]
    async fn parallel_dispatches_all_ready_tasks_concurrently() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        add_task(&h, "A").await;
        add_task(&h, "B").await;

        // One drive launches BOTH independent tasks' implement iterations at once —
        // true concurrency, not the old one-at-a-time gate.
        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
        let running = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Implement))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .all(&h.db.conn)
            .await
            .unwrap();
        assert_eq!(
            running.len(),
            2,
            "both ready tasks have a running implement in the same tick"
        );
    }

    #[tokio::test]
    async fn worktree_add_failure_retries_then_blocks() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        let a = add_task(&h, "A").await;
        let b = add_task(&h, "B").await;
        // B depends on A. Mark A Done but freeze it at a BOGUS commit, so
        // ensure_task_worktree(B) cannot resolve its base ref — a deterministic
        // infra failure on every attempt.
        link::create_link(&h.db.conn, h.space_id, b, a, LinkKind::DependsOn, None)
            .await
            .unwrap();
        cas_artifact_status(&h.db.conn, a, ArtifactStatus::Pending, ArtifactStatus::InProgress)
            .await
            .unwrap();
        crate::loop_engine::transitions::cas_task_done_with_contribution(
            &h.db.conn,
            a,
            crate::loop_engine::transitions::TaskContribution::Delta("dead".repeat(10)),
        )
        .await
        .unwrap();
        assert_eq!(task_node(&h, a).await.status, ArtifactStatus::Done);

        let mut retries: HashMap<i32, u32> = HashMap::new();
        // Each of the first INFRA_RETRY_MAX-1 drives skips B (retry pending) without
        // blocking; the failure streak is counted in driver memory.
        for i in 1..INFRA_RETRY_MAX {
            let out = drive_tracking(&h, &IssueConfig::default(), &mut retries).await;
            assert_eq!(out, StepOutcome::Idle, "skip, awaiting retry");
            assert_eq!(retries.get(&b), Some(&i), "failure streak counted in memory");
            assert_eq!(load_issue(&h).await.status, IssueStatus::Running);
        }
        // The next failure trips the breaker: B + issue blocked, card filed, streak cleared.
        let out = drive_tracking(&h, &IssueConfig::default(), &mut retries).await;
        assert_eq!(out, StepOutcome::Advanced, "block is durable progress");
        assert_eq!(task_node(&h, b).await.status, ArtifactStatus::Blocked);
        assert_eq!(load_issue(&h).await.status, IssueStatus::Blocked);
        assert!(!retries.contains_key(&b), "streak cleared once blocked");
        let inbox = loop_service::inbox::list_inbox(&h.db.conn, h.space_id, None)
            .await
            .unwrap();
        assert!(
            inbox.iter().any(|i| i.kind == InboxKind::Blocked
                && i.subject_key == format!("infra_failure:{b}")),
            "infra_failure card filed for the task"
        );
    }

    #[tokio::test]
    async fn dead_dependency_blocks_issue() {
        let h = setup().await;
        let t1 = add_task(&h, "T1").await;
        let t2 = add_task(&h, "T2").await;
        // T2 depends on T1; T1 is blocked → T2 can never start.
        link::create_link(&h.db.conn, h.space_id, t2, t1, LinkKind::DependsOn, None)
            .await
            .unwrap();
        cas_artifact_status(&h.db.conn, t1, ArtifactStatus::Pending, ArtifactStatus::Blocked)
            .await
            .unwrap();
        // No gate, no in-flight, no ready task → detect the dead dependency and
        // block the issue (retry-reachable) with a clear card, never park silently.
        assert_eq!(drive(&h).await, StepOutcome::Advanced);
        assert_eq!(load_issue(&h).await.status, IssueStatus::Blocked);
        let inbox = loop_service::inbox::list_inbox(&h.db.conn, h.space_id, None)
            .await
            .unwrap();
        assert!(
            inbox.iter().any(|i| i.kind == InboxKind::Blocked
                && i.subject_key == format!("dependency_unsatisfiable:{}", h.issue_id)),
            "files a dependency_unsatisfiable card"
        );
    }

    async fn set_execution_mode(h: &Harness, mode: &str) {
        loop_issue::Entity::update_many()
            .col_expr(loop_issue::Column::ExecutionMode, Expr::value(mode))
            .filter(loop_issue::Column::Id.eq(h.issue_id))
            .exec(&h.db.conn)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn parallel_task_implements_in_its_own_worktree() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        let task = add_task(&h, "T").await;

        // Parallel mode: drive ensures the task's own worktree and dispatches
        // implement there.
        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
        let impl_id = running_implement_id(&h).await;

        // The task worktree is distinct from the issue worktree.
        let task_wt = worktree::ensure_task_worktree(&h.db.conn, h.data.path(), h.issue_id, task)
            .await
            .unwrap();
        assert_ne!(task_wt.worktree_path, h.worktree_path);

        // The agent edits the TASK worktree; settle; drive → checkpoint commits
        // there (not the issue worktree).
        std::fs::write(task_wt.worktree_path.join("feature.txt"), "work\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, impl_id)
            .await
            .unwrap();
        assert_eq!(drive(&h).await, StepOutcome::Advanced);

        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::InProgress);
        assert!(task_wt.worktree_path.join("feature.txt").exists());
        assert!(
            !h.worktree_path.join("feature.txt").exists(),
            "parallel work stays off the issue worktree until fan-in"
        );
    }

    /// Regression for the driver wedge: an implement settle must report `Advanced`
    /// (not the old `Ok(false)` that folded into Idle and parked forever), promote
    /// the task, and let the very next drive dispatch review — no manual tick.
    #[tokio::test]
    async fn implement_settle_advances_then_next_drive_dispatches_review() {
        let h = setup().await;
        let task = add_task(&h, "T").await;

        // A round: gate free → claim + dispatch implement (in flight).
        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
        let impl_id = running_implement_id(&h).await;

        // Simulate the agent editing the tree, then the turn settling.
        std::fs::write(h.worktree_path.join("feature.txt"), "work\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, impl_id)
            .await
            .unwrap();

        // The post-settle drive must report Advanced (was wedged as Idle before),
        // with the task promoted to in_progress (checkpoint + validate ran; the
        // default config has no validation_commands).
        assert_eq!(drive(&h).await, StepOutcome::Advanced);
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::InProgress);

        // The next drive dispatches review immediately — no external tick / resume.
        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
        let has_review = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Review))
            .one(&h.db.conn)
            .await
            .unwrap()
            .is_some();
        assert!(has_review, "review dispatched right after implement advanced");
    }

    /// Simulate `run_driver`'s fixpoint: keep driving while it reports `Advanced`.
    /// **Bounded** — a non-converging chain panics (fail-fast in CI) rather than
    /// hanging, which a real fix never reaches.
    async fn drive_to_quiescence(h: &Harness) -> StepOutcome {
        for _ in 0..256 {
            match drive(h).await {
                StepOutcome::Advanced => continue,
                other => return other,
            }
        }
        panic!("driver did not reach quiescence within bound — non-progressing Advanced");
    }

    /// End-to-end fixpoint: a single implement settle, then the fixpoint loop must
    /// reach a *running* review with no external tick / manual resume.
    #[tokio::test]
    async fn settle_then_fixpoint_reaches_review_without_manual_tick() {
        let h = setup().await;
        add_task(&h, "T").await;

        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
        let impl_id = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "work\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, impl_id)
            .await
            .unwrap();

        // One settle, then run the fixpoint: it should stop at "review dispatched"
        // (Dispatched), entirely without external intervention.
        assert_eq!(drive_to_quiescence(&h).await, StepOutcome::Dispatched);
        let review = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Review))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .one(&h.db.conn)
            .await
            .unwrap();
        assert!(review.is_some(), "review running after a single implement settle");
    }

    #[tokio::test]
    async fn undecided_mode_drives_one_task_at_a_time() {
        let h = setup().await;
        let t1 = add_task(&h, "Task 1").await;
        let t2 = add_task(&h, "Task 2").await;

        // execution_mode is unset (not `parallel`): the two tasks would share the
        // issue worktree, so the drive serializes to the lowest-ordered task.
        assert_eq!(
            drive(&h).await,
            StepOutcome::Dispatched,
            "first tick dispatches an implement"
        );

        // A second tick (no completion yet) must not start the other task.
        assert_eq!(
            drive(&h).await,
            StepOutcome::Idle,
            "no second implement while the first is in flight"
        );
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
    async fn repeated_tick_no_duplicate_dispatch_per_task() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        let a = add_task(&h, "A").await;
        let b = add_task(&h, "B").await;

        // First tick fans out implement to both independent tasks.
        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
        // A second tick (nothing settled) must not start a duplicate for either —
        // the `(issue, target)` implement lease makes the re-dispatch a no-op.
        assert_eq!(drive(&h).await, StepOutcome::Idle);
        for t in [a, b] {
            assert_eq!(
                implement_iterations(&h.db, h.issue_id, t)
                    .await
                    .unwrap()
                    .len(),
                1,
                "exactly one implement iteration per task across repeated ticks"
            );
        }
    }

    #[tokio::test]
    async fn implement_success_checkpoints_and_advances() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;

        // Tick 1: dispatch implement for the task.
        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
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
        assert_eq!(
            drive(&h).await,
            StepOutcome::Advanced,
            "checkpoint/advance is an advance, not a dispatch"
        );
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
    }

    #[tokio::test]
    async fn implement_empty_diff_counts_no_progress() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;

        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
        let iter_id = running_implement_id(&h).await;
        // Agent produced no change. Settle, then drive: empty diff → no progress.
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // C3 invariant: a settled implement before its checkpoint has NO outcome yet.
        assert_eq!(
            loop_iteration::Entity::find_by_id(iter_id)
                .one(&h.db.conn)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            None,
            "implement outcome stays NULL until the checkpoint runs (C3)"
        );

        // Tick 2: checkpoint finds nothing → rework bump + retry dispatch.
        assert_eq!(
            drive(&h).await,
            StepOutcome::Dispatched,
            "no-progress retries implement"
        );
        // D11: the checkpoint recorded the implement's outcome as empty_diff.
        assert_eq!(
            loop_iteration::Entity::find_by_id(iter_id)
                .one(&h.db.conn)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            Some(IterationOutcome::EmptyDiff)
        );
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

    #[tokio::test]
    async fn implement_declared_complete_routes_to_review_without_rework() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;

        assert_eq!(drive(&h).await, StepOutcome::Dispatched);
        let iter_id = running_implement_id(&h).await;
        // Agent makes NO change but declares the task already satisfied (D12).
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();
        loop_iteration::Entity::update_many()
            .col_expr(
                loop_iteration::Column::AgentCompletionReason,
                Expr::value("dependency #2 already delivered this"),
            )
            .filter(loop_iteration::Column::Id.eq(iter_id))
            .exec(&h.db.conn)
            .await
            .unwrap();

        // Checkpoint: empty diff BUT declared → route to review, NOT a no-progress
        // rework. (The driver advances the now-in_progress task into its review
        // round in the same tick.)
        let _ = drive(&h).await;

        let node = task_node(&h, task).await;
        assert_eq!(
            node.status,
            ArtifactStatus::InProgress,
            "declared no-op routes to review, not blocked/pending"
        );
        assert_eq!(node.attempt, 0, "a declared no-op is NOT a rework — attempt unchanged");
        assert_eq!(
            loop_iteration::Entity::find_by_id(iter_id)
                .one(&h.db.conn)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            Some(IterationOutcome::DeclaredComplete),
        );
        assert!(
            task_model(&h, task).await.last_failure_sig.is_none(),
            "no empty_diff failure is recorded for a declared no-op"
        );
    }

    #[tokio::test]
    async fn oscillation_breaker_promotes_after_repeated_block_epochs() {
        use crate::db::entities::loop_inbox_item::{self, InboxStatus};

        async fn card_status(h: &Harness, subject: &str) -> Option<InboxStatus> {
            loop_inbox_item::Entity::find()
                .filter(loop_inbox_item::Column::IssueId.eq(h.issue_id))
                .filter(loop_inbox_item::Column::SubjectKey.eq(subject.to_string()))
                .one(&h.db.conn)
                .await
                .unwrap()
                .map(|c| c.status)
        }
        async fn rearm(h: &Harness, task: i32) {
            cas_artifact_status(&h.db.conn, task, ArtifactStatus::Blocked, ArtifactStatus::Pending)
                .await
                .unwrap();
        }

        let h = setup().await;
        let task = add_task(&h, "T").await;
        let cfg = IssueConfig { oscillation_limit: 2, ..IssueConfig::default() };
        let issue = load_issue(&h).await;
        let np = format!("no_progress:{task}");
        let osc = format!("oscillation:{task}");

        // Epoch 1: first block → ordinary no_progress card, count=1, no promotion.
        mark_blocked(&h.db, &EventEmitter::Noop, &issue, &cfg, task, None, "repeated_failure", "S", 1)
            .await
            .unwrap();
        assert_eq!(task_model(&h, task).await.oscillation_count, 1);
        assert_eq!(card_status(&h, &np).await, Some(InboxStatus::Pending));
        assert_eq!(card_status(&h, &osc).await, None, "no oscillation card below limit");

        // Epoch 2 (same sig, after a re-arm): count hits the limit → promote.
        rearm(&h, task).await;
        mark_blocked(&h.db, &EventEmitter::Noop, &issue, &cfg, task, None, "repeated_failure", "S", 2)
            .await
            .unwrap();
        assert_eq!(task_model(&h, task).await.oscillation_count, 2);
        assert_eq!(
            card_status(&h, &osc).await,
            Some(InboxStatus::Pending),
            "promoted to an oscillation card"
        );
        assert_eq!(
            card_status(&h, &np).await,
            Some(InboxStatus::Handled),
            "the ordinary no_progress card is superseded"
        );

        // Replay WITHOUT a re-arm (task already blocked) must NOT inflate the count.
        mark_blocked(&h.db, &EventEmitter::Noop, &issue, &cfg, task, None, "repeated_failure", "S", 3)
            .await
            .unwrap();
        assert_eq!(
            task_model(&h, task).await.oscillation_count,
            2,
            "an idempotent replay does not step the epoch"
        );
    }

    #[tokio::test]
    async fn oscillation_breaker_off_when_limit_zero() {
        use crate::db::entities::loop_inbox_item::{self, InboxStatus};
        let h = setup().await;
        let task = add_task(&h, "T").await;
        let cfg = IssueConfig { oscillation_limit: 0, ..IssueConfig::default() };
        let issue = load_issue(&h).await;

        // Many same-sig blocks with the breaker off → never an oscillation card.
        for attempt in 1..=4 {
            cas_artifact_status(&h.db.conn, task, ArtifactStatus::Blocked, ArtifactStatus::Pending)
                .await
                .ok();
            mark_blocked(
                &h.db, &EventEmitter::Noop, &issue, &cfg, task, None, "repeated_failure", "S",
                attempt,
            )
            .await
            .unwrap();
        }
        let osc = loop_inbox_item::Entity::find()
            .filter(loop_inbox_item::Column::IssueId.eq(h.issue_id))
            .filter(loop_inbox_item::Column::SubjectKey.eq(format!("oscillation:{task}")))
            .one(&h.db.conn)
            .await
            .unwrap();
        assert!(osc.is_none(), "limit=0 disables the oscillation breaker");
        assert!(
            card_status_pending(&h, &format!("no_progress:{task}")).await,
            "ordinary no_progress card still filed"
        );
        // local helper kept inline to avoid a module-level addition
        async fn card_status_pending(h: &Harness, subject: &str) -> bool {
            loop_inbox_item::Entity::find()
                .filter(loop_inbox_item::Column::IssueId.eq(h.issue_id))
                .filter(loop_inbox_item::Column::SubjectKey.eq(subject.to_string()))
                .filter(loop_inbox_item::Column::Status.eq(InboxStatus::Pending))
                .one(&h.db.conn)
                .await
                .unwrap()
                .is_some()
        }
    }

    // ---- Task 2.2: deterministic validation after implement ----

    fn config_with_validation(cmds: &[&str]) -> IssueConfig {
        IssueConfig {
            validation_commands: cmds.iter().map(|s| s.to_string()).collect(),
            ..IssueConfig::default()
        }
    }

    async fn drive_with(h: &Harness, config: &IssueConfig) -> StepOutcome {
        drive_tracking(h, config, &mut HashMap::new()).await
    }

    /// Implement → checkpoint → validation passes → task implemented (in_progress).
    #[cfg(unix)]
    #[tokio::test]
    async fn implement_passing_validation_advances() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_with_validation(&["true"]);

        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Dispatched,
            "tick 1 dispatches implement"
        );
        let iter_id = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // Tick 2: checkpoint + validation(pass) → advance (not a dispatch).
        assert_eq!(drive_with(&h, &cfg).await, StepOutcome::Advanced);
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::InProgress);
        let runs = loop_service::validation::list_for_task(&h.db.conn, task)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1, "one validation run recorded");
        assert!(runs[0].passed, "run passed");
        // D11: a validated implement records `succeeded`.
        assert_eq!(
            loop_iteration::Entity::find_by_id(iter_id)
                .one(&h.db.conn)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            Some(IterationOutcome::Succeeded)
        );
    }

    /// Implement → checkpoint → validation fails → rework (attempt++, re-dispatch).
    #[cfg(unix)]
    #[tokio::test]
    async fn implement_failing_validation_reworks() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_with_validation(&["false"]);

        assert_eq!(drive_with(&h, &cfg).await, StepOutcome::Dispatched);
        let iter_id = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // Tick 2: checkpoint + validation(fail) → rework + re-dispatch implement.
        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Dispatched,
            "validation failure retries implement"
        );
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
        // D11: a failed-validation implement records `validation_failed`.
        assert_eq!(
            loop_iteration::Entity::find_by_id(iter_id)
                .one(&h.db.conn)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            Some(IterationOutcome::ValidationFailed)
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

    /// Implement → checkpoint → validation can't run (missing tool) → task blocked
    /// + inbox card; no rework (not the agent's fault), no further dispatch.
    #[cfg(unix)]
    #[tokio::test]
    async fn implement_unrunnable_validation_blocks() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_with_validation(&["codeg-no-such-tool-xyzzy"]);

        assert_eq!(drive_with(&h, &cfg).await, StepOutcome::Dispatched);
        let iter_id = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // Tick 2: checkpoint + validation(unrunnable) → block the task AND the
        // issue (an advance, not a dispatch); the driver then re-ticks and stops,
        // and the now-blocked issue is reachable by the human `retry`.
        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Advanced,
            "unrunnable validation blocks the task"
        );
        let node = task_node(&h, task).await;
        assert_eq!(node.status, ArtifactStatus::Blocked);
        assert_eq!(node.attempt, 0, "config error does not consume a rework");
        assert_eq!(
            load_issue(&h).await.status,
            IssueStatus::Blocked,
            "issue blocked too, so the human retry can reach it"
        );
        // A blocked inbox card was filed for the task.
        let inbox = loop_service::inbox::list_inbox(&h.db.conn, h.space_id, None)
            .await
            .unwrap();
        assert!(
            inbox.iter().any(|i| i.kind == InboxKind::Blocked
                && i.subject_key == format!("validation_blocked:{task}")),
            "blocked inbox card filed"
        );
    }

    // ---- Task 2.3: review stage ----

    fn config_reviewers(n: u32, rule: ReviewPassRule) -> IssueConfig {
        IssueConfig {
            reviewers: (0..n)
                .map(|_| ReviewerEntry::Inherit(ReviewerInherit { inherit: true }))
                .collect(),
            review_pass_rule: rule,
            ..IssueConfig::default()
        }
    }

    /// Drive a fresh task from pending to `in_progress` (implemented + validated)
    /// so review tests can start at the review stage.
    async fn implement_to_in_progress(h: &Harness, cfg: &IssueConfig, marker: &str) {
        assert_eq!(
            drive_with(h, cfg).await,
            StepOutcome::Dispatched,
            "dispatch implement"
        );
        let iter_id = running_implement_id(h).await;
        std::fs::write(h.worktree_path.join(marker), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();
        assert_eq!(
            drive_with(h, cfg).await,
            StepOutcome::Advanced,
            "checkpoint + validate → in_progress"
        );
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

    /// A reviewer submits its per-criterion checks through the real ingest path
    /// (token → running iteration → review artifact + checks + link). Submits one
    /// check with `verdict` for EACH handle in the iteration's injected manifest
    /// (what dispatch stashed), so the reviewed task's whole checklist is answered.
    async fn submit_verdict(h: &Harness, review_iter_id: i32, verdict: &str, findings: &str) {
        let it = loop_iteration::Entity::find_by_id(review_iter_id)
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap();
        let manifest: serde_json::Value = it
            .context_manifest
            .as_deref()
            .map(|s| serde_json::from_str(s).unwrap())
            .unwrap_or(serde_json::Value::Null);
        let criteria = manifest
            .get("criteria")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let evidence = if verdict == "fail" {
            if findings.is_empty() { "defect found" } else { findings }
        } else {
            "verified"
        };
        let checks: Vec<serde_json::Value> = criteria
            .keys()
            .map(|handle| serde_json::json!({ "criterion": handle, "verdict": verdict, "evidence": evidence }))
            .collect();
        crate::loop_engine::ingest::ingest(
            &h.db.conn,
            &it.capability_token,
            "loop_submit_review",
            &serde_json::json!({ "checks": checks, "findings": findings }),
        )
        .await
        .unwrap();
    }

    /// Review passes → task done (its accepted tip frozen as the integration
    /// commit), dropping out of the next tick's drivable set.
    #[tokio::test]
    async fn review_pass_marks_done() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        implement_to_in_progress(&h, &cfg, "feature.txt").await;
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::InProgress);

        // Dispatch the reviewer, who passes.
        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Dispatched,
            "dispatches a reviewer"
        );
        let review = running_review(&h).await;
        submit_verdict(&h, review, "pass", "looks good").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review)
            .await
            .unwrap();

        // Aggregate → pass → task done.
        assert_eq!(drive_with(&h, &cfg).await, StepOutcome::Advanced);
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::Done);

        // The pass is recorded as an immutable gate decision over the reviewer's
        // checks (the durable replay pivot).
        let decisions = loop_service::gate_decision::list_for_issue(&h.db.conn, h.issue_id)
            .await
            .unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].outcome, GateOutcome::Pass);
        assert_eq!(decisions[0].stage, "review");
        assert!(!decisions[0].input_check_ids.is_empty(), "decision records its check ids");
    }

    /// Per-criterion aggregation (D8) is canonical: it can PASS where aggregating
    /// per-reviewer verdicts would FAIL. 3 reviewers × 2 criteria, each criterion
    /// clears a 2/3 majority, but only 1 of 3 reviewers is all-pass — so a verdict
    /// majority rejects while the gate (correctly) accepts.
    #[test]
    fn aggregate_checks_per_criterion_diverges_from_verdict_majority() {
        fn chk(criterion: i32, iteration: i32, v: CheckVerdict) -> LoopCriterionCheckRow {
            LoopCriterionCheckRow {
                id: 0,
                criterion_id: criterion,
                iteration_id: iteration,
                scope_artifact_id: 0,
                verdict: v,
                evidence: String::new(),
            }
        }
        // criteria 10 (A) & 11 (B); reviewers = iterations 100/101/102.
        let checks = vec![
            chk(10, 100, CheckVerdict::Pass), chk(11, 100, CheckVerdict::Fail), // r0
            chk(10, 101, CheckVerdict::Fail), chk(11, 101, CheckVerdict::Pass), // r1
            chk(10, 102, CheckVerdict::Pass), chk(11, 102, CheckVerdict::Pass), // r2
        ];
        // Per criterion (Majority, n=3): A passes 2/3, B passes 2/3 → gate PASS.
        assert_eq!(
            aggregate_checks(ReviewPassRule::Majority, 3, &checks, &[10, 11]),
            GateOutcome::Pass
        );
        // Verdict-majority would reject: r0 & r1 each have a failing check (display
        // verdict Fail), only r2 is all-pass → 2 of 3 fail.
        let per_reviewer = [ReviewVerdict::Fail, ReviewVerdict::Fail, ReviewVerdict::Pass];
        assert!(
            matches!(aggregate(ReviewPassRule::Majority, 3, &per_reviewer), ReviewDecision::Fail),
            "aggregating per-reviewer verdicts would (wrongly) reject"
        );
        // An empty injected set is never a vacuous pass.
        assert_eq!(
            aggregate_checks(ReviewPassRule::Unanimous, 1, &[], &[]),
            GateOutcome::Undecided
        );
    }

    /// One failing criterion → gate Fail recorded + rework (task back to pending at
    /// the next attempt). The decision is keyed at the DECIDING attempt (0).
    #[tokio::test]
    async fn review_fail_records_gate_decision() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        implement_to_in_progress(&h, &cfg, "feature.txt").await;

        assert_eq!(drive_with(&h, &cfg).await, StepOutcome::Dispatched);
        let review = running_review(&h).await;
        submit_verdict(&h, review, "fail", "missing error handling").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review).await.unwrap();

        assert_eq!(drive_with(&h, &cfg).await, StepOutcome::Advanced);
        // Decision recorded Fail at attempt 0; task reworked to pending at attempt 1.
        assert_eq!(
            loop_service::gate_decision::outcome_for(&h.db.conn, task, "review", 0).await.unwrap(),
            Some(GateOutcome::Fail)
        );
        let node = task_node(&h, task).await;
        assert_eq!(node.status, ArtifactStatus::Pending);
        assert_eq!(node.attempt, 1);
    }

    /// Replay pivot (D4): a recorded decision drives the side-effects on the next
    /// tick without re-running reviewers — a crash after recording but before the
    /// freeze is completed from the decision. Recording a Pass for an InProgress
    /// task and driving freezes it Done with NO reviewer dispatched.
    #[tokio::test]
    async fn review_replay_freezes_from_recorded_decision() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        implement_to_in_progress(&h, &cfg, "feature.txt").await;
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::InProgress);

        // Simulate "decision recorded, crash before freeze": persist a Pass decision
        // at the current attempt with no reviewers run.
        loop_service::gate_decision::record_decision(
            &h.db.conn, h.space_id, h.issue_id, task, "review", 0, &[], &[], "{}", GateOutcome::Pass,
        )
        .await
        .unwrap();

        // The next drive resolves the pivot → freeze the task Done, dispatching no
        // reviewer.
        assert_eq!(drive_with(&h, &cfg).await, StepOutcome::Advanced);
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::Done);
        assert!(
            review_iters_of(&h, task).await.is_empty(),
            "replay drove from the decision without dispatching a reviewer"
        );
    }

    /// Each configured reviewer runs as its own slot with its own agent.
    #[tokio::test]
    async fn reviews_dispatch_per_reviewer_agent() {
        use crate::db::entities::conversation;
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        // Two heterogeneous reviewers → two slots, each with its own agent.
        let cfg = IssueConfig {
            reviewers: vec![
                ReviewerEntry::Spec(ReviewerSpec {
                    agent: AgentType::ClaudeCode,
                    mode_id: None,
                    config_values: Default::default(),
                }),
                ReviewerEntry::Spec(ReviewerSpec {
                    agent: AgentType::Codex,
                    mode_id: None,
                    config_values: Default::default(),
                }),
            ],
            ..IssueConfig::default()
        };
        implement_to_in_progress(&h, &cfg, "feature.txt").await;

        // One drive dispatches both review slots.
        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Dispatched,
            "dispatches reviewers"
        );
        let reviews = review_iters_of(&h, task).await; // sorted by slot_no
        assert_eq!(reviews.len(), 2, "one iteration per configured reviewer");

        // Slot 0 → claude_code, slot 1 → codex (the conversation records the agent).
        let mut agents = Vec::new();
        for r in &reviews {
            let conv = conversation::Entity::find_by_id(r.conversation_id.unwrap())
                .one(&h.db.conn)
                .await
                .unwrap()
                .unwrap();
            agents.push(conv.agent_type);
        }
        assert_eq!(
            agents,
            vec!["claude_code".to_string(), "codex".to_string()]
        );
    }

    /// Review fails → rework (task pending, attempt++, findings recorded); the
    /// findings surface for the next implement briefing.
    #[tokio::test]
    async fn review_fail_reworks_with_findings() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        implement_to_in_progress(&h, &cfg, "feature.txt").await;

        assert_eq!(drive_with(&h, &cfg).await, StepOutcome::Dispatched);
        let review = running_review(&h).await;
        submit_verdict(&h, review, "fail", "missing error handling").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review)
            .await
            .unwrap();

        // Aggregate → fail → rework (an advance, not a dispatch).
        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Advanced,
            "review fail reworks, not a dispatch"
        );
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
        let cfg = config_reviewers(2, ReviewPassRule::Unanimous);
        implement_to_in_progress(&h, &cfg, "feature.txt").await;

        // Dispatch both review slots.
        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Dispatched,
            "dispatches reviewers"
        );
        let reviews = review_iters_of(&h, task).await;
        assert_eq!(reviews.len(), 2, "two review slots");

        // Slot 0 fails; slot 1 is still running → unanimous fail-fast.
        submit_verdict(&h, reviews[0].id, "fail", "regression").await;
        settle_iteration(&h.db, &EventEmitter::Noop, reviews[0].id)
            .await
            .unwrap();

        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Advanced,
            "fail-fast reworks"
        );
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

        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Dispatched,
            "tick 1 dispatches implement"
        );
        let iter_id = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter_id)
            .await
            .unwrap();

        // Tick 2: validation fails at attempt 0 → bump→1 ≥ max(1) → block (an
        // advance: the issue is now blocked, so the driver re-ticks then stops).
        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Advanced,
            "a breaker block advances (then stops), not a dispatch"
        );
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
        assert_eq!(drive_with(&h, &cfg).await, StepOutcome::Dispatched);
        let iter0 = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter0)
            .await
            .unwrap();
        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Dispatched,
            "attempt 0 failure retries"
        );
        assert_eq!(task_node(&h, task).await.attempt, 1);
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::Pending);

        // Attempt 1: an identical validation failure → repeated-failure breaker.
        let iter1 = running_implement_id(&h).await;
        std::fs::write(h.worktree_path.join("feature.txt"), "code2\n").unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, iter1)
            .await
            .unwrap();
        assert_eq!(
            drive_with(&h, &cfg).await,
            StepOutcome::Advanced,
            "the repeat trips the breaker (advance, then stop)"
        );

        let node = task_node(&h, task).await;
        assert_eq!(node.status, ArtifactStatus::Blocked, "task blocked on repeat");
        assert_eq!(node.attempt, 2);
        assert_eq!(load_issue(&h).await.status, IssueStatus::Blocked);
    }

    /// Settle-time budget breaker: once accumulated `token_used` crosses
    /// `token_budget`, settling the iteration pauses the issue (`pause_reason =
    /// budget`) and files a `budget_exhausted` card. (Complements the dispatch-time
    /// pre-check below — here the overspend lands *during* an in-flight iteration.)
    #[tokio::test]
    async fn breaker_budget_pause_on_exhaustion() {
        let h = setup().await;
        let _task = add_task(&h, "Task 1").await;

        // Under budget at dispatch time so the pre-check admits the implement.
        loop_issue::Entity::update_many()
            .col_expr(loop_issue::Column::TokenUsed, Expr::value(0_i64))
            .col_expr(loop_issue::Column::TokenBudget, Expr::value(500_i64))
            .filter(loop_issue::Column::Id.eq(h.issue_id))
            .exec(&h.db.conn)
            .await
            .unwrap();
        assert_eq!(drive(&h).await, StepOutcome::Dispatched, "dispatch implement");
        let iter_id = running_implement_id(&h).await;

        // The iteration's usage lands over budget; settling re-evaluates the breaker.
        loop_issue::Entity::update_many()
            .col_expr(loop_issue::Column::TokenUsed, Expr::value(1000_i64))
            .filter(loop_issue::Column::Id.eq(h.issue_id))
            .exec(&h.db.conn)
            .await
            .unwrap();
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

    /// Dispatch-time budget pre-check: when the issue is already at/over budget, a
    /// drive must NOT start new task work — it pauses the issue instead. Bounds the
    /// overspend a parallel fan-out could otherwise cause by launching many writes
    /// before any settles.
    #[tokio::test]
    async fn budget_exhausted_skips_dispatch() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        add_task(&h, "A").await;
        add_task(&h, "B").await;

        // Budget already fully consumed.
        loop_issue::Entity::update_many()
            .col_expr(loop_issue::Column::TokenUsed, Expr::value(500_i64))
            .col_expr(loop_issue::Column::TokenBudget, Expr::value(500_i64))
            .filter(loop_issue::Column::Id.eq(h.issue_id))
            .exec(&h.db.conn)
            .await
            .unwrap();

        // Drive: over budget → pause, nothing dispatched.
        assert_eq!(drive(&h).await, StepOutcome::Advanced);
        let issue = load_issue(&h).await;
        assert_eq!(issue.status, IssueStatus::Paused, "issue paused, not driven");
        assert_eq!(issue.pause_reason, Some(loop_issue::PauseReason::Budget));
        let any_impl = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Implement))
            .one(&h.db.conn)
            .await
            .unwrap();
        assert!(any_impl.is_none(), "no implement dispatched when over budget");
    }

    // ---- Task 2.5: finalize → result ----

    async fn drive_finalize(h: &Harness, cfg: &IssueConfig) -> StepOutcome {
        let issue = load_issue(h).await;
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        run_finalize(
            &h.db,
            h.data.path(),
            &StubSpawner,
            &EventEmitter::Noop,
            &issue,
            &dag,
            cfg,
            h.worktree_folder_id,
        )
        .await
        .unwrap()
    }

    async fn running_finalize(h: &Harness) -> loop_iteration::Model {
        loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Finalize))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .one(&h.db.conn)
            .await
            .unwrap()
            .expect("a running finalize iteration")
    }

    /// Drive a fresh task all the way to `done` via a passing review, so the issue
    /// is ready to finalize.
    async fn complete_task(h: &Harness, cfg: &IssueConfig, marker: &str, task: i32) {
        implement_to_in_progress(h, cfg, marker).await;
        assert_eq!(
            drive_with(h, cfg).await,
            StepOutcome::Dispatched,
            "dispatch reviewer"
        );
        let review = running_review(h).await;
        submit_verdict(h, review, "pass", "ok").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review)
            .await
            .unwrap();
        assert_eq!(
            drive_with(h, cfg).await,
            StepOutcome::Advanced,
            "review pass → task done"
        );
        assert_eq!(task_node(h, task).await.status, ArtifactStatus::Done);
    }

    fn git_head(dir: &Path) -> String {
        let out = StdCommand::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .expect("git rev-parse");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn task_done_records_frozen_commit() {
        let h = setup().await;
        let task = add_task(&h, "T").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        complete_task(&h, &cfg, "feature.txt", task).await;

        // Done ⟹ fan_in_commit set, equal to the accepted worktree tip (serial
        // mode → the issue branch tip the checkpoint landed on).
        let model = task_model(&h, task).await;
        assert_eq!(model.status, ArtifactStatus::Done);
        let frozen = model
            .fan_in_commit
            .expect("a Done task carries a frozen integration commit");
        assert_eq!(
            frozen,
            git_head(&h.worktree_path),
            "frozen commit == accepted worktree tip"
        );
    }

    // ---- Parallel result-stage fan-in (Phase 1) ----

    /// The running iteration of `stage` targeting `task`. Scoped by target so a
    /// parallel issue's concurrent sibling tasks (each with its own in-flight
    /// implement/review) don't collide — `running_implement_id`/`running_review`
    /// assume a single in-flight write, which only holds in serial mode.
    async fn running_iter_for(h: &Harness, stage: Stage, task: i32) -> i32 {
        loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(stage))
            .filter(loop_iteration::Column::TargetArtifactId.eq(task))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .one(&h.db.conn)
            .await
            .unwrap()
            .expect("a running iteration for the task")
            .id
    }

    /// Drive one parallel task through its full implement→review→pass lifecycle in
    /// its OWN worktree, leaving it Done with a frozen commit. Robust to Phase-2
    /// concurrency: a prior tick's fan-out may already have dispatched this task's
    /// implement (and a sibling's), so this scopes every lookup to `task` and
    /// asserts the task's own state rather than the whole-issue drive outcome.
    async fn complete_parallel_task(
        h: &Harness,
        cfg: &IssueConfig,
        marker: &str,
        body: &str,
        task: i32,
    ) {
        // Ensure the task's implement is running (dispatched now, or already in
        // flight from an earlier fan-out tick — driving is idempotent).
        drive_with(h, cfg).await;
        let impl_id = running_iter_for(h, Stage::Implement, task).await;
        let task_wt = worktree::ensure_task_worktree(&h.db.conn, h.data.path(), h.issue_id, task)
            .await
            .unwrap();
        std::fs::write(task_wt.worktree_path.join(marker), body).unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, impl_id)
            .await
            .unwrap();
        // Checkpoint + validate → in_progress.
        drive_with(h, cfg).await;
        assert_eq!(task_node(h, task).await.status, ArtifactStatus::InProgress);
        // Dispatch this task's review.
        drive_with(h, cfg).await;
        let review = running_iter_for(h, Stage::Review, task).await;
        submit_verdict(h, review, "pass", "ok").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review)
            .await
            .unwrap();
        // Review pass → done + freeze.
        drive_with(h, cfg).await;
        assert_eq!(task_node(h, task).await.status, ArtifactStatus::Done);
    }

    async fn integrate_path(h: &Harness) -> std::path::PathBuf {
        let seq = load_issue(h).await.seq_no;
        h.data
            .path()
            .join("loop-worktrees")
            .join(h.space_id.to_string())
            .join(format!("issue-{seq}-integrate"))
    }

    async fn set_fan_in_manifest(h: &Harness, json: &str) {
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::FanInManifest,
                Expr::value(json.to_string()),
            )
            .filter(loop_issue::Column::Id.eq(h.issue_id))
            .exec(&h.db.conn)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn parallel_two_independent_tasks_clean_fan_in_to_result() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        let t1 = add_task(&h, "T1").await;
        let t2 = add_task(&h, "T2").await;

        complete_parallel_task(&h, &cfg, "a.txt", "A\n", t1).await;
        complete_parallel_task(&h, &cfg, "b.txt", "B\n", t2).await;
        assert!(task_model(&h, t1).await.fan_in_commit.is_some());
        assert!(task_model(&h, t2).await.fan_in_commit.is_some());

        // One drive lands the whole fan-in: integrate both task branches, CAS onto
        // the issue branch, synthesize the result.
        assert_eq!(
            drive_finalize(&h, &cfg).await,
            StepOutcome::Advanced,
            "fan-in lands + produces result"
        );

        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        assert!(dag.artifacts.iter().any(|a| a.kind == ArtifactKind::Result));
        assert!(
            load_issue(&h).await.fan_in_manifest.is_none(),
            "session lock cleared after landing"
        );
        assert!(
            h.worktree_path.join("a.txt").exists() && h.worktree_path.join("b.txt").exists(),
            "both tasks landed on the issue branch (worktree synced to the new tip)"
        );
    }

    #[tokio::test]
    async fn parallel_conflict_dispatches_resolution_then_lands() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        let t1 = add_task(&h, "T1").await;
        let t2 = add_task(&h, "T2").await;
        // Both tasks add the SAME file with different content → fan-in conflict.
        complete_parallel_task(&h, &cfg, "shared.txt", "A\n", t1).await;
        complete_parallel_task(&h, &cfg, "shared.txt", "B\n", t2).await;

        // Fan-in conflicts on the second task → dispatches a result-stage resolver.
        assert_eq!(
            drive_finalize(&h, &cfg).await,
            StepOutcome::Dispatched,
            "conflict dispatches a resolver"
        );
        let resolver = running_finalize(&h).await;
        let integ = integrate_path(&h).await;
        assert!(
            worktree::integrate_in_progress(&integ).await,
            "the in-progress merge is left for the resolver"
        );

        // Simulate the resolver: resolve the conflict + complete the merge.
        std::fs::write(integ.join("shared.txt"), "A+B\n").unwrap();
        git(&integ, &["add", "-A"]);
        git(&integ, &["commit", "--no-edit"]);
        settle_iteration(&h.db, &EventEmitter::Noop, resolver.id)
            .await
            .unwrap();

        // Re-drive: resume (resolved task now an ancestor) → land + result.
        assert_eq!(
            drive_finalize(&h, &cfg).await,
            StepOutcome::Advanced,
            "resumes the fan-in and lands"
        );
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        assert!(dag.artifacts.iter().any(|a| a.kind == ArtifactKind::Result));
        assert!(load_issue(&h).await.fan_in_manifest.is_none());
        assert!(
            h.worktree_path.join("shared.txt").exists(),
            "the resolved merge landed on the issue branch"
        );
    }

    #[tokio::test]
    async fn fan_in_cas_fail_restarts() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        let t1 = add_task(&h, "T1").await;
        complete_parallel_task(&h, &cfg, "a.txt", "A\n", t1).await;

        // Advance the issue branch, then inject a manifest whose issue_base_oid is
        // the STALE (pre-advance) tip → the CAS landing must miss and restart,
        // never land.
        let stale_base = git_head(&h.worktree_path);
        std::fs::write(h.worktree_path.join("drift.txt"), "drift\n").unwrap();
        git(&h.worktree_path, &["add", "-A"]);
        git(&h.worktree_path, &["commit", "-m", "issue branch drift"]);
        let frozen = task_model(&h, t1).await.fan_in_commit.unwrap();
        let manifest = format!(
            r#"{{"v":1,"issue_base_oid":"{stale_base}","ordered":[{{"task_id":{t1},"sha":"{frozen}"}}]}}"#
        );
        set_fan_in_manifest(&h, &manifest).await;

        assert_eq!(
            drive_finalize(&h, &cfg).await,
            StepOutcome::Advanced,
            "stale-base CAS miss → restart"
        );
        assert!(
            load_issue(&h).await.fan_in_manifest.is_none(),
            "stale session cleared for a fresh retry"
        );
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        assert!(
            !dag.artifacts.iter().any(|a| a.kind == ArtifactKind::Result),
            "nothing landed → no result row stranded"
        );
    }

    #[tokio::test]
    async fn parallel_resolver_left_unresolved_blocks() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        let t1 = add_task(&h, "T1").await;
        let t2 = add_task(&h, "T2").await;
        complete_parallel_task(&h, &cfg, "shared.txt", "A\n", t1).await;
        complete_parallel_task(&h, &cfg, "shared.txt", "B\n", t2).await;

        // Conflict → resolver dispatched (records fan_in_resolver_tip).
        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Dispatched);
        let resolver = running_finalize(&h).await;
        let integ = integrate_path(&h).await;
        assert!(worktree::integrate_in_progress(&integ).await);

        // The resolver ends WITHOUT completing the merge — MERGE_HEAD still set at
        // the recorded tip.
        settle_iteration(&h.db, &EventEmitter::Noop, resolver.id)
            .await
            .unwrap();

        // Re-drive: an unresolved MERGE_HEAD at the recorded resolver tip blocks the
        // issue — NOT a re-dispatch loop, NOT a phantom finish. (Distinguishes this
        // from a crash-before-dispatch, which would re-dispatch instead.)
        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Advanced);
        assert_eq!(load_issue(&h).await.status, IssueStatus::Blocked);
        assert!(
            worktree::integrate_in_progress(&integ).await,
            "the in-progress merge is preserved for human diagnosis"
        );
    }

    #[tokio::test]
    async fn parallel_already_landed_recovers_without_revalidation() {
        let h = setup().await;
        set_execution_mode(&h, "parallel").await;
        // A validation command that FAILS — proves the recovery path does NOT
        // re-validate (else it would block instead of finishing).
        let mut cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        let t1 = add_task(&h, "T1").await;
        complete_parallel_task(&h, &cfg, "a.txt", "A\n", t1).await;

        let base_old = git_head(&h.worktree_path);
        let f1 = task_model(&h, t1).await.fan_in_commit.unwrap();
        let seq = load_issue(&h).await.seq_no;
        let issue_branch = format!("loop/{}/issue-{}", h.space_id, seq);

        // Manually land the frozen commit onto the issue branch — simulating a
        // fan-in that landed but crashed before synthesizing the result + clearing
        // the session lock.
        let integ = worktree::ensure_integrate_worktree(&h.db.conn, h.data.path(), h.issue_id, &base_old)
            .await
            .unwrap();
        let landed = match worktree::fan_in_tasks(&integ.worktree_path, &[(t1, f1.clone())], &[], None)
            .await
            .unwrap()
        {
            worktree::FanInOutcome::Integrated { tip } => tip,
            o => panic!("expected Integrated, got {o:?}"),
        };
        assert!(
            worktree::cas_advance_branch(h._repo.path(), &issue_branch, &landed, &base_old)
                .await
                .unwrap(),
            "manual land applied"
        );

        // Arm the session lock as if mid-flight, and make any re-validation FAIL.
        cfg.validation_commands = vec!["git rev-parse --verify refs/heads/no-such-ref".to_string()];
        let manifest = format!(
            r#"{{"v":1,"issue_base_oid":"{base_old}","ordered":[{{"task_id":{t1},"sha":"{f1}"}}]}}"#
        );
        set_fan_in_manifest(&h, &manifest).await;

        // Drive: already-landed detection finishes idempotently WITHOUT re-running
        // the (now-failing) validation → result synthesized, issue not blocked.
        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Advanced);
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        assert!(
            dag.artifacts.iter().any(|a| a.kind == ArtifactKind::Result),
            "result synthesized on already-landed recovery"
        );
        assert_eq!(
            load_issue(&h).await.status,
            IssueStatus::Running,
            "not blocked by stale re-validation"
        );
        assert!(
            load_issue(&h).await.fan_in_manifest.is_none(),
            "session lock cleared"
        );
        assert!(
            h.worktree_path.join("a.txt").exists(),
            "issue worktree synced to the landed tip"
        );
    }

    /// All tasks done → finalize dispatches; the agent submits a result; the DAG
    /// gains a `result` artifact with a `results_from` edge to each task.
    #[tokio::test]
    async fn finalize_produces_result_and_results_from_edges() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        complete_task(&h, &cfg, "feature.txt", task).await;

        // Finalize dispatches (issue-level, target = None).
        assert_eq!(
            drive_finalize(&h, &cfg).await,
            StepOutcome::Dispatched,
            "finalize dispatched"
        );
        let fin = running_finalize(&h).await;

        // Simulate the finalize agent submitting the result summary via ingest.
        crate::loop_engine::ingest::ingest(
            &h.db.conn,
            &fin.capability_token,
            "loop_submit_artifacts",
            &serde_json::json!({ "artifacts": [{ "title": "Result", "content": "shipped" }] }),
        )
        .await
        .unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, fin.id)
            .await
            .unwrap();

        // Next tick: result exists → the INTEGRATION gate dispatches a reviewer on
        // the result (the whole-issue closure must be verified before merge).
        assert_eq!(
            drive_finalize(&h, &cfg).await,
            StepOutcome::Dispatched,
            "result exists → integration reviewer dispatched"
        );

        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        let results: Vec<_> = dag
            .artifacts
            .iter()
            .filter(|a| a.kind == ArtifactKind::Result)
            .collect();
        assert_eq!(results.len(), 1, "one result artifact");
        let result_id = results[0].id;
        let edges = dag
            .links
            .iter()
            .filter(|l| {
                l.kind == LinkKind::ResultsFrom
                    && l.from_artifact_id == result_id
                    && l.to_artifact_id == task
            })
            .count();
        assert_eq!(edges, 1, "results_from edge from result to the task");
    }

    /// Drive a single-task issue to a produced `result` sitting at the integration
    /// gate (the finalize agent has submitted it). Returns the result artifact id.
    async fn finalize_to_result(h: &Harness, cfg: &IssueConfig, task: i32) -> i32 {
        complete_task(h, cfg, "feature.txt", task).await;
        assert_eq!(
            drive_finalize(h, cfg).await,
            StepOutcome::Dispatched,
            "finalize dispatched"
        );
        let fin = running_finalize(h).await;
        crate::loop_engine::ingest::ingest(
            &h.db.conn,
            &fin.capability_token,
            "loop_submit_artifacts",
            &serde_json::json!({ "artifacts": [{ "title": "Result", "content": "shipped" }] }),
        )
        .await
        .unwrap();
        settle_iteration(&h.db, &EventEmitter::Noop, fin.id).await.unwrap();
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        live_result(&dag).expect("result produced").id
    }

    /// Integration gate PASS opens the merge gate: the result exists, an integration
    /// reviewer is dispatched on it, and once it passes the whole-issue closure the
    /// finalize decision is recorded and `integration_passed` holds (merge allowed).
    #[tokio::test]
    async fn integration_pass_opens_merge_gate() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        let result_id = finalize_to_result(&h, &cfg, task).await;

        // Result exists → integration reviewer dispatched (target = result), and the
        // gate is not yet passed.
        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Dispatched);
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        assert!(!integration_passed(&h.db.conn, &dag).await.unwrap());
        let review = running_review(&h).await; // the only running Review = integration
        submit_verdict(&h, review, "pass", "closure holds").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review).await.unwrap();

        // Next tick: integration Pass → merge gate open (no further dispatch).
        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Idle);
        assert_eq!(
            loop_service::gate_decision::outcome_for(&h.db.conn, result_id, "finalize", 0)
                .await
                .unwrap(),
            Some(GateOutcome::Pass)
        );
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        assert!(integration_passed(&h.db.conn, &dag).await.unwrap(), "merge gate open");
    }

    /// Integration gate FAIL loops back: the live result and tasks are superseded
    /// (freeing `uniq_result_per_issue`), the fail decision is recorded, and the
    /// next plan can produce a fresh result. The integration reviewer failing a
    /// closure criterion is exactly the cross-task-violation path.
    #[tokio::test]
    async fn integration_fail_supersedes_result_and_tasks() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = IssueConfig { max_attempts: 3, ..config_reviewers(1, ReviewPassRule::Unanimous) };
        let result_id = finalize_to_result(&h, &cfg, task).await;

        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Dispatched);
        let review = running_review(&h).await;
        submit_verdict(&h, review, "fail", "requirement unmet by the combined result").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review).await.unwrap();

        // Integration Fail → bounded loop-back (advance + re-plan), result+task superseded.
        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Advanced);
        assert_eq!(
            loop_service::gate_decision::outcome_for(&h.db.conn, result_id, "finalize", 0)
                .await
                .unwrap(),
            Some(GateOutcome::Fail)
        );
        let dag = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        assert_eq!(
            dag.artifacts.iter().find(|a| a.id == result_id).unwrap().status,
            ArtifactStatus::Superseded,
            "live result superseded"
        );
        assert_eq!(task_node(&h, task).await.status, ArtifactStatus::Superseded, "task superseded");
        assert!(live_result(&dag).is_none(), "uniq_result_per_issue freed for a fresh result");
        assert_eq!(load_issue(&h).await.status, IssueStatus::Running, "still running (bounded retry)");
    }

    /// Integration loop-back is bounded: with `max_attempts = 1` the first integration
    /// failure exhausts the bound → issue blocked + `integration_gap` card.
    #[tokio::test]
    async fn integration_gap_exhausts_to_block() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = IssueConfig { max_attempts: 1, ..config_reviewers(1, ReviewPassRule::Unanimous) };
        let _ = finalize_to_result(&h, &cfg, task).await;

        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Dispatched);
        let review = running_review(&h).await;
        submit_verdict(&h, review, "fail", "unmet").await;
        settle_iteration(&h.db, &EventEmitter::Noop, review).await.unwrap();

        // count_fail(finalize) reaches max_attempts(1) → block, not loop-back.
        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Advanced);
        assert_eq!(load_issue(&h).await.status, IssueStatus::Blocked);
        let inbox = loop_service::inbox::list_inbox(&h.db.conn, h.space_id, None).await.unwrap();
        assert!(
            inbox.iter().any(|i| i.kind == InboxKind::Blocked
                && i.subject_key == format!("integration_gap:{}", h.issue_id)),
            "integration_gap card filed"
        );
    }

    /// Empty integration closure (D11): a result with NOTHING to verify — no
    /// requirements, and the (direct-route) tasks carry no acceptance criteria —
    /// blocks the issue with an `unverifiable` card rather than a vacuous pass.
    #[tokio::test]
    async fn integration_empty_closure_blocks_unverifiable() {
        let h = setup().await;
        let dag0 = artifact::list_dag(&h.db.conn, h.issue_id).await.unwrap();
        let root = dag0.artifacts.iter().find(|a| a.kind == ArtifactKind::Issue).unwrap().id;
        // A Done task with NO acceptance criteria + a Done result. No requirements.
        let task = artifact::create_artifact(&h.db.conn, h.space_id, h.issue_id, ArtifactKind::Task, "T", ArtifactStatus::Done, ActorKind::Agent, None).await.unwrap();
        link::create_link(&h.db.conn, h.space_id, task.id, root, LinkKind::DerivesFrom, None).await.unwrap();
        artifact::create_artifact(&h.db.conn, h.space_id, h.issue_id, ArtifactKind::Result, "R", ArtifactStatus::Done, ActorKind::Agent, None).await.unwrap();

        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        // Result exists, all live tasks Done, quiescent → integration gate → empty
        // closure → block (advance + stop), never a vacuous pass.
        assert_eq!(drive_finalize(&h, &cfg).await, StepOutcome::Advanced);
        assert_eq!(load_issue(&h).await.status, IssueStatus::Blocked);
        let inbox = loop_service::inbox::list_inbox(&h.db.conn, h.space_id, None).await.unwrap();
        assert!(
            inbox.iter().any(|i| i.kind == InboxKind::Blocked
                && i.subject_key == format!("unverifiable:{}", h.issue_id)),
            "unverifiable card filed"
        );
    }

    /// Finalize must wait for the issue to be **fully quiescent** — every task
    /// `Done` is not enough if any iteration is still in flight (e.g. a losing
    /// review slot mid-settle). The old per-issue write gate proxied this; the
    /// replacement is an explicit "no in-flight iteration of any stage" check.
    #[tokio::test]
    async fn finalize_waits_for_all_inflight_including_losing_review_slots() {
        use crate::loop_engine::transitions::{try_claim_iteration, IterationClaim};
        let h = setup().await;
        let task = add_task(&h, "T").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        complete_task(&h, &cfg, "feature.txt", task).await;

        // Inject a still-running review iteration (a losing slot that has not yet
        // settled): every task is Done, but the issue is NOT quiescent.
        let lingering = try_claim_iteration(
            &h.db.conn,
            IterationClaim {
                space_id: h.space_id,
                issue_id: h.issue_id,
                stage: Stage::Review,
                target_artifact_id: Some(task),
                slot_no: Some(7),
                capability_token: "lingering".into(),
                attempt: 99,
            },
        )
        .await
        .unwrap()
        .expect("claim a spare review slot");
        cas_iteration_status(
            &h.db.conn,
            lingering.id,
            IterationStatus::Queued,
            IterationStatus::Running,
        )
        .await
        .unwrap();

        // Finalize waits while the review slot is in flight.
        assert_eq!(
            drive_finalize(&h, &cfg).await,
            StepOutcome::Idle,
            "finalize waits for the in-flight review slot"
        );
        let fins = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Finalize))
            .all(&h.db.conn)
            .await
            .unwrap();
        assert!(fins.is_empty(), "no finalize dispatched while not quiescent");

        // Once the slot settles, finalize proceeds.
        cas_iteration_status(
            &h.db.conn,
            lingering.id,
            IterationStatus::Running,
            IterationStatus::Cancelled,
        )
        .await
        .unwrap();
        assert_eq!(
            drive_finalize(&h, &cfg).await,
            StepOutcome::Dispatched,
            "finalize proceeds once the issue is quiescent"
        );
    }

    /// A dirty worktree at finalize time (stray uncommitted state) blocks the
    /// issue + files a card, and dispatches no finalize iteration.
    #[tokio::test]
    async fn finalize_dirty_tree_blocks() {
        let h = setup().await;
        let task = add_task(&h, "Task 1").await;
        let cfg = config_reviewers(1, ReviewPassRule::Unanimous);
        complete_task(&h, &cfg, "feature.txt", task).await;

        // Stray uncommitted file in the worktree.
        std::fs::write(h.worktree_path.join("stray.txt"), "uncommitted\n").unwrap();

        assert_eq!(
            drive_finalize(&h, &cfg).await,
            StepOutcome::Advanced,
            "dirty tree blocks (advance), not a dispatch"
        );
        assert_eq!(
            load_issue(&h).await.status,
            IssueStatus::Blocked,
            "issue blocked on a dirty tree"
        );
        let inbox = loop_service::inbox::list_inbox(&h.db.conn, h.space_id, None)
            .await
            .unwrap();
        assert!(
            inbox.iter().any(|i| i.kind == InboxKind::Blocked
                && i.subject_key == format!("finalize_dirty:{}", h.issue_id)),
            "finalize_dirty inbox card filed"
        );
        let fins = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Finalize))
            .all(&h.db.conn)
            .await
            .unwrap();
        assert!(fins.is_empty(), "no finalize iteration on a dirty tree");
    }

    #[tokio::test]
    async fn set_task_status_cas_rejects_wrong_from() {
        let h = setup().await;
        let task = add_task(&h, "T").await; // pending

        // Wrong `from` (task is Pending, not InProgress) → no-op, returns false.
        let applied =
            set_task_status_cas(&h.db, task, ArtifactStatus::InProgress, ArtifactStatus::Done)
                .await
                .unwrap();
        assert!(!applied, "CAS with the wrong from does not apply");
        let row = loop_artifact::Entity::find_by_id(task)
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, ArtifactStatus::Pending, "status unchanged");

        // Correct `from` applies.
        let applied =
            set_task_status_cas(&h.db, task, ArtifactStatus::Pending, ArtifactStatus::InProgress)
                .await
                .unwrap();
        assert!(applied, "CAS with the right from applies");
    }

    /// Records disconnects and maps conversation ids to connection ids, so the
    /// reviewer-kill path is observable without a live connection manager.
    struct RecordingSpawner {
        conn_for: std::collections::HashMap<i32, String>,
        disconnected: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl LoopAgentSpawner for RecordingSpawner {
        async fn spawn_loop_agent(
            &self,
            _db: &AppDatabase,
            _data_dir: &Path,
            _agent_type: AgentType,
            _working_dir: String,
            _emitter: EventEmitter,
            _preferred_mode_id: Option<String>,
            _preferred_config_values: std::collections::BTreeMap<String, String>,
            _capability_token: String,
        ) -> Result<String, AcpError> {
            Ok("conn".to_string())
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
        async fn disconnect_loop_agent(&self, conn_id: &str) {
            self.disconnected.lock().unwrap().push(conn_id.to_string());
        }
        async fn find_loop_connection(&self, conversation_id: i32) -> Option<String> {
            self.conn_for.get(&conversation_id).cloned()
        }
    }

    #[tokio::test]
    async fn cancel_active_reviews_kills_live_reviewer_agent() {
        use crate::loop_engine::transitions::{try_claim_iteration, IterationClaim};
        let h = setup().await;
        let task = add_task(&h, "T").await;

        // A running reviewer iteration backed by conversation 7777.
        let claimed = try_claim_iteration(
            &h.db.conn,
            IterationClaim {
                space_id: h.space_id,
                issue_id: h.issue_id,
                stage: Stage::Review,
                target_artifact_id: Some(task),
                slot_no: Some(0),
                capability_token: "tok-review".into(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .expect("claimed review iteration");
        loop_iteration::Entity::update_many()
            .col_expr(
                loop_iteration::Column::Status,
                Expr::value(IterationStatus::Running.to_value()),
            )
            .col_expr(loop_iteration::Column::ConversationId, Expr::value(7777))
            .filter(loop_iteration::Column::Id.eq(claimed.id))
            .exec(&h.db.conn)
            .await
            .unwrap();
        let running = loop_iteration::Entity::find_by_id(claimed.id)
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap();

        let spawner = RecordingSpawner {
            conn_for: std::collections::HashMap::from([(7777, "conn-7777".to_string())]),
            disconnected: std::sync::Mutex::new(Vec::new()),
        };
        cancel_active_reviews(&h.db, &spawner, &[running])
            .await
            .unwrap();

        // The reviewer iteration is voided AND its live agent process reaped.
        let row = loop_iteration::Entity::find_by_id(claimed.id)
            .one(&h.db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, IterationStatus::Cancelled);
        assert_eq!(
            *spawner.disconnected.lock().unwrap(),
            vec!["conn-7777".to_string()],
            "the cancelled reviewer's live agent is disconnected"
        );
    }
}
