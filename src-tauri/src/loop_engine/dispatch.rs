//! Single-iteration dispatch (§4.3) + settlement (§4.9).
//!
//! Given a frontier decision (`DispatchInput`) chosen upstream by the driver,
//! [`dispatch_iteration`] runs the seven-step launch sequence:
//!
//! 1. resolve the issue's worktree path;
//! 2. assemble the briefing prompt + audit manifest;
//! 3. claim the DB-authoritative dispatch lease (a lost race → `Ok(None)`, no
//!    orphan conversation);
//! 4. mint the backing `kind=loop` conversation and link it to the lease;
//! 5. spawn the agent in the worktree, injecting the per-iteration capability
//!    token (turns on the codeg-mcp companion's loop tools);
//! 6. CAS the lease `queued → running`;
//! 7. send the briefing as the iteration's first prompt.
//!
//! [`settle_iteration`] finalizes a completed run: token accounting (§4.9), the
//! success CAS, and — when nothing was produced — the no-progress signal the
//! circuit breaker reads (enforced in M2.2).
//!
//! This module never decides *what* to dispatch; that is the driver's job
//! (Task 1.6). The agent spawn is abstracted behind [`LoopAgentSpawner`] so the
//! whole sequence is testable without launching a real agent subprocess.

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveEnum, ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::acp::error::AcpError;
use crate::acp::manager::ConnectionManager;
use crate::acp::types::PromptInputBlock;
use crate::commands::acp::build_session_runtime_env;
use crate::commands::conversations::get_folder_conversation_core;
use crate::db::entities::loop_inbox_item::InboxKind;
use crate::db::entities::loop_issue::{IssueStatus, PauseReason};
use crate::db::entities::loop_iteration::{self, IterationStatus, Stage};
use crate::db::entities::{loop_artifact, loop_issue};
use crate::db::service::conversation_service::create_loop;
use crate::db::service::folder_service;
use crate::db::service::loop_service::{inbox, iteration};
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::loops::{LoopChanged, LOOP_CHANGED_EVENT};
use crate::web::event_bridge::{emit_event, EventEmitter};

use crate::loop_engine::briefing::{assemble_briefing, BriefingOutput};
use crate::loop_engine::config_resolver::effective_config;
use crate::loop_engine::error::LoopError;
use crate::loop_engine::transitions::{
    cas_issue_status, cas_iteration_status, try_claim_iteration, IterationClaim,
};

/// Emit the coarse `loop://changed` event so every client refetches the issue's
/// DAG. The autonomous pipeline (dispatch + settle) is the engine's own write
/// path — distinct from the command layer's CRUD emits — so without this a
/// triggered issue would grow its DAG silently until something else refetched.
pub(crate) fn emit_changed(
    emitter: &EventEmitter,
    space_id: i32,
    issue_id: i32,
    subject_id: i32,
    kind: &str,
) {
    emit_event(
        emitter,
        LOOP_CHANGED_EVENT,
        LoopChanged {
            v: 1,
            space_id,
            issue_id: Some(issue_id),
            subject_kind: "iteration".to_string(),
            subject_id,
            kind: kind.to_string(),
        },
    );
}

/// Block an issue whose node burned `max_attempts` with no progress: CAS the
/// issue `running → blocked` (so the driver stops on its next tick) and file an
/// idempotent `no_progress:{node}` inbox card the human resolves via retry/cancel.
/// This is the shared no-progress terminal for the settle-time read-stage / abandon
/// breaker. The write pipeline files the same card kind and key shape for tasks,
/// so the two dedupe naturally if they ever land on the same implement node.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn block_issue_no_progress(
    db: &AppDatabase,
    emitter: &EventEmitter,
    space_id: i32,
    issue_id: i32,
    node_artifact_id: i32,
    iteration_id: Option<i32>,
    reason: &str,
    sig: &str,
    attempt: i32,
) -> Result<(), LoopError> {
    cas_issue_status(&db.conn, issue_id, IssueStatus::Running, IssueStatus::Blocked).await?;
    inbox::upsert_inbox(
        &db.conn,
        space_id,
        issue_id,
        iteration_id,
        InboxKind::Blocked,
        &format!("no_progress:{node_artifact_id}"),
        serde_json::json!({
            "v": 1,
            "node_artifact_id": node_artifact_id,
            "reason": reason,
            "failure_sig": sig,
            "attempt": attempt,
        }),
    )
    .await?;
    emit_changed(emitter, space_id, issue_id, issue_id, "blocked");
    Ok(())
}

