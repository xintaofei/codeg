use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, LazyLock, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use base64::Engine as _;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::Semaphore;
use walkdir::WalkDir;

use crate::app_error::AppCommandError;
use crate::db::error::DbError;
use crate::db::service::folder_service;
use crate::db::AppDatabase;
use crate::models::{FolderDetail, FolderHistoryEntry, OpenedConversation};

#[derive(Debug, Serialize)]
pub struct GitStatusEntry {
    pub status: String,
    pub file: String,
}

#[derive(Debug, Serialize)]
pub struct GitBranchList {
    pub local: Vec<String>,
    pub remote: Vec<String>,
    pub worktree_branches: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct GitPullResult {
    pub updated_files: usize,
}

#[derive(Debug, Serialize)]
pub struct GitPushResult {
    pub pushed_commits: usize,
    pub upstream_set: bool,
}

#[derive(Debug, Serialize)]
pub struct GitMergeResult {
    pub merged_commits: usize,
}

#[derive(Debug, Serialize)]
pub struct GitCommitResult {
    pub committed_files: usize,
}

#[derive(Debug, Serialize)]
pub struct GitRemote {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
struct GitCommitSucceededEvent {
    folder_id: i32,
    committed_files: usize,
}

struct FileWatchEntry {
    root_canonical: PathBuf,
    root_display: String,
    watcher: RecommendedWatcher,
    worker: Option<std::thread::JoinHandle<()>>,
    ref_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct FileTreeChangedEvent {
    root_path: String,
    changed_paths: Vec<String>,
    kind: String,
    full_reload: bool,
    refresh_git_status: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileTreeNode {
    File {
        name: String,
        path: String,
    },
    Dir {
        name: String,
        path: String,
        children: Vec<FileTreeNode>,
    },
}

#[derive(Debug, Serialize)]
pub struct FilePreviewContent {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct FileEditContent {
    pub path: String,
    pub content: String,
    pub etag: String,
    pub mtime_ms: Option<i64>,
    pub readonly: bool,
    pub truncated: bool,
    pub line_ending: String,
}

#[derive(Debug, Serialize)]
pub struct FileSaveResult {
    pub path: String,
    pub etag: String,
    pub mtime_ms: Option<i64>,
    pub readonly: bool,
    pub line_ending: String,
}

#[derive(Debug, Serialize)]
pub struct GitLogEntry {
    pub hash: String,
    pub full_hash: String,
    pub author: String,
    pub date: String,
    pub message: String,
    pub files: Vec<GitLogFileChange>,
    pub pushed: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct GitLogFileChange {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

fn count_non_empty_lines(content: &str) -> usize {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .count()
}

fn parse_count_from_output(stdout: &[u8]) -> Option<usize> {
    String::from_utf8_lossy(stdout).trim().parse::<usize>().ok()
}

fn git_command_error(operation: &str, stderr: &[u8]) -> AppCommandError {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    AppCommandError::external_command(format!("git {operation} failed"), stderr)
}

async fn get_head_hash(path: &str) -> Result<Option<String>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Ok(None);
    }

    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if head.is_empty() {
        return Ok(None);
    }
    Ok(Some(head))
}

async fn count_files_in_commit(path: &str, commit: &str) -> Result<usize, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["show", "--name-only", "--pretty=format:", commit])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("show", &output.stderr));
    }

    Ok(count_non_empty_lines(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

async fn count_changed_files_between(
    path: &str,
    base: &str,
    head: &str,
) -> Result<usize, AppCommandError> {
    let range = format!("{}..{}", base, head);
    let output = crate::process::tokio_command("git")
        .args(["diff", "--name-only", &range])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("diff", &output.stderr));
    }

    Ok(count_non_empty_lines(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

async fn estimate_push_commit_count(path: &str) -> usize {
    let upstream_ahead = crate::process::tokio_command("git")
        .args(["rev-list", "--count", "@{push}..HEAD"])
        .current_dir(path)
        .output()
        .await;
    if let Ok(output) = upstream_ahead {
        if output.status.success() {
            if let Some(count) = parse_count_from_output(&output.stdout) {
                return count;
            }
        }
    }

    let branch_output = crate::process::tokio_command("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .await;
    let Ok(branch_output) = branch_output else {
        return 0;
    };
    if !branch_output.status.success() {
        return 0;
    }

    let branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();
    if branch.is_empty() || branch == "HEAD" {
        return 0;
    }

    let remote_key = format!("branch.{}.remote", branch);
    let remote_output = crate::process::tokio_command("git")
        .args(["config", "--get", &remote_key])
        .current_dir(path)
        .output()
        .await;
    let remote = remote_output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "origin".to_string());

    let remote_arg = format!("--remotes={}", remote);
    let output = crate::process::tokio_command("git")
        .args(["rev-list", "--count", "HEAD", "--not", &remote_arg])
        .current_dir(path)
        .output()
        .await;
    let Ok(output) = output else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }

    parse_count_from_output(&output.stdout).unwrap_or(0)
}

#[tauri::command]
pub async fn get_folder(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<FolderDetail, DbError> {
    folder_service::get_folder_by_id(&db.conn, folder_id)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Folder {} not found", folder_id)))
}

#[tauri::command]
pub async fn load_folder_history(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<FolderHistoryEntry>, AppCommandError> {
    folder_service::list_folders(&db.conn)
        .await
        .map_err(AppCommandError::from)
}

#[tauri::command]
pub async fn add_folder_to_history(
    db: tauri::State<'_, AppDatabase>,
    path: String,
) -> Result<FolderHistoryEntry, DbError> {
    folder_service::add_folder(&db.conn, &path).await
}

#[tauri::command]
pub async fn set_folder_parent_branch(
    db: tauri::State<'_, AppDatabase>,
    path: String,
    parent_branch: Option<String>,
) -> Result<(), AppCommandError> {
    // Find folder by path first
    use crate::db::entities::folder;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let row = folder::Entity::find()
        .filter(folder::Column::Path.eq(&path))
        .filter(folder::Column::DeletedAt.is_null())
        .one(&db.conn)
        .await
        .map_err(|e| {
            AppCommandError::database_error("Failed to query folder").with_detail(e.to_string())
        })?;

    if let Some(folder_model) = row {
        folder_service::set_folder_parent_branch(&db.conn, folder_model.id, parent_branch)
            .await
            .map_err(AppCommandError::from)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_folder_from_history(
    db: tauri::State<'_, AppDatabase>,
    path: String,
) -> Result<(), AppCommandError> {
    folder_service::remove_folder(&db.conn, &path)
        .await
        .map_err(AppCommandError::from)
}

#[tauri::command]
pub async fn save_folder_opened_conversations(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    items: Vec<OpenedConversation>,
) -> Result<(), DbError> {
    folder_service::save_opened_conversations(&db.conn, folder_id, items).await
}

#[tauri::command]
pub async fn create_folder_directory(path: String) -> Result<(), AppCommandError> {
    std::fs::create_dir_all(&path).map_err(AppCommandError::io)
}

#[tauri::command]
pub async fn clone_repository(url: String, target_dir: String) -> Result<(), AppCommandError> {
    if url.trim().is_empty() || target_dir.trim().is_empty() {
        return Err(AppCommandError::invalid_input(
            "Repository URL and target directory are required",
        ));
    }

    let output = crate::process::tokio_command("git")
        .args(["clone", &url, &target_dir])
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppCommandError::dependency_missing(
                    "Git is not installed. Please install Git first.",
                )
                .with_detail("https://git-scm.com")
            } else {
                AppCommandError::external_command("Failed to run git clone", e.to_string())
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(classify_git_clone_error(stderr.trim()));
    }
    Ok(())
}

fn classify_git_clone_error(stderr: &str) -> AppCommandError {
    let normalized = stderr.to_lowercase();

    if normalized.contains("already exists and is not an empty directory") {
        return AppCommandError::already_exists("Target directory already exists and is not empty")
            .with_detail(stderr.to_string());
    }

    if normalized.contains("repository not found") {
        return AppCommandError::not_found(
            "Repository not found. Check URL and access permissions.",
        )
        .with_detail(stderr.to_string());
    }

    if normalized.contains("could not resolve host")
        || normalized.contains("network is unreachable")
        || normalized.contains("connection timed out")
        || normalized.contains("failed to connect")
    {
        return AppCommandError::network("Network is unavailable while cloning repository")
            .with_detail(stderr.to_string());
    }

    if normalized.contains("authentication failed")
        || normalized.contains("could not read username")
        || normalized.contains("permission denied (publickey)")
    {
        return AppCommandError::authentication_failed(
            "Authentication failed while cloning repository",
        )
        .with_detail(stderr.to_string());
    }

    if normalized.contains("permission denied") {
        return AppCommandError::permission_denied("Permission denied while cloning repository")
            .with_detail(stderr.to_string());
    }

    AppCommandError::external_command("Git clone failed", stderr.to_string())
}

#[tauri::command]
pub async fn get_git_branch(path: String) -> Result<Option<String>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() && branch != "HEAD" {
            return Ok(Some(branch));
        }
    }

    // Fallback: symbolic-ref works on unborn branches (after git init, before first commit)
    let sym_output = crate::process::tokio_command("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if sym_output.status.success() {
        let branch = String::from_utf8_lossy(&sym_output.stdout)
            .trim()
            .to_string();
        if !branch.is_empty() {
            return Ok(Some(branch));
        }
    }

    Ok(None)
}

#[tauri::command]
pub async fn git_init(path: String) -> Result<(), AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["init"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("init", &output.stderr));
    }
    Ok(())
}

#[tauri::command]
pub async fn git_pull(path: String) -> Result<GitPullResult, AppCommandError> {
    let head_before = get_head_hash(&path).await?;

    let output = crate::process::tokio_command("git")
        .args(["pull"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("pull", &output.stderr));
    }

    let head_after = get_head_hash(&path).await?;
    let updated_files = match (head_before.as_deref(), head_after.as_deref()) {
        (Some(before), Some(after)) if before != after => {
            count_changed_files_between(&path, before, after).await?
        }
        (None, Some(after)) => count_files_in_commit(&path, after).await?,
        _ => 0,
    };

    Ok(GitPullResult { updated_files })
}

#[tauri::command]
pub async fn git_fetch(path: String) -> Result<String, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["fetch", "--all"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("fetch --all", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

#[tauri::command]
pub async fn git_push(path: String) -> Result<GitPushResult, AppCommandError> {
    let pushed_commits = estimate_push_commit_count(&path).await;

    // Check if the current branch has an upstream configured
    let upstream_check = crate::process::tokio_command("git")
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    let has_upstream = upstream_check.status.success();

    let output = if !has_upstream {
        // No upstream: get current branch name and push with --set-upstream
        let branch_output = crate::process::tokio_command("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&path)
            .output()
            .await
            .map_err(AppCommandError::io)?;
        let branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();

        crate::process::tokio_command("git")
            .args(["push", "--set-upstream", "origin", &branch])
            .current_dir(&path)
            .output()
            .await
            .map_err(AppCommandError::io)?
    } else {
        crate::process::tokio_command("git")
            .args(["push"])
            .current_dir(&path)
            .output()
            .await
            .map_err(AppCommandError::io)?
    };

    if !output.status.success() {
        return Err(git_command_error("push", &output.stderr));
    }

    Ok(GitPushResult {
        pushed_commits,
        upstream_set: !has_upstream,
    })
}

#[tauri::command]
pub async fn git_new_branch(
    path: String,
    branch_name: String,
    start_point: Option<String>,
) -> Result<(), AppCommandError> {
    let mut args = vec!["checkout".to_string(), "-b".to_string(), branch_name];
    if let Some(start_point) = start_point {
        let trimmed = start_point.trim();
        if !trimmed.is_empty() {
            args.push(trimmed.to_string());
        }
    }

    let output = crate::process::tokio_command("git")
        .args(&args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("checkout -b", &output.stderr));
    }
    Ok(())
}

#[tauri::command]
pub async fn git_worktree_add(
    path: String,
    branch_name: String,
    worktree_path: String,
) -> Result<(), AppCommandError> {
    // 校验分支是否已存在
    let check = crate::process::tokio_command("git")
        .args([
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}", branch_name),
        ])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;
    if check.status.success() {
        return Err(
            AppCommandError::already_exists("Branch already exists").with_detail(branch_name)
        );
    }

    // 校验目录是否已存在
    if std::path::Path::new(&worktree_path).exists() {
        return Err(
            AppCommandError::already_exists("Worktree directory already exists")
                .with_detail(worktree_path),
        );
    }

    // 执行 git worktree add -b <branch> <path>
    let output = crate::process::tokio_command("git")
        .args(["worktree", "add", "-b", &branch_name, &worktree_path])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("worktree add", &output.stderr));
    }
    Ok(())
}

#[tauri::command]
pub async fn git_checkout(path: String, branch_name: String) -> Result<(), AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["checkout", &branch_name])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("checkout", &output.stderr));
    }
    Ok(())
}

