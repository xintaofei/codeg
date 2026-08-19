//! Task-specific git building blocks: base-pinned revs, commit-all, the
//! two-stage merge steps, and worktree removal. Thin wrappers over the git CLI
//! (mirroring `commands::folders`), composed by the task engine which owns the
//! per-folder git mutex.

use crate::app_error::AppCommandError;
use crate::commands::folders::{detect_conflicts, git_command_error};
use crate::models::WorkTaskChangedFile;

async fn run_git(path: &str, args: &[&str]) -> Result<std::process::Output, AppCommandError> {
    crate::process::tokio_command("git")
        .args(args)
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)
}

/// A worktree's own git directory (`<repo>/.git/worktrees/<name>` for a linked
/// worktree, `<repo>/.git` for the main one). Engine-private markers live here:
/// nothing under it can show up in `git status` or be committed by the agent.
pub async fn git_dir(path: &str) -> Result<std::path::PathBuf, AppCommandError> {
    let out = run_git(path, &["rev-parse", "--absolute-git-dir"]).await?;
    if !out.status.success() {
        return Err(git_command_error("rev-parse --absolute-git-dir", &out.stderr));
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if dir.is_empty() {
        return Err(AppCommandError::external_command(
            "git rev-parse --absolute-git-dir returned nothing",
            path.to_string(),
        ));
    }
    Ok(std::path::PathBuf::from(dir))
}

/// Resolve a revision to a full sha.
pub async fn rev_parse(path: &str, rev: &str) -> Result<String, AppCommandError> {
    let out = run_git(path, &["rev-parse", "--verify", &format!("{rev}^{{commit}}")]).await?;
    if !out.status.success() {
        return Err(git_command_error("rev-parse", &out.stderr));
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() {
        return Err(AppCommandError::external_command(
            "git rev-parse returned nothing",
            rev.to_string(),
        ));
    }
    Ok(sha)
}

/// Whether the index has staged changes (stage-B preflight requires a clean
/// index in the project folder).
pub async fn staged_clean(path: &str) -> Result<bool, AppCommandError> {
    let out = run_git(path, &["diff", "--cached", "--quiet"]).await?;
    Ok(out.status.success())
}

/// Whether the working tree has anything to commit (tracked or untracked).
pub async fn has_changes(path: &str) -> Result<bool, AppCommandError> {
    let out = run_git(path, &["status", "--porcelain=v1", "-unormal"]).await?;
    if !out.status.success() {
        return Err(git_command_error("status", &out.stderr));
    }
    Ok(!out.stdout.iter().all(|b| b.is_ascii_whitespace()))
}

/// Stage everything and commit with the user's resolved author. Returns
/// `false` when there was nothing to commit.
pub async fn commit_all(
    conn: &sea_orm::DatabaseConnection,
    path: &str,
    message: &str,
) -> Result<bool, AppCommandError> {
    if !has_changes(path).await? {
        return Ok(false);
    }
    let add = run_git(path, &["add", "-A"]).await?;
    if !add.status.success() {
        return Err(git_command_error("add -A", &add.stderr));
    }
    commit_staged(conn, path, message).await?;
    Ok(true)
}

/// Commit whatever is staged (used by commit-all and the squash landing).
/// Resolves the commit author from the matching configured account, like
/// `git_commit_core`.
pub async fn commit_staged(
    conn: &sea_orm::DatabaseConnection,
    path: &str,
    message: &str,
) -> Result<(), AppCommandError> {
    let author_override = crate::git_credential::resolve_commit_author(path, conn).await;
    let mut cmd = crate::process::tokio_command("git");
    if let Some((ref name, ref email)) = author_override {
        cmd.args([
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
        ]);
    }
    cmd.args(["commit", "-m", message]).current_dir(path);
    let out = cmd.output().await.map_err(AppCommandError::io)?;
    if !out.status.success() {
        return Err(git_command_error("commit", &out.stderr));
    }
    Ok(())
}

/// Outcome of a merge attempt that treats conflicts as data, not errors.
pub enum MergeAttempt {
    Ok,
    /// Conflicted; the merge has ALREADY been aborted (worktree left clean).
    Conflict(Vec<String>),
}

/// Stage A: merge the base branch INTO the task worktree, so conflicts always
/// land in the worktree. On conflict the merge is aborted and the conflicted
/// paths are returned.
pub async fn merge_base_into_worktree(
    worktree_path: &str,
    base_branch: &str,
) -> Result<MergeAttempt, AppCommandError> {
    let out = run_git(worktree_path, &["merge", "--no-edit", base_branch]).await?;
    if out.status.success() {
        return Ok(MergeAttempt::Ok);
    }
    let conflicts = detect_conflicts(worktree_path).await?;
    if conflicts.is_empty() {
        return Err(git_command_error("merge", &out.stderr));
    }
    let abort = run_git(worktree_path, &["merge", "--abort"]).await?;
    if !abort.status.success() {
        tracing::warn!(
            "[work_task] merge --abort failed in {worktree_path}: {}",
            String::from_utf8_lossy(&abort.stderr)
        );
    }
    Ok(MergeAttempt::Conflict(conflicts))
}

/// Stage B (squash): stage the work branch's tree onto the base branch. The
/// caller commits via [`commit_staged`]. On failure the caller cleans up with
/// [`reset_merge`].
pub async fn merge_squash(path: &str, work_branch: &str) -> Result<(), AppCommandError> {
    let out = run_git(path, &["merge", "--squash", work_branch]).await?;
    if !out.status.success() {
        return Err(git_command_error("merge --squash", &out.stderr));
    }
    Ok(())
}

/// Stage B (merge commit): `git merge --no-ff -m <message> <branch>`.
pub async fn merge_no_ff(
    path: &str,
    work_branch: &str,
    message: &str,
) -> Result<(), AppCommandError> {
    let out = run_git(path, &["merge", "--no-ff", "-m", message, work_branch]).await?;
    if !out.status.success() {
        return Err(git_command_error("merge --no-ff", &out.stderr));
    }
    Ok(())
}

/// Clean a failed/interrupted stage B out of the project folder. Safe because
/// the preflight guaranteed the index was clean before we touched it.
pub async fn reset_merge(path: &str) -> Result<(), AppCommandError> {
    let out = run_git(path, &["reset", "--merge"]).await?;
    if !out.status.success() {
        return Err(git_command_error("reset --merge", &out.stderr));
    }
    Ok(())
}

/// Whether a merge is in progress (MERGE_HEAD exists) — crash-recovery probe.
pub async fn has_merge_head(path: &str) -> Result<bool, AppCommandError> {
    let out = run_git(path, &["rev-parse", "--verify", "MERGE_HEAD"]).await?;
    Ok(out.status.success())
}

/// Whether `ancestor` is reachable from `descendant`. Exit 0 = yes, 1 = no,
/// anything else is a real error.
pub async fn is_ancestor(
    path: &str,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, AppCommandError> {
    let out = run_git(path, &["merge-base", "--is-ancestor", ancestor, descendant]).await?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_command_error("merge-base --is-ancestor", &out.stderr)),
    }
}

/// Whether two revisions point at identical trees (`git diff --quiet a b`) —
/// how a squash landing is recognized without knowing the commit message.
pub async fn trees_equal(path: &str, a: &str, b: &str) -> Result<bool, AppCommandError> {
    let out = run_git(path, &["diff", "--quiet", a, b]).await?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_command_error("diff --quiet", &out.stderr)),
    }
}

