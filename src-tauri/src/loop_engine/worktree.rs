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

use sea_orm::{ActiveModelTrait, DatabaseConnection, IntoActiveModel, Set};

use crate::db::service::{folder_service, loop_service};
use crate::loop_engine::LoopError;

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

async fn head_commit(repo: &Path) -> Result<String, LoopError> {
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
    let out = run_git(&repo_path, &["worktree", "add", "-b", &branch, &wt]).await?;
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

/// Remove the worktree directory and its administrative entry (best-effort
/// `--force` to tolerate a dirty tree). The branch is left intact.
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
            &IssueConfig::default(),
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
}