#[tauri::command]
pub async fn git_list_branches(path: String) -> Result<Vec<String>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("branch", &output.stderr));
    }

    let branches = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    Ok(branches)
}

#[tauri::command]
pub async fn git_stash(path: String) -> Result<String, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["stash"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("stash", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
pub async fn git_stash_pop(path: String) -> Result<String, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["stash", "pop"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("stash pop", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
pub async fn git_status(path: String) -> Result<Vec<GitStatusEntry>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["status", "--porcelain=v1", "-uall"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("status", &output.stderr));
    }

    let entries = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let status = line[..2].trim().to_string();
            let file = line[3..].to_string();
            GitStatusEntry { status, file }
        })
        .collect();
    Ok(entries)
}

#[tauri::command]
pub async fn git_is_tracked(path: String, file: String) -> Result<bool, AppCommandError> {
    let literal_file = to_git_literal_pathspec(&file);
    let output = crate::process::tokio_command("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(&literal_file)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    Ok(output.status.success())
}

#[tauri::command]
pub async fn git_diff(path: String, file: Option<String>) -> Result<String, AppCommandError> {
    let literal_file = file.as_deref().map(to_git_literal_pathspec);
    let mut args = vec!["diff".to_string(), "HEAD".to_string()];
    if let Some(ref f) = literal_file {
        args.push("--".to_string());
        args.push(f.clone());
    }

    let output = crate::process::tokio_command("git")
        .args(&args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        // For new repos with no HEAD, fall back to diff --cached
        let mut fallback_args = vec!["diff".to_string(), "--cached".to_string()];
        if let Some(ref f) = literal_file {
            fallback_args.push("--".to_string());
            fallback_args.push(f.clone());
        }
        let fallback = crate::process::tokio_command("git")
            .args(&fallback_args)
            .current_dir(&path)
            .output()
            .await
            .map_err(AppCommandError::io)?;
        return Ok(String::from_utf8_lossy(&fallback.stdout).to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[tauri::command]
pub async fn git_diff_with_branch(
    path: String,
    branch: String,
    file: Option<String>,
) -> Result<String, AppCommandError> {
    let target_branch = branch.trim();
    if target_branch.is_empty() {
        return Err(AppCommandError::invalid_input(
            "Branch name cannot be empty",
        ));
    }

    let literal_file = file.as_deref().map(to_git_literal_pathspec);
    let mut args = vec![
        "diff".to_string(),
        "--no-color".to_string(),
        target_branch.to_string(),
    ];
    if let Some(ref f) = literal_file {
        args.push("--".to_string());
        args.push(f.clone());
    }

    let output = crate::process::tokio_command("git")
        .args(&args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AppCommandError::external_command(
            "git diff failed",
            format!("branch={target_branch}; {stderr}"),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[tauri::command]
pub async fn git_show_diff(
    path: String,
    commit: String,
    file: Option<String>,
) -> Result<String, AppCommandError> {
    let literal_file = file.as_deref().map(to_git_literal_pathspec);
    let mut args = vec![
        "show".to_string(),
        "--no-color".to_string(),
        "--format=".to_string(),
        commit,
    ];
    if let Some(ref f) = literal_file {
        args.push("--".to_string());
        args.push(f.clone());
    }

    let output = crate::process::tokio_command("git")
        .args(&args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("show", &output.stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[tauri::command]
pub async fn git_show_file(
    path: String,
    file: String,
    ref_name: Option<String>,
) -> Result<String, AppCommandError> {
    let git_ref = ref_name.unwrap_or_else(|| "HEAD".to_string());
    let file_spec = format!("{}:{}", git_ref, file);

    let output = crate::process::tokio_command("git")
        .args(["show", &file_spec])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        // File doesn't exist at this ref (e.g. new/untracked file) — return empty
        return Ok(String::new());
    }

    let bytes = &output.stdout;
    if bytes.iter().take(2048).any(|b| *b == 0) {
        return Err(
            AppCommandError::invalid_input("Binary files are not supported").with_detail(file_spec),
        );
    }

    Ok(String::from_utf8_lossy(bytes).to_string())
}

#[tauri::command]
pub async fn git_commit(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    path: String,
    message: String,
    files: Vec<String>,
) -> Result<GitCommitResult, AppCommandError> {
    // Stage selected files
    let mut add_args = vec!["add".to_string(), "--".to_string()];
    add_args.extend(files.iter().map(|file| to_git_literal_pathspec(file)));

    let add_output = crate::process::tokio_command("git")
        .args(&add_args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !add_output.status.success() {
        return Err(git_command_error("add", &add_output.stderr));
    }

    // Commit
    let commit_output = crate::process::tokio_command("git")
        .args(["commit", "-m", &message])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !commit_output.status.success() {
        return Err(git_command_error("commit", &commit_output.stderr));
    }

    let committed_files = count_files_in_commit(&path, "HEAD")
        .await
        .unwrap_or(files.len());

    if let Some(folder_id) = window
        .label()
        .strip_prefix("commit-")
        .and_then(|value| value.parse::<i32>().ok())
    {
        let _ = app.emit(
            "folder://git-commit-succeeded",
            GitCommitSucceededEvent {
                folder_id,
                committed_files,
            },
        );
    }

    Ok(GitCommitResult { committed_files })
}

#[tauri::command]
pub async fn git_rollback_file(path: String, file: String) -> Result<(), AppCommandError> {
    let target = file.trim();
    if target.is_empty() {
        return Err(AppCommandError::invalid_input("File path cannot be empty"));
    }

    let literal_file = to_git_literal_pathspec(target);
    let restore_output = crate::process::tokio_command("git")
        .args([
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "--",
            &literal_file,
        ])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if restore_output.status.success() {
        return Ok(());
    }

    let restore_stderr = String::from_utf8_lossy(&restore_output.stderr)
        .trim()
        .to_string();
    let restore_stderr_lower = restore_stderr.to_lowercase();
    let supports_restore = !restore_stderr_lower.contains("unknown option")
        && !restore_stderr_lower.contains("unknown switch")
        && !restore_stderr_lower.contains("not a git command")
        && !restore_stderr_lower.contains("did you mean");

    if supports_restore {
        return Err(AppCommandError::external_command(
            "git restore failed",
            restore_stderr,
        ));
    }

    let _ = crate::process::tokio_command("git")
        .args(["reset", "HEAD", "--", &literal_file])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    let checkout_output = crate::process::tokio_command("git")
        .args(["checkout", "--", &literal_file])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !checkout_output.status.success() {
        return Err(git_command_error("checkout --", &checkout_output.stderr));
    }

    Ok(())
}

#[tauri::command]
pub async fn git_add_files(path: String, files: Vec<String>) -> Result<(), AppCommandError> {
    if files.is_empty() {
        return Ok(());
    }

    let mut args = vec!["add".to_string(), "--".to_string()];
    args.extend(files.iter().map(|file| to_git_literal_pathspec(file)));

    let output = crate::process::tokio_command("git")
        .args(&args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("add", &output.stderr));
    }

    Ok(())
}

#[tauri::command]
pub async fn git_list_all_branches(path: String) -> Result<GitBranchList, AppCommandError> {
    let local_fut = crate::process::tokio_command("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(&path)
        .output();

    let remote_fut = crate::process::tokio_command("git")
        .args(["branch", "-r", "--format=%(refname:short)"])
        .current_dir(&path)
        .output();

    let wt_fut = crate::process::tokio_command("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&path)
        .output();

    let (local_output, remote_output, wt_output) = tokio::join!(local_fut, remote_fut, wt_fut);

    let local_output = local_output.map_err(AppCommandError::io)?;
    if !local_output.status.success() {
        return Err(git_command_error("branch", &local_output.stderr));
    }

    let local: Vec<String> = String::from_utf8_lossy(&local_output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let remote: Vec<String> = match remote_output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.contains("HEAD") && l.contains('/'))
            .collect(),
        _ => vec![],
    };

    // Parse worktree entries, excluding the current worktree (path itself)
    let worktree_branches: Vec<String> = match wt_output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let canonical_path =
                std::fs::canonicalize(&path).unwrap_or_else(|_| PathBuf::from(&path));
            let mut branches = Vec::new();
            let mut current_wt_path: Option<String> = None;
            for line in stdout.lines() {
                if let Some(wt) = line.strip_prefix("worktree ") {
                    current_wt_path = Some(wt.trim().to_string());
                } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    if let Some(ref wt) = current_wt_path {
                        let wt_canonical =
                            std::fs::canonicalize(wt).unwrap_or_else(|_| PathBuf::from(wt));
                        if wt_canonical != canonical_path {
                            branches.push(b.trim().to_string());
                        }
                    }
                } else if line.is_empty() {
                    current_wt_path = None;
                }
            }
            branches
        }
        _ => vec![],
    };

    Ok(GitBranchList {
        local,
        remote,
        worktree_branches,
    })
}

#[tauri::command]
pub async fn git_list_remotes(path: String) -> Result<Vec<GitRemote>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["remote", "-v"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("remote -v", &output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut seen = HashSet::new();
    let mut remotes = Vec::new();
    for line in stdout.lines() {
        // Format: "name\turl (fetch|push)"
        if !line.ends_with("(fetch)") {
            continue;
        }
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let url = rest.trim_end_matches("(fetch)").trim();
        if seen.insert(name.to_string()) {
            remotes.push(GitRemote {
                name: name.to_string(),
                url: url.to_string(),
            });
        }
    }
    Ok(remotes)
}

#[tauri::command]
pub async fn git_fetch_remote(path: String, name: String) -> Result<String, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["fetch", &name])
        .current_dir(&path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("fetch", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

#[tauri::command]
pub async fn git_add_remote(
    path: String,
    name: String,
    url: String,
) -> Result<(), AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["remote", "add", &name, &url])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("remote add", &output.stderr));
    }
    Ok(())
}

#[tauri::command]
pub async fn git_remove_remote(path: String, name: String) -> Result<(), AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["remote", "remove", &name])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("remote remove", &output.stderr));
    }
    Ok(())
}

#[tauri::command]
pub async fn git_set_remote_url(
    path: String,
    name: String,
    url: String,
) -> Result<(), AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["remote", "set-url", &name, &url])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("remote set-url", &output.stderr));
    }
    Ok(())
}