/// The frontier decision the driver hands to dispatch: which iteration to run
/// for which issue / stage / target. Everything here is chosen upstream;
/// dispatch only executes it.
pub struct DispatchInput {
    pub space_id: i32,
    pub issue_id: i32,
    pub stage: Stage,
    pub target_artifact_id: Option<i32>,
    /// Review slot `[0, reviewer_count)`; `None` for non-review stages.
    pub slot_no: Option<i32>,
    pub attempt: i32,
    pub agent_type: AgentType,
    /// Startup mode for the spawned agent (per-reviewer override); `None` for
    /// stages that take the agent's own default.
    pub mode_id: Option<String>,
    /// Startup config values for the spawned agent (per-reviewer override);
    /// empty for stages that take no extra config.
    pub config_values: BTreeMap<String, String>,
    /// The issue's engine-created worktree folder (`folder.id`).
    pub worktree_folder_id: i32,
}

/// What a successful dispatch produced — enough for the driver to track the
/// live iteration and correlate its turn-complete event back to the lease.
pub struct DispatchHandle {
    pub iteration_id: i32,
    pub conversation_id: i32,
    pub connection_id: String,
    pub capability_token: String,
}

/// Outcome of settling a finished iteration.
pub struct SettleOutcome {
    pub iteration_id: i32,
    pub produced_artifact_ids: Vec<i32>,
    pub tokens_used: i64,
    /// `true` when the iteration produced at least one artifact.
    pub made_progress: bool,
}

/// How a settle resolves the iteration's terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleResolution {
    /// The agent turn completed normally (incl. an empty turn). → `Succeeded`.
    Completed,
    /// The backing connection died with no completed turn (reconcile `Missing`).
    /// → `Failed` + a no-progress signature so redispatch stays bounded by
    /// `max_attempts`. A do-nothing orphan must never be faked as success.
    Abandoned,
}

/// Abstraction over the `ConnectionManager` calls dispatch makes, so the
/// seven-step sequence is testable without spawning a real agent. Production
/// wires this to [`ConnectionManager`]; tests use a stub. Runtime-env assembly
/// lives inside the production impl (it touches settings + the filesystem) so
/// tests never run it.
#[async_trait]
pub trait LoopAgentSpawner: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn spawn_loop_agent(
        &self,
        db: &AppDatabase,
        data_dir: &Path,
        agent_type: AgentType,
        working_dir: String,
        emitter: EventEmitter,
        preferred_mode_id: Option<String>,
        preferred_config_values: BTreeMap<String, String>,
        capability_token: String,
    ) -> Result<String, AcpError>;

    async fn send_loop_prompt(
        &self,
        db: &AppDatabase,
        conn_id: &str,
        text: String,
        folder_id: i32,
        conversation_id: i32,
    ) -> Result<(), AcpError>;

    async fn disconnect_loop_agent(&self, conn_id: &str);
}

#[async_trait]
impl LoopAgentSpawner for ConnectionManager {
    #[allow(clippy::too_many_arguments)]
    async fn spawn_loop_agent(
        &self,
        db: &AppDatabase,
        data_dir: &Path,
        agent_type: AgentType,
        working_dir: String,
        emitter: EventEmitter,
        preferred_mode_id: Option<String>,
        preferred_config_values: BTreeMap<String, String>,
        capability_token: String,
    ) -> Result<String, AcpError> {
        let runtime_env = build_session_runtime_env(db, agent_type, None, data_dir).await?;
        self.spawn_agent(
            agent_type,
            Some(working_dir),
            None, // fresh session — loop iterations never resume
            runtime_env,
            "loop-engine".to_string(),
            emitter,
            preferred_mode_id,
            preferred_config_values,
            Some(capability_token),
        )
        .await
    }

    async fn send_loop_prompt(
        &self,
        db: &AppDatabase,
        conn_id: &str,
        text: String,
        folder_id: i32,
        conversation_id: i32,
    ) -> Result<(), AcpError> {
        self.send_prompt_linked(
            db,
            conn_id,
            vec![PromptInputBlock::Text { text }],
            Some(folder_id),
            Some(conversation_id),
            None,
        )
        .await
        .map(|_| ())
    }

    async fn disconnect_loop_agent(&self, conn_id: &str) {
        let _ = self.disconnect(conn_id).await;
    }
}

