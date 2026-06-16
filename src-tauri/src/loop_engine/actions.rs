//! Human-driven engine actions (§4.6): trigger / pause / resume / cancel.
//!
//! These are the only points where a person steers a loop; everything else is
//! engine-autonomous. Each is a small, DB-authoritative state transition layered
//! on the driver registry:
//! - **trigger**: pending → running; create the issue worktree; start a driver.
//! - **pause**: running → paused(manual); stop the driver. In-flight agents are
//!   left alive — a pause halts *new* dispatch, it does not kill running work.
//! - **resume**: paused → running; start a fresh driver.
//! - **cancel**: → cancelled; stop the driver, kill every in-flight iteration's
//!   agent subprocess, invalidate its capability token (so the host rejects late
//!   submissions), and remove the worktree.
//!
//! The **merge gate** (§4.10) also lives here: [`LoopEngine::merge_issue`] lands
//! a finalized issue's loop branch onto its base branch under a per-repo lock,
//! with a stale-base check; a clean landing closes the issue, any fault blocks it
//! with an inbox card.
//!
//! Every transition is guarded: a miss (the issue is not in the expected source
//! state) surfaces as [`LoopError::Conflict`], which the frontend retries. The
//! merge gate is the exception — it is idempotent (already-`done` → `Ok`) and
//! returns the non-retryable [`LoopError::NotMergeable`] for other non-mergeable
//! states; see [`LoopEngine::merge_issue`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveEnum, ColumnTrait, EntityTrait, QueryFilter};

use crate::db::entities::loop_artifact::{self, ArtifactKind, ArtifactStatus};
use crate::db::entities::loop_artifact_revision::ActorKind;
use crate::db::entities::loop_inbox_item::{self, InboxKind, InboxStatus};
use crate::db::entities::loop_issue::{self, IssueStatus, PauseReason};
use crate::db::entities::loop_iteration::{self, IterationStatus};
use crate::db::service::folder_service;
use crate::db::service::loop_service::{artifact, inbox, issue, space};
use crate::models::loops::{LoopChanged, LOOP_CHANGED_EVENT};
use crate::web::event_bridge::emit_event;

use crate::loop_engine::transitions::{cas_artifact_status, cas_issue_status};
use crate::loop_engine::worktree::{self, MergeOutcome};
use crate::loop_engine::{LoopEngine, LoopError};

impl LoopEngine {
    /// Trigger a pending issue: create its worktree, flip it running, and start
    /// the driver. The worktree is created *before* the status flip so a non-git
    /// repo (or any git failure) leaves the issue cleanly `pending`, retryable.
    pub async fn trigger_issue(self: &Arc<Self>, issue_id: i32) -> Result<(), LoopError> {
        let issue = issue::get_issue(&self.db.conn, issue_id)
            .await?
            .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
        if issue.status != IssueStatus::Pending {
            return Err(LoopError::Conflict);
        }
        // Validates the space repo is git, creates the worktree + branch, and
        // records the merge base on the issue (idempotent).
        worktree::ensure_worktree(&self.db.conn, &self.data_dir, issue_id).await?;
        if !cas_issue_status(
            &self.db.conn,
            issue_id,
            IssueStatus::Pending,
            IssueStatus::Running,
        )
        .await?
        {
            return Err(LoopError::Conflict);
        }
        self.start_issue(issue_id).await;
        Ok(())
    }

    /// Pause a running issue: halt new dispatch without killing in-flight agents.
    /// `stop_issue` removes the driver from the registry synchronously, so a
    /// follow-up resume always spawns a fresh driver (no handoff race).
    pub async fn pause_issue(&self, issue_id: i32) -> Result<(), LoopError> {
        let res = loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Status,
                Expr::value(IssueStatus::Paused.to_value()),
            )
            .col_expr(
                loop_issue::Column::PauseReason,
                Expr::value(PauseReason::Manual.to_value()),
            )
            .col_expr(loop_issue::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(loop_issue::Column::Id.eq(issue_id))
            .filter(loop_issue::Column::Status.eq(IssueStatus::Running))
            .exec(&self.db.conn)
            .await?;
        if res.rows_affected != 1 {
            return Err(LoopError::Conflict);
        }
        self.stop_issue(issue_id).await;
        Ok(())
    }

    /// Resume a paused issue: clear the pause reason and start a fresh driver,
    /// which re-evaluates the frontier (picking up any progress made while the
    /// in-flight iteration finished during the pause).
    pub async fn resume_issue(self: &Arc<Self>, issue_id: i32) -> Result<(), LoopError> {
        let res = loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Status,
                Expr::value(IssueStatus::Running.to_value()),
            )
            .col_expr(
                loop_issue::Column::PauseReason,
                Expr::value(Option::<String>::None),
            )
            .col_expr(loop_issue::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(loop_issue::Column::Id.eq(issue_id))
            .filter(loop_issue::Column::Status.eq(IssueStatus::Paused))
            .exec(&self.db.conn)
            .await?;
        if res.rows_affected != 1 {
            return Err(LoopError::Conflict);
        }
        self.start_issue(issue_id).await;
        Ok(())
    }