#[tauri::command]
pub async fn git_merge(
    path: String,
    branch_name: String,
) -> Result<GitMergeResult, AppCommandError> {
    // Count commits to be merged before performing merge
    let count_output = crate::process::tokio_command("git")
        .args(["rev-list", "--count", &format!("HEAD..{}", branch_name)])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    let merged_commits = if count_output.status.success() {
        String::from_utf8_lossy(&count_output.stdout)
            .trim()
            .parse::<usize>()
            .unwrap_or(0)
    } else {
        0
    };

    let output = crate::process::tokio_command("git")
        .args(["merge", &branch_name])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("merge", &output.stderr));
    }
    Ok(GitMergeResult { merged_commits })
}

#[tauri::command]
pub async fn git_rebase(path: String, branch_name: String) -> Result<String, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["rebase", &branch_name])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("rebase", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
pub async fn git_delete_branch(
    path: String,
    branch_name: String,
    force: bool,
) -> Result<String, AppCommandError> {
    let flag = if force { "-D" } else { "-d" };
    let output = crate::process::tokio_command("git")
        .args(["branch", flag, &branch_name])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error(&format!("branch {flag}"), &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

const WATCH_IGNORED_DIRS: &[&str] = &["__pycache__"];
const FILE_TREE_IGNORED_DIRS: &[&str] = &[".git", "__pycache__"];

const FILE_PREVIEW_DEFAULT_MAX_BYTES: usize = 200_000;
const FILE_PREVIEW_MIN_BYTES: usize = 4_096;
const FILE_PREVIEW_MAX_BYTES: usize = 2_000_000;
const FILE_EDIT_DEFAULT_MAX_BYTES: usize = 400_000;
const FILE_EDIT_MAX_BYTES: usize = 2_000_000;
const FILE_BASE64_DEFAULT_MAX_BYTES: usize = 20_000_000;
const FILE_BASE64_MAX_BYTES: usize = 100_000_000;
const FILE_IO_MAX_CONCURRENT_OPS: usize = 8;

static FILE_IO_SEMAPHORE: LazyLock<Semaphore> =
    LazyLock::new(|| Semaphore::new(FILE_IO_MAX_CONCURRENT_OPS));
static FILE_WATCHERS: LazyLock<Mutex<HashMap<String, FileWatchEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const FILE_WATCH_DEBOUNCE_MS: u64 = 150;
const FILE_WATCH_MAX_BATCH_WINDOW_MS: u64 = 500;
const FILE_WATCH_MAX_CHANGED_PATHS: usize = 2_000;

fn to_git_literal_pathspec(path: &str) -> String {
    format!(":(literal){path}")
}

fn normalize_slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn is_git_metadata_rel_path(path: &str) -> bool {
    path == ".git" || path.starts_with(".git/")
}

fn is_gitignore_rel_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy() == ".gitignore")
        .unwrap_or(false)
}

fn is_codeg_edit_temp_path(path: &Path) -> bool {
    path.file_name()
        .map(|name| {
            let name = name.to_string_lossy();
            name.starts_with(".codeg-edit-") && name.ends_with(".tmp")
        })
        .unwrap_or(false)
}

fn git_check_ignored_paths(
    repo_path: &str,
    paths: &[String],
) -> Result<HashSet<String>, AppCommandError> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }

    let mut child = Command::new("git")
        .args(["check-ignore", "--stdin", "-z"])
        .current_dir(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(AppCommandError::io)?;

    if let Some(mut stdin) = child.stdin.take() {
        for path in paths {
            stdin
                .write_all(path.as_bytes())
                .map_err(AppCommandError::io)?;
            stdin.write_all(&[0]).map_err(AppCommandError::io)?;
        }
    }

    let output = child.wait_with_output().map_err(AppCommandError::io)?;

    // Exit code 1 means "no matches", which is expected.
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(git_command_error("check-ignore", &output.stderr));
    }

    let mut ignored = HashSet::new();
    for raw in output.stdout.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        ignored.insert(String::from_utf8_lossy(raw).to_string());
    }
    Ok(ignored)
}