/// §4.3 single-iteration dispatch. Returns `Ok(None)` when the dispatch lease is
/// already held (lost the race — no conversation created), `Ok(Some(handle))`
/// on a launched iteration, and `Err` after marking the claimed lease failed +
/// filing a blocked inbox item.
pub async fn dispatch_iteration(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: EventEmitter,
    input: DispatchInput,
) -> Result<Option<DispatchHandle>, LoopError> {
    let conn = &db.conn;

    // Step 1: resolve the issue's worktree path.
    let folder = folder_service::get_folder_by_id(conn, input.worktree_folder_id)
        .await?
        .ok_or_else(|| {
            LoopError::NotFound(format!("worktree folder {}", input.worktree_folder_id))
        })?;
    let worktree_path = folder.path;

    let issue = loop_issue::Entity::find_by_id(input.issue_id)
        .one(conn)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("issue {}", input.issue_id)))?;

    // Step 2: assemble the briefing prompt + audit manifest.
    let briefing = assemble_briefing(conn, &issue, input.stage, input.target_artifact_id).await?;

    // Step 3: claim the dispatch lease (conversation attached afterwards). A
    // lost race surfaces as `Ok(None)` — the driver simply skips, no orphan.
    let capability_token = uuid::Uuid::new_v4().to_string();
    let iter = match try_claim_iteration(
        conn,
        IterationClaim {
            space_id: input.space_id,
            issue_id: input.issue_id,
            stage: input.stage,
            target_artifact_id: input.target_artifact_id,
            slot_no: input.slot_no,
            capability_token: capability_token.clone(),
            attempt: input.attempt,
        },
    )
    .await?
    {
        Some(iter) => iter,
        None => return Ok(None),
    };

    // From here the lease row exists: any failure must mark it failed and file
    // a blocked inbox item so the issue doesn't silently stall.
    match launch_claimed_iteration(
        db,
        data_dir,
        spawner,
        emitter.clone(),
        &input,
        &issue,
        &iter,
        &worktree_path,
        &capability_token,
        briefing,
    )
    .await
    {
        Ok(handle) => {
            // A new iteration is now running — surface it live so the DAG's
            // "executing now" highlight appears without waiting for settlement.
            emit_changed(
                &emitter,
                input.space_id,
                input.issue_id,
                handle.iteration_id,
                "dispatched",
            );
            Ok(Some(handle))
        }
        Err(e) => {
            fail_iteration(conn, &input, &iter, &e).await;
            Err(e)
        }
    }
}

/// Steps 4–7, isolated so the caller can run failure cleanup on any error.
#[allow(clippy::too_many_arguments)]
async fn launch_claimed_iteration(
    db: &AppDatabase,
    data_dir: &Path,
    spawner: &dyn LoopAgentSpawner,
    emitter: EventEmitter,
    input: &DispatchInput,
    issue: &loop_issue::Model,
    iter: &loop_iteration::Model,
    worktree_path: &str,
    capability_token: &str,
    briefing: BriefingOutput,
) -> Result<DispatchHandle, LoopError> {
    let conn = &db.conn;

    // Step 4: mint the backing kind=loop conversation, link it to the lease,
    // and stash the briefing manifest for audit.
    let title = Some(format!("{} · #{}", stage_title(input.stage), issue.seq_no));
    let conv = create_loop(conn, input.worktree_folder_id, input.agent_type, title, None).await?;

    let mut linked: loop_iteration::ActiveModel = iter.clone().into();
    linked.conversation_id = Set(Some(conv.id));
    linked.context_manifest = Set(Some(briefing.manifest.to_string()));
    linked.update(conn).await?;

    // Step 5: spawn the agent in the worktree, injecting the capability token.
    let conn_id = spawner
        .spawn_loop_agent(
            db,
            data_dir,
            input.agent_type,
            worktree_path.to_string(),
            emitter,
            input.mode_id.clone(),
            input.config_values.clone(),
            capability_token.to_string(),
        )
        .await
        .map_err(|e| LoopError::Acp(e.to_string()))?;

    // Steps 6–7: flip the lease to running, then send the briefing. Any failure
    // after the spawn must also tear down the live connection we just created.
    if let Err(e) = finish_launch(
        db,
        spawner,
        iter.id,
        &conn_id,
        conv.id,
        input.worktree_folder_id,
        briefing.text,
    )
    .await
    {
        spawner.disconnect_loop_agent(&conn_id).await;
        return Err(e);
    }

    Ok(DispatchHandle {
        iteration_id: iter.id,
        conversation_id: conv.id,
        connection_id: conn_id,
        capability_token: capability_token.to_string(),
    })
}

