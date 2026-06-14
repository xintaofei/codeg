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
//! M2.1 scope is the read pipeline only: triage → refine → design → plan. Once
//! the plan stage has produced tasks (which land `pending`, awaiting implement)
//! the frontier is empty and the driver idles — implement / review / finalize
//! arrive in M2.2.

use std::path::Path;
use std::sync::Arc;

use sea_orm::{ActiveEnum, ColumnTrait, EntityTrait, QueryFilter};
use tokio::sync::Notify;
use tokio::time::{interval, Duration, MissedTickBehavior};

use crate::acp::manager::ConnectionManager;
use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus};
use crate::db::entities::loop_inbox_item::InboxKind;
use crate::db::entities::loop_issue::{self, IssueRoute, IssueStatus};
use crate::db::entities::loop_iteration::{self, IterationStatus, Stage};
use crate::db::entities::loop_link::LinkKind;
use crate::db::service::loop_service::{artifact, inbox, link};
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::loops::{IssueConfig, LoopArtifactRow, LoopDagView};
use crate::web::event_bridge::EventEmitter;

use crate::loop_engine::config_resolver::effective_config;
use crate::loop_engine::dispatch::{
    dispatch_iteration, emit_changed, settle_iteration, DispatchInput, LoopAgentSpawner,
};
use crate::loop_engine::error::LoopError;
use crate::loop_engine::gates;
use crate::loop_engine::transitions::cas_issue_status;
use crate::loop_engine::LoopEngine;