fn should_refresh_git_status_for_paths(root_display: &str, changed_paths: &[String]) -> bool {
    if changed_paths.is_empty() {
        return true;
    }

    let mut candidates: Vec<String> = Vec::new();
    for path in changed_paths {
        if is_git_metadata_rel_path(path) || is_gitignore_rel_path(path) {
            return true;
        }
        candidates.push(path.clone());
    }

    if candidates.is_empty() {
        return false;
    }

    let ignored = match git_check_ignored_paths(root_display, &candidates) {
        Ok(ignored) => ignored,
        // Fail safe: if detection fails, keep current behavior and refresh status.
        Err(_) => return true,
    };

    candidates
        .iter()
        .any(|path| !ignored.contains(path.as_str()))
}

fn canonicalize_watch_root(root: &Path) -> Result<(PathBuf, String), AppCommandError> {
    let canonical = std::fs::canonicalize(root).map_err(|e| {
        AppCommandError::not_found("Unable to resolve workspace root").with_detail(e.to_string())
    })?;
    let key = normalize_slash_path(&canonical);
    Ok((canonical, key))
}

fn is_allowed_git_watch_path(relative: &Path) -> bool {
    let mut components = relative.components();

    let Some(Component::Normal(first)) = components.next() else {
        return false;
    };
    if first.to_string_lossy() != ".git" {
        return false;
    }

    let Some(Component::Normal(second)) = components.next() else {
        // Allow top-level .git events.
        return true;
    };

    let second_name = second.to_string_lossy();
    match second_name.as_ref() {
        "HEAD" | "index" | "packed-refs" | "FETCH_HEAD" | "ORIG_HEAD" | "MERGE_HEAD"
        | "CHERRY_PICK_HEAD" | "REVERT_HEAD" => true,
        "refs" => {
            let Some(Component::Normal(scope)) = components.next() else {
                return true;
            };
            matches!(
                scope.to_string_lossy().as_ref(),
                "heads" | "remotes" | "stash"
            )
        }
        "rebase-merge" | "rebase-apply" => true,
        _ => false,
    }
}

fn is_ignored_watch_path(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };

    if is_codeg_edit_temp_path(relative) {
        return true;
    }

    let mut components = relative.components();
    if let Some(Component::Normal(first)) = components.next() {
        if first.to_string_lossy() == ".git" {
            return !is_allowed_git_watch_path(relative);
        }
    }

    relative.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        let component_name = name.to_string_lossy();
        WATCH_IGNORED_DIRS
            .iter()
            .any(|ignored| *ignored == component_name.as_ref())
    })
}

fn should_emit_watch_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

#[derive(Default)]
struct WatchEventBatch {
    changed_paths: HashSet<String>,
    has_create: bool,
    has_remove: bool,
    overflowed: bool,
}

impl WatchEventBatch {
    fn clear(&mut self) {
        self.changed_paths.clear();
        self.has_create = false;
        self.has_remove = false;
        self.overflowed = false;
    }

    fn is_empty(&self) -> bool {
        !self.overflowed && self.changed_paths.is_empty()
    }

    fn kind(&self) -> &'static str {
        if self.has_remove {
            "remove"
        } else if self.has_create {
            "create"
        } else {
            "modify"
        }
    }

    fn ingest_event(&mut self, root_canonical: &Path, event: notify::Event) {
        if !should_emit_watch_event(&event.kind) {
            return;
        }

        if self.overflowed {
            return;
        }

        let mut has_relevant_path = false;
        for path in event.paths {
            if is_ignored_watch_path(&path, root_canonical) {
                continue;
            }

            let relative = if let Ok(relative) = path.strip_prefix(root_canonical) {
                normalize_slash_path(relative)
            } else {
                normalize_slash_path(&path)
            };

            if relative.is_empty() {
                continue;
            }

            self.changed_paths.insert(relative);
            has_relevant_path = true;
            if self.changed_paths.len() > FILE_WATCH_MAX_CHANGED_PATHS {
                self.overflowed = true;
                self.changed_paths.clear();
                break;
            }
        }

        if !has_relevant_path {
            return;
        }

        match event.kind {
            EventKind::Create(_) => self.has_create = true,
            EventKind::Remove(_) => self.has_remove = true,
            _ => {}
        }
    }

    fn emit(&self, app: &tauri::AppHandle, root_display: &str) {
        if self.is_empty() {
            return;
        }

        let changed_paths = if self.overflowed {
            Vec::new()
        } else {
            let mut paths = self.changed_paths.iter().cloned().collect::<Vec<_>>();
            paths.sort();
            paths
        };

        let payload = FileTreeChangedEvent {
            root_path: root_display.to_string(),
            refresh_git_status: if self.overflowed {
                true
            } else {
                should_refresh_git_status_for_paths(root_display, &changed_paths)
            },
            changed_paths,
            kind: self.kind().to_string(),
            full_reload: self.overflowed,
        };

        let _ = app.emit("folder://file-tree-changed", payload);
    }
}

