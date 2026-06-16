//! Per-issue driver: the autonomous tick loop (§4.2) plus the pure frontier
//! computation that decides what to dispatch next.
//!
//! A driver is one tokio task per `running` issue. It is event-driven, not a
//! poller: each tick computes the ready frontier, dispatches it (idempotently,
//! guarded by the §4.1a DB leases), then parks on a per-issue `Notify` that the
//! completion watcher fires when an iteration settles. The DB is the
//! concurrency authority; this loop is just the scheduler that turns DAG state
//! into dispatch calls.
//!
//! The driver runs the full pipeline: the read stages (triage → refine → design
//! → plan) compute their frontier here in [`ready_nodes`]; once the plan stage has
//! produced `pending` tasks the read frontier empties and the write pipeline
//! (implement → verify → review → finalize, in [`crate::loop_engine::gates`])
//! takes over for each task. Both are dispatched from [`tick_once`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use tokio::sync::Notify;
use tokio::time::{interval, Duration, MissedTickBehavior};
use tracing::Instrument;

use crate::acp::manager::{ConnectionManager, TurnLiveness};
use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus};
use crate::db::entities::loop_inbox_item::InboxKind;
use crate::db::entities::loop_issue::{self, IssueRoute, IssueStatus};
use crate::db::entities::loop_iteration::{self, IterationStatus, Stage};
use crate::db::entities::loop_link::LinkKind;
use crate::db::service::loop_service::{artifact, coverage, inbox, link};
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::loops::{AgentSpec, IssueConfig, LoopArtifactRow, LoopDagView};
use crate::web::event_bridge::EventEmitter;

use crate::loop_engine::config_resolver::effective_config;
use crate::loop_engine::dispatch::{
    dispatch_iteration, emit_changed, settle_iteration, settle_iteration_as, DispatchInput,
    LoopAgentSpawner, SettleResolution,
};
use crate::loop_engine::error::LoopError;
use crate::loop_engine::gates;
use crate::loop_engine::transitions::cas_issue_status;
use crate::loop_engine::LoopEngine;

/// Liveness oracle for the reconcile backstop — implemented by `ConnectionManager`
/// in prod, stubbed in tests. Mirrors the `LoopAgentSpawner` seam so reconcile's
/// three-state handling is unit-testable without live ACP connections.
#[async_trait]
pub(crate) trait IterationLiveness {
    async fn turn_state(&self, conversation_id: i32) -> TurnLiveness;
}

#[async_trait]
impl IterationLiveness for ConnectionManager {
    async fn turn_state(&self, conversation_id: i32) -> TurnLiveness {
        self.connection_turn_state(conversation_id).await
    }
}

/// Result of a single tick.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TickOutcome {
    /// The issue is no longer `running`; the driver should exit.
    Stop,
    /// At least one iteration was dispatched this tick.
    Dispatched,
    /// Durable state moved forward but nothing is in flight — the driver should
    /// re-tick immediately (to dispatch the follow-on step, or observe the issue
    /// leaving `running` and stop) rather than park on the no-timeout wake.
    Advanced,
    /// Nothing to dispatch right now (frontier empty / all in-flight / lease
    /// held). The driver parks until the next completion or external wake.
    Idle,
    /// The result is produced and `auto_merge` is on — the driver should land it
    /// via the engine merge gate (which needs `&LoopEngine`, so `tick_once` only
    /// signals; `run_driver` performs the merge).
    AutoMerge,
}

/// One unit of work the frontier wants dispatched.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FrontierItem {
    pub stage: Stage,
    pub target_artifact_id: Option<i32>,
    pub attempt: i32,
}

/// The issue's root artifact (`kind = issue`), seeded at issue creation.
fn root_artifact_id(dag: &LoopDagView) -> Option<i32> {
    dag.artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Issue)
        .map(|a| a.id)
}

/// Live artifacts of a kind — excludes `superseded` / `cancelled` nodes (e.g. a
/// rejected design) so the frontier ignores dead branches and can re-dispatch
/// the stage fresh.
fn artifacts_of_kind(dag: &LoopDagView, kind: ArtifactKind) -> Vec<&LoopArtifactRow> {
    dag.artifacts
        .iter()
        .filter(|a| {
            a.kind == kind
                && !matches!(
                    a.status,
                    ArtifactStatus::Superseded | ArtifactStatus::Cancelled
                )
        })
        .collect()
}

fn all_done(rows: &[&LoopArtifactRow]) -> bool {
    rows.iter().all(|a| a.status == ArtifactStatus::Done)
}

fn node_attempt(dag: &LoopDagView, id: i32) -> i32 {
    dag.artifacts
        .iter()
        .find(|a| a.id == id)
        .map(|a| a.attempt)
        .unwrap_or(0)
}

/// Compute the next dispatch(es) for the read pipeline, per route. Pure over the
/// DAG snapshot — no I/O — so it is unit-tested directly.
///
/// The pipeline advances one stage at a time: a stage is dispatched only when
/// its output kind is absent, and the driver waits (empty frontier) while a
/// stage's outputs exist but aren't all `done`. Routes shorten the pipeline:
/// `full` = refine→design→plan, `skip_design` = refine→plan, `direct` = plan.
/// Once tasks exist the read frontier is empty — the write pipeline takes over.
pub(crate) fn ready_nodes(dag: &LoopDagView, route: IssueRoute) -> Vec<FrontierItem> {
    let Some(root) = root_artifact_id(dag) else {
        return Vec::new();
    };

    let needs_refine = matches!(route, IssueRoute::Full | IssueRoute::SkipDesign);
    let needs_design = matches!(route, IssueRoute::Full);

    let reqs = artifacts_of_kind(dag, ArtifactKind::Requirement);
    let designs = artifacts_of_kind(dag, ArtifactKind::Design);
    let tasks = artifacts_of_kind(dag, ArtifactKind::Task);

    let one = |stage: Stage, target: i32| {
        vec![FrontierItem {
            stage,
            target_artifact_id: Some(target),
            attempt: node_attempt(dag, target),
        }]
    };

    // 1. Refine → requirements (derive from the issue root).
    if needs_refine {
        if reqs.is_empty() {
            return one(Stage::Refine, root);
        }
        if !all_done(&reqs) {
            return Vec::new(); // refinement in flight
        }
    }

    // 2. Design → design. Anchored at the issue root (a stable target); the
    //    design's real lineage — `derives_from` edges to EVERY requirement, each
    //    bound to its revision — is wired by ingest's design fan-in, not by this
    //    single dispatch target.
    if needs_design {
        if designs.is_empty() {
            return one(Stage::Design, root);
        }
        if !all_done(&designs) {
            return Vec::new();
        }
    }

    // 3. Plan → tasks. Target is the nearest upstream node the route reached.
    if tasks.is_empty() {
        let target = match route {
            IssueRoute::Full => designs.last().map(|d| d.id),
            IssueRoute::SkipDesign => reqs.last().map(|r| r.id),
            IssueRoute::Direct | IssueRoute::Undecided => None,
        }
        .unwrap_or(root);
        return one(Stage::Plan, target);
    }

    // 4. Tasks exist → read frontier done; the write pipeline (gates) drives them.
    Vec::new()
}

/// True if the task DAG ever has ≥2 tasks simultaneously ready — i.e. real
/// concurrency. Frontier simulation; correct for FAN-OUT (A→{B,C}, which has
/// edges==n-1 yet is parallel — the naive edge-count heuristic was WRONG here)
/// and multiple roots. (v1 deps are a forest, but this stays general.) Cycles
/// are rejected at submit, so the simulation always terminates.
fn dag_has_parallelism(dag: &LoopDagView) -> bool {
    let tasks: Vec<i32> = dag
        .artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::Task)
        .map(|a| a.id)
        .collect();
    if tasks.len() < 2 {
        return false;
    }
    let preds = |t: i32| -> Vec<i32> {
        dag.links
            .iter()
            .filter(|l| l.kind == LinkKind::DependsOn && l.from_artifact_id == t)
            .map(|l| l.to_artifact_id)
            .collect()
    };
    let mut done: std::collections::HashSet<i32> = Default::default();
    loop {
        let ready: Vec<i32> = tasks
            .iter()
            .copied()
            .filter(|t| !done.contains(t) && preds(*t).iter().all(|p| done.contains(p)))
            .collect();
        if ready.len() >= 2 {
            return true; // ≥2 concurrently ready ⇒ parallel
        }
        if ready.is_empty() {
            return false; // all settled (or only dead-pred tasks left) ⇒ serial
        }
        done.insert(ready[0]);
    }
}

/// Decide and persist the issue's `execution_mode` exactly once — the first tick
/// at which its task DAG exists. Returns the resolved mode (`Some` once decided,
/// `None` while there are still no tasks).
///
/// Timing safety: a plan submission is atomic (all tasks + their `depends_on`
/// edges land in a single `submit_artifacts` call), so the moment any task is
/// present the whole task set is too — there is no half-built-DAG window. A
/// write-once conditional UPDATE (`WHERE execution_mode IS NULL`) then guarantees
/// a re-read or a racing tick can never relatch a different mode.
async fn ensure_execution_mode(
    conn: &sea_orm::DatabaseConnection,
    issue: &loop_issue::Model,
    dag: &LoopDagView,
) -> Result<Option<String>, LoopError> {
    // Already decided → keep it (in-memory guard before any write).
    if let Some(mode) = &issue.execution_mode {
        return Ok(Some(mode.clone()));
    }
    // No tasks yet (read stages still in flight) → nothing to decide.
    if !dag.artifacts.iter().any(|a| a.kind == ArtifactKind::Task) {
        return Ok(None);
    }
    let mode = if dag_has_parallelism(dag) {
        "parallel"
    } else {
        "serial"
    };
    // Write-once CAS: only the first writer sets it; a racing tick no-ops.
    loop_issue::Entity::update_many()
        .col_expr(loop_issue::Column::ExecutionMode, Expr::value(mode))
        .filter(loop_issue::Column::Id.eq(issue.id))
        .filter(loop_issue::Column::ExecutionMode.is_null())
        .exec(conn)
        .await?;
    // Read back the authoritative value (in case another writer won the CAS).
    let resolved = loop_issue::Entity::find_by_id(issue.id)
        .one(conn)
        .await?
        .and_then(|i| i.execution_mode);
    Ok(resolved)
}