/// Step 6 (CAS `queued → running` + stamp `started_at`) and step 7 (deliver the
/// briefing as the iteration's first prompt).
async fn finish_launch(
    db: &AppDatabase,
    spawner: &dyn LoopAgentSpawner,
    iteration_id: i32,
    conn_id: &str,
    conversation_id: i32,
    folder_id: i32,
    briefing_text: String,
) -> Result<(), LoopError> {
    let conn = &db.conn;
    let swapped = cas_iteration_status(
        conn,
        iteration_id,
        IterationStatus::Queued,
        IterationStatus::Running,
    )
    .await?;
    if !swapped {
        // Cancelled/changed between claim and spawn — abort this launch.
        return Err(LoopError::Conflict);
    }
    loop_iteration::Entity::update_many()
        .col_expr(loop_iteration::Column::StartedAt, Expr::value(Utc::now()))
        .filter(loop_iteration::Column::Id.eq(iteration_id))
        .exec(conn)
        .await?;

    spawner
        .send_loop_prompt(db, conn_id, briefing_text, folder_id, conversation_id)
        .await
        .map_err(|e| LoopError::Acp(e.to_string()))?;
    Ok(())
}

/// Best-effort failure cleanup for a claimed-but-not-launched iteration: mark
/// the lease failed, stamp `ended_at`, and surface a blocked inbox item.
async fn fail_iteration(
    conn: &sea_orm::DatabaseConnection,
    input: &DispatchInput,
    iter: &loop_iteration::Model,
    err: &LoopError,
) {
    // The lease may be in `queued` (spawn never ran) or `running` (post-spawn
    // failure); fail it from whichever it holds.
    let _ = cas_iteration_status(
        conn,
        iter.id,
        IterationStatus::Queued,
        IterationStatus::Failed,
    )
    .await;
    let _ = cas_iteration_status(
        conn,
        iter.id,
        IterationStatus::Running,
        IterationStatus::Failed,
    )
    .await;
    let _ = loop_iteration::Entity::update_many()
        .col_expr(loop_iteration::Column::EndedAt, Expr::value(Utc::now()))
        .filter(loop_iteration::Column::Id.eq(iter.id))
        .exec(conn)
        .await;
    let _ = inbox::upsert_inbox(
        conn,
        input.space_id,
        input.issue_id,
        Some(iter.id),
        InboxKind::Blocked,
        &format!("dispatch_failed:{}", iter.id),
        serde_json::json!({
            "stage": input.stage.to_value(),
            "error": err.to_string(),
        }),
    )
    .await;
}

/// §4.9 settlement: finalize a completed iteration. Re-parses the session file
/// for token usage, succeeds the lease, and — when the run produced nothing —
/// bumps the target node's rework counter + records a failure signature for the
/// no-progress breaker.
pub async fn settle_iteration(
    db: &AppDatabase,
    emitter: &EventEmitter,
    iteration_id: i32,
) -> Result<SettleOutcome, LoopError> {
    settle_iteration_as(db, emitter, iteration_id, SettleResolution::Completed).await
}