fn run_file_watch_event_loop(
    event_rx: mpsc::Receiver<notify::Event>,
    app: tauri::AppHandle,
    root_display: String,
    root_canonical: PathBuf,
) {
    let debounce = Duration::from_millis(FILE_WATCH_DEBOUNCE_MS);
    let max_batch_window = Duration::from_millis(FILE_WATCH_MAX_BATCH_WINDOW_MS);
    let mut batch = WatchEventBatch::default();
    let mut batch_started_at: Option<Instant> = None;

    loop {
        match event_rx.recv_timeout(debounce) {
            Ok(event) => {
                batch.ingest_event(&root_canonical, event);
                if !batch.is_empty() && batch_started_at.is_none() {
                    batch_started_at = Some(Instant::now());
                }

                while let Ok(next_event) = event_rx.try_recv() {
                    batch.ingest_event(&root_canonical, next_event);
                    if !batch.is_empty() && batch_started_at.is_none() {
                        batch_started_at = Some(Instant::now());
                    }
                }

                let should_flush = if batch.overflowed {
                    true
                } else {
                    batch_started_at
                        .map(|started| started.elapsed() >= max_batch_window)
                        .unwrap_or(false)
                };

                if should_flush {
                    batch.emit(&app, &root_display);
                    batch.clear();
                    batch_started_at = None;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !batch.is_empty() {
                    batch.emit(&app, &root_display);
                    batch.clear();
                    batch_started_at = None;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !batch.is_empty() {
                    batch.emit(&app, &root_display);
                }
                break;
            }
        }
    }
}

fn resolve_tree_path(root: &Path, rel_path: &str) -> Result<PathBuf, AppCommandError> {
    let rel = Path::new(rel_path);
    if rel.is_absolute() {
        return Err(AppCommandError::invalid_input("Path must be relative"));
    }

    for component in rel.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(AppCommandError::invalid_input("Path cannot contain '..'"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(AppCommandError::invalid_input("Invalid path component"));
            }
        }
    }

    Ok(root.join(rel))
}

fn validate_new_name(new_name: &str) -> Result<&str, AppCommandError> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err(AppCommandError::invalid_input("New name cannot be empty"));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(AppCommandError::invalid_input("Invalid file name"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(AppCommandError::invalid_input(
            "New name cannot contain path separators",
        ));
    }
    Ok(trimmed)
}

#[tauri::command]
pub async fn start_file_tree_watch(
    app: tauri::AppHandle,
    root_path: String,
) -> Result<(), AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let (root_canonical, key) = canonicalize_watch_root(&root)?;

    {
        let mut watchers = FILE_WATCHERS.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock file watcher registry")
        })?;
        if let Some(entry) = watchers.get_mut(&key) {
            entry.ref_count += 1;
            return Ok(());
        }
    }

    let root_display_for_worker = root_path.clone();
    let root_display_for_error = root_path.clone();
    let root_canonical_for_worker = root_canonical.clone();
    let app_for_worker = app.clone();
    let (event_tx, event_rx) = mpsc::channel::<notify::Event>();
    let mut worker = Some(std::thread::spawn(move || {
        run_file_watch_event_loop(
            event_rx,
            app_for_worker,
            root_display_for_worker,
            root_canonical_for_worker,
        )
    }));

    let mut watcher = Some(
        notify::recommended_watcher(
            move |result: Result<notify::Event, notify::Error>| match result {
                Ok(event) => {
                    let _ = event_tx.send(event);
                }
                Err(err) => {
                    eprintln!(
                        "[file-watch] failed event for {}: {}",
                        root_display_for_error, err
                    );
                }
            },
        )
        .map_err(|e| {
            AppCommandError::io_error("Failed to create file watcher").with_detail(e.to_string())
        })?,
    );

    watcher
        .as_mut()
        .ok_or_else(|| AppCommandError::task_execution_failed("Failed to create file watcher"))?
        .watch(&root_canonical, RecursiveMode::Recursive)
        .map_err(|e| {
            AppCommandError::io_error("Failed to start file watcher").with_detail(e.to_string())
        })?;

    let should_cleanup_new_watcher = {
        let mut watchers = FILE_WATCHERS.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock file watcher registry")
        })?;
        if let Some(entry) = watchers.get_mut(&key) {
            entry.ref_count += 1;
            true
        } else {
            watchers.insert(
                key,
                FileWatchEntry {
                    root_canonical,
                    root_display: root_path,
                    watcher: watcher.take().ok_or_else(|| {
                        AppCommandError::task_execution_failed(
                            "Failed to initialize file watcher state",
                        )
                    })?,
                    worker: worker.take(),
                    ref_count: 1,
                },
            );
            false
        }
    };

    if !should_cleanup_new_watcher {
        return Ok(());
    }

    drop(watcher.take());
    if let Some(handle) = worker.take() {
        let _ = handle.join();
    }

    Ok(())
}

#[tauri::command]
pub async fn stop_file_tree_watch(root_path: String) -> Result<(), AppCommandError> {
    let root = PathBuf::from(&root_path);
    let key = canonicalize_watch_root(&root)
        .map(|(_, key)| key)
        .unwrap_or_else(|_| normalize_slash_path(&root));

    let mut watchers = FILE_WATCHERS.lock().map_err(|_| {
        AppCommandError::task_execution_failed("Failed to lock file watcher registry")
    })?;

    let target_key = if watchers.contains_key(&key) {
        Some(key)
    } else {
        watchers.iter().find_map(|(candidate_key, entry)| {
            if entry.root_display == root_path {
                Some(candidate_key.clone())
            } else {
                None
            }
        })
    };

    let Some(target_key) = target_key else {
        return Ok(());
    };

    if let Some(entry) = watchers.get_mut(&target_key) {
        if entry.ref_count > 1 {
            entry.ref_count -= 1;
            return Ok(());
        }
    }

    let mut removed_entry = watchers.remove(&target_key);
    drop(watchers);

    if let Some(mut entry) = removed_entry.take() {
        let _ = entry.watcher.unwatch(&entry.root_canonical);
        drop(entry.watcher);
        if let Some(worker) = entry.worker.take() {
            let _ = worker.join();
        }
    }

    Ok(())
}

fn file_mtime_ms(metadata: &std::fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let elapsed = modified.duration_since(UNIX_EPOCH).ok()?;
    let millis = elapsed.as_millis();
    if millis > i64::MAX as u128 {
        return Some(i64::MAX);
    }
    Some(millis as i64)
}

fn detect_line_ending(content: &[u8]) -> String {
    let mut has_lf = false;
    let mut has_crlf = false;

    for index in 0..content.len() {
        if content[index] != b'\n' {
            continue;
        }

        if index > 0 && content[index - 1] == b'\r' {
            has_crlf = true;
        } else {
            has_lf = true;
        }

        if has_lf && has_crlf {
            return "mixed".to_string();
        }
    }

    if has_crlf {
        "crlf".to_string()
    } else if has_lf {
        "lf".to_string()
    } else {
        "none".to_string()
    }
}

fn compute_etag(content: &[u8], metadata: &std::fs::Metadata) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    if let Some(mtime_ms) = file_mtime_ms(metadata) {
        mtime_ms.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

fn ensure_path_in_workspace(root: &Path, target: &Path) -> Result<(), AppCommandError> {
    let canonical_root = std::fs::canonicalize(root).map_err(AppCommandError::io)?;
    let canonical_target = std::fs::canonicalize(target).map_err(AppCommandError::io)?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(AppCommandError::invalid_input(
            "Path is outside workspace root",
        ));
    }
    Ok(())
}

fn read_text_preview(target: &Path, limit: usize) -> Result<(String, bool), AppCommandError> {
    let metadata = std::fs::metadata(target).map_err(AppCommandError::io)?;
    let mut file = File::open(target).map_err(AppCommandError::io)?;
    let mut bytes = Vec::new();
    let mut limited_reader = (&mut file).take(limit as u64 + 1);
    limited_reader
        .read_to_end(&mut bytes)
        .map_err(AppCommandError::io)?;

    if bytes.iter().take(2_048).any(|b| *b == 0) {
        return Err(AppCommandError::invalid_input(
            "Binary files are not supported in preview",
        ));
    }

    let truncated = bytes.len() > limit || metadata.len() > limit as u64;
    if bytes.len() > limit {
        bytes.truncate(limit);
    }
    Ok((String::from_utf8_lossy(&bytes).to_string(), truncated))
}