/// Result of a single tick.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TickOutcome {
    /// The issue is no longer `running`; the driver should exit.
    Stop,
    /// At least one iteration was dispatched this tick.
    Dispatched,
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
/// Once tasks exist the frontier is empty (M2.1 stops before implement).
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

    // 2. Design → design (derives from a requirement, or the root if none).
    if needs_design {
        if designs.is_empty() {
            let target = reqs.last().map(|r| r.id).unwrap_or(root);
            return one(Stage::Design, target);
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

    // 4. Tasks exist → M2.1 stops here (implement lands in M2.2).
    Vec::new()
}

/// Resolve the agent for a stage from the issue's Loop Contract: a stage-keyed
/// override (e.g. `"review"`) falls back to `"default"`, then to Claude Code.
pub(crate) fn resolve_agent(config: &IssueConfig, stage: Stage) -> AgentType {
    let key = stage.to_value();
    config
        .agents
        .get(&key)
        .or_else(|| config.agents.get("default"))
        .copied()
        .unwrap_or(AgentType::ClaudeCode)
}

/// Has this issue already had a triage iteration dispatched (and not failed)?
/// Triage targets the whole issue (`target = None`), so the §4.1a node lease
/// can't dedup it (SQLite treats NULL targets as distinct) — this app-level
/// gate is what stops a re-tick from launching a second triage.
async fn has_live_triage(conn: &sea_orm::DatabaseConnection, issue_id: i32) -> Result<bool, LoopError> {
    let triage = loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .filter(loop_iteration::Column::Stage.eq(Stage::Triage))
        .all(conn)
        .await?;
    Ok(triage.iter().any(|it| {
        matches!(
            it.status,
            IterationStatus::Queued | IterationStatus::Running | IterationStatus::Succeeded
        )
    }))
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
            link::create_link(&db.conn, space_id, task.id, root, LinkKind::SkipsTo).await?;
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
/// iterations whose backing agent connection no longer exists. The completion
/// watcher settles on `TurnComplete`, but that single in-process event can be
/// missed or race the connection teardown — and the driver is event-driven with
/// no other periodic settle — so without this an iteration (e.g. triage) can
/// park at `running` forever. Idempotent: `settle_iteration` is a CAS, so a
/// double settle (event + reconcile) is a no-op the second time.
pub(crate) async fn reconcile_orphaned_iterations(
    db: &AppDatabase,
    emitter: &EventEmitter,
    manager: &ConnectionManager,
    issue_id: i32,
) -> Result<(), LoopError> {
    let running = loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
        .all(&db.conn)
        .await?;
    for it in running {
        let Some(cid) = it.conversation_id else {
            continue;
        };
        if manager.find_connection_by_conversation_id(cid).await.is_none() {
            eprintln!(
                "[loop][reconcile] settling orphaned iteration {} (issue {issue_id}, conv {cid}): no live connection",
                it.id
            );
            if let Err(e) = settle_iteration(db, emitter, it.id).await {
                eprintln!("[loop][reconcile] settle {} failed: {e}", it.id);
            }
        }
    }
    Ok(())
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
) -> Result<TickOutcome, LoopError> {
    let conn = &db.conn;
    let issue = loop_issue::Entity::find_by_id(issue_id)
        .one(conn)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
    if issue.status != IssueStatus::Running {
        return Ok(TickOutcome::Stop);
    }

    let config = effective_config(conn, &issue).await;

    let Some(worktree_folder_id) = issue.worktree_folder_id else {
        // No worktree yet (trigger sets it up before starting the driver). Can't
        // make progress; idle until a wake.
        eprintln!("[loop][driver] issue {issue_id} has no worktree folder; idling");
        return Ok(TickOutcome::Idle);
    };

    // Triage first: it decides the route the rest of the pipeline follows.
    if !has_live_triage(conn, issue_id).await? {
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
                agent_type: resolve_agent(&config, Stage::Triage),
                mode_id: None,
                config_values: Default::default(),
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
                    agent_type: resolve_agent(&config, item.stage),
                    mode_id: None,
                    config_values: Default::default(),
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

    // Read pipeline complete (tasks exist) → drive the write pipeline. A no-op
    // when there are no tasks yet (read stages still in flight), so it is safe
    // to call on every "frontier empty" tick.
    let dispatched = gates::drive_active_task(
        db,
        data_dir,
        spawner,
        emitter,
        &issue,
        &dag,
        &config,
        worktree_folder_id,
    )
    .await?;
    if dispatched {
        return Ok(TickOutcome::Dispatched);
    }

    // Write pipeline drained → finalize when every task is done (produce the
    // result artifact). A no-op until then.
    let finalized = gates::run_finalize(
        db,
        data_dir,
        spawner,
        emitter,
        &issue,
        &dag,
        &config,
        worktree_folder_id,
    )
    .await?;
    if finalized {
        return Ok(TickOutcome::Dispatched);
    }

    // Result produced → merge gate. With `auto_merge` on, signal the driver to
    // land it (the merge needs `&LoopEngine`). Otherwise idle: the human gate
    // awaits an explicit approve_merge (its inbox card arrives in Task 2.7).
    if config.auto_merge && dag.artifacts.iter().any(|a| a.kind == ArtifactKind::Result) {
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
        eprintln!(
            "[loop][triage] issue {} undecided after {attempts} triage attempt(s); re-dispatching",
            issue.id
        );
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
                agent_type: resolve_agent(config, Stage::Triage),
                mode_id: None,
                config_values: Default::default(),
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
    eprintln!(
        "[loop][triage] issue {} gave up after {attempts} triage attempts with no route; blocking",
        issue.id
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
    Ok(TickOutcome::Idle)
}

/// How often the driver re-ticks absent a wake, so the liveness reconcile runs.
/// A poll cadence for a real signal (connection liveness), not a cap on work.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

/// The per-issue driver task body: tick, then park on the wake `Notify` until a
/// completion (or external nudge) arrives. Exits when the issue leaves
/// `running`, deregistering itself from the engine's driver registry.
pub(crate) async fn run_driver(engine: Arc<LoopEngine>, issue_id: i32, wake: Arc<Notify>) {
    // Periodic liveness heartbeat: re-tick even without a wake so the reconcile
    // below catches iterations whose turn-complete event was missed or raced.
    let mut heartbeat = interval(RECONCILE_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await; // consume the immediate first fire
    loop {
        // DB-authoritative backstop before each tick: settle iterations whose
        // agent connection is gone (the event-driven settle alone can wedge).
        if let Err(e) =
            reconcile_orphaned_iterations(&engine.db, &engine.emitter, &engine.manager, issue_id)
                .await
        {
            eprintln!("[loop][driver] reconcile failed for issue {issue_id}: {e}");
        }
        match tick_once(
            &engine.db,
            &engine.data_dir,
            &engine.manager,
            &engine.emitter,
            issue_id,
        )
        .await
        {
            Ok(TickOutcome::Stop) => break,
            Ok(TickOutcome::AutoMerge) => {
                // Land the finalized work without a human gate. `merge_issue`
                // moves the issue to a terminal (or blocked) state, so re-tick
                // immediately — the next tick observes the new status and stops.
                // On error, fall through and park to avoid busy-spinning on a
                // transient failure (a later wake retries).
                match engine.merge_issue(issue_id).await {
                    Ok(()) => continue,
                    Err(e) => {
                        eprintln!("[loop][driver] auto-merge failed for issue {issue_id}: {e}");
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[loop][driver] tick failed for issue {issue_id}: {e}");
            }
        }
        // Park until an iteration settles (the completion watcher fires `wake`)
        // or the periodic heartbeat elapses (which runs the reconcile above).
        // `notify_one` buffers a permit, so a wake that races ahead is not lost.
        tokio::select! {
            _ = wake.notified() => {}
            _ = heartbeat.tick() => {}
        }
    }
    engine.deregister_driver(issue_id).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::error::AcpError;
    use crate::db::entities::loop_artifact::ArtifactKind;
    use crate::db::entities::loop_inbox_item::{self, InboxStatus};
    use crate::db::entities::loop_issue::IssuePriority;
    use crate::db::service::loop_service::{issue, space};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::loop_engine::dispatch::settle_iteration;
    use crate::loop_engine::ingest::ingest;
    use crate::loop_engine::transitions::cas_artifact_status;
    use crate::models::loops::IssueConfig;
    use async_trait::async_trait;
    use sea_orm::sea_query::Expr;
    use serde_json::json;
    use std::path::PathBuf;

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
            &IssueConfig::default(),
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

    #[tokio::test]
    async fn reconcile_settles_running_iteration_without_live_connection() {
        let (db, data_dir, issue_id) = setup().await;
        let spawner = StubSpawner;
        let mgr = ConnectionManager::new();
        // One tick dispatches triage → a running iteration with a conversation_id.
        tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id)
            .await
            .unwrap();
        let running = loop_iteration::Entity::find()
            .filter(loop_iteration::Column::IssueId.eq(issue_id))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(running.len(), 1, "triage dispatched and running");
        // No live connection exists for it → reconcile settles the orphan.
        reconcile_orphaned_iterations(&db, &EventEmitter::Noop, &mgr, issue_id)
            .await
            .unwrap();
        let it = loop_iteration::Entity::find_by_id(running[0].id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(it.status, IterationStatus::Succeeded);
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
        tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id)
            .await
            .unwrap();
        settle_running_triage_without_route(&db).await;

        // Tick 2: triage settled but undecided → re-dispatch (attempt 1).
        let out = tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id)
            .await
            .unwrap();
        assert_eq!(out, TickOutcome::Dispatched);
        settle_running_triage_without_route(&db).await;

        // Tick 3: attempts hit max → block + inbox card.
        let out = tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id)
            .await
            .unwrap();
        assert_eq!(out, TickOutcome::Idle);
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
            let _ = tick_once(db, data_dir, &spawner, &EventEmitter::Noop, issue_id)
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
        let (db, data_dir, issue_id) = setup().await;
        drive_through_read_pipeline(&db, &data_dir, issue_id, "full").await;

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
        let (db, data_dir, issue_id) = setup().await;
        drive_through_read_pipeline(&db, &data_dir, issue_id, "direct").await;

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
            tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id)
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
        tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id)
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
            tick_once(&db, &data_dir, &spawner, &EventEmitter::Noop, issue_id)
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
        };
        assert_eq!(ready_nodes(&dag, IssueRoute::Full)[0].stage, Stage::Plan);

        // Tasks exist → idle (M2.1 stops before implement).
        let dag = LoopDagView {
            artifacts: vec![
                root.clone(),
                mk(2, ArtifactKind::Requirement, ArtifactStatus::Done),
                mk(3, ArtifactKind::Design, ArtifactStatus::Done),
                mk(4, ArtifactKind::Task, ArtifactStatus::Pending),
            ],
            links: vec![],
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
        };
        assert_eq!(ready_nodes(&dag, IssueRoute::SkipDesign)[0].stage, Stage::Plan);
    }
}