/// Resolve the full agent spec (agent + startup mode + config) for a stage from
/// the issue's Loop Contract: the per-stage override if set, else `agents.default`.
pub(crate) fn resolve_agent_spec(config: &IssueConfig, stage: Stage) -> AgentSpec {
    config.agents.for_stage(stage).clone()
}

/// Just the agent type for a stage (e.g. to route a question to the right
/// agent). For dispatch, prefer [`resolve_agent_spec`] so the per-stage mode/
/// config overrides are carried through.
pub(crate) fn resolve_agent(config: &IssueConfig, stage: Stage) -> AgentType {
    resolve_agent_spec(config, stage).agent
}

/// Does this issue already have a triage iteration on record (in ANY state)?
/// Triage targets the whole issue (`target = None`), so the §4.1a node lease
/// can't dedup it (SQLite treats NULL targets as distinct) — this app-level gate
/// is what stops `tick_once` from launching a *second* initial triage.
///
/// It counts every status, not just the live/succeeded ones: once any triage
/// exists, all further (bounded) redispatch is owned by `recover_undecided_triage`
/// — never `tick_once`'s own branch. If this gate excluded `failed`/`interrupted`,
/// an abandoned triage (now settled `Failed` by the reconcile) would re-trigger
/// `tick_once`'s unbounded attempt-0 dispatch here instead of going through the
/// bounded recovery, looping forever. So the rule is simply "any triage row =
/// the slot is taken; defer to recovery".
async fn has_any_triage(
    conn: &sea_orm::DatabaseConnection,
    issue_id: i32,
) -> Result<bool, LoopError> {
    let triage = loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .filter(loop_iteration::Column::Stage.eq(Stage::Triage))
        .all(conn)
        .await?;
    Ok(!triage.is_empty())
}

/// Record `skips_to` provenance for routes that skip stages: every task gets a
/// `skips_to` edge to the issue root marking that it bypassed the normal
/// refine/design steps. Idempotent (skips a task that already has one).
async fn ensure_skip_provenance(
    db: &AppDatabase,
    space_id: i32,
    dag: &LoopDagView,
    route: IssueRoute,
) -> Result<(), LoopError> {
    if matches!(route, IssueRoute::Full | IssueRoute::Undecided) {
        return Ok(());
    }
    let Some(root) = root_artifact_id(dag) else {
        return Ok(());
    };
    for task in artifacts_of_kind(dag, ArtifactKind::Task) {
        let has_skip = dag
            .links
            .iter()
            .any(|l| l.from_artifact_id == task.id && l.kind == LinkKind::SkipsTo);
        if !has_skip {
            link::create_link(&db.conn, space_id, task.id, root, LinkKind::SkipsTo, None).await?;
        }
    }
    Ok(())
}

/// Keep the design-approval inbox card filed while any design sits
/// `awaiting_approval`. Idempotent (the upsert dedups), so it is safe to call
/// every tick; the card is resolved by `approve_design` / `reject_design`.
async fn ensure_design_gate_card(
    db: &AppDatabase,
    issue: &loop_issue::Model,
    dag: &LoopDagView,
) -> Result<(), LoopError> {
    let awaiting = dag.artifacts.iter().any(|a| {
        a.kind == ArtifactKind::Design && a.status == ArtifactStatus::AwaitingApproval
    });
    if awaiting {
        inbox::upsert_inbox(
            &db.conn,
            issue.space_id,
            issue.id,
            None,
            InboxKind::Approval,
            &format!("design:{}", issue.id),
            serde_json::json!({ "v": 1, "gate": "design" }),
        )
        .await?;
    }
    Ok(())
}

/// Liveness backstop (DB-authoritative): settle any of this issue's `running`
/// iterations whose turn is no longer actually in flight. The completion watcher
/// settles on `TurnComplete`, but that single in-process event can be dropped
/// (broadcast lag) or race the connection teardown — and a finished loop
/// connection stays *alive and idle* (it is never disconnected on turn complete),
/// so a check keyed on connection *existence* alone would never settle it. We
/// therefore inspect the turn's three-state liveness:
///
/// - `Missing`  (no live connection) → abandon: settle `Failed` (the run died
///   with no completed turn; never faked as success). Bounded by `max_attempts`.
/// - `Idle`     (connection alive, no turn in flight) → the turn finished but its
///   settle event was missed → settle `Succeeded` (the normal completion result).
/// - `InFlight` (a turn is genuinely running) → leave it; killing live work is
///   the operator's call (surfaced via opt-in stall alerts), never a timer here.
///
/// Idempotent: `settle_iteration`/`settle_iteration_as` are CAS, so a double
/// settle (event + reconcile) is a no-op the second time.
pub(crate) async fn reconcile_orphaned_iterations<L: IterationLiveness>(
    db: &AppDatabase,
    emitter: &EventEmitter,
    liveness: &L,
    issue_id: i32,
) -> Result<(), LoopError> {
    let running = loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
        .all(&db.conn)
        .await?;
    if running.is_empty() {
        return Ok(());
    }
    // Opt-in stall watchdog threshold (None = off; the common case skips the
    // config read entirely once there is nothing running anyway, handled above).
    let stall_alert_secs = match loop_issue::Entity::find_by_id(issue_id).one(&db.conn).await? {
        Some(issue) => effective_config(&db.conn, &issue).await?.stall_alert_secs,
        None => None,
    };
    for it in running {
        let Some(cid) = it.conversation_id else {
            continue;
        };
        match liveness.turn_state(cid).await {
            TurnLiveness::InFlight => {
                // Genuinely working — never disturbed here. If the operator armed
                // the opt-in watchdog, surface a (idempotent) stall card so they
                // can decide; the iteration itself is left untouched.
                if let Some(threshold) = stall_alert_secs {
                    if let Err(e) = maybe_file_stall_alert(db, emitter, &it, threshold).await {
                        tracing::warn!(iteration_id = it.id, error = %e, "reconcile: stall alert failed");
                    }
                }
            }
            TurnLiveness::Idle => {
                tracing::debug!(
                    iteration_id = it.id,
                    issue_id,
                    conv = cid,
                    "reconcile: settling idle-but-unsettled iteration (turn finished, event missed)"
                );
                if let Err(e) = settle_iteration(db, emitter, it.id).await {
                    tracing::warn!(iteration_id = it.id, error = %e, "reconcile: settle failed");
                }
            }
            TurnLiveness::Missing => {
                tracing::warn!(
                    iteration_id = it.id,
                    issue_id,
                    conv = cid,
                    "reconcile: abandoning orphaned iteration (no live connection)"
                );
                if let Err(e) =
                    settle_iteration_as(db, emitter, it.id, SettleResolution::Abandoned).await
                {
                    tracing::warn!(iteration_id = it.id, error = %e, "reconcile: abandon failed");
                }
            }
        }
    }
    Ok(())
}

/// Opt-in stall watchdog: when an in-flight iteration has been running at least
/// `threshold_secs` (measured from `started_at`), file an idempotent `stalled:{id}`
/// inbox card so the human can decide whether to step in. Surface-only — it never
/// settles or kills the iteration. A long turn is not necessarily a dead one, and
/// "no artificial limits" means this timer reports, never enforces. The card
/// dedups on `(issue, kind, stalled:{id})`, so re-running every reconcile tick is
/// a no-op once the card is filed.
async fn maybe_file_stall_alert(
    db: &AppDatabase,
    emitter: &EventEmitter,
    iter: &loop_iteration::Model,
    threshold_secs: u64,
) -> Result<(), LoopError> {
    let Some(started) = iter.started_at else {
        return Ok(()); // not actually started yet — nothing to time
    };
    let elapsed = (Utc::now() - started).num_seconds();
    if elapsed < threshold_secs as i64 {
        return Ok(());
    }
    inbox::upsert_inbox(
        &db.conn,
        iter.space_id,
        iter.issue_id,
        Some(iter.id),
        InboxKind::Blocked,
        &format!("stalled:{}", iter.id),
        serde_json::json!({
            "v": 1,
            "reason": "stalled",
            "stage": iter.stage,
            "elapsed_secs": elapsed,
            "threshold_secs": threshold_secs,
        }),
    )
    .await?;
    emit_changed(emitter, iter.space_id, iter.issue_id, iter.id, "stalled");
    Ok(())
}