/// Settle with an explicit terminal resolution. `Completed` succeeds the lease
/// (the normal turn-complete path); `Abandoned` fails it (reconcile of a dead
/// connection with no completed turn) and records a no-progress signature so
/// redispatch stays bounded by `max_attempts`.
pub async fn settle_iteration_as(
    db: &AppDatabase,
    emitter: &EventEmitter,
    iteration_id: i32,
    resolution: SettleResolution,
) -> Result<SettleOutcome, LoopError> {
    let conn = &db.conn;
    let iter = iteration::get_iteration(conn, iteration_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("iteration {iteration_id}")))?;

    // §4.9 token settlement: re-parse the session file for usage. A missing or
    // not-yet-written session file settles to 0 rather than failing.
    let tokens_used = match iter.conversation_id {
        Some(conv_id) => match get_folder_conversation_core(conn, conv_id).await {
            Ok((detail, _)) => detail
                .session_stats
                .and_then(|s| s.total_tokens)
                .unwrap_or(0) as i64,
            Err(_) => 0,
        },
        None => 0,
    };

    // Write the iteration's token total + accumulate into the issue.
    let mut am: loop_iteration::ActiveModel = iter.clone().into();
    am.tokens_used = Set(tokens_used);
    am.ended_at = Set(Some(Utc::now()));
    am.update(conn).await?;
    if tokens_used > 0 {
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::TokenUsed,
                Expr::col(loop_issue::Column::TokenUsed).add(tokens_used),
            )
            .filter(loop_issue::Column::Id.eq(iter.issue_id))
            .exec(conn)
            .await?;
    }

    // Which artifacts did this iteration produce?
    let produced_artifact_ids: Vec<i32> = loop_artifact::Entity::find()
        .filter(loop_artifact::Column::ProducedByIterationId.eq(iteration_id))
        .all(conn)
        .await?
        .into_iter()
        .map(|a| a.id)
        .collect();

    // Settle the lease (idempotent CAS). A normal completion succeeds; an
    // abandoned orphan (dead connection, no completed turn) fails — it must never
    // be faked as success.
    let terminal = match resolution {
        SettleResolution::Completed => IterationStatus::Succeeded,
        SettleResolution::Abandoned => IterationStatus::Failed,
    };
    cas_iteration_status(conn, iteration_id, IterationStatus::Running, terminal).await?;

    let made_progress = !produced_artifact_ids.is_empty();
    let abandoned = resolution == SettleResolution::Abandoned;
    // Bump the target node's rework counter + record a failure signature when the
    // run made no progress, or was abandoned (dead connection) — so redispatch is
    // bounded by the breaker. Implement measures progress by the worktree diff, so
    // its counter is owned by the gates checkpoint, not this artifact-count
    // heuristic — skip the no-progress bump for it, but still bump on abandon
    // (no checkpoint runs on a dead connection, so nothing else would bound it).
    if abandoned || (!made_progress && iter.stage != Stage::Implement) {
        if let Some(target) = iter.target_artifact_id {
            // No output → bump the node rework counter + record a failure
            // signature for the no-progress breaker.
            let sig = if abandoned {
                format!("abandoned:{}", iter.stage.to_value())
            } else {
                format!("no_artifacts:{}", iter.stage.to_value())
            };
            loop_artifact::Entity::update_many()
                .col_expr(
                    loop_artifact::Column::Attempt,
                    Expr::col(loop_artifact::Column::Attempt).add(1),
                )
                .col_expr(
                    loop_artifact::Column::LastFailureSig,
                    Expr::value(sig.clone()),
                )
                .col_expr(loop_artifact::Column::UpdatedAt, Expr::value(Utc::now()))
                .filter(loop_artifact::Column::Id.eq(target))
                .exec(conn)
                .await?;

            // No-progress breaker — the read-stage / abandon analogue of the write
            // pipeline's `record_rework`. The node lease guarantees no other
            // iteration bumped this node between our dispatch and now, so its new
            // attempt is exactly `iter.attempt + 1`. Once that reaches the issue's
            // `max_attempts` (0 = unlimited → no breaker), stop redispatching:
            // block the issue and file a `no_progress:{node}` card. Deliberately
            // settle-time, not dispatch-time — a human "retry" does not reset node
            // attempts, so gating *before* dispatch would make retry a no-op;
            // gating *after* lets each retry burn one real attempt before the
            // breaker re-trips, matching the write pipeline's retry contract.
            let new_attempt = iter.attempt + 1;
            if let Some(issue_row) = loop_issue::Entity::find_by_id(iter.issue_id).one(conn).await? {
                let max = effective_config(conn, &issue_row).await.max_attempts as i32;
                if max > 0 && new_attempt >= max {
                    block_issue_no_progress(
                        db,
                        emitter,
                        iter.space_id,
                        iter.issue_id,
                        target,
                        Some(iteration_id),
                        "max_attempts",
                        &sig,
                        new_attempt,
                    )
                    .await?;
                }
            }
        }
    }

    // Issue-level budget breaker: this iteration's tokens have now accumulated,
    // so re-evaluate whether the issue has crossed its budget.
    trip_budget_if_exhausted(conn, iter.issue_id, iteration_id).await?;

    // The iteration's outputs (new artifacts, route, token totals) have landed —
    // tell every client to refetch so the DAG grows in real time.
    emit_changed(
        emitter,
        iter.space_id,
        iter.issue_id,
        iteration_id,
        "settled",
    );

    Ok(SettleOutcome {
        iteration_id,
        produced_artifact_ids,
        tokens_used,
        made_progress,
    })
}

