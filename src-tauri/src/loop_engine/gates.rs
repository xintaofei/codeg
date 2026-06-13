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

use std::path::Path;

use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveEnum, ColumnTrait, EntityTrait, QueryFilter};

use crate::db::entities::loop_artifact::{self, ArtifactKind, ArtifactStatus};
use crate::db::entities::loop_issue;
use crate::db::entities::loop_iteration::{self, IterationStatus, Stage};
use crate::db::service::folder_service;
use crate::db::AppDatabase;
use crate::models::loops::{IssueConfig, LoopArtifactRow, LoopDagView};
use crate::web::event_bridge::EventEmitter;

use crate::loop_engine::dispatch::{dispatch_iteration, DispatchInput, LoopAgentSpawner};
use crate::loop_engine::driver::resolve_agent;
use crate::loop_engine::error::LoopError;
use crate::loop_engine::transitions::try_acquire_task_gate;
use crate::loop_engine::worktree;

/// Outcome of checkpointing a settled implement iteration.
enum ImplementOutcome {
    /// Non-empty diff committed → task promoted to `in_progress` (implemented,
    /// awaiting validation).
    Advanced,
    /// Empty diff → rework counter bumped; the caller re-dispatches implement.
    NoProgress,
}

/// Drive the implement stage for one tick. Returns `true` when it dispatched a
/// new implement iteration (the caller maps that to a `Dispatched` tick).
///
/// A no-op while no task exists yet (read stages still in flight), so the driver
/// can call it on every "read frontier empty" tick.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_implement(
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

/// Advance the task currently holding the gate: wait while its implement
/// iteration is in flight; checkpoint once it has settled; (re)dispatch when
/// nothing is live.
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
    // Implement only owns the pending → implemented transition. Once a task is
    // `in_progress` (implemented, awaiting validation) or terminal, idle here —
    // validate/review take over in later tasks.
    if task.status != ArtifactStatus::Pending {
        return Ok(false);
    }

    let impls = implement_iterations(db, issue.id, active_task_id).await?;
    if impls
        .iter()
        .any(|it| matches!(it.status, IterationStatus::Queued | IterationStatus::Running))
    {
        // Implement in flight — wait for its completion to wake us.
        return Ok(false);
    }

    // A succeeded implement at the current attempt is awaiting its checkpoint.
    let settled_current = impls
        .iter()
        .any(|it| it.status == IterationStatus::Succeeded && it.attempt == task.attempt);
    if settled_current {
        match finish_implement(db, issue, worktree_folder_id, task).await? {
            ImplementOutcome::Advanced => Ok(false),
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

/// Checkpoint the worktree for a settled implement iteration. A committed diff
/// promotes the task to `in_progress`; an empty diff is discarded and counted as
/// no progress.
async fn finish_implement(
    db: &AppDatabase,
    issue: &loop_issue::Model,
    worktree_folder_id: i32,
    task: &LoopArtifactRow,
) -> Result<ImplementOutcome, LoopError> {
    let conn = &db.conn;
    let folder = folder_service::get_folder_by_id(conn, worktree_folder_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("worktree folder {worktree_folder_id}")))?;
    let worktree_path = Path::new(&folder.path);

    let message = format!("loop: implement #{} (issue #{})", task.id, issue.seq_no);
    match worktree::checkpoint(worktree_path, &message).await? {
        Some(_sha) => {
            set_task_status(db, task.id, ArtifactStatus::InProgress).await?;
            Ok(ImplementOutcome::Advanced)
        }
        None => {
            // No diff to accept. Discard any stray uncommitted state and record
            // a no-progress signature for the breaker (enforced in Task 2.4).
            worktree::reset_to_head(worktree_path).await?;
            bump_rework(db, task.id, "empty_diff:implement").await?;
            Ok(ImplementOutcome::NoProgress)
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
        drive_implement(
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
}