fn atomic_write_text(path: &Path, bytes: &[u8]) -> Result<(), AppCommandError> {
    let parent = path.parent().ok_or_else(|| {
        AppCommandError::invalid_input("Cannot determine parent directory for target file")
            .with_detail(path.display().to_string())
    })?;
    if !parent.exists() {
        return Err(
            AppCommandError::not_found("Parent directory does not exist")
                .with_detail(parent.display().to_string()),
        );
    }

    let temp_path = parent.join(format!(
        ".codeg-edit-{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let existing_permissions = std::fs::metadata(path).ok().map(|m| m.permissions());

    let write_result = (|| -> Result<(), AppCommandError> {
        let mut temp = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(AppCommandError::io)?;

        temp.write_all(bytes).map_err(AppCommandError::io)?;
        temp.sync_all().map_err(AppCommandError::io)?;

        if let Some(permissions) = existing_permissions {
            std::fs::set_permissions(&temp_path, permissions).map_err(AppCommandError::io)?;
        }

        replace_file(&temp_path, path)?;
        sync_directory(parent)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }

    write_result
}

#[cfg(unix)]
fn replace_file(temp_path: &Path, target_path: &Path) -> Result<(), AppCommandError> {
    std::fs::rename(temp_path, target_path).map_err(AppCommandError::io)
}

#[cfg(target_os = "windows")]
fn replace_file(temp_path: &Path, target_path: &Path) -> Result<(), AppCommandError> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    fn to_wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let src = to_wide(temp_path);
    let dst = to_wide(target_path);

    // SAFETY: pointers are valid and UTF-16 null-terminated for the duration of the call.
    let ok = unsafe {
        MoveFileExW(
            src.as_ptr(),
            dst.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };

    if ok == 0 {
        return Err(
            AppCommandError::io_error("Failed to atomically replace file")
                .with_detail(std::io::Error::last_os_error().to_string()),
        );
    }

    Ok(())
}

#[cfg(not(any(unix, target_os = "windows")))]
fn replace_file(temp_path: &Path, target_path: &Path) -> Result<(), AppCommandError> {
    std::fs::rename(temp_path, target_path).map_err(AppCommandError::io)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), AppCommandError> {
    let dir = File::open(path).map_err(AppCommandError::io)?;
    dir.sync_all().map_err(AppCommandError::io)
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), AppCommandError> {
    Ok(())
}

async fn run_file_io<T, F>(f: F) -> Result<T, AppCommandError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppCommandError> + Send + 'static,
{
    let _permit = FILE_IO_SEMAPHORE
        .acquire()
        .await
        .map_err(|_| AppCommandError::task_execution_failed("File I/O runtime is unavailable"))?;

    tokio::task::spawn_blocking(f).await.map_err(|e| {
        AppCommandError::task_execution_failed("File I/O task failed").with_detail(e.to_string())
    })?
}

#[tauri::command]
pub async fn get_file_tree(
    path: String,
    max_depth: Option<usize>,
) -> Result<Vec<FileTreeNode>, AppCommandError> {
    let root = PathBuf::from(&path);
    let depth = max_depth.unwrap_or(usize::MAX);

    // Collect all entries, skipping ignored directories
    let mut dir_children: HashMap<PathBuf, Vec<FileTreeNode>> = HashMap::new();
    let mut dir_order: Vec<PathBuf> = Vec::new();
    let mut dir_paths_by_rel: HashMap<String, PathBuf> = HashMap::new();

    for entry in WalkDir::new(&root)
        .max_depth(depth)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            if e.file_type().is_dir() {
                !FILE_TREE_IGNORED_DIRS.contains(&name.as_ref())
            } else {
                name != ".DS_Store"
            }
        })
    {
        let entry = entry.map_err(|e| {
            AppCommandError::io_error("Failed to walk file tree").with_detail(e.to_string())
        })?;
        let entry_path = entry.path().to_path_buf();

        // Skip the root itself
        if entry_path == root {
            dir_children.entry(root.clone()).or_default();
            dir_order.push(root.clone());
            continue;
        }

        let parent = entry_path.parent().unwrap_or(&root).to_path_buf();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel_path = entry_path
            .strip_prefix(&root)
            .unwrap_or(&entry_path)
            .to_string_lossy()
            .replace('\\', "/");

        if entry.file_type().is_dir() {
            dir_paths_by_rel.insert(rel_path.clone(), entry_path.clone());
            dir_children.entry(entry_path.clone()).or_default();
            dir_order.push(entry_path);
            // Add a placeholder Dir node to parent (children filled later)
            dir_children
                .entry(parent)
                .or_default()
                .push(FileTreeNode::Dir {
                    name,
                    path: rel_path,
                    children: vec![],
                });
        } else {
            dir_children
                .entry(parent)
                .or_default()
                .push(FileTreeNode::File {
                    name,
                    path: rel_path,
                });
        }
    }

    // Build tree bottom-up: process dirs in reverse order so children are ready
    for dir_path in dir_order.iter().rev() {
        let children = dir_children.remove(dir_path).unwrap_or_default();

        // Sort: dirs first, then files, alphabetically within each group
        let mut dirs: Vec<FileTreeNode> = Vec::new();
        let mut files: Vec<FileTreeNode> = Vec::new();
        for child in children {
            match &child {
                FileTreeNode::Dir { .. } => dirs.push(child),
                FileTreeNode::File { .. } => files.push(child),
            }
        }
        dirs.sort_by(|a, b| {
            let a_name = match a {
                FileTreeNode::Dir { name, .. } => name,
                _ => unreachable!(),
            };
            let b_name = match b {
                FileTreeNode::Dir { name, .. } => name,
                _ => unreachable!(),
            };
            a_name.to_lowercase().cmp(&b_name.to_lowercase())
        });
        files.sort_by(|a, b| {
            let a_name = match a {
                FileTreeNode::File { name, .. } => name,
                _ => unreachable!(),
            };
            let b_name = match b {
                FileTreeNode::File { name, .. } => name,
                _ => unreachable!(),
            };
            a_name.to_lowercase().cmp(&b_name.to_lowercase())
        });

        let mut sorted: Vec<FileTreeNode> = Vec::with_capacity(dirs.len() + files.len());

        // Fill dir children from the map
        for d in dirs {
            if let FileTreeNode::Dir {
                name,
                path: rel_path,
                ..
            } = d
            {
                let full_path = dir_paths_by_rel
                    .get(&rel_path)
                    .cloned()
                    .unwrap_or_else(|| root.join(Path::new(&rel_path)));
                let sub_children = dir_children.remove(&full_path).unwrap_or_default();
                sorted.push(FileTreeNode::Dir {
                    name,
                    path: rel_path,
                    children: sub_children,
                });
            }
        }
        sorted.extend(files);

        dir_children.insert(dir_path.clone(), sorted);
    }

    Ok(dir_children.remove(&root).unwrap_or_default())
}

#[tauri::command]
pub async fn read_file_base64(
    path: String,
    max_bytes: Option<usize>,
) -> Result<String, AppCommandError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppCommandError::invalid_input("Path cannot be empty"));
    }
    let target = PathBuf::from(trimmed);
    if !target.exists() {
        return Err(AppCommandError::not_found("File does not exist"));
    }
    if !target.is_file() {
        return Err(AppCommandError::invalid_input("Path is not a file"));
    }

    let limit = max_bytes
        .unwrap_or(FILE_BASE64_DEFAULT_MAX_BYTES)
        .clamp(FILE_PREVIEW_MIN_BYTES, FILE_BASE64_MAX_BYTES);

    run_file_io(move || {
        let metadata = std::fs::metadata(&target).map_err(AppCommandError::io)?;
        if metadata.len() > limit as u64 {
            return Err(
                AppCommandError::invalid_input("File is too large to attach")
                    .with_detail(format!("max_bytes={limit}")),
            );
        }
        let bytes = std::fs::read(&target).map_err(AppCommandError::io)?;
        if bytes.len() > limit {
            return Err(
                AppCommandError::invalid_input("File is too large to attach")
                    .with_detail(format!("max_bytes={limit}")),
            );
        }
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    })
    .await
}

#[tauri::command]
pub async fn read_file_preview(
    root_path: String,
    path: String,
    max_bytes: Option<usize>,
) -> Result<FilePreviewContent, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let target = resolve_tree_path(&root, &path)?;
    if !target.exists() {
        return Err(AppCommandError::not_found("File does not exist"));
    }
    if !target.is_file() {
        return Err(AppCommandError::invalid_input("Path is not a file"));
    }
    let limit = max_bytes
        .unwrap_or(FILE_PREVIEW_DEFAULT_MAX_BYTES)
        .clamp(FILE_PREVIEW_MIN_BYTES, FILE_PREVIEW_MAX_BYTES);
    let path_for_response = path.clone();

    run_file_io(move || {
        ensure_path_in_workspace(&root, &target)?;
        let (content, truncated) = read_text_preview(&target, limit)?;
        Ok(FilePreviewContent {
            path: path_for_response,
            content,
            truncated,
        })
    })
    .await
}