/// Coverage loop-back (spec §3.3). When a plan leaves some requirement
/// acceptance criterion unclaimed by any live task, supersede the under-covering
/// tasks so the read frontier re-emits Plan (whose briefing then carries the
/// gap) — a bounded feedback edge, not a dead end. Bounded by `max_attempts`
/// (0 = unlimited): on exhaustion, block the issue and file a `coverage_gap`
/// card for a human (raise the cap / fix the requirements / retry).
///
/// Returns `Some(Advanced)` when it acted (caller returns it and re-ticks),
/// `None` when coverage is complete so the caller proceeds to the write pipeline.
///
/// The transient replan does NOT file an inbox card: it is the engine converging
/// as designed, not a state needing human action, and the churn is already
/// visible via `emit_changed` (superseded tasks + a fresh plan attempt). Only the
/// terminal blocked state is an inbox item.
async fn maybe_coverage_loopback(
    db: &AppDatabase,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    dag: &LoopDagView,
) -> Result<Option<TickOutcome>, LoopError> {
    let conn = &db.conn;

    // Live (non-superseded/cancelled) task rows. If none exist the read frontier
    // would have re-emitted Plan, so there is nothing to gate here.
    let live_tasks: Vec<&LoopArtifactRow> = dag
        .artifacts
        .iter()
        .filter(|a| {
            a.kind == ArtifactKind::Task
                && !matches!(a.status, ArtifactStatus::Superseded | ArtifactStatus::Cancelled)
        })
        .collect();
    if live_tasks.is_empty() {
        return Ok(None);
    }
    let live_ids: std::collections::HashSet<i32> = live_tasks.iter().map(|a| a.id).collect();

    let ordinals = coverage::acceptance_ordinals_for_issue(conn, issue.id).await?;
    let uncovered = coverage::uncovered_ordinals(&ordinals, &dag.coverage, &live_ids);
    if uncovered.is_empty() {
        return Ok(None); // every acceptance criterion is covered → proceed
    }

    // The gate runs BEFORE any implement, so a clean replan supersedes the
    // still-`pending` plan output. Bound the loop by the rework cap (every plan
    // dispatch is one plan iteration on record).
    let pending: Vec<i32> = live_tasks
        .iter()
        .filter(|a| a.status == ArtifactStatus::Pending)
        .map(|a| a.id)
        .collect();
    let plan_attempts = loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue.id))
        .filter(loop_iteration::Column::Stage.eq(Stage::Plan))
        .count(conn)
        .await? as u32;
    let exhausted = config.max_attempts != 0 && plan_attempts >= config.max_attempts;
    // If a task already advanced past `pending` while a gap remains (shouldn't
    // happen — coverage is monotonic for a fixed task set — but defend against
    // it), superseding the pending subset would NOT clear the read frontier, so
    // auto-replan can't make progress. Block instead of spinning on `Advanced`.
    let can_replan = pending.len() == live_tasks.len();

    if exhausted || !can_replan {
        cas_issue_status(conn, issue.id, IssueStatus::Running, IssueStatus::Blocked).await?;
        let reason = if exhausted {
            "coverage_gap_exhausted"
        } else {
            "coverage_gap_unresolvable"
        };
        inbox::upsert_inbox(
            conn,
            issue.space_id,
            issue.id,
            None,
            InboxKind::Blocked,
            &format!("coverage_gap:{}", issue.id),
            serde_json::json!({
                "v": 1,
                "reason": reason,
                "uncovered": uncovered,
                "plan_attempts": plan_attempts,
            }),
        )
        .await?;
        emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
        return Ok(Some(TickOutcome::Advanced));
    }

    // All live tasks are still pending → supersede them so the read frontier
    // re-emits Plan next tick (the briefing then carries the gap).
    for &tid in &pending {
        crate::loop_engine::transitions::cas_artifact_status_from(
            conn,
            tid,
            &[ArtifactStatus::Pending],
            ArtifactStatus::Superseded,
        )
        .await?;
    }
    tracing::info!(
        issue_id = issue.id,
        plan_attempts,
        uncovered = ?uncovered,
        "coverage gap: superseding tasks and replanning"
    );
    emit_changed(emitter, issue.space_id, issue.id, issue.id, "issue");
    Ok(Some(TickOutcome::Advanced))
}

/// One scheduling tick for a single issue: ensure triage, then dispatch the
/// ready frontier. Idempotent and side-effect-guarded by the DB leases, so it
/// is safe to call repeatedly. Takes explicit handles (not `&LoopEngine`) so it
/// is testable with just a database + a stub spawner.
pub(crate) async fn tick_once(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue_id: i32,
    infra_retries: &mut HashMap<i32, u32>,
) -> Result<TickOutcome, LoopError> {
    let conn = &db.conn;
    let mut issue = loop_issue::Entity::find_by_id(issue_id)
        .one(conn)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
    if issue.status != IssueStatus::Running {
        return Ok(TickOutcome::Stop);
    }

    let config = effective_config(conn, &issue).await?;

    let Some(worktree_folder_id) = issue.worktree_folder_id else {
        // No worktree yet (trigger sets it up before starting the driver). Can't
        // make progress; idle until a wake.
        tracing::debug!(issue_id, "driver: issue has no worktree folder; idling");
        return Ok(TickOutcome::Idle);
    };

    // Triage first: it decides the route the rest of the pipeline follows. This
    // branch dispatches only the *initial* triage (none on record yet); every
    // retry afterwards is owned by `recover_undecided_triage`, which bounds it by
    // `max_attempts`.
    if !has_any_triage(conn, issue_id).await? {
        let spec = resolve_agent_spec(&config, Stage::Triage);
        let dispatched = dispatch_iteration(
            db,
            data_dir,
            spawner,
            emitter.clone(),
            DispatchInput {
                space_id: issue.space_id,
                issue_id,
                stage: Stage::Triage,
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
        return Ok(if dispatched.is_some() {
            TickOutcome::Dispatched
        } else {
            TickOutcome::Idle
        });
    }

    // Route is written by triage; honor a human force_route override. While the
    // route is still undecided, recover instead of parking forever: wait if a
    // triage is in flight, else re-dispatch (bounded) or block.
    let route = match issue.route {
        IssueRoute::Undecided => match config.force_route {
            Some(r) => r,
            None => {
                return recover_undecided_triage(
                    db,
                    data_dir,
                    spawner,
                    emitter,
                    &issue,
                    &config,
                    worktree_folder_id,
                )
                .await;
            }
        },
        r => r,
    };

    let dag = artifact::list_dag(conn, issue_id).await?;
    ensure_skip_provenance(db, issue.space_id, &dag, route).await?;
    // Design approval gate (route=full): while a produced design awaits human
    // approval, keep its inbox card filed; the read frontier idles until approved.
    ensure_design_gate_card(db, &issue, &dag).await?;

    // Read pipeline first (triage → refine → design → plan). While it has work,
    // the write pipeline waits.
    let frontier = ready_nodes(&dag, route);
    if !frontier.is_empty() {
        let mut dispatched_any = false;
        for item in frontier {
            let spec = resolve_agent_spec(&config, item.stage);
            let handle = dispatch_iteration(
                db,
                data_dir,
                spawner,
                emitter.clone(),
                DispatchInput {
                    space_id: issue.space_id,
                    issue_id,
                    stage: item.stage,
                    target_artifact_id: item.target_artifact_id,
                    slot_no: None,
                    attempt: item.attempt,
                    agent_type: spec.agent,
                    mode_id: spec.mode_id,
                    config_values: spec.config_values,
                    worktree_folder_id,
                },
            )
            .await?;
            if handle.is_some() {
                dispatched_any = true;
            }
        }
        return Ok(if dispatched_any {
            TickOutcome::Dispatched
        } else {
            TickOutcome::Idle
        });
    }

    // Coverage gate (spec §3.3): on a route that produces requirements, a plan
    // that leaves any requirement acceptance criterion unclaimed is incomplete —
    // implementing it would let a verifier pass a partial solution (Goodhart).
    // Supersede the under-covering tasks and replan (bounded by `max_attempts`),
    // before latching the execution mode or driving the write pipeline. Runs only
    // once live tasks exist (otherwise the read frontier above re-emits Plan).
    if matches!(route, IssueRoute::Full | IssueRoute::SkipDesign) {
        if let Some(outcome) = maybe_coverage_loopback(db, emitter, &issue, &config, &dag).await? {
            return Ok(outcome);
        }
    }

    // Decide the issue's execution mode once the task DAG exists (write-once;
    // no-op while read stages are still in flight), and apply the authoritative
    // value to the in-memory issue for THIS tick. The write pipeline below picks
    // each task's worktree by it (parallel → per-task tree; else the shared issue
    // tree) and fans out parallel tasks; a stale in-memory `None` would drive the
    // first task of a freshly decided parallel issue into the wrong (shared) tree
    // and only fan out from the next tick.
    if let Some(mode) = ensure_execution_mode(conn, &issue, &dag).await? {
        issue.execution_mode = Some(mode);
    }

    // Read pipeline complete (tasks exist) → drive the write pipeline. A no-op
    // when there are no tasks yet (read stages still in flight), so it is safe
    // to call on every "frontier empty" tick.
    match gates::drive_active_task(
        db,
        data_dir,
        spawner,
        emitter,
        &issue,
        &dag,
        &config,
        worktree_folder_id,
        infra_retries,
    )
    .await?
    {
        gates::StepOutcome::Dispatched => return Ok(TickOutcome::Dispatched),
        gates::StepOutcome::Advanced => return Ok(TickOutcome::Advanced),
        gates::StepOutcome::Idle => {}
    }

    // Write pipeline drained → finalize when every task is done (produce the
    // result artifact). A no-op until then.
    match gates::run_finalize(
        db,
        data_dir,
        spawner,
        emitter,
        &issue,
        &dag,
        &config,
        worktree_folder_id,
    )
    .await?
    {
        gates::StepOutcome::Dispatched => return Ok(TickOutcome::Dispatched),
        gates::StepOutcome::Advanced => return Ok(TickOutcome::Advanced),
        gates::StepOutcome::Idle => {}
    }

    // Result produced AND integration-verified → merge gate. With `auto_merge` on,
    // signal the driver to land it (the merge needs `&LoopEngine`). Gating on
    // `integration_passed` (not just "a result exists") means auto-merge fires only
    // after the whole-issue closure is verified — and never on a superseded result a
    // loop-back left behind. Otherwise idle: the human gate awaits approve_merge.
    if config.auto_merge && gates::integration_passed(&db.conn, &dag).await? {
        return Ok(TickOutcome::AutoMerge);
    }
    Ok(TickOutcome::Idle)
}

/// Recover a triage that finished without producing a route. Triage decides the
/// pipeline's route; if its agent's turn ended without `loop_submit_route`, the
/// issue would otherwise idle forever on `route = undecided`. While a triage is
/// still in flight we keep waiting; once all triage iterations have settled and
/// the route is still undecided we re-dispatch a fresh triage (bounded by
/// `max_attempts`, 0 = unlimited), and give up into `blocked` + an inbox card
/// once the bound is hit. Never parks silently.
#[allow(clippy::too_many_arguments)]
async fn recover_undecided_triage(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: &EventEmitter,
    issue: &loop_issue::Model,
    config: &IssueConfig,
    worktree_folder_id: i32,
) -> Result<TickOutcome, LoopError> {
    let conn = &db.conn;
    let triage: Vec<loop_iteration::Model> = loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue.id))
        .filter(loop_iteration::Column::Stage.eq(Stage::Triage))
        .all(conn)
        .await?;
    // Still deciding → keep waiting.
    if triage
        .iter()
        .any(|it| matches!(it.status, IterationStatus::Queued | IterationStatus::Running))
    {
        return Ok(TickOutcome::Idle);
    }
    // All triage settled but no route. Bounded recovery.
    let attempts = triage.len() as i32;
    let max = config.max_attempts as i32; // 0 = unlimited
    if max == 0 || attempts < max {
        tracing::debug!(
            issue_id = issue.id,
            attempts,
            "triage: undecided; re-dispatching"
        );
        let spec = resolve_agent_spec(config, Stage::Triage);
        let dispatched = dispatch_iteration(
            db,
            data_dir,
            spawner,
            emitter.clone(),
            DispatchInput {
                space_id: issue.space_id,
                issue_id: issue.id,
                stage: Stage::Triage,
                target_artifact_id: None,
                slot_no: None,
                attempt: attempts,
                agent_type: spec.agent,
                mode_id: spec.mode_id,
                config_values: spec.config_values,
                worktree_folder_id,
            },
        )
        .await?;
        return Ok(if dispatched.is_some() {
            TickOutcome::Dispatched
        } else {
            TickOutcome::Idle
        });
    }
    // Bound hit → block + inbox card (the human can retry or cancel).
    tracing::warn!(
        issue_id = issue.id,
        attempts,
        "triage: gave up with no route; blocking"
    );
    cas_issue_status(conn, issue.id, IssueStatus::Running, IssueStatus::Blocked).await?;
    inbox::upsert_inbox(
        conn,
        issue.space_id,
        issue.id,
        None,
        InboxKind::Blocked,
        &format!("triage_no_route:{}", issue.id),
        serde_json::json!({
            "v": 1,
            "reason": "triage produced no route",
            "attempts": attempts,
        }),
    )
    .await?;
    emit_changed(emitter, issue.space_id, issue.id, issue.id, "blocked");
    // Issue is now blocked → re-tick so the driver observes it and stops cleanly
    // (a human retry then respawns the driver).
    Ok(TickOutcome::Advanced)
}

