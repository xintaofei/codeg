//! worktree-per-issue lifecycle.
//!
//! Each running issue gets its own git worktree + branch so issues run fully in
//! parallel without touching each other's tree. The engine drives git directly
//! (returning `LoopError`) rather than through the `AppCommandError`-returning
//! command helpers, and registers the worktree as a hidden `loop_worktree`
//! folder so cwd resolution works while it stays out of every user folder list.
//!
//! Invariants (spec §4.4 / §4.10): the engine checkpoint-commits accepted work
//! onto the issue branch; `reset_to_head` only ever discards *uncommitted*
//! side-effects (never rewinds a committed checkpoint).

use std::path::{Path, PathBuf};
use std::time::Duration;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    Set,
};

use crate::db::entities::loop_artifact;
use crate::db::entities::loop_link::{self, LinkKind};
use crate::db::service::{folder_service, loop_service};
use crate::loop_engine::{validation, LoopError};

/// Identity stamped on engine checkpoint commits.
const ENGINE_NAME: &str = "codeg loop engine";
const ENGINE_EMAIL: &str = "loop@codeg.local";

/// Resolved location of an issue's worktree.
#[derive(Debug, Clone)]
pub struct WorktreeContext {
    pub worktree_path: PathBuf,
    pub worktree_folder_id: i32,
    pub branch: String,
    pub base_branch: String,
    pub base_commit: String,
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

async fn run_git(dir: &Path, args: &[&str]) -> Result<std::process::Output, LoopError> {
    crate::process::tokio_command("git")
        .args(args)
        .current_dir(dir)
        .output()
        .await
        .map_err(|e| LoopError::Git(format!("git {args:?}: {e}")))
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

fn stdout_trimmed(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// stdout followed by stderr (trimmed) — git writes conflict reports to both, so
/// merge-fault details want the union.
fn combined_output(out: &std::process::Output) -> String {
    let mut s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    if !err.is_empty() {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(err);
    }
    s
}

async fn ensure_git_repo(repo: &Path) -> Result<(), LoopError> {
    let out = run_git(repo, &["rev-parse", "--is-inside-work-tree"]).await?;
    if out.status.success() && stdout_trimmed(&out) == "true" {
        Ok(())
    } else {
        Err(LoopError::NotGitRepo)
    }
}

async fn current_branch(repo: &Path) -> Result<String, LoopError> {
    let head = run_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    if head.status.success() {
        let name = stdout_trimmed(&head);
        if !name.is_empty() && name != "HEAD" {
            return Ok(name);
        }
    }
    // Unborn branch (init before first commit): symbolic-ref still resolves.
    let sym = run_git(repo, &["symbolic-ref", "--short", "HEAD"]).await?;
    if sym.status.success() {
        let name = stdout_trimmed(&sym);
        if !name.is_empty() {
            return Ok(name);
        }
    }
    Err(LoopError::Git("could not resolve current branch".into()))
}

/// The commit OID at a worktree's HEAD (its branch tip).
pub async fn head_commit(repo: &Path) -> Result<String, LoopError> {
    let out = run_git(repo, &["rev-parse", "HEAD"]).await?;
    if !out.status.success() {
        return Err(LoopError::Git(format!("rev-parse HEAD: {}", stderr_of(&out))));
    }
    Ok(stdout_trimmed(&out))
}

/// Create (or re-attach) the issue's worktree, branch, and hidden folder, and
/// record the merge base on the issue. Idempotent: if the issue already has a
/// live worktree folder whose directory exists, returns it untouched.
pub async fn ensure_worktree(
    conn: &DatabaseConnection,
    data_dir: &Path,
    issue_id: i32,
) -> Result<WorktreeContext, LoopError> {
    let issue = loop_service::issue::get_issue(conn, issue_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
    let branch = format!("loop/{}/issue-{}", issue.space_id, issue.seq_no);

    // Re-attach path: an existing, on-disk worktree folder is reused as-is.
    if let Some(folder_id) = issue.worktree_folder_id {
        if let Some(folder) = folder_service::get_folder_by_id(conn, folder_id).await? {
            if Path::new(&folder.path).exists() {
                return Ok(WorktreeContext {
                    worktree_path: PathBuf::from(folder.path),
                    worktree_folder_id: folder_id,
                    branch,
                    base_branch: issue.base_branch.clone().unwrap_or_default(),
                    base_commit: issue.base_commit.clone().unwrap_or_default(),
                });
            }
        }
    }

    let space = loop_service::space::get_space(conn, issue.space_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("space {}", issue.space_id)))?;
    let repo = folder_service::get_folder_by_id(conn, space.folder_id)
        .await?
        .ok_or(LoopError::Detached)?;
    let repo_path = PathBuf::from(&repo.path);
    ensure_git_repo(&repo_path).await?;

    let base_branch = current_branch(&repo_path).await?;
    let base_commit = head_commit(&repo_path).await?;

    let worktree_path = data_dir
        .join("loop-worktrees")
        .join(issue.space_id.to_string())
        .join(format!("issue-{}", issue.seq_no));
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| LoopError::Git(format!("create worktree parent dir: {e}")))?;
    }

    let wt = path_str(&worktree_path);
    // Reconcile leftovers from a prior life of this (space_id, seq_no): a stale
    // admin entry (data_dir wiped, dir gone), an orphaned worktree dir (DB-only
    // reset), or a leftover `loop/*` branch — teardown intentionally keeps
    // cancel/merge branches, and a DB reset reuses the same name. `loop/*` is an
    // engine-owned, disposable namespace, so prune dangling entries, force-remove
    // anything still at the path, then `-B` (create-or-reset) the branch from the
    // current base HEAD. A (re)triggered issue always starts from base HEAD
    // (matching the `base_commit` recorded just above) — reset is correct here,
    // never a reuse of stale loop commits.
    let _ = run_git(&repo_path, &["worktree", "prune"]).await;
    if worktree_path.exists() {
        let _ = run_git(&repo_path, &["worktree", "remove", "--force", &wt]).await;
        let _ = std::fs::remove_dir_all(&worktree_path);
    }
    let out = run_git(&repo_path, &["worktree", "add", "-B", &branch, &wt]).await?;
    if !out.status.success() {
        return Err(LoopError::Git(format!("worktree add: {}", stderr_of(&out))));
    }

    let folder = folder_service::add_loop_worktree_folder(conn, &wt, space.folder_id).await?;

    let mut active = issue.into_active_model();
    active.worktree_folder_id = Set(Some(folder.id));
    active.base_branch = Set(Some(base_branch.clone()));
    active.base_commit = Set(Some(base_commit.clone()));
    active.update(conn).await?;

    Ok(WorktreeContext {
        worktree_path,
        worktree_folder_id: folder.id,
        branch,
        base_branch,
        base_commit,
    })
}

/// Peel a ref / sha to a concrete commit OID (`<refspec>^{commit}`), erroring if
/// it doesn't resolve.
pub async fn resolve_oid(repo: &Path, refspec: &str) -> Result<String, LoopError> {
    let out = run_git(
        repo,
        &["rev-parse", "--verify", &format!("{refspec}^{{commit}}")],
    )
    .await?;
    if !out.status.success() {
        return Err(LoopError::Git(format!(
            "rev-parse {refspec}: {}",
            stderr_of(&out)
        )));
    }
    Ok(stdout_trimmed(&out))
}

/// Whether `oid` is an ancestor of the worktree's HEAD (`merge-base
/// --is-ancestor`: exit 0 = yes, 1 = no, other = error → treated as no, which
/// triggers a safe rebuild).
async fn is_ancestor_of_head(worktree: &Path, oid: &str) -> Result<bool, LoopError> {
    let out = run_git(worktree, &["merge-base", "--is-ancestor", oid, "HEAD"]).await?;
    Ok(out.status.success())
}

/// Resolve the base ref a task's worktree branches from: its single `DependsOn`
/// predecessor's **frozen** integration commit (`loop_artifact.fan_in_commit`),
/// or — for a root task (no predecessor) — the issue branch tip. NEVER the live
/// predecessor branch ref, which would smuggle post-Done drift (spec §3.2).
async fn task_base_ref(
    conn: &DatabaseConnection,
    space_id: i32,
    issue_seq: i32,
    task_id: i32,
) -> Result<String, LoopError> {
    // `DependsOn`: from = successor (this task), to = predecessor.
    let pred = loop_link::Entity::find()
        .filter(loop_link::Column::FromArtifactId.eq(task_id))
        .filter(loop_link::Column::Kind.eq(LinkKind::DependsOn))
        .one(conn)
        .await?;
    if let Some(link) = pred {
        let pred = loop_artifact::Entity::find_by_id(link.to_artifact_id)
            .one(conn)
            .await?
            .ok_or_else(|| {
                LoopError::NotFound(format!("predecessor task {}", link.to_artifact_id))
            })?;
        let sha = pred.fan_in_commit.ok_or_else(|| {
            LoopError::Git(format!(
                "predecessor task {} has no frozen commit yet",
                pred.id
            ))
        })?;
        return Ok(sha);
    }
    Ok(format!("loop/{space_id}/issue-{issue_seq}"))
}

/// Create (or re-attach) a per-task worktree + branch for **parallel-mode** task
/// execution, so concurrent tasks never share a tree (spec §3.2).
///
/// Branch `loop/{space}/issue-{seq}/task-{id}`; path
/// `loop-worktrees/{space}/issue-{seq}-tasks/task-{id}` — a **sibling** of the
/// issue worktree, never nested under it (else the issue worktree's `clean -fd`
/// during finalize/recovery would delete live task trees). The branch is cut from
/// [`task_base_ref`] (predecessor's frozen sha, or the issue branch tip).
///
/// Attach-first: an on-disk worktree whose current branch is the expected task
/// branch AND whose HEAD descends from the expected base is reused untouched
/// (never `-B`, so a task branch with committed work is not rewound). Otherwise
/// prune + force-remove + `-B` from the base. Task worktree folders have no issue
/// column; they re-attach by their deterministic path (`add_loop_worktree_folder`
/// upserts on path).
pub async fn ensure_task_worktree(
    conn: &DatabaseConnection,
    data_dir: &Path,
    issue_id: i32,
    task_id: i32,
) -> Result<WorktreeContext, LoopError> {
    let issue = loop_service::issue::get_issue(conn, issue_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
    let space = loop_service::space::get_space(conn, issue.space_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("space {}", issue.space_id)))?;
    let repo = folder_service::get_folder_by_id(conn, space.folder_id)
        .await?
        .ok_or(LoopError::Detached)?;
    let repo_path = PathBuf::from(&repo.path);
    ensure_git_repo(&repo_path).await?;

    // Hyphen, not a slash: `loop/{s}/issue-{n}/task-{id}` would be a git ref
    // D/F conflict with the issue branch `loop/{s}/issue-{n}` (a ref file cannot
    // also be a directory). `issue-{n}-task-{id}` is a sibling ref.
    let branch = format!("loop/{}/issue-{}-task-{}", issue.space_id, issue.seq_no, task_id);
    let worktree_path = data_dir
        .join("loop-worktrees")
        .join(issue.space_id.to_string())
        .join(format!("issue-{}-tasks", issue.seq_no))
        .join(format!("task-{task_id}"));

    let base_ref = task_base_ref(conn, issue.space_id, issue.seq_no, task_id).await?;
    let base_oid = resolve_oid(&repo_path, &base_ref).await?;

    attach_or_rebuild_worktree(
        conn,
        &repo_path,
        space.folder_id,
        &branch,
        &worktree_path,
        &base_oid,
        issue.base_branch.clone().unwrap_or_default(),
    )
    .await
}

/// Create (or re-attach) the issue's temp **integrate** worktree + branch, where
/// the parallel result-stage fan-in merges the frozen task commits before the
/// atomic CAS landing onto the issue branch (spec §4.4).
///
/// Branch `loop/{space}/issue-{seq}-integrate`; path a sibling of the issue + task
/// worktrees, cut from `base_oid` (the manifest's `issue_base_oid`). Attach-first
/// reuse is crucial here: a partially-merged integration — or one mid-conflict
/// (`MERGE_HEAD` set) — must be preserved across ticks and crashes so the fan-in
/// resumes from it rather than restarting.
pub async fn ensure_integrate_worktree(
    conn: &DatabaseConnection,
    data_dir: &Path,
    issue_id: i32,
    base_oid: &str,
) -> Result<WorktreeContext, LoopError> {
    let issue = loop_service::issue::get_issue(conn, issue_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("issue {issue_id}")))?;
    let space = loop_service::space::get_space(conn, issue.space_id)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("space {}", issue.space_id)))?;
    let repo = folder_service::get_folder_by_id(conn, space.folder_id)
        .await?
        .ok_or(LoopError::Detached)?;
    let repo_path = PathBuf::from(&repo.path);
    ensure_git_repo(&repo_path).await?;

    let branch = format!("loop/{}/issue-{}-integrate", issue.space_id, issue.seq_no);
    let worktree_path = data_dir
        .join("loop-worktrees")
        .join(issue.space_id.to_string())
        .join(format!("issue-{}-integrate", issue.seq_no));

    attach_or_rebuild_worktree(
        conn,
        &repo_path,
        space.folder_id,
        &branch,
        &worktree_path,
        base_oid,
        issue.base_branch.clone().unwrap_or_default(),
    )
    .await
}