/// `git diff --numstat <base>` — the task's change set vs its recorded base.
/// Binary files report `-` counts; they are counted as a changed file with 0/0.
pub async fn diff_numstat(
    path: &str,
    base: &str,
) -> Result<Vec<WorkTaskChangedFile>, AppCommandError> {
    let out = run_git(path, &["diff", "--numstat", base]).await?;
    if !out.status.success() {
        return Err(git_command_error("diff --numstat", &out.stderr));
    }
    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(adds), Some(dels), Some(file)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        files.push(WorkTaskChangedFile {
            file: file.to_string(),
            additions: adds.parse().unwrap_or(0),
            deletions: dels.parse().unwrap_or(0),
        });
    }
    Ok(files)
}

/// Tip commit of a LOCAL branch (`refs/heads/<name>`), or `None` when no such
/// branch exists. Deliberately not `rev-parse <name>`: an unqualified name is
/// resolved with tags taking precedence over branches, so a same-name tag
/// would silently answer for the branch. `for-each-ref` looks the ref up fully
/// qualified, and a missing branch is a clean empty result instead of an exit
/// code shared with real failures — callers can tell "absent" from "probe
/// broke".
pub async fn local_branch_tip(
    repo_path: &str,
    branch: &str,
) -> Result<Option<String>, AppCommandError> {
    let refname = format!("refs/heads/{branch}");
    let out = run_git(
        repo_path,
        &["for-each-ref", "--format=%(refname)\t%(objectname)", &refname],
    )
    .await?;
    if !out.status.success() {
        return Err(git_command_error("for-each-ref", &out.stderr));
    }
    // The pattern also matches refs UNDER `refs/heads/<name>/` — filter to the
    // exact ref so a branch named `<name>/sub` cannot answer for `<name>`.
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some((name, oid)) = line.split_once('\t') {
            if name == refname && !oid.trim().is_empty() {
                return Ok(Some(oid.trim().to_string()));
            }
        }
    }
    Ok(None)
}