#[tauri::command]
pub async fn read_file_for_edit(
    root_path: String,
    path: String,
    max_bytes: Option<usize>,
) -> Result<FileEditContent, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let target = resolve_tree_path(&root, &path)?;
    if !target.exists() {
        return Err(AppCommandError::not_found("File does not exist"));
    }
    if !target.is_file() {
        return Err(AppCommandError::invalid_input("Path is not a file"));
    }

    let limit = max_bytes
        .unwrap_or(FILE_EDIT_DEFAULT_MAX_BYTES)
        .clamp(FILE_PREVIEW_MIN_BYTES, FILE_EDIT_MAX_BYTES);
    let path_for_response = path.clone();

    run_file_io(move || {
        ensure_path_in_workspace(&root, &target)?;
        let metadata = std::fs::metadata(&target).map_err(AppCommandError::io)?;
        let (content, truncated) = read_text_preview(&target, limit)?;
        let readonly = metadata.permissions().readonly() || truncated;
        let mtime_ms = file_mtime_ms(&metadata);
        let etag_source = if truncated {
            format!("{}:{}", metadata.len(), mtime_ms.unwrap_or_default()).into_bytes()
        } else {
            content.as_bytes().to_vec()
        };
        let etag = compute_etag(&etag_source, &metadata);
        let line_ending = detect_line_ending(content.as_bytes());

        Ok(FileEditContent {
            path: path_for_response,
            content,
            etag,
            mtime_ms,
            readonly,
            truncated,
            line_ending,
        })
    })
    .await
}

#[tauri::command]
pub async fn save_file_content(
    root_path: String,
    path: String,
    content: String,
    expected_etag: Option<String>,
) -> Result<FileSaveResult, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }
    if content.len() > FILE_EDIT_MAX_BYTES {
        return Err(
            AppCommandError::invalid_input("File is too large to save in editor")
                .with_detail(format!("max_bytes={FILE_EDIT_MAX_BYTES}")),
        );
    }

    let target = resolve_tree_path(&root, &path)?;
    if !target.exists() {
        return Err(AppCommandError::not_found("File does not exist"));
    }
    if !target.is_file() {
        return Err(AppCommandError::invalid_input("Path is not a file"));
    }
    let path_for_response = path.clone();

    run_file_io(move || {
        ensure_path_in_workspace(&root, &target)?;

        let link_meta = std::fs::symlink_metadata(&target).map_err(AppCommandError::io)?;
        if link_meta.file_type().is_symlink() {
            return Err(AppCommandError::invalid_input(
                "Saving symlink targets is not supported",
            ));
        }

        let before_meta = std::fs::metadata(&target).map_err(AppCommandError::io)?;
        if before_meta.permissions().readonly() {
            return Err(AppCommandError::permission_denied("File is read-only"));
        }

        let current_bytes = std::fs::read(&target).map_err(AppCommandError::io)?;
        if current_bytes.iter().take(2_048).any(|b| *b == 0) {
            return Err(AppCommandError::invalid_input(
                "Binary files are not supported in editor",
            ));
        }
        let current_etag = compute_etag(&current_bytes, &before_meta);
        if let Some(expected) = expected_etag {
            if expected != current_etag {
                return Err(AppCommandError::invalid_input(
                    "File has changed on disk. Reload the file before saving.",
                ));
            }
        }

        atomic_write_text(&target, content.as_bytes())?;

        let after_meta = std::fs::metadata(&target).map_err(AppCommandError::io)?;
        let etag = compute_etag(content.as_bytes(), &after_meta);
        let mtime_ms = file_mtime_ms(&after_meta);
        let readonly = after_meta.permissions().readonly();
        let line_ending = detect_line_ending(content.as_bytes());

        Ok(FileSaveResult {
            path: path_for_response,
            etag,
            mtime_ms,
            readonly,
            line_ending,
        })
    })
    .await
}

fn build_local_copy_file_name(original_name: &str, attempt: usize) -> String {
    let original = Path::new(original_name);
    let stem = original
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(original_name);
    let extension = original
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty());

    let suffix = if attempt <= 1 {
        ".local".to_string()
    } else {
        format!(".local.{}", attempt)
    };

    match extension {
        Some(ext) => format!("{stem}{suffix}.{ext}"),
        None => format!("{stem}{suffix}"),
    }
}

#[tauri::command]
pub async fn save_file_copy(
    root_path: String,
    path: String,
    content: String,
) -> Result<FileSaveResult, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }
    if content.len() > FILE_EDIT_MAX_BYTES {
        return Err(
            AppCommandError::invalid_input("File is too large to save in editor")
                .with_detail(format!("max_bytes={FILE_EDIT_MAX_BYTES}")),
        );
    }

    let source = resolve_tree_path(&root, &path)?;
    if !source.exists() {
        return Err(AppCommandError::not_found("File does not exist"));
    }
    if !source.is_file() {
        return Err(AppCommandError::invalid_input("Path is not a file"));
    }

    run_file_io(move || {
        ensure_path_in_workspace(&root, &source)?;

        let source_meta = std::fs::symlink_metadata(&source).map_err(AppCommandError::io)?;
        if source_meta.file_type().is_symlink() {
            return Err(AppCommandError::invalid_input(
                "Saving symlink targets is not supported",
            ));
        }

        let parent = source
            .parent()
            .ok_or_else(|| {
                AppCommandError::invalid_input("Cannot determine parent directory for source file")
            })?
            .to_path_buf();
        ensure_path_in_workspace(&root, &parent)?;

        let source_name = source
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .ok_or_else(|| AppCommandError::invalid_input("Cannot determine source file name"))?;

        let mut created_path: Option<PathBuf> = None;
        for attempt in 1..=9_999 {
            let candidate_name = build_local_copy_file_name(&source_name, attempt);
            let candidate_path = parent.join(candidate_name);
            if candidate_path.exists() {
                continue;
            }
            created_path = Some(candidate_path);
            break;
        }

        let created_path = created_path.ok_or_else(|| {
            AppCommandError::already_exists(
                "Unable to create copy file: too many existing local copies",
            )
        })?;
        atomic_write_text(&created_path, content.as_bytes())?;

        let metadata = std::fs::metadata(&created_path).map_err(AppCommandError::io)?;
        let etag = compute_etag(content.as_bytes(), &metadata);
        let mtime_ms = file_mtime_ms(&metadata);
        let readonly = metadata.permissions().readonly();
        let line_ending = detect_line_ending(content.as_bytes());
        let rel_path = created_path
            .strip_prefix(&root)
            .map_err(|e| {
                AppCommandError::invalid_input("Failed to compute relative path for copy")
                    .with_detail(e.to_string())
            })?
            .to_string_lossy()
            .replace('\\', "/");

        Ok(FileSaveResult {
            path: rel_path,
            etag,
            mtime_ms,
            readonly,
            line_ending,
        })
    })
    .await
}

#[tauri::command]
pub async fn rename_file_tree_entry(
    root_path: String,
    path: String,
    new_name: String,
) -> Result<String, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let target = resolve_tree_path(&root, &path)?;
    if !target.exists() {
        return Err(AppCommandError::not_found("Target file does not exist"));
    }
    if target == root {
        return Err(AppCommandError::invalid_input(
            "Cannot rename workspace root",
        ));
    }

    let parent = target
        .parent()
        .ok_or_else(|| AppCommandError::invalid_input("Cannot rename path without parent"))?;
    let validated_name = validate_new_name(&new_name)?;
    let next_path = parent.join(validated_name);

    if next_path == target {
        return Ok(path);
    }
    if next_path.exists() {
        return Err(AppCommandError::already_exists(
            "A file with this name already exists",
        ));
    }

    std::fs::rename(&target, &next_path).map_err(AppCommandError::io)?;

    let rel = next_path
        .strip_prefix(&root)
        .map_err(|e| {
            AppCommandError::invalid_input("Failed to compute relative path")
                .with_detail(e.to_string())
        })?
        .to_string_lossy()
        .to_string();
    Ok(rel)
}

#[tauri::command]
pub async fn delete_file_tree_entry(
    root_path: String,
    path: String,
) -> Result<(), AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let target = resolve_tree_path(&root, &path)?;
    if !target.exists() {
        return Err(AppCommandError::not_found("Target file does not exist"));
    }
    if target == root {
        return Err(AppCommandError::invalid_input(
            "Cannot delete workspace root",
        ));
    }

    let meta = std::fs::symlink_metadata(&target).map_err(AppCommandError::io)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(&target).map_err(AppCommandError::io)?;
    } else {
        std::fs::remove_file(&target).map_err(AppCommandError::io)?;
    }

    Ok(())
}