/// Attach-or-rebuild a worktree at `worktree_path` on `branch`, cut from
/// `base_oid`. Reused as-is iff it exists on `branch` with `base_oid` an ancestor
/// of its HEAD (so committed work / an in-progress merge survive); otherwise
/// pruned, force-removed, and recreated `-B` from `base_oid`. Upserts the hidden
/// `loop_worktree` folder (parented to `repo_folder_id`) — task / integrate
/// worktrees have no issue column, so they re-attach by their deterministic path.
async fn attach_or_rebuild_worktree(
    conn: &DatabaseConnection,
    repo_path: &Path,
    repo_folder_id: i32,
    branch: &str,
    worktree_path: &Path,
    base_oid: &str,
    base_branch: String,
) -> Result<WorktreeContext, LoopError> {
    let ctx = |folder_id: i32| WorktreeContext {
        worktree_path: worktree_path.to_path_buf(),
        worktree_folder_id: folder_id,
        branch: branch.to_string(),
        base_branch: base_branch.clone(),
        base_commit: base_oid.to_string(),
    };

    if worktree_path.exists() {
        if let Ok(cur) = current_branch(worktree_path).await {
            if cur == branch && is_ancestor_of_head(worktree_path, base_oid).await? {
                let folder = folder_service::add_loop_worktree_folder(
                    conn,
                    &path_str(worktree_path),
                    repo_folder_id,
                )
                .await?;
                return Ok(ctx(folder.id));
            }
        }
    }

    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| LoopError::Git(format!("create worktree parent dir: {e}")))?;
    }
    let wt = path_str(worktree_path);
    let _ = run_git(repo_path, &["worktree", "prune"]).await;
    if worktree_path.exists() {
        let _ = run_git(repo_path, &["worktree", "remove", "--force", &wt]).await;
        let _ = std::fs::remove_dir_all(worktree_path);
    }
    let out = run_git(repo_path, &["worktree", "add", "-B", branch, &wt, base_oid]).await?;
    if !out.status.success() {
        return Err(LoopError::Git(format!("worktree add: {}", stderr_of(&out))));
    }
    let folder = folder_service::add_loop_worktree_folder(conn, &wt, repo_folder_id).await?;
    Ok(ctx(folder.id))
}