/// Issue-level budget circuit breaker (§4.10). Once accumulated `token_used`
/// crosses the issue's `token_budget` (`NULL` = unlimited, the default — no
/// artificial cap), CAS the issue `running → paused`, stamp `pause_reason =
/// budget`, and file a `budget_exhausted` card. The per-issue driver then stops
/// dispatching on its next tick (status no longer `running`).
///
/// Idempotent: the CAS only fires on the `running → paused` edge, and the inbox
/// upsert dedupes on `(issue, budget_exhausted, budget:{id})`.
async fn trip_budget_if_exhausted(
    conn: &sea_orm::DatabaseConnection,
    issue_id: i32,
    iteration_id: i32,
) -> Result<(), LoopError> {
    let Some(issue) = loop_issue::Entity::find_by_id(issue_id).one(conn).await? else {
        return Ok(());
    };
    let Some(budget) = issue.token_budget else {
        return Ok(());
    };
    if issue.token_used <= budget {
        return Ok(());
    }
    if cas_issue_status(conn, issue.id, IssueStatus::Running, IssueStatus::Paused).await? {
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::PauseReason,
                Expr::value(PauseReason::Budget.to_value()),
            )
            .filter(loop_issue::Column::Id.eq(issue.id))
            .exec(conn)
            .await?;
        inbox::upsert_inbox(
            conn,
            issue.space_id,
            issue.id,
            Some(iteration_id),
            InboxKind::BudgetExhausted,
            &format!("budget:{}", issue.id),
            serde_json::json!({
                "token_used": issue.token_used,
                "token_budget": budget,
            }),
        )
        .await?;
    }
    Ok(())
}