#[tauri::command]
pub async fn create_file_tree_entry(
    root_path: String,
    path: String,
    name: String,
    kind: String,
) -> Result<String, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let validated_name = validate_new_name(&name)?;

    let parent_dir = if path.is_empty() {
        root.clone()
    } else {
        let resolved = resolve_tree_path(&root, &path)?;
        if !resolved.exists() {
            return Err(AppCommandError::not_found("Parent path does not exist"));
        }
        if resolved.is_file() {
            resolved
                .parent()
                .map(|p| p.to_path_buf())
                .ok_or_else(|| AppCommandError::invalid_input("Cannot determine parent directory"))?
        } else {
            resolved
        }
    };

    let target = parent_dir.join(validated_name);
    if target.exists() {
        return Err(AppCommandError::already_exists(
            "A file or directory with this name already exists",
        ));
    }

    match kind.as_str() {
        "file" => {
            std::fs::File::create(&target).map_err(AppCommandError::io)?;
        }
        "dir" => {
            std::fs::create_dir(&target).map_err(AppCommandError::io)?;
        }
        _ => {
            return Err(AppCommandError::invalid_input(
                "Kind must be 'file' or 'dir'",
            ));
        }
    }

    let rel = target
        .strip_prefix(&root)
        .map_err(|e| {
            AppCommandError::invalid_input("Failed to compute relative path")
                .with_detail(e.to_string())
        })?
        .to_string_lossy()
        .to_string();
    Ok(rel)
}

#[tauri::command]
pub async fn git_log(
    path: String,
    limit: Option<u32>,
    branch: Option<String>,
) -> Result<Vec<GitLogEntry>, AppCommandError> {
    const COMMIT_META_PREFIX: &str = "__COMMIT__\0";
    const MESSAGE_END_MARKER: &str = "__COMMIT_MESSAGE_END__";

    let limit_str = format!("-{}", limit.unwrap_or(100));
    let mut args = vec![
        "log".to_string(),
        limit_str,
        format!(
            "--format=__COMMIT__%x00%h%x00%H%x00%an%x00%aI%n%B%n{MESSAGE_END_MARKER}"
        ),
        "--raw".to_string(),
        "--numstat".to_string(),
        "--no-renames".to_string(),
    ];
    if let Some(ref b) = branch {
        args.push(b.clone());
    }
    let output = crate::process::tokio_command("git")
        .args(&args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        // Empty repo (no commits yet) — return empty list instead of error
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        if stderr_str.contains("does not have any commits yet")
            || stderr_str.contains("unknown revision or path not in the working tree")
        {
            return Ok(Vec::new());
        }
        return Err(git_command_error("log", &output.stderr));
    }

    let mut entries = Vec::<GitLogEntry>::new();
    let mut current: Option<GitLogEntryBuilder> = None;
    let mut reading_message = false;

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(meta) = line.strip_prefix(COMMIT_META_PREFIX) {
            if let Some(entry) = current.take() {
                entries.push(entry.finish());
            }

            let parts: Vec<&str> = meta.splitn(4, '\0').collect();
            if parts.len() == 4 {
                current = Some(GitLogEntryBuilder::new(parts));
                reading_message = true;
            } else {
                reading_message = false;
            }
            continue;
        }

        let Some(entry) = current.as_mut() else {
            continue;
        };

        if reading_message {
            if line == MESSAGE_END_MARKER {
                reading_message = false;
                entry.finalize_message();
            } else {
                entry.push_message_line(line);
            }
            continue;
        }

        if line.is_empty() {
            continue;
        }

        if line.starts_with(':') {
            if let Some((status, file_path)) = parse_raw_file_line(line) {
                let file = entry.get_or_insert_file(file_path);
                file.status = status;
            }
            continue;
        }

        if let Some((additions, deletions, file_path)) = parse_numstat_file_line(line) {
            let file = entry.get_or_insert_file(file_path);
            file.additions = additions;
            file.deletions = deletions;
        }
    }

    if let Some(entry) = current {
        entries.push(entry.finish());
    }

    let unpushed_hashes = get_unpushed_hashes(&path).await.ok().flatten();
    for entry in entries.iter_mut() {
        entry.pushed = unpushed_hashes
            .as_ref()
            .map(|hashes| !hashes.contains(&entry.full_hash));
    }

    Ok(entries)
}

#[tauri::command]
pub async fn git_commit_branches(
    path: String,
    commit: String,
) -> Result<Vec<String>, AppCommandError> {
    let contains_arg = format!("--contains={commit}");
    let output = crate::process::tokio_command("git")
        .args([
            "for-each-ref",
            &contains_arg,
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("for-each-ref", &output.stderr));
    }

    let mut seen = HashSet::new();
    let mut branches = Vec::new();

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let branch = line.trim();
        if branch.is_empty() || branch.ends_with("/HEAD") {
            continue;
        }

        if seen.insert(branch.to_string()) {
            branches.push(branch.to_string());
        }
    }

    branches.sort();
    Ok(branches)
}

struct GitLogEntryBuilder {
    hash: String,
    full_hash: String,
    author: String,
    date: String,
    message: String,
    files: Vec<GitLogFileChange>,
    index_by_path: HashMap<String, usize>,
}

impl GitLogEntryBuilder {
    fn new(parts: Vec<&str>) -> Self {
        Self {
            hash: parts[0].to_string(),
            full_hash: parts[1].to_string(),
            author: parts[2].to_string(),
            date: parts[3].to_string(),
            message: String::new(),
            files: Vec::new(),
            index_by_path: HashMap::new(),
        }
    }

    fn push_message_line(&mut self, line: &str) {
        if !self.message.is_empty() {
            self.message.push('\n');
        }
        self.message.push_str(line);
    }

    fn finalize_message(&mut self) {
        self.message = self.message.trim_end_matches('\n').to_string();
    }

    fn get_or_insert_file(&mut self, path: String) -> &mut GitLogFileChange {
        let index = if let Some(index) = self.index_by_path.get(&path) {
            *index
        } else {
            self.files.push(GitLogFileChange {
                path: path.clone(),
                status: "M".to_string(),
                additions: 0,
                deletions: 0,
            });
            let index = self.files.len() - 1;
            self.index_by_path.insert(path, index);
            index
        };

        &mut self.files[index]
    }

    fn finish(self) -> GitLogEntry {
        GitLogEntry {
            hash: self.hash,
            full_hash: self.full_hash,
            author: self.author,
            date: self.date,
            message: self.message,
            files: self.files,
            pushed: None,
        }
    }
}

fn parse_raw_file_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.split('\t');
    let meta = parts.next()?;
    let file_path = parts.next()?.to_string();
    let status = meta
        .split_whitespace()
        .last()
        .and_then(|v| v.chars().next())
        .unwrap_or('M')
        .to_string();
    Some((status, file_path))
}

fn parse_numstat_file_line(line: &str) -> Option<(u32, u32, String)> {
    let mut parts = line.splitn(3, '\t');
    let additions = parse_numstat_count(parts.next()?);
    let deletions = parse_numstat_count(parts.next()?);
    let file_path = parts.next()?.to_string();
    Some((additions, deletions, file_path))
}

fn parse_numstat_count(value: &str) -> u32 {
    if value == "-" {
        return 0;
    }

    value.parse::<u32>().unwrap_or(0)
}

async fn get_unpushed_hashes(path: &str) -> Result<Option<HashSet<String>>, AppCommandError> {
    let upstream_output = crate::process::tokio_command("git")
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !upstream_output.status.success() {
        return Ok(None);
    }

    let upstream = String::from_utf8_lossy(&upstream_output.stdout)
        .trim()
        .to_string();
    if upstream.is_empty() {
        return Ok(None);
    }

    let range = format!("{upstream}..HEAD");
    let rev_list_output = crate::process::tokio_command("git")
        .args(["rev-list", &range])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !rev_list_output.status.success() {
        return Ok(None);
    }

    let hashes = String::from_utf8_lossy(&rev_list_output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect::<HashSet<_>>();

    Ok(Some(hashes))
}