/// Stage everything and, if there is a non-empty diff, create an engine
/// checkpoint commit. Returns the new commit sha, or `None` when the tree was
/// already clean (no changes to accept).
pub async fn checkpoint(worktree_path: &Path, message: &str) -> Result<Option<String>, LoopError> {
    let add = run_git(worktree_path, &["add", "-A"]).await?;
    if !add.status.success() {
        return Err(LoopError::Git(format!("add -A: {}", stderr_of(&add))));
    }
    // `diff --cached --quiet`: exit 0 = no staged changes, 1 = changes present.
    let diff = run_git(worktree_path, &["diff", "--cached", "--quiet"]).await?;
    match diff.status.code() {
        Some(0) => return Ok(None),
        Some(1) => {}
        _ => {
            return Err(LoopError::Git(format!(
                "diff --cached: {}",
                stderr_of(&diff)
            )))
        }
    }

    let name_cfg = format!("user.name={ENGINE_NAME}");
    let email_cfg = format!("user.email={ENGINE_EMAIL}");
    let commit = run_git(
        worktree_path,
        &[
            "-c", &name_cfg, "-c", &email_cfg, "commit", "-m", message,
        ],
    )
    .await?;
    if !commit.status.success() {
        return Err(LoopError::Git(format!("commit: {}", stderr_of(&commit))));
    }
    let sha = run_git(worktree_path, &["rev-parse", "HEAD"]).await?;
    Ok(Some(stdout_trimmed(&sha)))
}

/// Discard all uncommitted changes, returning the worktree to its branch HEAD
/// (the latest accepted checkpoint). Never rewinds committed history.
pub async fn reset_to_head(worktree_path: &Path) -> Result<(), LoopError> {
    let reset = run_git(worktree_path, &["reset", "--hard", "HEAD"]).await?;
    if !reset.status.success() {
        return Err(LoopError::Git(format!("reset --hard: {}", stderr_of(&reset))));
    }
    let clean = run_git(worktree_path, &["clean", "-fd"]).await?;
    if !clean.status.success() {
        return Err(LoopError::Git(format!("clean -fd: {}", stderr_of(&clean))));
    }
    Ok(())
}

/// Whether the worktree has no uncommitted changes (tracked or untracked) — i.e.
/// it equals its branch HEAD. Used to assert all accepted work is committed
/// before finalize / merge.
pub async fn is_clean(worktree_path: &Path) -> Result<bool, LoopError> {
    let out = run_git(worktree_path, &["status", "--porcelain"]).await?;
    if !out.status.success() {
        return Err(LoopError::Git(format!(
            "status --porcelain: {}",
            stderr_of(&out)
        )));
    }
    Ok(stdout_trimmed(&out).is_empty())
}

/// Like [`is_clean`], but ignores untracked files (`--untracked-files=no`). Used
/// as the BASE-repo precondition before a `--no-ff` landing: untracked files are
/// harmless to a merge (git itself refuses if an incoming file would clobber an
/// untracked one, surfacing as a `Conflict`), so refusing on them would block
/// every merge in a normal dev checkout. Modified or staged TRACKED files remain
/// a real hazard — a checkout/merge could clobber them — and still report dirty.
pub async fn is_clean_tracked(repo_path: &Path) -> Result<bool, LoopError> {
    let out = run_git(
        repo_path,
        &["status", "--porcelain", "--untracked-files=no"],
    )
    .await?;
    if !out.status.success() {
        return Err(LoopError::Git(format!(
            "status --porcelain -uno: {}",
            stderr_of(&out)
        )));
    }
    Ok(stdout_trimmed(&out).is_empty())
}

/// Outcome of attempting to land an issue's loop branch onto its base branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The loop branch merged into the base branch (a `--no-ff` merge commit
    /// landed). The caller closes the issue and removes the worktree.
    Merged { merge_commit: String },
    /// `base_branch` no longer exists — a human deleted or renamed it.
    BaseGone,
    /// The base repo's working tree has uncommitted changes; we refuse to merge
    /// rather than disturb the human's in-progress state.
    BaseDirty,
    /// A merge conflict. `stage` is `"integrate"` when folding an advanced base
    /// into the loop branch, `"merge"` on the final landing. The merge was
    /// aborted and the trees restored; `detail` is git's conflict report.
    Conflict { stage: &'static str, detail: String },
    /// The base advanced and was folded in cleanly, but re-running the issue's
    /// validation suite on the integrated tree failed — the new base broke the
    /// work, so it must not land.
    RevalidationFailed { output: String },
}

/// Land an issue's loop branch onto its base branch (spec §4.10). Pure git +
/// deterministic validation — no DB, no engine state; the caller (engine
/// `merge_issue`) owns the per-repo serialization lock and the post-merge DB
/// lifecycle.
///
/// Stale-base aware: if `base_branch` advanced past the `base_commit` recorded
/// at trigger time, the new base is first folded into the loop branch (in the
/// worktree) and the issue's validation suite re-run there; only a clean,
/// re-validated integration proceeds to the final `--no-ff` landing. The landing
/// happens in the base repo's working tree, on `base_branch`, so that tree must
/// be clean — we never clobber uncommitted human state. A successful landing
/// leaves the base repo checked out on `base_branch` at the new merge commit.
#[allow(clippy::too_many_arguments)]
pub async fn merge_issue(
    repo_path: &Path,
    worktree_path: &Path,
    loop_branch: &str,
    base_branch: &str,
    base_commit: &str,
    validation_commands: &[String],
    iteration_timeout_secs: Option<u64>,
) -> Result<MergeOutcome, LoopError> {
    // 1. Base must still exist; its current tip tells us whether it advanced.
    let verify = run_git(
        repo_path,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{base_branch}"),
        ],
    )
    .await?;
    if !verify.status.success() {
        return Ok(MergeOutcome::BaseGone);
    }
    let base_tip = stdout_trimmed(&verify);

    // 2. Base advanced since trigger → fold it into the loop branch and re-validate
    //    on the integrated tree before landing.
    if base_tip != base_commit {
        let integrate = run_git(worktree_path, &["merge", "--no-edit", base_branch]).await?;
        if !integrate.status.success() {
            let detail = combined_output(&integrate);
            let _ = run_git(worktree_path, &["merge", "--abort"]).await;
            return Ok(MergeOutcome::Conflict {
                stage: "integrate",
                detail,
            });
        }
        let commands: Vec<String> = validation_commands
            .iter()
            .filter(|c| !c.trim().is_empty())
            .cloned()
            .collect();
        if !commands.is_empty() {
            let report = validation::run_validation(
                worktree_path,
                &commands,
                iteration_timeout_secs.map(Duration::from_secs),
            )
            .await?;
            if !report.passed() {
                return Ok(MergeOutcome::RevalidationFailed {
                    output: report.output,
                });
            }
        }
    }

    // 3. Land the loop branch on the base branch, in the base repo's working tree.
    //    Refuse modified/staged TRACKED files — never clobber uncommitted human
    //    state — but tolerate untracked files (harmless to the merge; git itself
    //    refuses if an incoming file would overwrite one).
    if !is_clean_tracked(repo_path).await? {
        return Ok(MergeOutcome::BaseDirty);
    }
    let original_branch = current_branch(repo_path).await?;
    if original_branch != base_branch {
        // `--no-overwrite-ignore`: abort rather than silently clobber a locally
        // gitignored file if the base branch tracks that path (git's default would
        // overwrite ignored files on checkout). Non-ignored untracked files are
        // refused by git regardless.
        let checkout =
            run_git(repo_path, &["checkout", "--no-overwrite-ignore", base_branch]).await?;
        if !checkout.status.success() {
            return Err(LoopError::Git(format!(
                "checkout {base_branch}: {}",
                stderr_of(&checkout)
            )));
        }
    }
    let merge = run_git(repo_path, &["merge", "--no-ff", "--no-edit", loop_branch]).await?;
    if !merge.status.success() {
        let detail = combined_output(&merge);
        let _ = run_git(repo_path, &["merge", "--abort"]).await;
        if original_branch != base_branch {
            let _ = run_git(repo_path, &["checkout", &original_branch]).await;
        }
        return Ok(MergeOutcome::Conflict {
            stage: "merge",
            detail,
        });
    }
    let merge_commit = head_commit(repo_path).await?;
    Ok(MergeOutcome::Merged { merge_commit })
}