/// Re-create the checkout of an EXISTING branch after its worktree directory
/// disappeared (`git worktree add <path> <branch>`, no `-b`). Prunes stale
/// registrations first: the vanished directory usually still holds the branch
/// checked out in git's bookkeeping, and a checked-out branch cannot be added
/// again.
pub async fn worktree_add_existing_branch(
    repo_path: &str,
    worktree_path: &str,
    branch: &str,
) -> Result<(), AppCommandError> {
    let prune = run_git(repo_path, &["worktree", "prune"]).await?;
    if !prune.status.success() {
        return Err(git_command_error("worktree prune", &prune.stderr));
    }
    let out = run_git(repo_path, &["worktree", "add", worktree_path, branch]).await?;
    if !out.status.success() {
        return Err(git_command_error("worktree add", &out.stderr));
    }
    // `worktree add <path> <name>` falls back to a DETACHED checkout when
    // `<name>` is not a local branch (a same-name tag, say). A detached task
    // worktree would collect commits nothing points to while the merge targets
    // the wrong ref — verify the checkout is attached to the branch, and back
    // the add out when it is not. The FULL ref, not `--short`: when a
    // same-name tag exists alongside the branch, `--short` disambiguates to
    // `heads/<name>` and a plain name comparison would throw away a perfectly
    // valid recovery.
    let head = run_git(worktree_path, &["symbolic-ref", "--quiet", "HEAD"]).await?;
    let attached = head.status.success()
        && String::from_utf8_lossy(&head.stdout).trim() == format!("refs/heads/{branch}");
    if !attached {
        let _ = run_git(repo_path, &["worktree", "remove", "--force", worktree_path]).await;
        return Err(AppCommandError::external_command(
            "worktree add produced a detached checkout instead of the branch",
            branch.to_string(),
        ));
    }
    Ok(())
}

/// Whether the LOCAL branch still holds work the base never received — the
/// guard that keeps the convergence of a MISSING worktree from deleting real
/// commits with `branch -D`. True ⟺ the branch exists and its tip is neither
/// an ancestor of the base ref nor tree-equal to it (the two shapes a landed
/// merge takes). The base ref is the base branch's CURRENT tip when that
/// branch still exists, else the recorded base sha. Every ref is resolved
/// fully qualified and compared as commit ids — an unqualified name would let
/// a same-name tag answer for the branch and clear unlanded work for
/// deletion. A probe that errors reads as "holds work": deletion is the
/// irreversible half, so uncertainty keeps the branch.
pub async fn branch_holds_unlanded_work(
    repo_path: &str,
    branch: &str,
    base_branch: Option<&str>,
    base_sha: Option<&str>,
) -> bool {
    let tip = match local_branch_tip(repo_path, branch).await {
        Ok(Some(tip)) => tip,
        Ok(None) => return false, // no branch, nothing a deletion could destroy
        Err(_) => return true,
    };
    let mut base_ref = None;
    if let Some(base_branch) = base_branch {
        match local_branch_tip(repo_path, base_branch).await {
            Ok(Some(t)) => base_ref = Some(t),
            Ok(None) => {} // base branch deleted — the recorded sha still anchors
            Err(_) => return true,
        }
    }
    if base_ref.is_none() {
        if let Some(sha) = base_sha {
            if rev_parse(repo_path, sha).await.is_ok() {
                base_ref = Some(sha.to_string());
            }
        }
    }
    let Some(base_ref) = base_ref else {
        return true; // nothing to compare against — keep the branch
    };
    match is_ancestor(repo_path, &tip, &base_ref).await {
        Ok(true) => return false, // merged (or never diverged)
        Ok(false) => {}
        Err(_) => return true,
    }
    match trees_equal(repo_path, &base_ref, &tip).await {
        Ok(equal) => !equal, // tree-equal = squash-landed
        Err(_) => true,
    }
}