    /// Cancel an issue from any non-terminal state: close it, stop the driver,
    /// invalidate in-flight tokens, and remove the worktree.
    pub async fn cancel_issue(&self, issue_id: i32) -> Result<(), LoopError> {
        let now = Utc::now();
        let res = loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Status,
                Expr::value(IssueStatus::Cancelled.to_value()),
            )
            .col_expr(loop_issue::Column::EndedAt, Expr::value(now))
            .col_expr(loop_issue::Column::UpdatedAt, Expr::value(now))
            .filter(loop_issue::Column::Id.eq(issue_id))
            .filter(loop_issue::Column::Status.is_in([
                IssueStatus::Pending,
                IssueStatus::Running,
                IssueStatus::Paused,
                IssueStatus::Blocked,
            ]))
            .exec(&self.db.conn)
            .await?;
        if res.rows_affected != 1 {
            return Err(LoopError::Conflict);
        }
        // Stop the driver, kill the agent processes, then invalidate every
        // in-flight iteration: marking them cancelled releases their leases AND
        // makes the host reject any late capability-token submission (ingest
        // requires a `running` iteration). Killing precedes the worktree removal
        // so no agent is still writing into the tree as it is torn down.
        self.stop_issue(issue_id).await;
        self.kill_in_flight_agents(issue_id).await;
        loop_iteration::Entity::update_many()
            .col_expr(
                loop_iteration::Column::Status,
                Expr::value(IterationStatus::Cancelled.to_value()),
            )
            .col_expr(loop_iteration::Column::EndedAt, Expr::value(now))
            .filter(loop_iteration::Column::IssueId.eq(issue_id))
            .filter(
                loop_iteration::Column::Status
                    .is_in([IterationStatus::Queued, IterationStatus::Running]),
            )
            .exec(&self.db.conn)
            .await?;
        self.remove_issue_worktree(issue_id).await;
        Ok(())
    }

    /// Best-effort removal of an issue's git worktree (directory + admin entry).
    /// The hidden folder row and its iteration conversations are kept for audit;
    /// a cancelled issue's driver never restarts, so a stale `worktree_folder_id`
    /// is never read again. Any failure is logged, not fatal — the cancel's DB
    /// closure already succeeded.
    async fn remove_issue_worktree(&self, issue_id: i32) {
        let conn = &self.db.conn;
        let Ok(Some(issue)) = issue::get_issue(conn, issue_id).await else {
            return;
        };
        let Some(folder_id) = issue.worktree_folder_id else {
            return;
        };
        let Ok(Some(folder)) = folder_service::get_folder_by_id(conn, folder_id).await else {
            return;
        };
        if !Path::new(&folder.path).exists() {
            return;
        }
        let Ok(Some(space_row)) = space::get_space(conn, issue.space_id).await else {
            return;
        };
        let Ok(Some(repo)) = folder_service::get_folder_by_id(conn, space_row.folder_id).await
        else {
            return;
        };
        if let Err(e) =
            worktree::remove_worktree(Path::new(&repo.path), Path::new(&folder.path)).await
        {
            tracing::warn!(path = %folder.path, error = %e, "cancel: remove worktree failed");
        }
        // Also remove any per-task / integrate worktrees of a parallel issue. Keep
        // their branches for audit — cancel is not a permanent delete.
        let _ =
            worktree::remove_issue_subtree(Path::new(&repo.path), Path::new(&folder.path), false)
                .await;
    }

    /// Tear down the OS processes of an issue's in-flight iterations. Each live
    /// iteration's `conversation_id` resolves to its agent connection;
    /// `disconnect` sends the connection its shutdown command, reaping the child.
    /// Best-effort: a connection that already exited just isn't found. Reads the
    /// iteration rows directly (independent of the subsequent cancel CAS).
    async fn kill_in_flight_agents(&self, issue_id: i32) {
        let in_flight = match loop_iteration::Entity::find()
            .filter(loop_iteration::Column::IssueId.eq(issue_id))
            .filter(
                loop_iteration::Column::Status
                    .is_in([IterationStatus::Queued, IterationStatus::Running]),
            )
            .all(&self.db.conn)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "cancel: load in-flight iterations failed");
                return;
            }
        };
        for it in in_flight {
            let Some(cid) = it.conversation_id else {
                continue;
            };
            if let Some(conn_id) = self.manager.find_connection_by_conversation_id(cid).await {
                if let Err(e) = self.manager.disconnect(&conn_id).await {
                    tracing::warn!(conn_id = %conn_id, error = %e, "cancel: disconnect failed");
                }
            }
        }
    }

    /// Land a finalized issue's work onto its base branch — the merge gate
    /// (§4.10). Invoked by `approve_merge` (the human gate) or the driver
    /// (auto-merge); both take the same per-repo lock and run the same stale-base
    /// checks. A clean landing closes the issue (`done`) and removes its
    /// worktree; any fault (conflict / dirty base / failed re-validation / missing
    /// base) blocks the issue with an inbox card naming the cause AND returns a
    /// [`LoopError::MergeFailed`] carrying the reason — never a silent success that
    /// would leave the issue stuck "running" with no visible explanation.
    ///
    /// **Idempotent and race-free.** Preconditions are evaluated *under* the
    /// per-repo lock (not before it), so two actors — the human gate and the
    /// driver's auto-merge, or two clicks across surfaces — cannot both pass the
    /// gate and race the landing. A second call after the issue is already `done`
    /// (a concurrent actor merged it) returns `Ok(())` and re-emits `merged`,
    /// rather than the misleading `Conflict`/"retry". Any other non-`running` or
    /// no-`result` state returns the non-retryable [`LoopError::NotMergeable`] and
    /// emits a resync so a stale "running" view refetches the true status.
    pub async fn merge_issue(&self, issue_id: i32) -> Result<(), LoopError> {
        let conn = &self.db.conn;

        // Resolve the base repo path first — ONLY to choose which per-repo lock to
        // take. The authoritative precondition check happens after the lock (below),
        // so this pre-lock read cannot cause a TOCTOU. The repo path is immutable
        // for a space (folder paths have no mutation path; `space.folder_id` is
        // set once), so both actors derive the same lock key.
        let issue_probe = issue::get_issue(conn, issue_id)
            .await?
            .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
        let space_row = space::get_space(conn, issue_probe.space_id)
            .await?
            .ok_or_else(|| LoopError::NotFound(format!("space {}", issue_probe.space_id)))?;
        let repo = folder_service::get_folder_by_id(conn, space_row.folder_id)
            .await?
            .ok_or(LoopError::Detached)?;
        let repo_path = PathBuf::from(&repo.path);

        // Serialize merges per base repo, THEN evaluate preconditions under the
        // lock: two issues sharing a repo must not race their --no-ff landings, and
        // two actors on the same issue must collapse to one effective merge.
        let lock = self.repo_merge_lock(&repo_path).await;
        let _guard = lock.lock().await;

        // Authoritative re-read under the lock.
        let issue = issue::get_issue(conn, issue_id)
            .await?
            .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;

        // Idempotent: a concurrent actor (driver auto-merge or another click)
        // already landed and closed this issue. `done` is written ONLY by the
        // landing below and is terminal, so it unambiguously means "merged".
        // Report success and re-emit so a stale view converges — never "retry".
        if issue.status == IssueStatus::Done {
            self.emit_changed(issue.space_id, issue_id, "merged");
            return Ok(());
        }
        // Not mergeable: any other non-running state (blocked / cancelled / paused
        // / pending), or the live result has not passed integration (D6) — finalize
        // produced no result, or its whole-issue closure isn't verified
        // (`gate_decision(result, finalize) == Pass`). Emit a resync FIRST so a view
        // still showing "running" refetches the true status (the original
        // transition's event may have been missed), then return the non-retryable
        // error.
        let dag = artifact::list_dag(conn, issue_id).await?;
        let integration_passed =
            crate::loop_engine::gates::integration_passed(conn, &dag).await?;
        if issue.status != IssueStatus::Running || !integration_passed {
            self.emit_changed(issue.space_id, issue_id, "merge_unavailable");
            return Err(LoopError::NotMergeable);
        }

        let folder_id = issue
            .worktree_folder_id
            .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id} worktree")))?;
        let folder = folder_service::get_folder_by_id(conn, folder_id)
            .await?
            .ok_or(LoopError::Detached)?;
        let worktree_path = PathBuf::from(&folder.path);
        let branch = format!("loop/{}/issue-{}", issue.space_id, issue.seq_no);
        let base_branch = issue
            .base_branch
            .clone()
            .ok_or_else(|| LoopError::Git("issue has no recorded base branch".into()))?;
        let base_commit = issue
            .base_commit
            .clone()
            .ok_or_else(|| LoopError::Git("issue has no recorded base commit".into()))?;
        let config =
            crate::loop_engine::config_resolver::effective_config(&self.db.conn, &issue).await?;

        let outcome = worktree::merge_issue(
            &repo_path,
            &worktree_path,
            &branch,
            &base_branch,
            &base_commit,
            &config.validation_commands,
            config.iteration_timeout_secs,
        )
        .await?;

        // A non-`Merged` outcome means the landing could not happen. Surface the
        // concrete reason as an error — NEVER a silent success that leaves the
        // issue stuck "running" with no visible cause. Block the issue + file a
        // durable card so the fault is visible to BOTH the human gate and the
        // driver's auto-merge (which only logs the error) — no silent stall on
        // "running". Supersede any pending merge-approval card so the blocked
        // issue shows only the retry path, not a now-dead "approve".
        if !matches!(outcome, MergeOutcome::Merged { .. }) {
            let (reason, message, detail) = merge_fault_report(&outcome);
            cas_issue_status(conn, issue_id, IssueStatus::Running, IssueStatus::Blocked).await?;
            inbox::upsert_inbox(
                conn,
                issue.space_id,
                issue_id,
                None,
                InboxKind::Blocked,
                &format!("merge_blocked:{issue_id}"),
                serde_json::json!({ "reason": reason, "detail": detail }),
            )
            .await?;
            resolve_approval_card(
                conn,
                issue_id,
                &format!("merge:{issue_id}"),
                serde_json::json!({ "action": "merge_failed", "reason": reason }),
            )
            .await?;
            self.emit_changed(issue.space_id, issue_id, "blocked");
            self.wake(issue_id).await;
            return Err(LoopError::MergeFailed(message));
        }

        // Merged. Best-effort teardown; the DB update below is the source of truth —
        // a merged issue never restarts, so a stale folder/worktree is inert.
        let _ = worktree::remove_worktree(&repo_path, &worktree_path).await;
        // Drop any per-task / integrate worktrees + their branches (a parallel
        // issue's task work is now in base via the fan-in, so they are merged).
        let _ = worktree::remove_issue_subtree(&repo_path, &worktree_path, true).await;
        // The loop branch is now in base behind the --no-ff merge commit, so drop
        // it. Safe `-d`: git refuses if it is somehow not merged, so this can never
        // discard unlanded work.
        let _ = worktree::delete_branch(&repo_path, &branch, false).await;
        let _ = folder_service::remove_folder(conn, &folder.path).await;
        resolve_approval_card(
            conn,
            issue_id,
            &format!("merge:{issue_id}"),
            serde_json::json!({ "action": "merged" }),
        )
        .await?;
        let now = Utc::now();
        let landed = loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Status,
                Expr::value(IssueStatus::Done.to_value()),
            )
            .col_expr(loop_issue::Column::EndedAt, Expr::value(now))
            .col_expr(loop_issue::Column::UpdatedAt, Expr::value(now))
            .filter(loop_issue::Column::Id.eq(issue_id))
            .filter(loop_issue::Column::Status.eq(IssueStatus::Running))
            .exec(conn)
            .await?;
        if landed.rows_affected != 1 {
            // Unreachable under the lock (status was a freshly-confirmed `running`
            // re-read); the git work already landed, so warn rather than fail —
            // failing would falsely imply nothing merged.
            tracing::warn!(
                issue_id,
                rows = landed.rows_affected,
                "merge: status CAS to done affected an unexpected row count after landing"
            );
        }
        self.emit_changed(issue.space_id, issue_id, "merged");
        // Nudge the driver: it re-ticks, sees the terminal status, and stops.
        self.wake(issue_id).await;
        Ok(())
    }

    /// Approve the design gate (route=full): mark every design that is awaiting
    /// approval `done` and wake the driver, which then advances to planning.
    /// [`LoopError::Conflict`] when no design is awaiting (already approved /
    /// rejected, or none produced).
    pub async fn approve_design(&self, issue_id: i32) -> Result<(), LoopError> {
        let conn = &self.db.conn;
        let issue = issue::get_issue(conn, issue_id)
            .await?
            .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
        let awaiting = awaiting_design_ids(conn, issue_id).await?;
        if awaiting.is_empty() {
            return Err(LoopError::Conflict);
        }
        for id in &awaiting {
            cas_artifact_status(conn, *id, ArtifactStatus::AwaitingApproval, ArtifactStatus::Done)
                .await?;
        }
        resolve_approval_card(
            conn,
            issue_id,
            &format!("design:{issue_id}"),
            serde_json::json!({ "action": "approve" }),
        )
        .await?;
        self.emit_changed(issue.space_id, issue_id, "design_approved");
        self.wake(issue_id).await;
        Ok(())
    }

    /// Reject the design gate: supersede every awaiting design (recording the
    /// reviewer's comment as a human revision so the re-dispatched design isn't
    /// blind) and wake the driver, which produces a fresh design. Conflict when
    /// no design is awaiting.
    pub async fn reject_design(
        &self,
        issue_id: i32,
        comment: Option<String>,
    ) -> Result<(), LoopError> {
        let conn = &self.db.conn;
        let issue = issue::get_issue(conn, issue_id)
            .await?
            .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
        let awaiting = awaiting_design_ids(conn, issue_id).await?;
        if awaiting.is_empty() {
            return Err(LoopError::Conflict);
        }
        let note = comment.unwrap_or_default();
        for id in &awaiting {
            cas_artifact_status(
                conn,
                *id,
                ArtifactStatus::AwaitingApproval,
                ArtifactStatus::Superseded,
            )
            .await?;
            if !note.trim().is_empty() {
                artifact::add_revision(
                    conn,
                    *id,
                    &format!("[design rejected] {}", note.trim()),
                    ActorKind::Human,
                    None,
                )
                .await?;
            }
        }
        resolve_approval_card(
            conn,
            issue_id,
            &format!("design:{issue_id}"),
            serde_json::json!({ "action": "reject", "comment": note }),
        )
        .await?;
        self.emit_changed(issue.space_id, issue_id, "design_rejected");
        self.wake(issue_id).await;
        Ok(())
    }

    /// Retry a blocked issue — the inbox "retry" escape hatch. Re-arms every
    /// blocked task for a fresh implement run, marks the blocking cards handled,
    /// and puts the issue back to `running` under a fresh driver. Conflict when
    /// the issue is not `blocked`.
    ///
    /// Each blocked task is reset `blocked → pending` with its failure signature
    /// cleared, so the repeated-failure breaker won't trip on the first new run.
    /// `attempt` is intentionally *kept*: a blocked task already sits one past its
    /// last iteration, so the next dispatch is fresh — and each retry grants one
    /// more attempt against `max_attempts` (raise it in per-issue settings for a
    /// larger budget). Issue-level blocks (dirty finalize, merge fault/reject)
    /// have no blocked task; retry simply re-drives so the engine re-evaluates.
    pub async fn retry_issue(self: &Arc<Self>, issue_id: i32) -> Result<(), LoopError> {
        let conn = &self.db.conn;
        let issue = issue::get_issue(conn, issue_id)
            .await?
            .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
        if issue.status != IssueStatus::Blocked {
            // Stale action: the issue is no longer blocked (e.g. already terminal
            // while a view still shows "running"). Nudge subscribers to refetch the
            // true status, then report the conflict.
            self.emit_changed(issue.space_id, issue_id, "retry_unavailable");
            return Err(LoopError::Conflict);
        }
        loop_artifact::Entity::update_many()
            .col_expr(
                loop_artifact::Column::Status,
                Expr::value(ArtifactStatus::Pending.to_value()),
            )
            .col_expr(
                loop_artifact::Column::LastFailureSig,
                Expr::value(Option::<String>::None),
            )
            .col_expr(loop_artifact::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(loop_artifact::Column::IssueId.eq(issue_id))
            .filter(loop_artifact::Column::Kind.eq(ArtifactKind::Task))
            .filter(loop_artifact::Column::Status.eq(ArtifactStatus::Blocked))
            .exec(conn)
            .await?;
        resolve_cards_of_kind(
            conn,
            issue_id,
            InboxKind::Blocked,
            serde_json::json!({ "action": "retry" }),
        )
        .await?;
        if !cas_issue_status(conn, issue_id, IssueStatus::Blocked, IssueStatus::Running).await? {
            return Err(LoopError::Conflict);
        }
        self.emit_changed(issue.space_id, issue_id, "retried");
        self.start_issue(issue_id).await;
        Ok(())
    }

    /// Top up a budget-paused issue's token budget and resume it — the inbox
    /// "add budget" escape hatch. `additional` (clamped to ≥ 0) is added to the
    /// current `token_budget`; the budget card is marked handled and the issue
    /// resumes under a fresh driver. Conflict when the issue is not `paused`.
    pub async fn add_budget(self: &Arc<Self>, issue_id: i32, additional: i64) -> Result<(), LoopError> {
        let conn = &self.db.conn;
        let issue = issue::get_issue(conn, issue_id)
            .await?
            .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
        if issue.status != IssueStatus::Paused {
            return Err(LoopError::Conflict);
        }
        let new_budget = issue.token_budget.unwrap_or(0).saturating_add(additional.max(0));
        loop_issue::Entity::update_many()
            .col_expr(loop_issue::Column::TokenBudget, Expr::value(new_budget))
            .col_expr(loop_issue::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(conn)
            .await?;
        resolve_cards_of_kind(
            conn,
            issue_id,
            InboxKind::BudgetExhausted,
            serde_json::json!({ "action": "add_budget", "additional": additional }),
        )
        .await?;
        self.emit_changed(issue.space_id, issue_id, "budget_added");
        // Flip paused → running, clear the pause reason, and start a fresh driver.
        self.resume_issue(issue_id).await
    }

    /// Emit the coarse `loop://changed` refetch signal for an issue.
    pub(crate) fn emit_changed(&self, space_id: i32, issue_id: i32, kind: &str) {
        emit_event(
            &self.emitter,
            LOOP_CHANGED_EVENT,
            LoopChanged {
                v: 1,
                space_id,
                issue_id: Some(issue_id),
                subject_kind: "issue".to_string(),
                subject_id: issue_id,
                kind: kind.to_string(),
            },
        );
    }
}

/// The issue's design artifacts currently `awaiting_approval`.
async fn awaiting_design_ids(
    conn: &sea_orm::DatabaseConnection,
    issue_id: i32,
) -> Result<Vec<i32>, LoopError> {
    let dag = artifact::list_dag(conn, issue_id).await?;
    Ok(dag
        .artifacts
        .iter()
        .filter(|a| {
            a.kind == ArtifactKind::Design && a.status == ArtifactStatus::AwaitingApproval
        })
        .map(|a| a.id)
        .collect())
}

/// Mark the pending approval inbox card (`kind=approval`, `subject_key=subject`)
/// for an issue handled. No-op when none exists — auto paths and direct calls
/// run fine without a card.
async fn resolve_approval_card(
    conn: &sea_orm::DatabaseConnection,
    issue_id: i32,
    subject: &str,
    resolution: serde_json::Value,
) -> Result<(), LoopError> {
    if let Some(card) = loop_inbox_item::Entity::find()
        .filter(loop_inbox_item::Column::IssueId.eq(issue_id))
        .filter(loop_inbox_item::Column::Kind.eq(InboxKind::Approval))
        .filter(loop_inbox_item::Column::SubjectKey.eq(subject.to_string()))
        .filter(loop_inbox_item::Column::Status.eq(InboxStatus::Pending))
        .one(conn)
        .await?
    {
        inbox::handle_inbox(conn, card.id, resolution).await?;
    }
    Ok(())
}

/// Mark every pending inbox card of `kind` for an issue handled. A blocked issue
/// may carry more than one card (e.g. several `no_progress:{task}` keys), so the
/// retry / add-budget escape hatches clear them all in one resolution.
async fn resolve_cards_of_kind(
    conn: &sea_orm::DatabaseConnection,
    issue_id: i32,
    kind: InboxKind,
    resolution: serde_json::Value,
) -> Result<(), LoopError> {
    let cards = loop_inbox_item::Entity::find()
        .filter(loop_inbox_item::Column::IssueId.eq(issue_id))
        .filter(loop_inbox_item::Column::Kind.eq(kind))
        .filter(loop_inbox_item::Column::Status.eq(InboxStatus::Pending))
        .all(conn)
        .await?;
    for card in cards {
        inbox::handle_inbox(conn, card.id, resolution.clone()).await?;
    }
    Ok(())
}

/// Map a non-`Merged` outcome to `(inbox reason code, user-facing message,
/// diagnostic detail)`. The reason code keys the inbox card; the message is the
/// error toast the user sees; the detail carries the git/validation output.
fn merge_fault_report(outcome: &MergeOutcome) -> (&'static str, String, String) {
    match outcome {
        MergeOutcome::BaseGone => (
            "base_gone",
            "The base branch no longer exists.".to_string(),
            "base branch no longer exists".to_string(),
        ),
        MergeOutcome::BaseDirty => (
            "base_dirty",
            "The base repository has uncommitted changes to tracked files. Commit or stash them, then merge again."
                .to_string(),
            "base repo working tree has uncommitted tracked changes".to_string(),
        ),
        MergeOutcome::Conflict { stage, detail } => {
            let (reason, message) = if *stage == "integrate" {
                (
                    "merge_conflict_integrate",
                    "Merge conflict while integrating the latest base into the issue branch.",
                )
            } else {
                (
                    "merge_conflict",
                    "Merge conflict while landing the issue branch onto the base branch.",
                )
            };
            (reason, message.to_string(), detail.clone())
        }
        MergeOutcome::RevalidationFailed { output } => (
            "revalidation_failed",
            "Re-validation failed on the merged result.".to_string(),
            output.clone(),
        ),
        // Not reached: the success arm is handled before this is called.
        MergeOutcome::Merged { .. } => ("merged", "Merge failed.".to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::manager::ConnectionManager;
    use crate::db::entities::loop_artifact::{self, ArtifactStatus};
    use crate::db::entities::loop_artifact_revision::ActorKind;
    use crate::db::entities::loop_issue::IssuePriority;
    use crate::db::entities::loop_iteration::Stage;
    use crate::db::test_helpers::{fresh_disk_db, fresh_in_memory_db, seed_folder};
    use crate::loop_engine::transitions::{cas_iteration_status, try_claim_iteration, IterationClaim};
    use crate::models::agent::AgentType;
    use crate::models::loops::IssueConfig;
    use crate::web::event_bridge::EventEmitter;
    use std::process::Command as StdCommand;

    /// Build an engine + a single issue already marked `running` (without going
    /// through trigger, so no worktree or driver is created — the pause/cancel
    /// paths under test never need one).
    async fn setup() -> (Arc<LoopEngine>, sea_orm::DatabaseConnection, i32, i32) {
        let db = fresh_in_memory_db().await;
        let conn = db.conn.clone();
        let folder_id = seed_folder(&db, "/tmp/loop-actions").await;
        let space = space::create_space(&conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &conn,
            space.id,
            "I",
            "b",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        cas_issue_status(&conn, issue.row.id, IssueStatus::Pending, IssueStatus::Running)
            .await
            .unwrap();
        let engine = LoopEngine::new(
            db,
            ConnectionManager::new(),
            std::path::PathBuf::from("/tmp/loop-actions-data"),
            EventEmitter::Noop,
        );
        (engine, conn, space.id, issue.row.id)
    }

    #[tokio::test]
    async fn pause_sets_manual_reason_then_conflicts() {
        let (engine, conn, _space, issue_id) = setup().await;
        engine.pause_issue(issue_id).await.unwrap();

        let issue = issue::get_issue(&conn, issue_id).await.unwrap().unwrap();
        assert_eq!(issue.status, IssueStatus::Paused);
        assert_eq!(issue.pause_reason, Some(PauseReason::Manual));

        // A second pause (no longer running) is a conflict, not a silent no-op.
        assert!(matches!(
            engine.pause_issue(issue_id).await,
            Err(LoopError::Conflict)
        ));
    }

    #[tokio::test]
    async fn cancel_closes_issue_and_invalidates_in_flight_token() {
        let (engine, conn, space_id, issue_id) = setup().await;
        // An in-flight iteration holding a lease + a live capability token.
        let iter = try_claim_iteration(
            &conn,
            IterationClaim {
                space_id,
                issue_id,
                stage: Stage::Triage,
                target_artifact_id: None,
                slot_no: None,
                capability_token: "live-token".into(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .unwrap();
        cas_iteration_status(&conn, iter.id, IterationStatus::Queued, IterationStatus::Running)
            .await
            .unwrap();

        engine.cancel_issue(issue_id).await.unwrap();

        let issue = issue::get_issue(&conn, issue_id).await.unwrap().unwrap();
        assert_eq!(issue.status, IssueStatus::Cancelled);
        assert!(issue.ended_at.is_some());
        let it = loop_iteration::Entity::find_by_id(iter.id)
            .one(&conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            it.status,
            IterationStatus::Cancelled,
            "the in-flight token is invalidated so the host rejects late writes"
        );
    }

    #[tokio::test]
    async fn cancel_works_from_paused_then_conflicts_when_terminal() {
        let (engine, conn, _space, issue_id) = setup().await;
        engine.pause_issue(issue_id).await.unwrap();
        engine.cancel_issue(issue_id).await.unwrap();
        assert_eq!(
            issue::get_issue(&conn, issue_id).await.unwrap().unwrap().status,
            IssueStatus::Cancelled
        );
        // Cancelling an already-terminal issue is a conflict.
        assert!(matches!(
            engine.cancel_issue(issue_id).await,
            Err(LoopError::Conflict)
        ));
    }

    #[tokio::test]
    async fn cancel_disconnects_live_agent() {
        let (engine, conn, space_id, issue_id) = setup().await;
        // An in-flight running iteration bound to a conversation.
        let iter = try_claim_iteration(
            &conn,
            IterationClaim {
                space_id,
                issue_id,
                stage: Stage::Triage,
                target_artifact_id: None,
                slot_no: None,
                capability_token: "tok".into(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .unwrap();
        cas_iteration_status(&conn, iter.id, IterationStatus::Queued, IterationStatus::Running)
            .await
            .unwrap();
        let convo = 4242;
        loop_iteration::Entity::update_many()
            .col_expr(loop_iteration::Column::ConversationId, Expr::value(convo))
            .filter(loop_iteration::Column::Id.eq(iter.id))
            .exec(&conn)
            .await
            .unwrap();
        // A live agent connection whose session is bound to that conversation.
        engine
            .manager
            .insert_test_connection("agent-conn", AgentType::ClaudeCode, None, EventEmitter::Noop)
            .await;
        engine
            .manager
            .get_state("agent-conn")
            .await
            .unwrap()
            .write()
            .await
            .conversation_id = Some(convo);
        assert!(engine
            .manager
            .find_connection_by_conversation_id(convo)
            .await
            .is_some());

        engine.cancel_issue(issue_id).await.unwrap();

        assert!(
            engine
                .manager
                .find_connection_by_conversation_id(convo)
                .await
                .is_none(),
            "the agent process connection is killed on cancel"
        );
    }

    // ── Blocked / budget escape hatches ─────────────────────────────────────

    #[tokio::test]
    async fn retry_rearms_blocked_task_and_resolves_cards() {
        let (engine, conn, space_id, issue_id) = setup().await;
        // A blocked task carrying a failure signature + its no-progress card.
        let task = artifact::create_artifact(
            &conn,
            space_id,
            issue_id,
            ArtifactKind::Task,
            "T",
            ArtifactStatus::Blocked,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        loop_artifact::Entity::update_many()
            .col_expr(
                loop_artifact::Column::LastFailureSig,
                Expr::value("validation_failed:abc".to_string()),
            )
            .filter(loop_artifact::Column::Id.eq(task.id))
            .exec(&conn)
            .await
            .unwrap();
        cas_issue_status(&conn, issue_id, IssueStatus::Running, IssueStatus::Blocked)
            .await
            .unwrap();
        inbox::upsert_inbox(
            &conn,
            space_id,
            issue_id,
            None,
            InboxKind::Blocked,
            &format!("no_progress:{}", task.id),
            serde_json::json!({ "reason": "max_attempts" }),
        )
        .await
        .unwrap();

        engine.retry_issue(issue_id).await.unwrap();

        let t = loop_artifact::Entity::find_by_id(task.id)
            .one(&conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(t.status, ArtifactStatus::Pending, "blocked task re-armed");
        assert!(t.last_failure_sig.is_none(), "failure signature cleared");
        assert_eq!(
            issue::get_issue(&conn, issue_id).await.unwrap().unwrap().status,
            IssueStatus::Running
        );
        assert!(
            inbox::list_inbox(&conn, space_id, Some(InboxStatus::Pending))
                .await
                .unwrap()
                .is_empty(),
            "blocking card resolved"
        );
        engine.stop_issue(issue_id).await;

        // Retrying an issue that is no longer blocked is a conflict.
        assert!(matches!(
            engine.retry_issue(issue_id).await,
            Err(LoopError::Conflict)
        ));
    }

    #[tokio::test]
    async fn add_budget_tops_up_and_resumes() {
        let (engine, conn, space_id, issue_id) = setup().await;
        // A budget-paused issue: budget set, paused(budget), card filed.
        loop_issue::Entity::update_many()
            .col_expr(loop_issue::Column::TokenBudget, Expr::value(1000_i64))
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(&conn)
            .await
            .unwrap();
        cas_issue_status(&conn, issue_id, IssueStatus::Running, IssueStatus::Paused)
            .await
            .unwrap();
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::PauseReason,
                Expr::value(PauseReason::Budget.to_value()),
            )
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(&conn)
            .await
            .unwrap();
        inbox::upsert_inbox(
            &conn,
            space_id,
            issue_id,
            None,
            InboxKind::BudgetExhausted,
            &format!("budget:{issue_id}"),
            serde_json::json!({ "token_used": 1200, "token_budget": 1000 }),
        )
        .await
        .unwrap();

        engine.add_budget(issue_id, 5000).await.unwrap();

        let issue = issue::get_issue(&conn, issue_id).await.unwrap().unwrap();
        assert_eq!(issue.token_budget, Some(6000), "budget topped up");
        assert_eq!(issue.status, IssueStatus::Running, "issue resumed");
        assert_eq!(issue.pause_reason, None, "pause reason cleared");
        assert!(
            inbox::list_inbox(&conn, space_id, Some(InboxStatus::Pending))
                .await
                .unwrap()
                .is_empty(),
            "budget card resolved"
        );
        engine.stop_issue(issue_id).await;

        // Adding budget to a running (non-paused) issue is a conflict.
        assert!(matches!(
            engine.add_budget(issue_id, 1000).await,
            Err(LoopError::Conflict)
        ));
    }

    // ── Merge gate (real git repo) ──────────────────────────────────────────

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

    /// Engine + real git repo + an issue triggered (worktree created, running)
    /// carrying one loop commit and a produced `result` artifact — i.e. a fully
    /// finalized issue sitting at the merge gate.
    async fn setup_repo() -> (
        Arc<LoopEngine>,
        sea_orm::DatabaseConnection,
        tempfile::TempDir,
        tempfile::TempDir,
        i32,
        i32,
    ) {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let data = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(data.path()).await;
        let conn = db.conn.clone();
        let folder_id = seed_folder(&db, &repo.path().to_string_lossy()).await;
        let space = space::create_space(&conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &conn,
            space.id,
            "I",
            "b",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        let engine = LoopEngine::new(
            db,
            ConnectionManager::new(),
            data.path().to_path_buf(),
            EventEmitter::Noop,
        );
        // Trigger: create the worktree (records the base), flip running.
        let ctx = worktree::ensure_worktree(&conn, data.path(), issue.row.id)
            .await
            .unwrap();
        cas_issue_status(&conn, issue.row.id, IssueStatus::Pending, IssueStatus::Running)
            .await
            .unwrap();
        // One loop commit so the landing has content.
        std::fs::write(ctx.worktree_path.join("feature.txt"), "work\n").unwrap();
        worktree::checkpoint(&ctx.worktree_path, "loop: feature")
            .await
            .unwrap()
            .expect("committed");
        // Finalize produced the result artifact.
        let result = artifact::create_artifact(
            &conn,
            space.id,
            issue.row.id,
            ArtifactKind::Result,
            "result",
            ArtifactStatus::Done,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        // Integration verified: the merge gate (D6) requires a recorded
        // `gate_decision(result, finalize) == Pass`, so a finalized issue at the
        // merge gate must carry it.
        crate::db::service::loop_service::gate_decision::record_decision(
            &conn,
            space.id,
            issue.row.id,
            result.id,
            crate::loop_engine::gates::FINALIZE_GATE_STAGE,
            result.attempt,
            &[],
            &[],
            "{}",
            crate::db::entities::loop_gate_decision::GateOutcome::Pass,
        )
        .await
        .unwrap();
        (engine, conn, repo, data, issue.row.id, ctx.worktree_folder_id)
    }

    #[tokio::test]
    async fn merge_issue_success_closes_issue_and_removes_worktree() {
        let (engine, conn, repo, _data, issue_id, folder_id) = setup_repo().await;
        let worktree_path = PathBuf::from(
            folder_service::get_folder_by_id(&conn, folder_id)
                .await
                .unwrap()
                .unwrap()
                .path,
        );

        engine.merge_issue(issue_id).await.unwrap();

        let issue = issue::get_issue(&conn, issue_id).await.unwrap().unwrap();
        assert_eq!(issue.status, IssueStatus::Done);
        assert!(issue.ended_at.is_some());
        // Worktree folder soft-deleted + directory gone.
        assert!(folder_service::get_folder_by_id(&conn, folder_id)
            .await
            .unwrap()
            .is_none());
        assert!(!worktree_path.exists());
        // The loop work landed on the base branch.
        assert!(repo.path().join("feature.txt").exists());
    }

    #[tokio::test]
    async fn merge_issue_dirty_base_blocks_and_errors() {
        let (engine, conn, repo, _data, issue_id, _folder_id) = setup_repo().await;
        // Modify a TRACKED file in the base repo (untracked files no longer block).
        std::fs::write(repo.path().join("README.md"), "locally modified\n").unwrap();

        // The fault surfaces as an error — not a silent "Ok" success.
        let err = engine.merge_issue(issue_id).await.unwrap_err();
        assert!(matches!(err, LoopError::MergeFailed(_)));

        // The issue is blocked + carries a durable card so the fault is visible
        // (also covers the auto-merge path, which only logs the error).
        let issue = issue::get_issue(&conn, issue_id).await.unwrap().unwrap();
        assert_eq!(issue.status, IssueStatus::Blocked);
        let cards = inbox::list_inbox(&conn, issue.space_id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert!(cards.iter().any(|c| c.kind == InboxKind::Blocked
            && c.subject_key == format!("merge_blocked:{issue_id}")));
        assert!(!repo.path().join("feature.txt").exists(), "nothing landed");
    }

    #[tokio::test]
    async fn merge_issue_conflict_blocks_with_inbox_and_errors() {
        let (engine, conn, repo, _data, issue_id, _folder_id) = setup_repo().await;
        // Advance the base branch with a CONFLICTING change to feature.txt (the
        // loop branch added feature.txt too), so integrating the base conflicts.
        std::fs::write(repo.path().join("feature.txt"), "base conflicting\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "base feature"]);

        // The fault surfaces as an error (never a silent success)...
        let err = engine.merge_issue(issue_id).await.unwrap_err();
        assert!(matches!(err, LoopError::MergeFailed(_)));

        // ...AND a branch/integration fault blocks the issue + files a card so it
        // is visible (also covers the auto-merge path, which only logs the error).
        let issue = issue::get_issue(&conn, issue_id).await.unwrap().unwrap();
        assert_eq!(issue.status, IssueStatus::Blocked);
        let cards = inbox::list_inbox(&conn, issue.space_id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert!(cards.iter().any(|c| c.kind == InboxKind::Blocked
            && c.subject_key == format!("merge_blocked:{issue_id}")));
        // The loop's work never landed: the base still holds its own version.
        assert_eq!(
            std::fs::read_to_string(repo.path().join("feature.txt")).unwrap(),
            "base conflicting\n"
        );
    }

    #[tokio::test]
    async fn merge_issue_without_result_not_mergeable() {
        let (engine, conn, _repo, _data, issue_id, _folder_id) = setup_repo().await;
        // Drop the result artifact to simulate "finalize not done".
        loop_artifact::Entity::delete_many()
            .filter(loop_artifact::Column::IssueId.eq(issue_id))
            .filter(loop_artifact::Column::Kind.eq(ArtifactKind::Result))
            .exec(&conn)
            .await
            .unwrap();
        assert!(matches!(
            engine.merge_issue(issue_id).await,
            Err(LoopError::NotMergeable)
        ));
    }

    #[tokio::test]
    async fn merge_issue_second_call_after_cleanup_is_idempotent_ok() {
        let (engine, conn, _repo, _data, issue_id, folder_id) = setup_repo().await;
        let worktree_path = PathBuf::from(
            folder_service::get_folder_by_id(&conn, folder_id)
                .await
                .unwrap()
                .unwrap()
                .path,
        );

        engine.merge_issue(issue_id).await.unwrap();
        // First merge landed and tore the worktree down.
        assert_eq!(
            issue::get_issue(&conn, issue_id).await.unwrap().unwrap().status,
            IssueStatus::Done
        );
        assert!(!worktree_path.exists(), "first merge removed the worktree");
        assert!(folder_service::get_folder_by_id(&conn, folder_id)
            .await
            .unwrap()
            .is_none());

        // Second merge, with the worktree already removed, is a no-op SUCCESS —
        // not LoopError::Conflict ("state changed concurrently; retry"). The
        // idempotent branch returns at the post-lock `done` re-read before it ever
        // touches the absent worktree.
        engine.merge_issue(issue_id).await.unwrap();
        assert_eq!(
            issue::get_issue(&conn, issue_id).await.unwrap().unwrap().status,
            IssueStatus::Done
        );
    }

    #[tokio::test]
    async fn merge_issue_blocked_is_not_mergeable() {
        let (engine, conn, _repo, _data, issue_id, _folder_id) = setup_repo().await;
        cas_issue_status(&conn, issue_id, IssueStatus::Running, IssueStatus::Blocked)
            .await
            .unwrap();
        assert!(matches!(
            engine.merge_issue(issue_id).await,
            Err(LoopError::NotMergeable)
        ));
    }

    // ── Design approval gate ────────────────────────────────────────────────

    /// Mint an `awaiting_approval` design + its inbox card on a running issue.
    async fn seed_awaiting_design(conn: &sea_orm::DatabaseConnection, space_id: i32, issue_id: i32) -> i32 {
        let d = artifact::create_artifact(
            conn,
            space_id,
            issue_id,
            ArtifactKind::Design,
            "D1",
            ArtifactStatus::AwaitingApproval,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        artifact::add_revision(conn, d.id, "design body", ActorKind::Agent, None)
            .await
            .unwrap();
        inbox::upsert_inbox(
            conn,
            space_id,
            issue_id,
            None,
            InboxKind::Approval,
            &format!("design:{issue_id}"),
            serde_json::json!({ "gate": "design" }),
        )
        .await
        .unwrap();
        d.id
    }

    #[tokio::test]
    async fn approve_design_marks_done_and_resolves_card() {
        let (engine, conn, space_id, issue_id) = setup().await;
        let design_id = seed_awaiting_design(&conn, space_id, issue_id).await;

        engine.approve_design(issue_id).await.unwrap();

        let detail = artifact::get_artifact_detail(&conn, design_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.row.status, ArtifactStatus::Done);
        let pending = inbox::list_inbox(&conn, space_id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert!(!pending
            .iter()
            .any(|c| c.subject_key == format!("design:{issue_id}")));
        // Nothing awaiting now → a second approve conflicts.
        assert!(matches!(
            engine.approve_design(issue_id).await,
            Err(LoopError::Conflict)
        ));
    }

    #[tokio::test]
    async fn reject_design_supersedes_and_records_comment() {
        let (engine, conn, space_id, issue_id) = setup().await;
        let design_id = seed_awaiting_design(&conn, space_id, issue_id).await;

        engine
            .reject_design(issue_id, Some("needs more detail".into()))
            .await
            .unwrap();

        let detail = artifact::get_artifact_detail(&conn, design_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.row.status, ArtifactStatus::Superseded);
        assert!(detail.revisions.iter().any(|r| r.actor_kind == ActorKind::Human
            && r.content.contains("needs more detail")));
        let pending = inbox::list_inbox(&conn, space_id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert!(!pending
            .iter()
            .any(|c| c.subject_key == format!("design:{issue_id}")));
    }
}