/// Backstop cadence for the liveness reconcile. The happy path is event-driven
/// (turn-complete → settle → wake); a `Lagged` burst is swept immediately by the
/// completion watcher. This heartbeat is only a coarse net for a missed wake, and
/// is armed ONLY while the issue has in-flight iterations — an idle driver parks
/// on `wake` alone and issues no periodic query.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(15);

/// Diagnostic-only ceiling on *consecutive* `Advanced` re-ticks. The write
/// pipeline is strictly forward-moving, so a correct engine converges in a few
/// ticks; crossing this only signals a logic bug (a gate reporting `Advanced`
/// with no durable progress). It logs — it never caps real work (honoring the
/// "no artificial limits" rule).
const ADVANCE_DIAG_THRESHOLD: u32 = 1000;

/// The per-issue driver task body: tick, then park on the wake `Notify` until a
/// completion (or external nudge) arrives. Exits when the issue leaves
/// `running`, deregistering itself from the engine's driver registry.
pub(crate) async fn run_driver(engine: Arc<LoopEngine>, issue_id: i32, wake: Arc<Notify>) {
    // Periodic liveness heartbeat: re-tick even without a wake so the reconcile
    // below catches iterations whose turn-complete event was missed or raced.
    let mut heartbeat = interval(RECONCILE_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await; // consume the immediate first fire
    // Counts consecutive `Advanced` re-ticks for the diagnostic above; reset on
    // any tick that parks or breaks.
    let mut consecutive_advances: u32 = 0;
    // Per-task infrastructure-failure streaks (worktree-ensure failures), in driver
    // memory. While non-empty, the park below arms the heartbeat so the driver
    // re-ticks to retry even when nothing is in flight. Reset/pruned per tick by
    // `drive_active_task`; a streak crossing INFRA_RETRY_MAX blocks the task.
    let mut infra_retries: HashMap<i32, u32> = HashMap::new();
    loop {
        // DB-authoritative backstop before each tick: settle iterations whose
        // agent connection is gone (the event-driven settle alone can wedge).
        if let Err(e) =
            reconcile_orphaned_iterations(&engine.db, &engine.emitter, &engine.manager, issue_id)
                .await
        {
            tracing::warn!(issue_id, error = %e, "driver: reconcile failed");
        }
        // §2.7 backfill: re-read and charge any iterations whose token total was
        // left pending (session file wasn't flushed at settle time). Cheap —
        // filtered by (issue_id, tokens_pending) on the new composite index.
        if let Err(e) = crate::loop_engine::dispatch::reconcile_pending_tokens(
            &engine.db,
            &engine.emitter,
            issue_id,
        )
        .await
        {
            tracing::warn!(issue_id, error = %e, "driver: pending-token reconcile failed");
        }
        match tick_once(
            &engine.db,
            &engine.data_dir,
            &engine.manager,
            &engine.emitter,
            issue_id,
            &mut infra_retries,
        )
        .instrument(tracing::info_span!("loop_tick", issue_id))
        .await
        {
            Ok(TickOutcome::Stop) => break,
            Ok(TickOutcome::Advanced) => {
                // Durable progress with nothing in flight: re-tick now to dispatch
                // the follow-on step, or observe a block and stop — instead of
                // parking on the no-timeout wake (the wedge that used to need a
                // manual pause→resume, and that left human retries ineffective).
                // `yield_now` keeps the re-tick cooperative rather than a hot loop.
                consecutive_advances += 1;
                if consecutive_advances == ADVANCE_DIAG_THRESHOLD {
                    tracing::warn!(
                        issue_id,
                        consecutive_advances,
                        "driver: unusually long advance chain; possible non-progressing Advanced"
                    );
                }
                tokio::task::yield_now().await;
                continue;
            }
            Ok(TickOutcome::AutoMerge) => {
                // Land the finalized work without a human gate. On success, only
                // re-tick immediately if the merge actually advanced the issue out
                // of `running` (→ Done, or → Blocked on a merge fault); the next
                // tick then observes that state and stops. If it returned Ok yet
                // left the issue `running` (a lost-CAS race, or a future merge
                // variant that defers), DON'T `continue` — that would re-attempt
                // the same merge every tick with no wait. Fall through to park
                // instead. On error, park too (a later wake retries).
                match engine.merge_issue(issue_id).await {
                    Ok(()) => {
                        let still_running = loop_issue::Entity::find_by_id(issue_id)
                            .one(&engine.db.conn)
                            .await
                            .ok()
                            .flatten()
                            .is_some_and(|i| i.status == IssueStatus::Running);
                        if !still_running {
                            continue; // advanced (or gone) → re-tick to stop
                        }
                        tracing::warn!(
                            issue_id,
                            "driver: auto-merge returned Ok but issue still running; parking instead of busy-looping"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(issue_id, error = %e, "driver: auto-merge failed");
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(issue_id, error = %e, "driver: tick failed");
            }
        }
        // A tick that parks (or errs) ends any advance chain.
        consecutive_advances = 0;
        // Park until an iteration settles (the completion watcher fires `wake`)
        // or — while work is in flight OR an infra-retry is pending — the periodic
        // heartbeat elapses (which runs the reconcile above and re-ticks to retry
        // the failed worktree). An otherwise-idle issue waits purely on `wake` and
        // issues no blind periodic query. `notify_one` buffers a permit, so a wake
        // that races ahead is not lost.
        if has_inflight_iteration(&engine.db, issue_id).await || !infra_retries.is_empty() {
            tokio::select! {
                _ = wake.notified() => {}
                _ = heartbeat.tick() => {}
            }
        } else {
            wake.notified().await;
        }
    }
    engine.deregister_driver(issue_id).await;
}

/// Whether the issue has any queued/running iteration. Gates the periodic
/// reconcile heartbeat so an idle driver parks on `wake` alone (uses the new
/// `(issue_id, status)` index).
async fn has_inflight_iteration(db: &AppDatabase, issue_id: i32) -> bool {
    loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .filter(
            loop_iteration::Column::Status
                .is_in([IterationStatus::Queued, IterationStatus::Running]),
        )
        .one(&db.conn)
        .await
        .ok()
        .flatten()
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::error::AcpError;
    use sea_orm::ActiveEnum; // for `IssueStatus::*.to_value()` in test helpers
    use crate::db::entities::loop_artifact::ArtifactKind;
    use crate::db::entities::loop_inbox_item::{self, InboxStatus};
    use crate::db::entities::loop_issue::IssuePriority;
    use crate::db::service::loop_service::{issue, space};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::loop_engine::dispatch::{settle_iteration, settle_iteration_as, SettleResolution};
    use crate::loop_engine::ingest::ingest;
    use crate::loop_engine::transitions::cas_artifact_status;
    use crate::models::loops::IssueConfig;
    use async_trait::async_trait;
    use sea_orm::sea_query::Expr;
    use serde_json::json;
    use std::path::PathBuf;

    use crate::db::entities::loop_artifact_revision::ActorKind;
    use crate::models::loops::LoopLinkRow;

    /// Minimal `Task` artifact row for the pure `dag_has_parallelism` tests.
    fn task_row(id: i32) -> LoopArtifactRow {
        LoopArtifactRow {
            id,
            issue_id: 1,
            issue_seq: 1,
            kind: ArtifactKind::Task,
            title: format!("T{id}"),
            status: ArtifactStatus::Pending,
            origin: ActorKind::Agent,
            produced_by_iteration_id: None,
            verdict: None,
            attempt: 0,
            sort: id,
            updated_at: Utc::now(),
        }
    }

    /// Build a task-only DAG. `edges` are `(successor, predecessor)` pairs — the
    /// `DependsOn` direction (from = successor, to = predecessor).
    fn dag_of(task_ids: &[i32], edges: &[(i32, i32)]) -> LoopDagView {
        LoopDagView {
            artifacts: task_ids.iter().map(|&id| task_row(id)).collect(),
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
        }
    }

    #[test]
    fn execution_mode_chain_is_serial() {
        // A→B→C: never ≥2 ready at once.
        assert!(!dag_has_parallelism(&dag_of(&[1, 2, 3], &[(2, 1), (3, 2)])));
    }

    #[test]
    fn execution_mode_independent_roots_is_parallel() {
        // Two tasks, no edges: both ready immediately.
        assert!(dag_has_parallelism(&dag_of(&[1, 2], &[])));
    }

    #[test]
    fn execution_mode_fanout_is_parallel() {
        // A→{B,C}: 2 edges over 3 tasks (edges == n-1) yet B and C are ready
        // together once A is done. Regression for the naive edge-count heuristic.
        assert!(dag_has_parallelism(&dag_of(&[1, 2, 3], &[(2, 1), (3, 1)])));
    }

    #[test]
    fn execution_mode_single_task_is_serial() {
        assert!(!dag_has_parallelism(&dag_of(&[1], &[])));
    }

    #[tokio::test]
    async fn execution_mode_write_once() {
        let (db, _data_dir, issue_id) = setup().await;
        let issue = loop_issue::Entity::find_by_id(issue_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert!(issue.execution_mode.is_none());

        // First decision: a parallel DAG (two independent tasks) → "parallel".
        let m1 = ensure_execution_mode(&db.conn, &issue, &dag_of(&[1, 2], &[]))
            .await
            .unwrap();
        assert_eq!(m1.as_deref(), Some("parallel"));
        let issue = loop_issue::Entity::find_by_id(issue_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(issue.execution_mode.as_deref(), Some("parallel"));

        // A later tick with a (hypothetically) serial DAG must NOT relatch it.
        let m2 = ensure_execution_mode(&db.conn, &issue, &dag_of(&[1], &[]))
            .await
            .unwrap();
        assert_eq!(m2.as_deref(), Some("parallel"), "write-once: never recomputed");
        let issue = loop_issue::Entity::find_by_id(issue_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(issue.execution_mode.as_deref(), Some("parallel"));
    }

    #[test]
    fn resolve_agent_spec_uses_stage_override_with_mode_and_config() {
        let mut cfg = IssueConfig::default();
        let mut cv = std::collections::BTreeMap::new();
        cv.insert("reasoning".to_string(), "high".to_string());
        cfg.agents.implement = Some(AgentSpec {
            agent: AgentType::Codex,
            mode_id: Some("auto".into()),
            config_values: cv.clone(),
        });
        let spec = resolve_agent_spec(&cfg, Stage::Implement);
        assert_eq!(spec.agent, AgentType::Codex);
        assert_eq!(spec.mode_id.as_deref(), Some("auto"));
        assert_eq!(spec.config_values, cv);
        // A stage with no override falls back to default (Claude Code, no extras).
        let plan = resolve_agent_spec(&cfg, Stage::Plan);
        assert_eq!(plan.agent, AgentType::ClaudeCode);
        assert!(plan.mode_id.is_none() && plan.config_values.is_empty());
    }

    #[tokio::test]
    async fn has_inflight_reflects_queued_and_running_only() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/inflight").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(&db.conn, space.id, "I", "b", IssuePriority::Medium, Some(&IssueConfig::default()))
            .await
            .unwrap();
        assert!(!has_inflight_iteration(&db, issue.row.id).await, "no iterations → idle");
        let it = crate::loop_engine::transitions::try_claim_iteration(
            &db.conn,
            crate::loop_engine::transitions::IterationClaim {
                space_id: space.id,
                issue_id: issue.row.id,
                stage: Stage::Triage,
                target_artifact_id: None,
                slot_no: None,
                capability_token: "t".into(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(has_inflight_iteration(&db, issue.row.id).await, "queued → in flight");
        crate::loop_engine::transitions::cas_iteration_status(&db.conn, it.id, IterationStatus::Queued, IterationStatus::Running)
            .await
            .unwrap();
        assert!(has_inflight_iteration(&db, issue.row.id).await, "running → in flight");
        crate::loop_engine::transitions::cas_iteration_status(&db.conn, it.id, IterationStatus::Running, IterationStatus::Succeeded)
            .await
            .unwrap();
        assert!(!has_inflight_iteration(&db, issue.row.id).await, "terminal → idle");
    }

    /// Simulate a human approving the design gate (route=full), so the read
    /// pipeline can proceed past it. The gate's blocking behavior has its own test.
    async fn approve_awaiting_designs(db: &AppDatabase, issue_id: i32) {
        let dag = artifact::list_dag(&db.conn, issue_id).await.unwrap();
        for a in dag.artifacts.iter().filter(|a| {
            a.kind == ArtifactKind::Design && a.status == ArtifactStatus::AwaitingApproval
        }) {
            cas_artifact_status(
                &db.conn,
                a.id,
                ArtifactStatus::AwaitingApproval,
                ArtifactStatus::Done,
            )
            .await
            .unwrap();
        }
    }

    /// Minimal spawner: records nothing, just hands back a connection id so
    /// dispatch can flip the lease to running. The "agent" is simulated by the
    /// test driving `ingest` + `settle_iteration` directly.
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

    /// Liveness oracle stub: every conversation reports the same fixed state, so
    /// reconcile's three branches are testable without live ACP connections.
    struct StubLiveness(TurnLiveness);

    #[async_trait]
    impl IterationLiveness for StubLiveness {
        async fn turn_state(&self, _conversation_id: i32) -> TurnLiveness {
            self.0
        }
    }

    async fn setup() -> (AppDatabase, PathBuf, i32) {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/loop-driver").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &db.conn,
            space.id,
            "Issue",
            "body",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        // Trigger: mark running + bind the worktree folder.
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Status,
                Expr::value(IssueStatus::Running.to_value()),
            )
            .col_expr(loop_issue::Column::WorktreeFolderId, Expr::value(folder_id))
            .filter(loop_issue::Column::Id.eq(issue.row.id))
            .exec(&db.conn)
            .await
            .unwrap();
        (db, PathBuf::from("/tmp/data"), issue.row.id)
    }

    fn git(dir: &Path, args: &[&str]) {
        let st = std::process::Command::new("git")
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

    /// Like [`setup`] but backed by a real git repo + on-disk worktree, so a tick
    /// that reaches the write pipeline (which provisions per-task worktrees) works.
    /// Returns the tempdir guards so the repo/data dirs outlive the test.
    async fn setup_real() -> (AppDatabase, tempfile::TempDir, tempfile::TempDir, i32) {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let data = tempfile::tempdir().unwrap();
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, &repo.path().to_string_lossy()).await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &db.conn,
            space.id,
            "Issue",
            "body",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        let ctx = crate::loop_engine::worktree::ensure_worktree(&db.conn, data.path(), issue.row.id)
            .await
            .unwrap();
        // Trigger: mark running + bind the issue's real worktree folder.
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Status,
                Expr::value(IssueStatus::Running.to_value()),
            )
            .col_expr(
                loop_issue::Column::WorktreeFolderId,
                Expr::value(ctx.worktree_folder_id),
            )
            .filter(loop_issue::Column::Id.eq(issue.row.id))
            .exec(&db.conn)
            .await
            .unwrap();
        (db, data, repo, issue.row.id)
    }

    /// Settle every currently-running triage iteration WITHOUT submitting a
    /// route (simulates a triage agent whose turn ended without
    /// `loop_submit_route`), leaving `issue.route` undecided.
    async fn settle_running_triage_without_route(db: &AppDatabase) {
        let running = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Triage))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .all(&db.conn)
            .await
            .unwrap();
        for it in running {
            settle_iteration(db, &EventEmitter::Noop, it.id)
                .await
                .unwrap();
        }
    }

    /// Drive one tick to dispatch triage, returning its single running iteration.
    async fn dispatch_one_running_triage(
        db: &AppDatabase,
        data_dir: &Path,
        issue_id: i32,
    ) -> loop_iteration::Model {
        let spawner = StubSpawner;
        tick_once(db, data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        let running = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::IssueId.eq(issue_id))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(running.len(), 1, "triage dispatched and running");
        running.into_iter().next().unwrap()
    }

    #[tokio::test]
    async fn reconcile_abandons_iteration_with_missing_connection() {
        let (db, data_dir, issue_id) = setup().await;
        let it = dispatch_one_running_triage(&db, &data_dir, issue_id).await;
        // No live connection (empty manager / Missing) → abandon → Failed, never
        // faked as Succeeded.
        reconcile_orphaned_iterations(
            &db,
            &EventEmitter::Noop,
            &StubLiveness(TurnLiveness::Missing),
            issue_id,
        )
        .await
        .unwrap();
        let row = loop_iteration::Entity::find_by_id(it.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, IterationStatus::Failed);
    }

    #[tokio::test]
    async fn reconcile_settles_idle_connection_as_succeeded() {
        let (db, data_dir, issue_id) = setup().await;
        let it = dispatch_one_running_triage(&db, &data_dir, issue_id).await;
        // Connection alive but no turn in flight → the turn finished, its settle
        // event was missed → reconcile completes it as Succeeded.
        reconcile_orphaned_iterations(
            &db,
            &EventEmitter::Noop,
            &StubLiveness(TurnLiveness::Idle),
            issue_id,
        )
        .await
        .unwrap();
        let row = loop_iteration::Entity::find_by_id(it.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, IterationStatus::Succeeded);
    }

    #[tokio::test]
    async fn reconcile_leaves_inflight_iteration_running() {
        let (db, data_dir, issue_id) = setup().await;
        let it = dispatch_one_running_triage(&db, &data_dir, issue_id).await;
        // A turn is genuinely in flight → reconcile must not disturb it.
        reconcile_orphaned_iterations(
            &db,
            &EventEmitter::Noop,
            &StubLiveness(TurnLiveness::InFlight),
            issue_id,
        )
        .await
        .unwrap();
        let row = loop_iteration::Entity::find_by_id(it.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, IterationStatus::Running);
    }

    /// Overwrite an issue's config with `stall_alert_secs` set (config_inherits is
    /// false after `create_issue`, so the issue's own config is what's resolved).
    async fn set_stall_alert(db: &AppDatabase, issue_id: i32, secs: Option<u64>) {
        let cfg = IssueConfig {
            stall_alert_secs: secs,
            ..IssueConfig::default()
        };
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Config,
                Expr::value(serde_json::to_string(&cfg).unwrap()),
            )
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(&db.conn)
            .await
            .unwrap();
    }

    /// Backdate an iteration's `started_at` so it reads as having run `secs` ago.
    async fn backdate_started(db: &AppDatabase, iter_id: i32, secs: i64) {
        loop_iteration::Entity::update_many()
            .col_expr(
                loop_iteration::Column::StartedAt,
                Expr::value(Utc::now() - chrono::Duration::seconds(secs)),
            )
            .filter(loop_iteration::Column::Id.eq(iter_id))
            .exec(&db.conn)
            .await
            .unwrap();
    }

    async fn stall_card(db: &AppDatabase, iter_id: i32) -> Option<loop_inbox_item::Model> {
        loop_inbox_item::Entity::find()
            .filter(loop_inbox_item::Column::SubjectKey.eq(format!("stalled:{iter_id}")))
            .one(&db.conn)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn stall_alert_files_card_only_when_configured() {
        // Configured: an in-flight iteration older than the threshold files a
        // `stalled` card — but is never killed (surface-only watchdog).
        let (db, data_dir, issue_id) = setup().await;
        set_stall_alert(&db, issue_id, Some(1)).await;
        let it = dispatch_one_running_triage(&db, &data_dir, issue_id).await;
        backdate_started(&db, it.id, 10).await;
        reconcile_orphaned_iterations(
            &db,
            &EventEmitter::Noop,
            &StubLiveness(TurnLiveness::InFlight),
            issue_id,
        )
        .await
        .unwrap();
        let row = loop_iteration::Entity::find_by_id(it.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, IterationStatus::Running, "stall alert never kills");
        let card = stall_card(&db, it.id).await.expect("configured → card filed");
        assert_eq!(card.kind, InboxKind::Blocked);
        assert_eq!(card.iteration_id, Some(it.id));

        // Not configured (default None): the iteration may run arbitrarily long
        // and no card is ever filed — honors "no artificial limits".
        let (db2, data_dir2, issue_id2) = setup().await;
        let it2 = dispatch_one_running_triage(&db2, &data_dir2, issue_id2).await;
        backdate_started(&db2, it2.id, 100_000).await;
        reconcile_orphaned_iterations(
            &db2,
            &EventEmitter::Noop,
            &StubLiveness(TurnLiveness::InFlight),
            issue_id2,
        )
        .await
        .unwrap();
        assert!(
            stall_card(&db2, it2.id).await.is_none(),
            "no threshold = no alert, ever"
        );
    }

    #[tokio::test]
    async fn undecided_triage_redispatches_then_blocks() {
        let (db, data_dir, issue_id) = setup().await;
        // max_attempts = 2 → one re-dispatch, then block.
        let cfg = IssueConfig {
            max_attempts: 2,
            ..IssueConfig::default()
        };
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Config,
                Expr::value(serde_json::to_string(&cfg).unwrap()),
            )
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(&db.conn)
            .await
            .unwrap();
        let spawner = StubSpawner;

        // Tick 1: dispatch triage, then settle it with no route.
        tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        settle_running_triage_without_route(&db).await;

        // Tick 2: triage settled but undecided → re-dispatch (attempt 1).
        let out = tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        assert_eq!(out, TickOutcome::Dispatched);
        settle_running_triage_without_route(&db).await;

        // Tick 3: attempts hit max → block + inbox card. The block reports
        // Advanced so the driver re-ticks and stops on the now-blocked issue.
        let out = tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        assert_eq!(out, TickOutcome::Advanced);
        let issue = loop_issue::Entity::find_by_id(issue_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(issue.status, IssueStatus::Blocked);
        let card = loop_inbox_item::Entity::find()
            .filter(loop_inbox_item::Column::IssueId.eq(issue_id))
            .filter(loop_inbox_item::Column::SubjectKey.eq(format!("triage_no_route:{issue_id}")))
            .one(&db.conn)
            .await
            .unwrap();
        assert!(card.is_some(), "blocked triage files an inbox card");
    }

    #[tokio::test]
    async fn abandoned_triage_uses_bounded_recovery_not_unbounded_redispatch() {
        let (db, data_dir, issue_id) = setup().await;
        // max_attempts = 1 → a single failed triage with no route must BLOCK. A
        // Failed triage still counts as "triage on record", so tick_once defers to
        // bounded recovery instead of its unbounded attempt-0 initial-dispatch.
        let cfg = IssueConfig {
            max_attempts: 1,
            ..IssueConfig::default()
        };
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Config,
                Expr::value(serde_json::to_string(&cfg).unwrap()),
            )
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(&db.conn)
            .await
            .unwrap();
        let spawner = StubSpawner;

        // Tick 1: dispatch triage, then abandon it (dead connection → Failed).
        tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        let running = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Triage))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .all(&db.conn)
            .await
            .unwrap();
        for it in running {
            settle_iteration_as(&db, &EventEmitter::Noop, it.id, SettleResolution::Abandoned)
                .await
                .unwrap();
        }

        // Tick 2: one Failed triage + undecided route + max_attempts=1 → block,
        // NOT a fresh attempt-0 dispatch (the pre-fix bug). The block reports
        // Advanced (re-tick → stop), not a redispatch.
        let out = tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        assert_eq!(out, TickOutcome::Advanced);
        let issue = loop_issue::Entity::find_by_id(issue_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            issue.status,
            IssueStatus::Blocked,
            "an abandoned triage must bound via recovery, not redispatch unbounded"
        );
    }

    /// Build a post-plan DAG with a coverage gap: route=full, two done
    /// requirements (each one acceptance criterion), a done design, a settled
    /// plan iteration on record, and two pending tasks — but coverage only for
    /// R1.AC1 (R2.AC1 left uncovered). Returns (db, data_dir, issue_id, task_ids).
    async fn seed_coverage_gap() -> (AppDatabase, PathBuf, i32, Vec<i32>) {
        use crate::db::entities::loop_criterion::CriterionKind;
        use crate::loop_engine::transitions::{
            cas_iteration_status, try_claim_iteration, IterationClaim,
        };

        let (db, data_dir, issue_id) = setup().await;
        let conn = &db.conn;
        let issue = loop_issue::Entity::find_by_id(issue_id)
            .one(conn)
            .await
            .unwrap()
            .unwrap();
        let space_id = issue.space_id;

        // Route = full (the gate only runs on routes that produce requirements).
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Route,
                Expr::value(IssueRoute::Full.to_value()),
            )
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(conn)
            .await
            .unwrap();

        // A triage iteration on record so tick_once doesn't dispatch the initial one.
        try_claim_iteration(
            conn,
            IterationClaim {
                space_id,
                issue_id,
                stage: Stage::Triage,
                target_artifact_id: None,
                slot_no: None,
                capability_token: "triage-tok".into(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .unwrap();

        // Two done requirements, each with one acceptance criterion.
        let r1 = artifact::create_artifact(conn, space_id, issue_id, ArtifactKind::Requirement, "R1", ArtifactStatus::Done, ActorKind::Agent, None).await.unwrap();
        artifact::add_criterion(conn, r1.id, CriterionKind::Acceptance, "r1 ac").await.unwrap();
        let r2 = artifact::create_artifact(conn, space_id, issue_id, ArtifactKind::Requirement, "R2", ArtifactStatus::Done, ActorKind::Agent, None).await.unwrap();
        artifact::add_criterion(conn, r2.id, CriterionKind::Acceptance, "r2 ac").await.unwrap();

        // A done design fanning into both requirements.
        let d = artifact::create_artifact(conn, space_id, issue_id, ArtifactKind::Design, "D", ArtifactStatus::Done, ActorKind::Agent, None).await.unwrap();
        link::create_link(conn, space_id, d.id, r1.id, LinkKind::DerivesFrom, None).await.unwrap();
        link::create_link(conn, space_id, d.id, r2.id, LinkKind::DerivesFrom, None).await.unwrap();

        // One settled plan iteration on record (the plan that produced the tasks).
        let plan_it = try_claim_iteration(
            conn,
            IterationClaim {
                space_id,
                issue_id,
                stage: Stage::Plan,
                target_artifact_id: Some(d.id),
                slot_no: None,
                capability_token: "plan-tok".into(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .unwrap();
        cas_iteration_status(conn, plan_it.id, IterationStatus::Queued, IterationStatus::Running).await.unwrap();
        cas_iteration_status(conn, plan_it.id, IterationStatus::Running, IterationStatus::Succeeded).await.unwrap();

        // Two pending tasks; coverage only for R1's acceptance criterion.
        let t1 = artifact::create_artifact(conn, space_id, issue_id, ArtifactKind::Task, "T1", ArtifactStatus::Pending, ActorKind::Agent, None).await.unwrap();
        let t2 = artifact::create_artifact(conn, space_id, issue_id, ArtifactKind::Task, "T2", ArtifactStatus::Pending, ActorKind::Agent, None).await.unwrap();
        let r1ac = artifact::get_artifact_detail(conn, r1.id).await.unwrap().unwrap().criteria[0].id;
        coverage::create_coverage(conn, space_id, t1.id, r1ac).await.unwrap();

        (db, data_dir, issue_id, vec![t1.id, t2.id])
    }

    #[tokio::test]
    async fn coverage_gap_supersedes_tasks_and_replans() {
        let (db, data_dir, issue_id, tasks) = seed_coverage_gap().await;
        let spawner = StubSpawner;
        let out = tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        assert_eq!(out, TickOutcome::Advanced, "coverage gap is durable progress");
        // Both under-covering tasks superseded.
        for t in &tasks {
            let node = artifact::get_artifact_detail(&db.conn, *t).await.unwrap().unwrap();
            assert_eq!(node.row.status, ArtifactStatus::Superseded, "task {t} superseded");
        }
        // Issue still running (bounded replan, not exhausted) and the read frontier
        // now re-emits Plan (live tasks empty).
        let issue = loop_issue::Entity::find_by_id(issue_id).one(&db.conn).await.unwrap().unwrap();
        assert_eq!(issue.status, IssueStatus::Running);
        let dag = artifact::list_dag(&db.conn, issue_id).await.unwrap();
        let frontier = ready_nodes(&dag, IssueRoute::Full);
        assert_eq!(frontier.len(), 1);
        assert_eq!(frontier[0].stage, Stage::Plan, "re-emits Plan to replan");
    }

    #[tokio::test]
    async fn coverage_gap_exhausts_to_blocked_with_card() {
        let (db, data_dir, issue_id, tasks) = seed_coverage_gap().await;
        // Tighten the rework cap to 1: the single plan iteration on record already
        // meets it, so the gap blocks instead of replanning.
        let cfg = IssueConfig {
            max_attempts: 1,
            ..IssueConfig::default()
        };
        write_config(&db, issue_id, &cfg).await;

        let spawner = StubSpawner;
        let out = tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        assert_eq!(out, TickOutcome::Advanced);
        // Issue blocked; tasks left untouched; a coverage_gap card filed.
        let issue = loop_issue::Entity::find_by_id(issue_id).one(&db.conn).await.unwrap().unwrap();
        assert_eq!(issue.status, IssueStatus::Blocked);
        let node = artifact::get_artifact_detail(&db.conn, tasks[0]).await.unwrap().unwrap();
        assert_eq!(node.row.status, ArtifactStatus::Pending, "tasks untouched on exhaustion");
        let cards = inbox::list_inbox(&db.conn, issue.space_id, None).await.unwrap();
        assert!(
            cards.iter().any(|c| c.subject_key == format!("coverage_gap:{issue_id}")
                && c.kind == InboxKind::Blocked),
            "coverage_gap card filed"
        );
    }

    /// Overwrite an issue's whole config (config_inherits is false after
    /// `create_issue`, so its own config is what `effective_config` resolves).
    async fn write_config(db: &AppDatabase, issue_id: i32, cfg: &IssueConfig) {
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Config,
                Expr::value(serde_json::to_string(cfg).unwrap()),
            )
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(&db.conn)
            .await
            .unwrap();
    }

    /// Settle every running iteration WITHOUT ingesting any artifact — simulates a
    /// read-stage agent whose turn ended having produced nothing (no-progress).
    async fn settle_all_running_without_output(db: &AppDatabase) {
        let running = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .all(&db.conn)
            .await
            .unwrap();
        for it in running {
            settle_iteration(db, &EventEmitter::Noop, it.id)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn read_stage_no_output_blocks_at_max_attempts() {
        let (db, data_dir, issue_id) = setup().await;
        // Small cap so the breaker trips quickly. skip_design route → refine is the
        // first read stage and targets the issue root, so the root node's attempt
        // is what the breaker counts.
        write_config(
            &db,
            issue_id,
            &IssueConfig {
                max_attempts: 2,
                ..IssueConfig::default()
            },
        )
        .await;
        let spawner = StubSpawner;

        // Get past triage with a decided route.
        tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        respond_and_settle(&db, "skip_design").await;

        // Refine now runs but produces nothing, repeatedly. The settle-time breaker
        // bumps the root node attempt each pass and blocks once it hits the cap —
        // it must terminate, never redispatch forever (the D5 bug).
        let mut stopped = false;
        for _ in 0..12 {
            let out = tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
                .await
                .unwrap();
            if out == TickOutcome::Stop {
                stopped = true;
                break; // issue already blocked; driver would exit
            }
            settle_all_running_without_output(&db).await;
        }
        assert!(stopped, "read-stage no-progress must stop, not loop forever");

        let issue = loop_issue::Entity::find_by_id(issue_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(issue.status, IssueStatus::Blocked);
        let card = loop_inbox_item::Entity::find()
            .filter(loop_inbox_item::Column::IssueId.eq(issue_id))
            .filter(loop_inbox_item::Column::Kind.eq(InboxKind::Blocked))
            .filter(loop_inbox_item::Column::SubjectKey.starts_with("no_progress:"))
            .one(&db.conn)
            .await
            .unwrap();
        assert!(card.is_some(), "read-stage breaker files a no_progress card");
    }

    /// Simulate the dispatched iteration's agent: submit the stage-appropriate
    /// output through the real ingest boundary, then settle it.
    async fn respond_and_settle(db: &AppDatabase, route: &str) {
        let running = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .all(&db.conn)
            .await
            .unwrap();
        for it in running {
            let tok = &it.capability_token;
            match it.stage {
                Stage::Triage => {
                    ingest(&db.conn, tok, "loop_submit_route", &json!({ "route": route }))
                        .await
                        .unwrap();
                }
                Stage::Refine => {
                    ingest(
                        &db.conn,
                        tok,
                        "loop_submit_artifacts",
                        &json!({ "artifacts": [{ "title": "R1" }, { "title": "R2" }] }),
                    )
                    .await
                    .unwrap();
                }
                Stage::Design => {
                    ingest(
                        &db.conn,
                        tok,
                        "loop_submit_artifacts",
                        &json!({ "artifacts": [{ "title": "D1" }] }),
                    )
                    .await
                    .unwrap();
                }
                Stage::Plan => {
                    ingest(
                        &db.conn,
                        tok,
                        "loop_submit_artifacts",
                        &json!({ "artifacts": [{ "title": "T1" }, { "title": "T2" }] }),
                    )
                    .await
                    .unwrap();
                }
                other => panic!("read pipeline helper got non-read stage: {other:?}"),
            }
            settle_iteration(db, &EventEmitter::Noop, it.id).await.unwrap();
        }
    }

    /// Drive `tick_once` through the read pipeline, simulating each dispatched
    /// read iteration, and stop at the first implement dispatch. That dispatch
    /// happens on the post-plan tick — the same tick that applies skip
    /// provenance once the read frontier empties — so on return the DAG is fully
    /// grown (incl. skips_to). The implement iteration is left freshly running
    /// (the gates tests own its checkpoint, which needs a real worktree).
    async fn drive_through_read_pipeline(
        db: &AppDatabase,
        data_dir: &Path,
        issue_id: i32,
        route: &str,
    ) {
        let spawner = StubSpawner;
        for _ in 0..30 {
            let _ = tick_once(db, data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
                .await
                .unwrap();
            let into_implement = loop_iteration::Entity::find()
                .filter(loop_iteration::Column::Stage.eq(Stage::Implement))
                .one(&db.conn)
                .await
                .unwrap()
                .is_some();
            if into_implement {
                return; // read pipeline + skip provenance complete
            }
            respond_and_settle(db, route).await;
            // A human approves the design gate so full-route pipelines advance.
            approve_awaiting_designs(db, issue_id).await;
        }
        panic!("read pipeline did not reach implement within the iteration budget");
    }

    fn kind_count(dag: &LoopDagView, kind: ArtifactKind) -> usize {
        dag.artifacts.iter().filter(|a| a.kind == kind).count()
    }

    #[tokio::test]
    async fn full_route_grows_dag_through_tasks() {
        let (db, data, _repo, issue_id) = setup_real().await;
        drive_through_read_pipeline(&db, data.path(), issue_id, "full").await;

        let dag = artifact::list_dag(&db.conn, issue_id).await.unwrap();
        assert_eq!(kind_count(&dag, ArtifactKind::Issue), 1);
        assert_eq!(kind_count(&dag, ArtifactKind::Requirement), 2);
        assert_eq!(kind_count(&dag, ArtifactKind::Design), 1);
        assert_eq!(kind_count(&dag, ArtifactKind::Task), 2);

        let derives = dag
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::DerivesFrom)
            .count();
        assert!(derives >= 5, "every produced node derives from a source");
        assert!(
            !dag.links.iter().any(|l| l.kind == LinkKind::SkipsTo),
            "full route skips nothing"
        );

        // Triage decided the route; the read pipeline ran to completion. (The
        // implement iteration just dispatched and is still running — excluded.)
        let settled = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::IssueId.eq(issue_id))
            .all(&db.conn)
            .await
            .unwrap();
        assert!(settled
            .iter()
            .filter(|it| it.stage != Stage::Implement)
            .all(|it| it.status == IterationStatus::Succeeded));
        assert_eq!(
            settled.iter().filter(|it| it.stage == Stage::Refine).count(),
            1
        );
        assert_eq!(
            settled.iter().filter(|it| it.stage == Stage::Design).count(),
            1
        );
        assert_eq!(settled.iter().filter(|it| it.stage == Stage::Plan).count(), 1);
    }

    #[tokio::test]
    async fn direct_route_skips_refine_and_design_with_skips_to() {
        let (db, data, _repo, issue_id) = setup_real().await;
        drive_through_read_pipeline(&db, data.path(), issue_id, "direct").await;

        let dag = artifact::list_dag(&db.conn, issue_id).await.unwrap();
        assert_eq!(kind_count(&dag, ArtifactKind::Requirement), 0, "no requirements");
        assert_eq!(kind_count(&dag, ArtifactKind::Design), 0, "no design");
        assert_eq!(kind_count(&dag, ArtifactKind::Task), 2);

        let skips = dag
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::SkipsTo)
            .count();
        assert_eq!(skips, 2, "each task records skip provenance to the root");

        // No refine/design iterations were dispatched.
        let iters = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::IssueId.eq(issue_id))
            .all(&db.conn)
            .await
            .unwrap();
        assert!(!iters
            .iter()
            .any(|it| matches!(it.stage, Stage::Refine | Stage::Design)));
    }

    /// Regression: the execution mode decided on the first write tick must take
    /// effect THAT tick. A parallel plan (two independent tasks) must fan out both
    /// implements into their OWN per-task worktrees on the deciding tick — not
    /// drive the first task serially into the shared issue worktree. (Bug: tick
    /// persisted the decided mode to the DB but kept driving with the stale
    /// pre-decision in-memory issue, so a freshly parallel issue's first task ran
    /// in the shared tree and only fanned out from the next tick — stranding that
    /// task's edits when its checkpoint later looked in the per-task tree.)
    #[tokio::test]
    async fn first_write_tick_applies_decided_parallel_mode() {
        let (db, data, _repo, issue_id) = setup_real().await;
        // "direct" route → the planner submits two independent tasks → parallel.
        drive_through_read_pipeline(&db, data.path(), issue_id, "direct").await;

        let issue = loop_issue::Entity::find_by_id(issue_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(issue.execution_mode.as_deref(), Some("parallel"));

        // Both tasks' implements launched on the SAME tick that decided the mode —
        // not just one. With the stale-`None` bug the deciding tick truncates to a
        // single task driven into the shared issue worktree; the fan-out (and the
        // per-task worktree each implement needs to dispatch) only appears here
        // because the decided mode is applied in-tick. (Each running implement
        // implies its per-task worktree was provisioned: a worktree failure would
        // have skipped the task, leaving fewer than two.)
        let impls = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::Stage.eq(Stage::Implement))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(
            impls.len(),
            2,
            "parallel fan-out applies on the deciding tick, not the next one"
        );
    }

    #[tokio::test]
    async fn design_gate_blocks_plan_until_approved() {
        let (db, data_dir, issue_id) = setup().await;
        let spawner = StubSpawner;
        let space_id = loop_issue::Entity::find_by_id(issue_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap()
            .space_id;

        // Drive triage(full) → refine → design, settling each but NOT approving.
        let mut awaiting = false;
        for _ in 0..12 {
            tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
                .await
                .unwrap();
            respond_and_settle(&db, "full").await;
            let dag = artifact::list_dag(&db.conn, issue_id).await.unwrap();
            if dag.artifacts.iter().any(|a| {
                a.kind == ArtifactKind::Design && a.status == ArtifactStatus::AwaitingApproval
            }) {
                awaiting = true;
                break;
            }
        }
        assert!(awaiting, "a design reached the approval gate");

        // The gate holds: a card is filed and no task is dispatched, even on a
        // further tick.
        tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
            .await
            .unwrap();
        let dag = artifact::list_dag(&db.conn, issue_id).await.unwrap();
        assert_eq!(
            kind_count(&dag, ArtifactKind::Task),
            0,
            "planning is blocked by the design gate"
        );
        let cards = inbox::list_inbox(&db.conn, space_id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert!(cards
            .iter()
            .any(|c| c.kind == InboxKind::Approval
                && c.subject_key == format!("design:{issue_id}")));

        // Approve → the pipeline advances and planning produces tasks.
        approve_awaiting_designs(&db, issue_id).await;
        let mut tasks = 0;
        for _ in 0..12 {
            tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id, &mut HashMap::new())
                .await
                .unwrap();
            respond_and_settle(&db, "full").await;
            approve_awaiting_designs(&db, issue_id).await;
            tasks = kind_count(&artifact::list_dag(&db.conn, issue_id).await.unwrap(), ArtifactKind::Task);
            if tasks > 0 {
                break;
            }
        }
        assert!(tasks > 0, "planning produced tasks after approval");
    }

    #[test]
    fn ready_nodes_full_pipeline_progression() {
        // Build DAG snapshots by hand to exercise the pure frontier function.
        let mk = |id: i32, kind: ArtifactKind, status: ArtifactStatus| LoopArtifactRow {
            id,
            issue_id: 1,
            issue_seq: 1,
            kind,
            title: "x".into(),
            status,
            origin: crate::db::entities::loop_artifact_revision::ActorKind::Agent,
            produced_by_iteration_id: None,
            verdict: None,
            attempt: 0,
            sort: 0,
            updated_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        };
        let root = mk(1, ArtifactKind::Issue, ArtifactStatus::Done);

        // Only the root → refine is next.
        let dag = LoopDagView {
            artifacts: vec![root.clone()],
            links: vec![],
            coverage: vec![],
            criterion_checks: vec![],
            gate_decisions: vec![],
        };
        let f = ready_nodes(&dag, IssueRoute::Full);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].stage, Stage::Refine);
        assert_eq!(f[0].target_artifact_id, Some(1));

        // Requirements done → design is next.
        let dag = LoopDagView {
            artifacts: vec![
                root.clone(),
                mk(2, ArtifactKind::Requirement, ArtifactStatus::Done),
            ],
            links: vec![],
            coverage: vec![],
            criterion_checks: vec![],
            gate_decisions: vec![],
        };
        assert_eq!(ready_nodes(&dag, IssueRoute::Full)[0].stage, Stage::Design);

        // Design done → plan is next.
        let dag = LoopDagView {
            artifacts: vec![
                root.clone(),
                mk(2, ArtifactKind::Requirement, ArtifactStatus::Done),
                mk(3, ArtifactKind::Design, ArtifactStatus::Done),
            ],
            links: vec![],
            coverage: vec![],
            criterion_checks: vec![],
            gate_decisions: vec![],
        };
        assert_eq!(ready_nodes(&dag, IssueRoute::Full)[0].stage, Stage::Plan);

        // Tasks exist → read frontier empty (the write pipeline drives them).
        let dag = LoopDagView {
            artifacts: vec![
                root.clone(),
                mk(2, ArtifactKind::Requirement, ArtifactStatus::Done),
                mk(3, ArtifactKind::Design, ArtifactStatus::Done),
                mk(4, ArtifactKind::Task, ArtifactStatus::Pending),
            ],
            links: vec![],
            coverage: vec![],
            criterion_checks: vec![],
            gate_decisions: vec![],
        };
        assert!(ready_nodes(&dag, IssueRoute::Full).is_empty());
    }

    #[test]
    fn ready_nodes_route_shortening() {
        let mk = |id: i32, kind: ArtifactKind, status: ArtifactStatus| LoopArtifactRow {
            id,
            issue_id: 1,
            issue_seq: 1,
            kind,
            title: "x".into(),
            status,
            origin: crate::db::entities::loop_artifact_revision::ActorKind::Agent,
            produced_by_iteration_id: None,
            verdict: None,
            attempt: 0,
            sort: 0,
            updated_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        };
        let root = mk(1, ArtifactKind::Issue, ArtifactStatus::Done);

        // direct: straight to plan, no refine/design.
        let dag = LoopDagView {
            artifacts: vec![root.clone()],
            links: vec![],
            coverage: vec![],
            criterion_checks: vec![],
            gate_decisions: vec![],
        };
        let f = ready_nodes(&dag, IssueRoute::Direct);
        assert_eq!(f[0].stage, Stage::Plan);
        assert_eq!(f[0].target_artifact_id, Some(1));

        // skip_design: refine first, then plan (no design step).
        let f = ready_nodes(&dag, IssueRoute::SkipDesign);
        assert_eq!(f[0].stage, Stage::Refine);
        let dag = LoopDagView {
            artifacts: vec![
                root.clone(),
                mk(2, ArtifactKind::Requirement, ArtifactStatus::Done),
            ],
            links: vec![],
            coverage: vec![],
            criterion_checks: vec![],
            gate_decisions: vec![],
        };
        assert_eq!(ready_nodes(&dag, IssueRoute::SkipDesign)[0].stage, Stage::Plan);
    }
}