/// Outcome of folding an issue's frozen task commits into the integrate branch
/// (the parallel result-stage fan-in, spec §4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FanInOutcome {
    /// Every frozen task commit is integrated and re-validation passed; `tip` is
    /// the integrate branch tip the caller CAS-lands onto the issue branch.
    Integrated { tip: String },
    /// A task commit's merge conflicted. The in-progress merge (`MERGE_HEAD`) is
    /// LEFT in place — NOT aborted — for a resolution agent to finish with
    /// `git commit` (preserving both parents). `task_id` is the conflicting task.
    Conflict { task_id: i32, detail: String },
    /// All commits merged cleanly, but re-running the issue's validation suite on
    /// the integrated tree failed — the combination broke; it must not land.
    RevalidationFailed { output: String },
}

/// Merge each frozen task commit into the integrate branch (the worktree's
/// current HEAD), in the given topological order, then re-validate the result.
///
/// Idempotent + resumable: a commit already an ancestor of the integrate tip is
/// skipped, so re-entry after a partial fan-in (crash, or returning to finish
/// after a resolved conflict) only does the remaining work. On conflict it leaves
/// the in-progress merge in place (no `--abort`) and returns `Conflict{task_id}`;
/// the caller dispatches a resolver that MUST `git commit` to complete the merge.
///
/// Pure git + deterministic validation — no DB, no engine state. The caller owns
/// the manifest session lock + the CAS landing.
pub async fn fan_in_tasks(
    integrate_worktree: &Path,
    ordered_frozen: &[(i32, String)],
    validation_commands: &[String],
    iteration_timeout_secs: Option<u64>,
) -> Result<FanInOutcome, LoopError> {
    for (task_id, sha) in ordered_frozen {
        // Already merged (resumable / idempotent) → skip.
        let anc = run_git(
            integrate_worktree,
            &["merge-base", "--is-ancestor", sha, "HEAD"],
        )
        .await?;
        if anc.status.success() {
            continue;
        }
        // `--no-edit` + `GIT_MERGE_AUTOEDIT=no`: never open an editor for the merge
        // commit message (which would hang a headless engine). `-c user.name/email`:
        // a `--no-ff` merge writes a merge commit, which needs a committer identity —
        // without it git would *fail the merge* in a checkout that has no configured
        // user, which we must not misread as a conflict (see below).
        let name_cfg = format!("user.name={ENGINE_NAME}");
        let email_cfg = format!("user.email={ENGINE_EMAIL}");
        let merge = crate::process::tokio_command("git")
            .args([
                "-c",
                &name_cfg,
                "-c",
                &email_cfg,
                "merge",
                "--no-ff",
                "--no-edit",
                sha.as_str(),
            ])
            .current_dir(integrate_worktree)
            .env("GIT_MERGE_AUTOEDIT", "no")
            .output()
            .await
            .map_err(|e| LoopError::Git(format!("git merge {sha}: {e}")))?;
        if !merge.status.success() {
            // A failed merge is only a *conflict* if it left a merge in progress
            // (`MERGE_HEAD` + unmerged index). Other failures — a bad/missing object,
            // unrelated histories, a rejecting hook — are NOT something a resolution
            // agent can fix; dispatching one would spin (it would find nothing to
            // resolve). Surface those as a hard error instead of a phantom conflict.
            if integrate_in_progress(integrate_worktree).await {
                // Leave the in-progress merge for a resolution agent; DO NOT abort.
                return Ok(FanInOutcome::Conflict {
                    task_id: *task_id,
                    detail: combined_output(&merge),
                });
            }
            return Err(LoopError::Git(format!(
                "fan-in merge of task {task_id} ({sha}) failed without a conflict: {}",
                combined_output(&merge)
            )));
        }
    }

    let commands: Vec<String> = validation_commands
        .iter()
        .filter(|c| !c.trim().is_empty())
        .cloned()
        .collect();
    if !commands.is_empty() {
        let report = validation::run_validation(
            integrate_worktree,
            &commands,
            iteration_timeout_secs.map(Duration::from_secs),
        )
        .await?;
        if !report.passed() {
            return Ok(FanInOutcome::RevalidationFailed {
                output: report.output,
            });
        }
    }
    let tip = head_commit(integrate_worktree).await?;
    Ok(FanInOutcome::Integrated { tip })
}