/// Remove a task worktree directory + its branch. Runs from the project repo.
/// Tolerant of a directory already gone (prunes the stale registration) and of
/// a branch already deleted; `-D` is required because a squash-landed branch is
/// unmerged in git's eyes.
pub async fn remove_worktree_and_branch(
    repo_path: &str,
    worktree_path: &str,
    work_branch: Option<&str>,
) -> Result<(), AppCommandError> {
    let removed = run_git(repo_path, &["worktree", "remove", "--force", worktree_path]).await?;
    if !removed.status.success() {
        if std::path::Path::new(worktree_path).exists() {
            return Err(git_command_error("worktree remove", &removed.stderr));
        }
        // Directory already gone — drop the stale registration so the branch
        // delete below isn't blocked by a phantom checkout.
        let prune = run_git(repo_path, &["worktree", "prune"]).await?;
        if !prune.status.success() {
            return Err(git_command_error("worktree prune", &prune.stderr));
        }
    }
    if let Some(branch) = work_branch {
        let del = run_git(repo_path, &["branch", "-D", branch]).await?;
        if !del.status.success() {
            let msg = String::from_utf8_lossy(&del.stderr).to_lowercase();
            if !msg.contains("not found") {
                return Err(git_command_error("branch -D", &del.stderr));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a git command in `dir`, supplying identity via env so the test does
    /// not depend on (or mutate) the developer's global git config.
    fn git_run(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Why removing a task worktree has to consult BOTH probes: each is blind
    /// to exactly what the other sees. `remove_worktree_and_branch` runs
    /// `worktree remove --force` + `branch -D`, so anything either probe would
    /// have found is destroyed silently if only one of them is asked.
    #[tokio::test]
    async fn status_and_base_diff_have_opposite_blind_spots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_str().expect("utf-8 path");
        git_run(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").expect("write");
        git_run(dir.path(), &["add", "-A"]);
        git_run(dir.path(), &["commit", "-q", "-m", "base"]);
        let base = rev_parse(path, "HEAD").await.expect("base sha");

        // Settled state: nothing on top of the base, by either measure.
        assert!(!has_changes(path).await.expect("status"));
        assert!(diff_numstat(path, &base).await.expect("diff").is_empty());

        // Work committed on the branch afterwards leaves a spotless worktree…
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").expect("write");
        git_run(dir.path(), &["commit", "-qam", "later work"]);
        assert!(
            !has_changes(path).await.expect("status"),
            "a commit leaves no trace in `git status` — status alone would clear a branch for deletion"
        );
        // …but it is exactly what a merge would have taken.
        let files = diff_numstat(path, &base).await.expect("diff");
        assert_eq!(files.len(), 1, "the base diff still holds the commit");
        assert_eq!(files[0].file, "a.txt");

        // The mirror image: an untracked file never reaches a diff against the
        // base, so the base diff alone would clear a worktree for deletion.
        git_run(dir.path(), &["reset", "-q", "--hard", &base]);
        std::fs::write(dir.path().join("scratch.txt"), "junk\n").expect("write");
        assert!(
            diff_numstat(path, &base).await.expect("diff").is_empty(),
            "untracked files are invisible to the base diff"
        );
        assert!(has_changes(path).await.expect("status"));
    }

    /// The branch guard behind converging a missing worktree: unlanded commits
    /// keep the branch, and both landed shapes (merge ancestry, squash tree
    /// equality) release it.
    #[tokio::test]
    async fn unlanded_branch_work_is_recognized_in_the_root_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_str().expect("utf-8 path");
        git_run(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").expect("write");
        git_run(dir.path(), &["add", "-A"]);
        git_run(dir.path(), &["commit", "-q", "-m", "base"]);
        let base_sha = rev_parse(path, "HEAD").await.expect("base sha");

        // A branch that never diverged has nothing a deletion could destroy.
        git_run(dir.path(), &["branch", "task/1"]);
        assert!(!branch_holds_unlanded_work(path, "task/1", Some("main"), Some(&base_sha)).await);
        // Neither does a branch that no longer exists.
        assert!(!branch_holds_unlanded_work(path, "task/9", Some("main"), Some(&base_sha)).await);

        // Commit on the branch → unlanded work, whichever base ref resolves.
        git_run(dir.path(), &["checkout", "-q", "task/1"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").expect("write");
        git_run(dir.path(), &["commit", "-qam", "work"]);
        git_run(dir.path(), &["checkout", "-q", "main"]);
        assert!(branch_holds_unlanded_work(path, "task/1", Some("main"), Some(&base_sha)).await);
        assert!(branch_holds_unlanded_work(path, "task/1", None, Some(&base_sha)).await);
        // No resolvable base at all → conservative: keep.
        assert!(branch_holds_unlanded_work(path, "task/1", Some("gone"), None).await);

        // A squash landing leaves no ancestry, but the trees match.
        git_run(dir.path(), &["merge", "-q", "--squash", "task/1"]);
        git_run(dir.path(), &["commit", "-qm", "landed as squash"]);
        assert!(!branch_holds_unlanded_work(path, "task/1", Some("main"), Some(&base_sha)).await);

        // A merge landing is plain ancestry — even after main moves on.
        git_run(dir.path(), &["checkout", "-q", "task/1"]);
        std::fs::write(dir.path().join("b.txt"), "more\n").expect("write");
        git_run(dir.path(), &["add", "-A"]);
        git_run(dir.path(), &["commit", "-qm", "more work"]);
        git_run(dir.path(), &["checkout", "-q", "main"]);
        assert!(branch_holds_unlanded_work(path, "task/1", Some("main"), Some(&base_sha)).await);
        git_run(dir.path(), &["merge", "-q", "--no-ff", "-m", "land", "task/1"]);
        assert!(!branch_holds_unlanded_work(path, "task/1", Some("main"), Some(&base_sha)).await);
    }

    /// A same-name TAG must never answer for the branch: unqualified
    /// resolution prefers tags, and both halves of the missing-worktree story
    /// would go wrong on it — the deletion guard would read the tag's landed
    /// commit and clear real work for `branch -D`, and the recovery add would
    /// produce a detached checkout of the tag.
    #[tokio::test]
    async fn a_same_name_tag_cannot_shadow_the_branch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).expect("mkdir");
        let repo_path = repo.to_str().expect("utf-8 path");
        git_run(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").expect("write");
        git_run(&repo, &["add", "-A"]);
        git_run(&repo, &["commit", "-q", "-m", "base"]);
        let base_sha = rev_parse(repo_path, "HEAD").await.expect("base sha");

        // Branch with unlanded work + a tag of the same name pinned at base.
        git_run(&repo, &["branch", "task/1"]);
        git_run(&repo, &["tag", "task/1", &base_sha]);
        git_run(&repo, &["checkout", "-q", "task/1"]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").expect("write");
        git_run(&repo, &["commit", "-qam", "work"]);
        git_run(&repo, &["checkout", "-q", "main"]);

        // The qualified probe sees the branch tip, not the tag's base commit.
        let tip = local_branch_tip(repo_path, "task/1")
            .await
            .expect("probe")
            .expect("branch exists");
        assert_ne!(tip, base_sha, "the probe must not resolve the tag");
        assert!(
            branch_holds_unlanded_work(repo_path, "task/1", Some("main"), Some(&base_sha)).await,
            "the tag at base must not clear the branch's unlanded commit for deletion"
        );

        // With BOTH the branch and the tag present, recovery must still work:
        // git attaches the checkout to the branch, and the attachment check
        // must recognize it even though `symbolic-ref --short` would
        // disambiguate the name to `heads/task/1` here.
        let wt_both = dir.path().join("repo-task-1-both");
        let wt_both_path = wt_both.to_str().expect("utf-8 path");
        worktree_add_existing_branch(repo_path, wt_both_path, "task/1")
            .await
            .expect("recovery with a shadowing tag present");
        assert_eq!(
            rev_parse(wt_both_path, "HEAD").await.expect("head"),
            tip,
            "the recovered checkout is the branch tip, not the tag"
        );
        git_run(&repo, &["worktree", "remove", "--force", wt_both_path]);

        // With the branch gone the tag still exists — recovery must refuse
        // rather than hand back a detached checkout of the tag.
        git_run(&repo, &["branch", "-D", "task/1"]);
        assert!(local_branch_tip(repo_path, "task/1")
            .await
            .expect("probe")
            .is_none());
        assert!(
            !branch_holds_unlanded_work(repo_path, "task/1", Some("main"), Some(&base_sha)).await
        );
        let wt = dir.path().join("repo-task-1");
        let wt_path = wt.to_str().expect("utf-8 path");
        assert!(
            worktree_add_existing_branch(repo_path, wt_path, "task/1")
                .await
                .is_err(),
            "a tag-only add must fail instead of leaving a detached worktree"
        );
        assert!(!wt.exists(), "the refused detached checkout is backed out");
    }

    /// Probe failures keep the branch: an unreadable repo must not read as
    /// "nothing to lose" on the path that ends in `branch -D`.
    #[tokio::test]
    async fn a_broken_probe_keeps_the_branch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let not_a_repo = dir.path().to_str().expect("utf-8 path");
        assert!(local_branch_tip(not_a_repo, "task/1").await.is_err());
        assert!(branch_holds_unlanded_work(not_a_repo, "task/1", Some("main"), None).await);
    }

    /// Task worktrees can be pointed at a directory of the user's choosing
    /// (the folder's `worktree_root` setting), and the folder's FIRST task
    /// meets that directory before it exists. Nothing in the engine creates
    /// it: `git worktree add` is expected to make the leading directories
    /// along with the checkout, so this pins that expectation on the exact
    /// call the fresh mint makes.
    #[tokio::test]
    async fn a_worktree_root_that_does_not_exist_yet_is_created_by_the_add() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).expect("mkdir");
        let repo_path = repo.to_str().expect("utf-8 path");
        git_run(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").expect("write");
        git_run(&repo, &["add", "-A"]);
        git_run(&repo, &["commit", "-q", "-m", "base"]);
        let base = rev_parse(repo_path, "HEAD").await.expect("base sha");

        let wt = dir.path().join("trees").join("nested").join("repo-task-7");
        crate::commands::folders::git_worktree_add(
            repo_path.to_string(),
            "task/7".to_string(),
            wt.to_string_lossy().into_owned(),
            Some(base.clone()),
        )
        .await
        .expect("add into a root that does not exist yet");

        assert_eq!(
            rev_parse(wt.to_str().expect("utf-8 path"), "HEAD")
                .await
                .expect("head"),
            base
        );
    }

    /// A retry after the checkout was removed must get the SAME branch back,
    /// prior commits included — not a fresh tree on a fresh base.
    #[tokio::test]
    async fn a_vanished_checkout_is_recreated_from_its_branch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).expect("mkdir");
        let repo_path = repo.to_str().expect("utf-8 path");
        git_run(&repo, &["init", "-q", "-b", "main"]);
        // `git_run` nulls the global/system config, but the checkout below is
        // made by `worktree_add_existing_branch` — a production git call that
        // inherits the host's. Git for Windows ships `core.autocrlf=true`, so
        // without a repo-local pin the restored file comes back CRLF and the
        // content assertion at the end reads as a data loss it isn't.
        git_run(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("a.txt"), "one\n").expect("write");
        git_run(&repo, &["add", "-A"]);
        git_run(&repo, &["commit", "-q", "-m", "base"]);

        let wt = dir.path().join("repo-task-1");
        let wt_path = wt.to_str().expect("utf-8 path");
        git_run(&repo, &["worktree", "add", "-q", "-b", "task/1", wt_path]);
        std::fs::write(wt.join("a.txt"), "one\ntwo\n").expect("write");
        git_run(&wt, &["commit", "-qam", "work"]);
        let tip = rev_parse(wt_path, "HEAD").await.expect("tip");

        // The directory vanishes behind git's back (the user deleted it).
        std::fs::remove_dir_all(&wt).expect("rm worktree");
        worktree_add_existing_branch(repo_path, wt_path, "task/1")
            .await
            .expect("recreate");
        // Same branch, same history, work restored on disk — and ATTACHED to
        // the branch, not a detached checkout of its tip.
        assert_eq!(rev_parse(wt_path, "HEAD").await.expect("head"), tip);
        let head = std::process::Command::new("git")
            .args(["symbolic-ref", "--short", "HEAD"])
            .current_dir(&wt)
            .output()
            .expect("spawn git");
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "task/1");
        assert_eq!(
            std::fs::read_to_string(wt.join("a.txt")).expect("read"),
            "one\ntwo\n"
        );
    }
}
