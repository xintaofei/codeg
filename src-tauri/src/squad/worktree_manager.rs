use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use crate::models::squad::{SquadRoleKind, SquadWorkspacePolicy};

/// Materialize the workspace on disk if needed.
///
/// - `ReadOnly` / `WriteShared` → returns the base path unchanged.
/// - `WriteIsolated`:
///   - if the planned path already exists, reuse it (idempotent reconnect).
///   - else, if base is a git repo, run `git worktree add -B <branch> <path> HEAD` to create it.
///   - else, fall back to a plain directory so non-git folders still work.
pub async fn ensure_role_workspace(
    base_working_dir: Option<&str>,
    run_id: i32,
    role_kind: SquadRoleKind,
    policy: SquadWorkspacePolicy,
) -> io::Result<Option<String>> {
    let Some(base_str) = base_working_dir.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    match policy {
        SquadWorkspacePolicy::ReadOnly | SquadWorkspacePolicy::WriteShared => {
            Ok(Some(base_str.to_string()))
        }
        SquadWorkspacePolicy::WriteIsolated => {
            let base = Path::new(base_str);
            let planned = planned_worktree_path(base, run_id, role_kind);

            if planned.exists() {
                // Reuse on reconnect — don't fail if the worktree (or plain dir) is already there.
                return Ok(Some(planned.display().to_string()));
            }

            if let Some(parent) = planned.parent() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("creating worktree parent {}: {e}", parent.display()),
                    )
                })?;
            }

            if is_git_repo(base).await {
                let branch = role_branch_name(run_id, role_kind);
                git_worktree_add(base, &branch, &planned)
                    .await
                    .map_err(|e| {
                        io::Error::other(format!(
                            "git worktree add -B {branch} {} (from {}): {e}",
                            planned.display(),
                            base.display()
                        ))
                    })?;
            } else {
                eprintln!(
                    "[squad/worktree] base {} is not a git repo; falling back to plain dir at {}",
                    base.display(),
                    planned.display()
                );
                fs::create_dir_all(&planned).await.map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("creating fallback dir {}: {e}", planned.display()),
                    )
                })?;
            }

            Ok(Some(planned.display().to_string()))
        }
    }
}

fn planned_worktree_path(base: &Path, run_id: i32, role_kind: SquadRoleKind) -> PathBuf {
    let repo_name = base
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace");
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    parent
        .join(".codeg-worktrees")
        .join(repo_name)
        .join(run_id.to_string())
        .join(role_slug(role_kind))
}

fn role_slug(role_kind: SquadRoleKind) -> String {
    serde_json::to_string(&role_kind)
        .unwrap_or_else(|_| "\"worker\"".to_string())
        .trim_matches('"')
        .to_string()
}

fn role_branch_name(run_id: i32, role_kind: SquadRoleKind) -> String {
    format!("codeg/squad/{run_id}/{}", role_slug(role_kind))
}

async fn is_git_repo(base: &Path) -> bool {
    crate::process::tokio_command("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(base)
        .output()
        .await
        .map(|out| out.status.success())
        .unwrap_or(false)
}

async fn git_worktree_add(base: &Path, branch: &str, target: &Path) -> io::Result<()> {
    let target_str = target.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "worktree target path is not valid UTF-8",
        )
    })?;
    let output = crate::process::tokio_command("git")
        .args(["worktree", "add", "-B", branch, target_str, "HEAD"])
        .current_dir(base)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "git worktree add failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn planned_path_components() {
        let base = Path::new("/tmp/myrepo");
        let p = planned_worktree_path(base, 42, SquadRoleKind::Worker);
        let s = p.display().to_string();
        assert!(s.contains(".codeg-worktrees/myrepo/42/"));
    }

    #[test]
    fn branch_name_includes_run_and_role() {
        let b = role_branch_name(7, SquadRoleKind::Worker);
        assert!(b.starts_with("codeg/squad/7/"));
    }

    #[tokio::test]
    async fn read_only_returns_base() {
        let res = ensure_role_workspace(
            Some("/tmp/some-base"),
            1,
            SquadRoleKind::Worker,
            SquadWorkspacePolicy::ReadOnly,
        )
        .await
        .unwrap();
        assert_eq!(res.as_deref(), Some("/tmp/some-base"));
    }

    #[tokio::test]
    async fn empty_base_returns_none() {
        let res = ensure_role_workspace(
            Some("   "),
            1,
            SquadRoleKind::Worker,
            SquadWorkspacePolicy::WriteIsolated,
        )
        .await
        .unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn isolated_creates_git_worktree_when_repo() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "hi").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);

        let res = ensure_role_workspace(
            repo.to_str(),
            99,
            SquadRoleKind::Worker,
            SquadWorkspacePolicy::WriteIsolated,
        )
        .await
        .unwrap();
        let path = res.expect("workspace path");
        assert!(Path::new(&path).join("a.txt").exists());

        // second call must be idempotent
        let res2 = ensure_role_workspace(
            repo.to_str(),
            99,
            SquadRoleKind::Worker,
            SquadWorkspacePolicy::WriteIsolated,
        )
        .await
        .unwrap();
        assert_eq!(res2.as_deref(), Some(path.as_str()));
    }

    #[tokio::test]
    async fn isolated_falls_back_for_non_git_base() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("plain");
        std::fs::create_dir_all(&base).unwrap();

        let res = ensure_role_workspace(
            base.to_str(),
            1,
            SquadRoleKind::Worker,
            SquadWorkspacePolicy::WriteIsolated,
        )
        .await
        .unwrap();
        let path = res.expect("workspace path");
        assert!(Path::new(&path).is_dir());
    }
}