/// Human-facing stage label for the loop conversation title.
fn stage_title(stage: Stage) -> &'static str {
    match stage {
        Stage::Triage => "Triage",
        Stage::Refine => "Refine",
        Stage::Design => "Design",
        Stage::Plan => "Plan",
        Stage::Implement => "Implement",
        Stage::Review => "Review",
        Stage::Finalize => "Finalize",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus};
    use crate::db::entities::loop_artifact_revision::ActorKind;
    use crate::db::entities::loop_inbox_item::InboxStatus;
    use crate::db::entities::conversation::ConversationKind;
    use crate::db::entities::loop_issue::IssuePriority;
    use crate::db::service::loop_service::{artifact, inbox, issue, space};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::models::loops::IssueConfig;
    use tokio::sync::Mutex as AsyncMutex;

    /// Records every call so tests can assert dispatch wired the right values.
    #[derive(Default)]
    struct StubCalls {
        /// (agent_type, working_dir, capability_token) per spawn.
        spawned: Vec<(AgentType, String, String)>,
        /// (conn_id, text, folder_id, conversation_id) per prompt.
        prompts: Vec<(String, String, i32, i32)>,
        disconnects: Vec<String>,
    }

    #[derive(Default)]
    struct StubSpawner {
        fail_spawn: bool,
        fail_prompt: bool,
        calls: AsyncMutex<StubCalls>,
    }

    #[async_trait]
    impl LoopAgentSpawner for StubSpawner {
        async fn spawn_loop_agent(
            &self,
            _db: &AppDatabase,
            _data_dir: &Path,
            agent_type: AgentType,
            working_dir: String,
            _emitter: EventEmitter,
            _preferred_mode_id: Option<String>,
            _preferred_config_values: BTreeMap<String, String>,
            capability_token: String,
        ) -> Result<String, AcpError> {
            if self.fail_spawn {
                return Err(AcpError::protocol("stub spawn failure"));
            }
            self.calls
                .lock()
                .await
                .spawned
                .push((agent_type, working_dir, capability_token));
            Ok("loop-conn-1".to_string())
        }

        async fn send_loop_prompt(
            &self,
            _db: &AppDatabase,
            conn_id: &str,
            text: String,
            folder_id: i32,
            conversation_id: i32,
        ) -> Result<(), AcpError> {
            if self.fail_prompt {
                return Err(AcpError::protocol("stub prompt failure"));
            }
            self.calls
                .lock()
                .await
                .prompts
                .push((conn_id.to_string(), text, folder_id, conversation_id));
            Ok(())
        }

        async fn disconnect_loop_agent(&self, conn_id: &str) {
            self.calls.lock().await.disconnects.push(conn_id.to_string());
        }
    }

    /// Seed a space + a triggered issue whose worktree folder is `folder_id`.
    /// Returns (db, data_dir, space_id, issue_id, worktree_folder_id).
    async fn seed() -> (crate::db::AppDatabase, std::path::PathBuf, i32, i32, i32) {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/loop-wt").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &db.conn,
            space.id,
            "Fix it",
            "body",
            IssuePriority::Medium,
            &IssueConfig::default(),
        )
        .await
        .unwrap();
        // Point the issue at its worktree folder (same folder row is fine here —
        // dispatch only reads `folder.path`).
        loop_issue::Entity::update_many()
            .col_expr(loop_issue::Column::WorktreeFolderId, Expr::value(folder_id))
            .filter(loop_issue::Column::Id.eq(issue.row.id))
            .exec(&db.conn)
            .await
            .unwrap();
        (db, std::path::PathBuf::from("/tmp/data"), space.id, issue.row.id, folder_id)
    }

    fn input(space_id: i32, issue_id: i32, stage: Stage, folder_id: i32) -> DispatchInput {
        DispatchInput {
            space_id,
            issue_id,
            stage,
            target_artifact_id: None,
            slot_no: None,
            attempt: 0,
            agent_type: AgentType::ClaudeCode,
            mode_id: None,
            config_values: Default::default(),
            worktree_folder_id: folder_id,
        }
    }

    async fn count_loop_conversations(db: &crate::db::AppDatabase) -> usize {
        use crate::db::entities::conversation;
        conversation::Entity::find()
            .filter(conversation::Column::Kind.eq(ConversationKind::Loop))
            .all(&db.conn)
            .await
            .unwrap()
            .len()
    }

    #[tokio::test]
    async fn dispatch_claims_creates_conversation_and_runs() {
        let (db, data_dir, space_id, issue_id, folder_id) = seed().await;
        let spawner = StubSpawner::default();

        let handle = dispatch_iteration(
            &db,
            &data_dir,
            &spawner,
            EventEmitter::Noop,
            input(space_id, issue_id, Stage::Triage, folder_id),
        )
        .await
        .unwrap()
        .expect("a fresh lease is claimed");

        // The lease is running, linked to a kind=loop conversation.
        let iter = iteration::get_iteration(&db.conn, handle.iteration_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(iter.status, IterationStatus::Running);
        assert_eq!(iter.conversation_id, Some(handle.conversation_id));
        assert!(iter.started_at.is_some());
        assert!(iter.context_manifest.is_some(), "briefing manifest stashed");
        assert_eq!(count_loop_conversations(&db).await, 1);

        // The spawn received the minted capability token (not a guessable id),
        // and the briefing was delivered to the right conversation.
        let calls = spawner.calls.lock().await;
        assert_eq!(calls.spawned.len(), 1);
        assert_eq!(calls.spawned[0].0, AgentType::ClaudeCode);
        assert_eq!(calls.spawned[0].1, "/tmp/loop-wt");
        assert_eq!(calls.spawned[0].2, handle.capability_token);
        assert_eq!(calls.spawned[0].2, iter.capability_token);
        assert_eq!(calls.prompts.len(), 1);
        assert_eq!(calls.prompts[0].0, handle.connection_id);
        assert_eq!(calls.prompts[0].3, handle.conversation_id);
        assert!(!calls.prompts[0].1.is_empty(), "briefing text is non-empty");
        assert!(calls.disconnects.is_empty());
    }

    #[tokio::test]
    async fn dispatch_lost_lease_returns_none_without_side_effects() {
        let (db, data_dir, space_id, issue_id, folder_id) = seed().await;

        // A node to advance. The `uniq_active_node(target, stage)` lease guards
        // by (target, stage), so we dispatch against a real target — a NULL
        // target wouldn't collide (SQLite treats NULLs as distinct).
        let target = artifact::create_artifact(
            &db.conn,
            space_id,
            issue_id,
            ArtifactKind::Requirement,
            "R",
            ArtifactStatus::Done,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();

        // Pre-claim the same (target, stage) lease so the dispatch loses the race.
        try_claim_iteration(
            &db.conn,
            IterationClaim {
                space_id,
                issue_id,
                stage: Stage::Design,
                target_artifact_id: Some(target.id),
                slot_no: None,
                capability_token: "held".into(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .expect("first claim wins");

        let mut lost = input(space_id, issue_id, Stage::Design, folder_id);
        lost.target_artifact_id = Some(target.id);

        let spawner = StubSpawner::default();
        let result = dispatch_iteration(&db, &data_dir, &spawner, EventEmitter::Noop, lost)
            .await
            .unwrap();

        assert!(result.is_none(), "lost race returns None");
        assert_eq!(count_loop_conversations(&db).await, 0, "no orphan conversation");
        assert!(spawner.calls.lock().await.spawned.is_empty(), "never spawned");
    }

    #[tokio::test]
    async fn dispatch_spawn_failure_marks_failed_and_files_inbox() {
        let (db, data_dir, space_id, issue_id, folder_id) = seed().await;
        let spawner = StubSpawner {
            fail_spawn: true,
            ..Default::default()
        };

        let err = dispatch_iteration(
            &db,
            &data_dir,
            &spawner,
            EventEmitter::Noop,
            input(space_id, issue_id, Stage::Triage, folder_id),
        )
        .await;
        assert!(err.is_err(), "spawn failure propagates");

        // The claimed lease is marked failed + ended, and a blocked inbox item
        // surfaces the stall.
        use crate::db::entities::loop_iteration as li;
        let iters = li::Entity::find()
            .filter(li::Column::IssueId.eq(issue_id))
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(iters.len(), 1);
        assert_eq!(iters[0].status, IterationStatus::Failed);
        assert!(iters[0].ended_at.is_some());

        let items = inbox::list_inbox(&db.conn, space_id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, InboxKind::Blocked);
    }

    #[tokio::test]
    async fn dispatch_prompt_failure_disconnects_the_connection() {
        let (db, data_dir, space_id, issue_id, folder_id) = seed().await;
        let spawner = StubSpawner {
            fail_prompt: true,
            ..Default::default()
        };

        let err = dispatch_iteration(
            &db,
            &data_dir,
            &spawner,
            EventEmitter::Noop,
            input(space_id, issue_id, Stage::Triage, folder_id),
        )
        .await;
        assert!(err.is_err());
        // The spawn succeeded but the prompt failed → the live connection is torn
        // down rather than leaked.
        assert_eq!(
            spawner.calls.lock().await.disconnects,
            vec!["loop-conn-1".to_string()]
        );
    }

    #[tokio::test]
    async fn settle_with_produced_artifact_succeeds() {
        let (db, _data_dir, space_id, issue_id, _folder_id) = seed().await;
        // A running iteration that produced a requirement artifact.
        let iter = try_claim_iteration(
            &db.conn,
            IterationClaim {
                space_id,
                issue_id,
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
        cas_iteration_status(
            &db.conn,
            iter.id,
            IterationStatus::Queued,
            IterationStatus::Running,
        )
        .await
        .unwrap();
        let produced = artifact::create_artifact(
            &db.conn,
            space_id,
            issue_id,
            ArtifactKind::Requirement,
            "R1",
            ArtifactStatus::Done,
            ActorKind::Agent,
            Some(iter.id),
        )
        .await
        .unwrap();

        let outcome = settle_iteration(&db, &EventEmitter::Noop, iter.id).await.unwrap();
        assert!(outcome.made_progress);
        assert_eq!(outcome.produced_artifact_ids, vec![produced.id]);

        let settled = iteration::get_iteration(&db.conn, iter.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.status, IterationStatus::Succeeded);
        assert!(settled.ended_at.is_some());
    }

    #[tokio::test]
    async fn settle_without_artifact_bumps_node_attempt() {
        let (db, _data_dir, space_id, issue_id, _folder_id) = seed().await;
        // An artifact-producing stage (implement is exempt — its progress is the
        // worktree checkpoint, exercised in the gates tests). A design iteration
        // that produces nothing bumps its target requirement's rework counter.
        let target = artifact::create_artifact(
            &db.conn,
            space_id,
            issue_id,
            ArtifactKind::Requirement,
            "R1",
            ArtifactStatus::Done,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        let iter = try_claim_iteration(
            &db.conn,
            IterationClaim {
                space_id,
                issue_id,
                stage: Stage::Design,
                target_artifact_id: Some(target.id),
                slot_no: None,
                capability_token: "t".into(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .unwrap();
        cas_iteration_status(
            &db.conn,
            iter.id,
            IterationStatus::Queued,
            IterationStatus::Running,
        )
        .await
        .unwrap();

        let outcome = settle_iteration(&db, &EventEmitter::Noop, iter.id).await.unwrap();
        assert!(!outcome.made_progress);
        assert!(outcome.produced_artifact_ids.is_empty());

        let node = loop_artifact::Entity::find_by_id(target.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(node.attempt, 1, "node rework counter bumped");
        assert_eq!(node.last_failure_sig.as_deref(), Some("no_artifacts:design"));
    }
}