/// Whether the worktree has a merge in progress (`MERGE_HEAD` exists) — a conflict
/// left for a resolver, or a merge mid-commit. Lets the driver tell "resume the
/// in-flight merge" from "start a fresh fan-in".
pub async fn integrate_in_progress(worktree: &Path) -> bool {
    run_git(worktree, &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"])
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Atomically advance `branch` from `expected_old` to `new` via
/// `git update-ref refs/heads/{branch} <new> <expected_old>`: succeeds
/// (`Ok(true)`) only if the branch still points at `expected_old`, else a
/// lost-CAS race (`Ok(false)`). Touches only the ref — never a working tree (so
/// the caller must `reset --hard` the issue worktree to the new tip afterward).
pub async fn cas_advance_branch(
    repo_path: &Path,
    branch: &str,
    new: &str,
    expected_old: &str,
) -> Result<bool, LoopError> {
    let refname = format!("refs/heads/{branch}");
    let out = run_git(repo_path, &["update-ref", &refname, new, expected_old]).await?;
    if out.status.success() {
        return Ok(true);
    }
    // A non-zero exit is NOT automatically a lost CAS — `update-ref` also fails on
    // lock contention, a bad object, or ref corruption. Treating those as "the
    // branch moved" would wrongly discard the whole integration (including any
    // conflict-resolution commits). Disambiguate by re-reading the ref: only a tip
    // that actually moved off `expected_old` is a genuine lost CAS (`Ok(false)`);
    // anything else is a hard error the caller must surface, not swallow.
    let cur = run_git(repo_path, &["rev-parse", "--verify", "--quiet", &refname]).await?;
    let cur = stdout_trimmed(&cur);
    if cur != expected_old {
        Ok(false)
    } else {
        Err(LoopError::Git(format!(
            "update-ref {branch} (ref still at expected tip): {}",
            stderr_of(&out)
        )))
    }
}

/// Whether `ancestor` is an ancestor of `descendant` in `repo` (`merge-base
/// --is-ancestor`: exit 0 = yes). Operates on the object store, so any path inside
/// the repo works. Backs the fan-in's "already landed" detection.
pub async fn is_ancestor(
    repo: &Path,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, LoopError> {
    let out = run_git(repo, &["merge-base", "--is-ancestor", ancestor, descendant]).await?;
    Ok(out.status.success())
}

/// Remove the worktree directory and its administrative entry (best-effort
/// `--force` to tolerate a dirty tree). The branch is left intact — call
/// [`delete_branch`] separately for paths that should also drop it.
pub async fn remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<(), LoopError> {
    let wt = path_str(worktree_path);
    let out = run_git(repo_path, &["worktree", "remove", "--force", &wt]).await?;
    if !out.status.success() {
        return Err(LoopError::Git(format!(
            "worktree remove: {}",
            stderr_of(&out)
        )));
    }
    Ok(())
}

/// Delete an engine-owned `loop/*` branch after its worktree has been removed (a
/// branch checked out in a worktree cannot be deleted). `force` selects `-D`
/// (unconditional — for permanent issue/space deletion, which discards unmerged
/// WIP by user intent) versus `-d` (safe — git refuses unless the branch is
/// already merged, used after a successful landing as a guard that we never drop
/// unmerged work). Call sites treat this as best-effort: a missing branch or a
/// safe-delete refusal is not fatal (the create path reconciles any leftover).
pub async fn delete_branch(repo_path: &Path, branch: &str, force: bool) -> Result<(), LoopError> {
    let flag = if force { "-D" } else { "-d" };
    let out = run_git(repo_path, &["branch", flag, branch]).await?;
    if !out.status.success() {
        return Err(LoopError::Git(format!(
            "branch {flag} {branch}: {}",
            stderr_of(&out)
        )));
    }
    Ok(())
}

/// `(path, branch)` for every worktree registered in `repo_path`
/// (`git worktree list --porcelain`); `branch` is `None` for a detached worktree.
/// Backs the per-issue subtree sweeps below.
pub async fn list_worktrees(repo_path: &Path) -> Result<Vec<(String, Option<String>)>, LoopError> {
    let out = run_git(repo_path, &["worktree", "list", "--porcelain"]).await?;
    if !out.status.success() {
        return Err(LoopError::Git(format!("worktree list: {}", stderr_of(&out))));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();
    let mut cur_path: Option<String> = None;
    let mut cur_branch: Option<String> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(path) = cur_path.take() {
                result.push((path, cur_branch.take()));
            }
            cur_path = Some(p.to_string());
            cur_branch = None;
        } else if let Some(b) = line.strip_prefix("branch ") {
            cur_branch = Some(b.trim().trim_start_matches("refs/heads/").to_string());
        }
    }
    if let Some(path) = cur_path.take() {
        result.push((path, cur_branch.take()));
    }
    Ok(result)
}

/// Best-effort canonical path (resolves symlinks like macOS `/var`→`/private/var`,
/// so `git worktree list`'s real paths compare equal to our data-dir paths);
/// falls back to the raw string when the path no longer exists.
fn canon(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|c| c.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string())
}

/// Whether `path` is one of an issue's per-task / integrate worktrees — a sibling
/// of `issue_worktree` at `{issue_worktree}-tasks*` or `{issue_worktree}-integrate`.
/// The `-` after the seq disambiguates `issue-1` from `issue-10`. Both sides are
/// canonicalized so a symlinked temp/data dir doesn't defeat the prefix match.
fn is_issue_subtree(path: &str, issue_worktree: &Path) -> bool {
    let base = canon(&issue_worktree.to_string_lossy());
    let p = canon(path);
    p.starts_with(&format!("{base}-tasks")) || p.starts_with(&format!("{base}-integrate"))
}

/// Reset every per-task + integrate worktree of an issue to its branch HEAD —
/// boot recovery's clean-tree restore for parallel work (discards only
/// uncommitted crash residue; committed task checkpoints survive). Best-effort.
pub async fn reset_issue_subtree(repo_path: &Path, issue_worktree: &Path) -> Result<(), LoopError> {
    for (path, _) in list_worktrees(repo_path).await? {
        if is_issue_subtree(&path, issue_worktree) {
            let p = Path::new(&path);
            if p.exists() {
                // NEVER reset a worktree with a merge in progress: the integrate
                // worktree's `MERGE_HEAD` (a fan-in conflict awaiting / under a
                // resolver) IS the state to preserve, and this sweep runs OUTSIDE
                // the fan-in's in-flight gate. `reset --hard` would discard the
                // in-progress merge and force the whole conflict to be re-resolved.
                // The fan-in's own recovery (`integrate_in_progress`) handles it.
                if integrate_in_progress(p).await {
                    continue;
                }
                let _ = reset_to_head(p).await;
            }
        }
    }
    Ok(())
}

/// Remove every per-task + integrate worktree of an issue and (when
/// `delete_branches`) force-delete their branches. Best-effort — used by cancel
/// (keep branches for audit) / merge teardown / permanent delete (drop branches).
pub async fn remove_issue_subtree(
    repo_path: &Path,
    issue_worktree: &Path,
    delete_branches: bool,
) -> Result<(), LoopError> {
    for (path, branch) in list_worktrees(repo_path).await? {
        if is_issue_subtree(&path, issue_worktree) {
            let _ = remove_worktree(repo_path, Path::new(&path)).await;
            if delete_branches {
                if let Some(b) = branch {
                    let _ = delete_branch(repo_path, &b, true).await;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::folder::FolderKind;
    use crate::db::entities::loop_issue::IssuePriority;
    use crate::db::service::loop_service;
    use crate::db::test_helpers::{fresh_disk_db, seed_folder};
    use crate::models::loops::IssueConfig;
    use std::process::Command as StdCommand;

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

    /// Build a repo + db + space + issue and return (db, data_dir, issue_id,
    /// space_id, seq). Keeps the tempdirs alive via the returned guards.
    async fn setup() -> (
        crate::db::AppDatabase,
        tempfile::TempDir,
        tempfile::TempDir,
        i32,
        i32,
        i32,
    ) {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let data = tempfile::tempdir().unwrap();
        let db = fresh_disk_db(data.path()).await;
        let folder_id = seed_folder(&db, &repo.path().to_string_lossy()).await;
        let space = loop_service::space::create_space(&db.conn, "S", folder_id)
            .await
            .unwrap();
        let issue = loop_service::issue::create_issue(
            &db.conn,
            space.id,
            "Build it",
            "do the thing",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        (db, repo, data, issue.row.id, space.id, issue.row.seq_no)
    }

    #[tokio::test]
    async fn ensure_worktree_creates_branch_dir_folder_and_records_base() {
        let (db, _repo, data, issue_id, space_id, seq) = setup().await;

        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();

        assert!(ctx.worktree_path.is_dir(), "worktree dir exists");
        assert_eq!(ctx.branch, format!("loop/{space_id}/issue-{seq}"));
        assert!(!ctx.base_branch.is_empty());
        assert_eq!(ctx.base_commit.len(), 40, "full sha recorded");

        // Folder registered as hidden loop_worktree, parented to the repo folder.
        let folder = folder_service::get_folder_by_id(&db.conn, ctx.worktree_folder_id)
            .await
            .unwrap()
            .expect("worktree folder row");
        assert_eq!(folder.kind, FolderKind::LoopWorktree);

        // Issue back-references the worktree and its base.
        let issue = loop_service::issue::get_issue(&db.conn, issue_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(issue.worktree_folder_id, Some(ctx.worktree_folder_id));
        assert_eq!(issue.base_commit.as_deref(), Some(ctx.base_commit.as_str()));

        // Idempotent: second call re-attaches the same folder, no new worktree.
        let ctx2 = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        assert_eq!(ctx2.worktree_folder_id, ctx.worktree_folder_id);
    }

    #[tokio::test]
    async fn ensure_worktree_reconciles_leftover_branch_and_dir() {
        let (db, repo, data, issue_id, space_id, seq) = setup().await;
        let branch = format!("loop/{space_id}/issue-{seq}");

        // A prior life left the branch behind (teardown keeps it; a DB reset reuses
        // the same name). Point it at the *old* HEAD, then advance the base so we
        // can prove the re-created worktree starts from the current HEAD, not the
        // stale branch tip.
        git(repo.path(), &["branch", &branch]);
        let stale_tip = git_out(repo.path(), &["rev-parse", &branch]);
        std::fs::write(repo.path().join("advance.txt"), "more\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "base advance"]);
        let new_head = git_out(repo.path(), &["rev-parse", "HEAD"]);
        assert_ne!(stale_tip, new_head, "base advanced past the stale branch");

        // An orphaned directory also sits exactly where the worktree will go.
        let wt = data
            .path()
            .join("loop-worktrees")
            .join(space_id.to_string())
            .join(format!("issue-{seq}"));
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("junk.txt"), "leftover\n").unwrap();

        // Was fatal: "a branch named 'loop/.../issue-...' already exists".
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        assert_eq!(ctx.branch, branch);
        // Branch reset to the current base HEAD; worktree checked out there.
        assert_eq!(ctx.base_commit, new_head);
        assert_eq!(git_out(&ctx.worktree_path, &["rev-parse", "HEAD"]), new_head);
        // The orphaned dir was wiped and recreated clean.
        assert!(!ctx.worktree_path.join("junk.txt").exists());
    }

    #[tokio::test]
    async fn delete_branch_safe_refuses_unmerged_force_removes() {
        let (db, repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        loop_commit(&ctx.worktree_path, "feature.txt", "work\n").await;

        // A branch checked out in a worktree can't be deleted — remove it first.
        remove_worktree(repo.path(), &ctx.worktree_path)
            .await
            .unwrap();

        // Safe delete refuses an unmerged branch (the guard behind the merge path)…
        assert!(delete_branch(repo.path(), &ctx.branch, false)
            .await
            .is_err());
        assert!(branch_exists(repo.path(), &ctx.branch));
        // …force delete drops it (the permanent-delete path).
        delete_branch(repo.path(), &ctx.branch, true)
            .await
            .unwrap();
        assert!(!branch_exists(repo.path(), &ctx.branch));
    }

    #[tokio::test]
    async fn checkpoint_commits_changes_then_noops_when_clean() {
        let (db, _repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();

        std::fs::write(ctx.worktree_path.join("feature.txt"), "work\n").unwrap();
        let sha = checkpoint(&ctx.worktree_path, "loop: checkpoint")
            .await
            .unwrap();
        assert!(sha.is_some(), "non-empty diff produces a commit");

        let again = checkpoint(&ctx.worktree_path, "loop: checkpoint")
            .await
            .unwrap();
        assert!(again.is_none(), "clean tree produces no commit");
    }

    #[tokio::test]
    async fn reset_to_head_discards_uncommitted_keeps_committed() {
        let (db, _repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();

        // Commit one file, then leave a second uncommitted + an untracked file.
        std::fs::write(ctx.worktree_path.join("kept.txt"), "keep\n").unwrap();
        checkpoint(&ctx.worktree_path, "loop: keep")
            .await
            .unwrap()
            .expect("committed");
        std::fs::write(ctx.worktree_path.join("kept.txt"), "dirty\n").unwrap();
        std::fs::write(ctx.worktree_path.join("scratch.txt"), "temp\n").unwrap();

        reset_to_head(&ctx.worktree_path).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(ctx.worktree_path.join("kept.txt")).unwrap(),
            "keep\n",
            "committed file restored to HEAD"
        );
        assert!(
            !ctx.worktree_path.join("scratch.txt").exists(),
            "untracked file removed"
        );
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = StdCommand::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn branch_exists(repo: &Path, branch: &str) -> bool {
        StdCommand::new("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .current_dir(repo)
            .status()
            .expect("spawn git")
            .success()
    }

    /// One loop commit (a feature file) checkpointed onto the issue branch.
    async fn loop_commit(worktree_path: &Path, file: &str, body: &str) {
        std::fs::write(worktree_path.join(file), body).unwrap();
        checkpoint(worktree_path, &format!("loop: {file}"))
            .await
            .unwrap()
            .expect("committed");
    }

    fn parent_count(repo: &Path) -> usize {
        git_out(repo, &["log", "-1", "--format=%P"])
            .split_whitespace()
            .count()
    }

    #[tokio::test]
    async fn merge_clean_base_unchanged_lands_loop() {
        let (db, repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        loop_commit(&ctx.worktree_path, "feature.txt", "work\n").await;

        let outcome = merge_issue(
            repo.path(),
            &ctx.worktree_path,
            &ctx.branch,
            &ctx.base_branch,
            &ctx.base_commit,
            &[],
            None,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, MergeOutcome::Merged { .. }));
        // The base repo (on the base branch) now carries the loop's work behind a
        // no-ff merge commit (two parents).
        assert!(repo.path().join("feature.txt").exists());
        assert_eq!(parent_count(repo.path()), 2, "--no-ff merge commit");
    }

    #[tokio::test]
    async fn merge_stale_base_integrates_then_lands() {
        let (db, repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        loop_commit(&ctx.worktree_path, "feature.txt", "work\n").await;

        // Advance the base branch (a non-conflicting file) after the worktree was cut.
        std::fs::write(repo.path().join("base-new.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "base advance"]);

        let outcome = merge_issue(
            repo.path(),
            &ctx.worktree_path,
            &ctx.branch,
            &ctx.base_branch,
            &ctx.base_commit,
            &[],
            None,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, MergeOutcome::Merged { .. }));
        // Both the advanced base file and the loop work are present on the base.
        assert!(repo.path().join("base-new.txt").exists());
        assert!(repo.path().join("feature.txt").exists());
    }

    #[tokio::test]
    async fn merge_conflict_integrate_aborts_and_preserves_base() {
        let (db, repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        // Loop and base both edit README differently → integrate conflicts.
        loop_commit(&ctx.worktree_path, "README.md", "loop change\n").await;
        std::fs::write(repo.path().join("README.md"), "base change\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "base readme"]);

        let outcome = merge_issue(
            repo.path(),
            &ctx.worktree_path,
            &ctx.branch,
            &ctx.base_branch,
            &ctx.base_commit,
            &[],
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            MergeOutcome::Conflict { stage: "integrate", .. }
        ));
        // Worktree restored (merge aborted) and the base branch untouched.
        assert!(is_clean(&ctx.worktree_path).await.unwrap());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
            "base change\n"
        );
        assert_eq!(parent_count(repo.path()), 1, "no merge landed on base");
    }

    #[tokio::test]
    async fn merge_revalidation_failure_does_not_land() {
        let (db, repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        loop_commit(&ctx.worktree_path, "feature.txt", "work\n").await;
        // Advance base on a different file so integration is clean.
        std::fs::write(repo.path().join("base-new.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "base advance"]);

        // A validation command that exits non-zero (git is available cross-platform).
        let cmds = vec!["git rev-parse --verify refs/heads/no-such-ref".to_string()];
        let outcome = merge_issue(
            repo.path(),
            &ctx.worktree_path,
            &ctx.branch,
            &ctx.base_branch,
            &ctx.base_commit,
            &cmds,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, MergeOutcome::RevalidationFailed { .. }));
        // The loop work never reached the base branch.
        assert!(!repo.path().join("feature.txt").exists());
    }

    #[tokio::test]
    async fn merge_dirty_base_refuses() {
        let (db, repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        loop_commit(&ctx.worktree_path, "feature.txt", "work\n").await;
        // Modify a TRACKED file in the base repo (README.md is committed by
        // init_repo) — a real hazard a merge could clobber.
        std::fs::write(repo.path().join("README.md"), "locally modified\n").unwrap();

        let outcome = merge_issue(
            repo.path(),
            &ctx.worktree_path,
            &ctx.branch,
            &ctx.base_branch,
            &ctx.base_commit,
            &[],
            None,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, MergeOutcome::BaseDirty));
        assert!(!repo.path().join("feature.txt").exists(), "nothing landed");
    }

    #[tokio::test]
    async fn merge_untracked_base_lands() {
        let (db, repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        loop_commit(&ctx.worktree_path, "feature.txt", "work\n").await;
        // An UNTRACKED file in the base repo must NOT block the merge — it is
        // harmless to a --no-ff landing (the common dev-checkout case).
        std::fs::write(repo.path().join("scratch.txt"), "untracked\n").unwrap();

        let outcome = merge_issue(
            repo.path(),
            &ctx.worktree_path,
            &ctx.branch,
            &ctx.base_branch,
            &ctx.base_commit,
            &[],
            None,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, MergeOutcome::Merged { .. }));
        assert!(repo.path().join("feature.txt").exists(), "loop work landed");
        // The untracked file is left untouched.
        assert!(repo.path().join("scratch.txt").exists());
    }

    #[tokio::test]
    async fn merge_base_gone() {
        let (db, repo, data, issue_id, _space, _seq) = setup().await;
        let ctx = ensure_worktree(&db.conn, data.path(), issue_id)
            .await
            .unwrap();
        loop_commit(&ctx.worktree_path, "feature.txt", "work\n").await;

        let outcome = merge_issue(
            repo.path(),
            &ctx.worktree_path,
            &ctx.branch,
            "no-such-base",
            &ctx.base_commit,
            &[],
            None,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, MergeOutcome::BaseGone));
    }

    // ---- Per-task worktrees (Phase 1) ----

    async fn mk_task(
        db: &crate::db::AppDatabase,
        space_id: i32,
        issue_id: i32,
        title: &str,
    ) -> i32 {
        use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus};
        use crate::db::entities::loop_artifact_revision::ActorKind;
        loop_service::artifact::create_artifact(
            &db.conn,
            space_id,
            issue_id,
            ArtifactKind::Task,
            title,
            ArtifactStatus::Pending,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap()
        .id
    }

    async fn set_fan_in_commit(db: &crate::db::AppDatabase, task_id: i32, sha: &str) {
        let row = loop_artifact::Entity::find_by_id(task_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let mut active = row.into_active_model();
        active.fan_in_commit = Set(Some(sha.to_string()));
        active.update(&db.conn).await.unwrap();
    }

    #[tokio::test]
    async fn ensure_task_worktree_creates_from_issue_head() {
        let (db, _repo, data, issue_id, space_id, seq) = setup().await;
        // The issue worktree (and its branch) must exist first — root tasks cut
        // from the issue branch tip.
        let issue_ctx = ensure_worktree(&db.conn, data.path(), issue_id).await.unwrap();
        let issue_head = git_out(&issue_ctx.worktree_path, &["rev-parse", "HEAD"]);

        let task = mk_task(&db, space_id, issue_id, "T").await;
        let ctx = ensure_task_worktree(&db.conn, data.path(), issue_id, task)
            .await
            .unwrap();

        assert!(ctx.worktree_path.is_dir());
        assert_eq!(ctx.branch, format!("loop/{space_id}/issue-{seq}-task-{task}"));
        assert_eq!(git_out(&ctx.worktree_path, &["rev-parse", "HEAD"]), issue_head);
        // Sibling of the issue worktree, never nested under it.
        assert!(!ctx.worktree_path.starts_with(&issue_ctx.worktree_path));
    }

    #[tokio::test]
    async fn ensure_task_worktree_creates_from_predecessor_frozen_sha() {
        let (db, _repo, data, issue_id, space_id, _seq) = setup().await;
        let issue_ctx = ensure_worktree(&db.conn, data.path(), issue_id).await.unwrap();
        // The frozen sha the predecessor "produced".
        loop_commit(&issue_ctx.worktree_path, "pred.txt", "pred work\n").await;
        let frozen = git_out(&issue_ctx.worktree_path, &["rev-parse", "HEAD"]);

        let pred = mk_task(&db, space_id, issue_id, "pred").await;
        set_fan_in_commit(&db, pred, &frozen).await;
        // Advance the issue branch PAST the frozen sha, to prove the successor
        // cuts from the FROZEN commit, never the live tip.
        loop_commit(&issue_ctx.worktree_path, "more.txt", "drift\n").await;
        let live_tip = git_out(&issue_ctx.worktree_path, &["rev-parse", "HEAD"]);
        assert_ne!(frozen, live_tip);

        let succ = mk_task(&db, space_id, issue_id, "succ").await;
        loop_service::link::create_link(&db.conn, space_id, succ, pred, LinkKind::DependsOn)
            .await
            .unwrap();
        let ctx = ensure_task_worktree(&db.conn, data.path(), issue_id, succ)
            .await
            .unwrap();

        assert_eq!(
            git_out(&ctx.worktree_path, &["rev-parse", "HEAD"]),
            frozen,
            "successor cut from predecessor's frozen sha, not the live tip"
        );
    }

    #[tokio::test]
    async fn ensure_task_worktree_reattach_validates_identity() {
        let (db, _repo, data, issue_id, space_id, _seq) = setup().await;
        ensure_worktree(&db.conn, data.path(), issue_id).await.unwrap();
        let task = mk_task(&db, space_id, issue_id, "T").await;
        let ctx1 = ensure_task_worktree(&db.conn, data.path(), issue_id, task)
            .await
            .unwrap();

        // Corrupt identity: switch the worktree onto a different branch.
        git(&ctx1.worktree_path, &["checkout", "-b", "rogue"]);
        assert_ne!(
            git_out(&ctx1.worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            ctx1.branch
        );

        // Re-attach detects the mismatch and rebuilds onto the task branch.
        let ctx2 = ensure_task_worktree(&db.conn, data.path(), issue_id, task)
            .await
            .unwrap();
        assert_eq!(
            git_out(&ctx2.worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            ctx2.branch,
            "rebuilt onto the task branch"
        );
    }

    #[tokio::test]
    async fn ensure_task_worktree_no_b_on_live_branch() {
        let (db, _repo, data, issue_id, space_id, _seq) = setup().await;
        ensure_worktree(&db.conn, data.path(), issue_id).await.unwrap();
        let task = mk_task(&db, space_id, issue_id, "T").await;
        let ctx1 = ensure_task_worktree(&db.conn, data.path(), issue_id, task)
            .await
            .unwrap();
        loop_commit(&ctx1.worktree_path, "feature.txt", "work\n").await;
        let committed = git_out(&ctx1.worktree_path, &["rev-parse", "HEAD"]);

        // Re-attach must NOT rewind a task branch carrying committed work.
        let ctx2 = ensure_task_worktree(&db.conn, data.path(), issue_id, task)
            .await
            .unwrap();
        assert_eq!(
            ctx2.worktree_folder_id, ctx1.worktree_folder_id,
            "same folder reused"
        );
        assert_eq!(
            git_out(&ctx2.worktree_path, &["rev-parse", "HEAD"]),
            committed,
            "committed work preserved (no -B rewind)"
        );
        assert!(ctx2.worktree_path.join("feature.txt").exists());
    }

    // ---- Fan-in (Phase 1) ----

    /// Two independent task commits (distinct files) off `base`, plus an
    /// `integrate` branch at `base` checked out. Returns (base, sha_a, sha_b).
    fn two_independent_tasks(dir: &Path) -> (String, String, String) {
        let base = git_out(dir, &["rev-parse", "HEAD"]);
        git(dir, &["checkout", "-q", "-b", "taskA", &base]);
        std::fs::write(dir.join("a.txt"), "A\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "A"]);
        let sha_a = git_out(dir, &["rev-parse", "HEAD"]);
        git(dir, &["checkout", "-q", "-b", "taskB", &base]);
        std::fs::write(dir.join("b.txt"), "B\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "B"]);
        let sha_b = git_out(dir, &["rev-parse", "HEAD"]);
        git(dir, &["checkout", "-q", "-b", "integrate", &base]);
        (base, sha_a, sha_b)
    }

    #[tokio::test]
    async fn fan_in_clean_two_branches() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let (_base, a, b) = two_independent_tasks(dir.path());
        let out = fan_in_tasks(dir.path(), &[(1, a), (2, b)], &[], None)
            .await
            .unwrap();
        let tip = match out {
            FanInOutcome::Integrated { tip } => tip,
            o => panic!("expected Integrated, got {o:?}"),
        };
        assert_eq!(git_out(dir.path(), &["rev-parse", "HEAD"]), tip);
        assert!(dir.path().join("a.txt").exists() && dir.path().join("b.txt").exists());
    }

    #[tokio::test]
    async fn fan_in_skips_already_merged() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let (_base, a, b) = two_independent_tasks(dir.path());
        let first = fan_in_tasks(dir.path(), &[(1, a.clone()), (2, b.clone())], &[], None)
            .await
            .unwrap();
        let tip1 = match first {
            FanInOutcome::Integrated { tip } => tip,
            o => panic!("{o:?}"),
        };
        // Re-run the same set → all ancestors → skipped, tip unchanged.
        let again = fan_in_tasks(dir.path(), &[(1, a), (2, b)], &[], None)
            .await
            .unwrap();
        assert_eq!(
            again,
            FanInOutcome::Integrated { tip: tip1 },
            "already-merged commits are skipped (idempotent)"
        );
    }

    #[tokio::test]
    async fn fan_in_resume_after_partial() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let (_base, a, b) = two_independent_tasks(dir.path());
        // Partial: integrate only A.
        fan_in_tasks(dir.path(), &[(1, a.clone())], &[], None)
            .await
            .unwrap();
        assert!(dir.path().join("a.txt").exists() && !dir.path().join("b.txt").exists());
        // Re-enter with [A, B]: A skipped, only B merged.
        let out = fan_in_tasks(dir.path(), &[(1, a), (2, b)], &[], None)
            .await
            .unwrap();
        assert!(matches!(out, FanInOutcome::Integrated { .. }));
        assert!(
            dir.path().join("b.txt").exists(),
            "resume integrated the remaining task"
        );
    }

    #[tokio::test]
    async fn fan_in_conflict_returns_task() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let base = git_out(dir.path(), &["rev-parse", "HEAD"]);
        // Both tasks edit the SAME file differently → the second merge conflicts.
        git(dir.path(), &["checkout", "-q", "-b", "taskA", &base]);
        std::fs::write(dir.path().join("README.md"), "A\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "A"]);
        let a = git_out(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["checkout", "-q", "-b", "taskB", &base]);
        std::fs::write(dir.path().join("README.md"), "B\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "B"]);
        let b = git_out(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["checkout", "-q", "-b", "integrate", &base]);

        let out = fan_in_tasks(dir.path(), &[(1, a), (2, b)], &[], None)
            .await
            .unwrap();
        match out {
            FanInOutcome::Conflict { task_id, .. } => {
                assert_eq!(task_id, 2, "the conflicting (second) task is reported")
            }
            o => panic!("expected Conflict, got {o:?}"),
        }
        // The in-progress merge is LEFT for a resolver (not aborted).
        assert!(
            integrate_in_progress(dir.path()).await,
            "MERGE_HEAD preserved for the resolution agent"
        );
    }

    #[tokio::test]
    async fn fan_in_revalidation_fail() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let (_base, a, _b) = two_independent_tasks(dir.path());
        // A validation command that exits non-zero (git is cross-platform).
        let cmds = vec!["git rev-parse --verify refs/heads/no-such-ref".to_string()];
        let out = fan_in_tasks(dir.path(), &[(1, a)], &cmds, None)
            .await
            .unwrap();
        assert!(matches!(out, FanInOutcome::RevalidationFailed { .. }));
    }

    #[tokio::test]
    async fn cas_advance_branch_atomic() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let base = git_out(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["checkout", "-q", "-b", "feature", &base]);
        std::fs::write(dir.path().join("f.txt"), "f\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "f"]);
        let new = git_out(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["branch", "target", &base]);

        // CAS base→new applies.
        assert!(cas_advance_branch(dir.path(), "target", &new, &base)
            .await
            .unwrap());
        assert_eq!(git_out(dir.path(), &["rev-parse", "target"]), new);
        // A stale expected_old now misses, leaving the ref unchanged.
        assert!(!cas_advance_branch(dir.path(), "target", &base, &base)
            .await
            .unwrap());
        assert_eq!(
            git_out(dir.path(), &["rev-parse", "target"]),
            new,
            "ref unchanged after a CAS miss"
        );
    }

    // ---- Per-issue subtree lifecycle (Phase 1) ----

    #[tokio::test]
    async fn remove_issue_subtree_removes_task_and_integrate() {
        let (db, repo, data, issue_id, space_id, _seq) = setup().await;
        let issue_ctx = ensure_worktree(&db.conn, data.path(), issue_id).await.unwrap();
        let t1 = mk_task(&db, space_id, issue_id, "T1").await;
        let task_ctx = ensure_task_worktree(&db.conn, data.path(), issue_id, t1)
            .await
            .unwrap();
        let integ_ctx =
            ensure_integrate_worktree(&db.conn, data.path(), issue_id, &issue_ctx.base_commit)
                .await
                .unwrap();
        assert!(task_ctx.worktree_path.is_dir() && integ_ctx.worktree_path.is_dir());

        remove_issue_subtree(repo.path(), &issue_ctx.worktree_path, true)
            .await
            .unwrap();

        assert!(!task_ctx.worktree_path.exists(), "task worktree removed");
        assert!(!integ_ctx.worktree_path.exists(), "integrate worktree removed");
        assert!(issue_ctx.worktree_path.is_dir(), "issue worktree untouched");
        assert!(!branch_exists(repo.path(), &task_ctx.branch));
        assert!(!branch_exists(repo.path(), &integ_ctx.branch));
        assert!(
            branch_exists(repo.path(), &issue_ctx.branch),
            "issue branch kept"
        );
    }

    #[tokio::test]
    async fn reset_issue_subtree_restores_task_worktrees() {
        let (db, repo, data, issue_id, space_id, _seq) = setup().await;
        let issue_ctx = ensure_worktree(&db.conn, data.path(), issue_id).await.unwrap();
        let t1 = mk_task(&db, space_id, issue_id, "T1").await;
        let task_ctx = ensure_task_worktree(&db.conn, data.path(), issue_id, t1)
            .await
            .unwrap();
        // Commit work, then leave uncommitted residue (simulating a crash).
        loop_commit(&task_ctx.worktree_path, "kept.txt", "keep\n").await;
        std::fs::write(task_ctx.worktree_path.join("kept.txt"), "dirty\n").unwrap();
        std::fs::write(task_ctx.worktree_path.join("scratch.txt"), "tmp\n").unwrap();

        reset_issue_subtree(repo.path(), &issue_ctx.worktree_path)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(task_ctx.worktree_path.join("kept.txt")).unwrap(),
            "keep\n",
            "committed work restored to HEAD"
        );
        assert!(
            !task_ctx.worktree_path.join("scratch.txt").exists(),
            "uncommitted residue discarded"
        );
    }

    #[tokio::test]
    async fn fan_in_nonconflict_failure_is_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let base = git_out(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["checkout", "-q", "-b", "integrate", &base]);
        // A merge of a non-existent object fails WITHOUT leaving a merge in progress
        // (no MERGE_HEAD). That is NOT a conflict a resolver could fix — it must
        // surface as a hard error, not a phantom `Conflict`.
        let bogus = "0".repeat(40);
        let out = fan_in_tasks(dir.path(), &[(7, bogus)], &[], None).await;
        assert!(out.is_err(), "non-conflict merge failure is a hard error");
        assert!(
            !integrate_in_progress(dir.path()).await,
            "no merge left in progress"
        );
    }

    #[tokio::test]
    async fn cas_advance_branch_hard_error_distinct_from_lost_cas() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let base = git_out(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["branch", "target", &base]);
        // `update-ref` fails (the new value is not a real object) but the ref is
        // STILL at `expected_old` → a hard error, NOT a lost-CAS `Ok(false)` (which
        // would wrongly discard a real integration over a transient git fault).
        // (A non-zero hex — the all-zero OID is git's delete sentinel.)
        let bogus = "deadbeef".repeat(5); // 40 hex chars, not a real object
        let res = cas_advance_branch(dir.path(), "target", &bogus, &base).await;
        assert!(res.is_err(), "bad-object update-ref is a hard error");
        assert_eq!(
            git_out(dir.path(), &["rev-parse", "target"]),
            base,
            "ref unchanged"
        );
    }

    #[tokio::test]
    async fn reset_issue_subtree_preserves_in_progress_merge() {
        let (db, repo, data, issue_id, _space, _seq) = setup().await;
        let issue_ctx = ensure_worktree(&db.conn, data.path(), issue_id).await.unwrap();
        let integ =
            ensure_integrate_worktree(&db.conn, data.path(), issue_id, &issue_ctx.base_commit)
                .await
                .unwrap();

        // Two commits that edit the SAME file → merging the second into the
        // integrate worktree conflicts and leaves MERGE_HEAD.
        let base = issue_ctx.base_commit.clone();
        git(repo.path(), &["checkout", "-q", "-b", "tmpA", &base]);
        std::fs::write(repo.path().join("README.md"), "A\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "A"]);
        let a = git_out(repo.path(), &["rev-parse", "HEAD"]);
        git(repo.path(), &["checkout", "-q", "-b", "tmpB", &base]);
        std::fs::write(repo.path().join("README.md"), "B\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["commit", "-q", "-m", "B"]);
        let b = git_out(repo.path(), &["rev-parse", "HEAD"]);

        let out = fan_in_tasks(&integ.worktree_path, &[(1, a), (2, b)], &[], None)
            .await
            .unwrap();
        assert!(matches!(out, FanInOutcome::Conflict { .. }));
        assert!(
            integrate_in_progress(&integ.worktree_path).await,
            "MERGE_HEAD set by the conflict"
        );

        // Boot recovery's subtree reset must PRESERVE the in-progress merge so the
        // fan-in can recover it — a `reset --hard` would force a full re-resolve.
        reset_issue_subtree(repo.path(), &issue_ctx.worktree_path)
            .await
            .unwrap();
        assert!(
            integrate_in_progress(&integ.worktree_path).await,
            "in-progress merge preserved (not reset) across boot recovery"
        );
    }
}
