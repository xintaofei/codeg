use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use base64::Engine as _;
use ignore::WalkBuilder;
use serde::Serialize;

use tokio::sync::Semaphore;
use walkdir::WalkDir;

#[cfg(feature = "tauri-runtime")]
use tauri::Manager;

use crate::app_error::AppCommandError;
use crate::db::error::DbError;
use crate::db::service::folder_service;
use crate::db::AppDatabase;
use crate::models::GitCredentials;
use crate::models::{FolderDetail, FolderHistoryEntry};
use crate::web::event_bridge::EventEmitter;

/// Configure a git command for remote operations:
/// - Always disable interactive prompts (prevent hanging in a GUI app)
/// - If explicit credentials are provided, use them directly
/// - Otherwise, try to inject stored account credentials
async fn prepare_remote_git_cmd(
    cmd: &mut tokio::process::Command,
    repo_path: &str,
    credentials: Option<&GitCredentials>,
    db: &AppDatabase,
    data_dir: &std::path::Path,
) {
    prepare_remote_git_cmd_with_remote(cmd, repo_path, None, credentials, db, data_dir).await;
}

/// Same as `prepare_remote_git_cmd` but allows specifying a remote name
/// to match credentials against the correct remote URL.
async fn prepare_remote_git_cmd_with_remote(
    cmd: &mut tokio::process::Command,
    repo_path: &str,
    remote_name: Option<&str>,
    credentials: Option<&GitCredentials>,
    db: &AppDatabase,
    data_dir: &std::path::Path,
) {
    cmd.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());

    if let Some(creds) = credentials {
        // Explicit credentials provided (e.g. from credential dialog)
        if let Ok(askpass) = crate::git_credential::ensure_askpass_script(data_dir) {
            crate::git_credential::inject_credentials(
                cmd,
                &creds.username,
                &creds.password,
                &askpass,
            );
        }
    } else {
        // Fall back to stored accounts, matching against the specified remote
        crate::git_credential::try_inject_for_repo_remote(
            cmd,
            repo_path,
            remote_name,
            &db.conn,
            data_dir,
        )
        .await;
    }
}

/// Same as `prepare_remote_git_cmd` but for clone (URL only, no repo yet).
async fn prepare_remote_git_cmd_for_url(
    cmd: &mut tokio::process::Command,
    clone_url: &str,
    credentials: Option<&GitCredentials>,
    db: &AppDatabase,
    data_dir: &std::path::Path,
) {
    cmd.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());

    if let Some(creds) = credentials {
        if let Ok(askpass) = crate::git_credential::ensure_askpass_script(data_dir) {
            crate::git_credential::inject_credentials(
                cmd,
                &creds.username,
                &creds.password,
                &askpass,
            );
        }
    } else {
        crate::git_credential::try_inject_for_url(cmd, clone_url, &db.conn, data_dir).await;
    }
}

/// Classify a git remote command error, detecting authentication failures.
fn classify_remote_git_error(operation: &str, stderr: &[u8]) -> AppCommandError {
    let msg = String::from_utf8_lossy(stderr).trim().to_string();
    tracing::error!("[GIT_CMD] {} failed, stderr: {}", operation, msg);
    let lower = msg.to_lowercase();

    if lower.contains("authentication failed")
        || lower.contains("invalid credentials")
        || lower.contains("could not read username")
        || lower.contains("could not read password")
        || lower.contains("logon failed")
        || lower.contains("terminal prompts disabled")
        || lower.contains("the requested url returned error: 401")
        || lower.contains("the requested url returned error: 403")
        || lower.contains("http basic: access denied")
    {
        return AppCommandError::authentication_failed(format!(
            "git {operation}: authentication failed. Configure a GitHub account in Settings → Version Control."
        ))
        .with_detail(msg);
    }

    if lower.contains("could not resolve host")
        || lower.contains("unable to access")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
    {
        return AppCommandError::network(format!("git {operation}: network error"))
            .with_detail(msg);
    }

    AppCommandError::external_command(format!("git {operation} failed"), msg)
}

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

/// Where a given branch is checked out, resolved against the registered folders.
/// `path` is the canonical filesystem path of the worktree (or main working
/// tree) hosting the branch — `None` when the branch is not checked out in any
/// worktree. `folder_id` is the registered folder whose canonicalized path
/// matches `path` — `None` for an external/unregistered worktree. Drives the
/// branch selector's "navigate vs checkout" decision.
#[derive(Debug, Serialize)]
pub struct WorktreeResolution {
    pub path: Option<String>,
    pub folder_id: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct GitConflictInfo {
    pub has_conflicts: bool,
    pub conflicted_files: Vec<String>,
    pub operation: String,
    pub upstream_commit: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GitPullResult {
    pub updated_files: usize,
    pub conflict: Option<GitConflictInfo>,
}

#[derive(Debug, Serialize)]
pub struct GitPushResult {
    pub pushed_commits: usize,
    pub upstream_set: bool,
}

#[derive(Debug, Serialize)]
pub struct GitPushInfo {
    pub branch: String,
    pub remotes: Vec<GitRemote>,
    pub tracking_remote: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GitMergeResult {
    pub merged_commits: usize,
    pub conflict: Option<GitConflictInfo>,
}

#[derive(Debug, Serialize)]
pub struct GitRebaseResult {
    pub message: String,
    pub conflict: Option<GitConflictInfo>,
}

#[derive(Debug, Serialize)]
pub struct GitConflictFileVersions {
    pub base: String,
    pub ours: String,
    pub theirs: String,
    pub merged: String,
}

#[derive(Debug, Serialize)]
pub struct GitCommitResult {
    pub committed_files: usize,
}

#[derive(Debug, Serialize)]
pub struct GitStashEntry {
    pub index: usize,
    pub message: String,
    pub branch: String,
    pub date: String,
    pub ref_name: String,
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

#[derive(Debug, Clone, Serialize)]
struct GitPushSucceededEvent {
    folder_id: i32,
    pushed_commits: usize,
    upstream_set: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceEntryKind {
    File,
    Dir,
}

/// A flat workspace entry produced by `list_workspace_files`. Unlike the nested
/// `FileTreeNode`, this is a single flat record suited to fuzzy file search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceFileEntry {
    pub name: String,
    /// Path relative to the workspace root, always forward-slashed.
    pub path: String,
    pub kind: WorkspaceEntryKind,
}

#[derive(Debug, Serialize)]
pub struct FilePreviewContent {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct FileEditContent {
    pub path: String,
    pub content: String,
    pub etag: String,
    pub mtime_ms: Option<i64>,
    pub readonly: bool,
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

#[derive(Debug, Serialize)]
pub struct GitLogResult {
    pub entries: Vec<GitLogEntry>,
    pub has_upstream: bool,
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

use crate::git_repo::ensure_git_repo;

async fn detect_conflicts(path: &str) -> Result<Vec<String>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["-c", "core.quotePath=false"])
        .args(["diff", "--name-only", "--diff-filter=U"])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(unquote_git_path)
        .filter(|l| !l.is_empty())
        .collect())
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

// ---------------------------------------------------------------------------
// Shared core functions (used by both Tauri commands and web handlers)
// ---------------------------------------------------------------------------

pub async fn get_folder_core(db: &AppDatabase, folder_id: i32) -> Result<FolderDetail, DbError> {
    folder_service::get_folder_by_id(&db.conn, folder_id)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Folder {} not found", folder_id)))
}

/// Emit a `folder://changed` Upsert so every client inserts-or-replaces the
/// folder in its workspace list in real time. Used by headless producers — the
/// automation engine minting a per-run worktree — whose folders no client would
/// otherwise learn about until the next full `fetchFolders` (leaving any
/// conversation produced inside them unplaceable in the sidebar). Best-effort:
/// the folder is already persisted, so a dropped event reconciles on the next
/// refresh / WS reconnect.
///
/// Unlike [`open_folder_in_workspace_core`], this ONLY syncs the folder list — it
/// never opens or focuses a tab — so a background emitter can't steal focus.
pub(crate) fn emit_folder_upsert(emitter: &EventEmitter, detail: FolderDetail) {
    crate::web::event_bridge::emit_event(
        emitter,
        crate::web::event_bridge::FOLDER_CHANGED_EVENT,
        crate::web::event_bridge::FolderChange::Upsert {
            folder: Box::new(detail),
        },
    );
}

pub async fn load_folder_history_core(
    db: &AppDatabase,
) -> Result<Vec<FolderHistoryEntry>, AppCommandError> {
    folder_service::list_folders(&db.conn)
        .await
        .map_err(AppCommandError::from)
}

pub async fn add_folder_to_history_core(
    db: &AppDatabase,
    path: String,
) -> Result<FolderHistoryEntry, DbError> {
    folder_service::add_folder(&db.conn, &path).await
}

pub async fn remove_folder_from_history_core(
    db: &AppDatabase,
    path: String,
) -> Result<(), AppCommandError> {
    folder_service::remove_folder(&db.conn, &path)
        .await
        .map_err(AppCommandError::from)
}

pub async fn list_open_folders_core(
    db: &AppDatabase,
) -> Result<Vec<FolderHistoryEntry>, AppCommandError> {
    folder_service::list_open_folders(&db.conn)
        .await
        .map_err(AppCommandError::from)
}

pub async fn list_open_folder_details_core(
    db: &AppDatabase,
) -> Result<Vec<FolderDetail>, AppCommandError> {
    folder_service::list_open_folder_details(&db.conn)
        .await
        .map_err(AppCommandError::from)
}

pub async fn list_all_folder_details_core(
    db: &AppDatabase,
) -> Result<Vec<FolderDetail>, AppCommandError> {
    folder_service::list_all_folder_details(&db.conn)
        .await
        .map_err(AppCommandError::from)
}

pub async fn open_folder_core(
    db: &AppDatabase,
    path: String,
) -> Result<FolderDetail, AppCommandError> {
    let entry = folder_service::add_folder(&db.conn, &path)
        .await
        .map_err(AppCommandError::from)?;
    folder_service::get_folder_by_id(&db.conn, entry.id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found after add"))
}

/// Open a freshly created git worktree directory as a folder, recording the
/// *root* folder it descends from. Parents are flattened: a worktree created
/// from another worktree still records the original root, so every worktree of a
/// repo groups under that one repo folder. An unknown / non-positive
/// `source_folder_id` degrades to a top-level folder (`parent_id = None`) rather
/// than erroring.
pub async fn open_worktree_folder_core(
    db: &AppDatabase,
    path: String,
    source_folder_id: i32,
) -> Result<FolderDetail, AppCommandError> {
    let parent_id = if source_folder_id > 0 {
        folder_service::get_folder_by_id(&db.conn, source_folder_id)
            .await
            .map_err(AppCommandError::from)?
            .map(|src| src.parent_id.unwrap_or(src.id))
    } else {
        None
    };
    let entry = folder_service::add_folder_with_parent(&db.conn, &path, parent_id)
        .await
        .map_err(AppCommandError::from)?;
    folder_service::get_folder_by_id(&db.conn, entry.id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found after add"))
}

/// Open a folder into the workspace and announce it so the workspace window
/// can surface it. Used by the project launcher, which lives in its own
/// window/tab and can't reach the workspace's React state directly. Emitting
/// through the shared `EventEmitter` routes the signal correctly in every
/// runtime — Tauri events (desktop), the WebSocket broadcaster (server), and
/// the remote server's broadcaster (remote desktop) — so only windows talking
/// to this same backend react.
pub async fn open_folder_in_workspace_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    path: String,
) -> Result<FolderDetail, AppCommandError> {
    let detail = open_folder_core(db, path).await?;
    crate::web::event_bridge::emit_event(emitter, "folder://open-in-workspace", &detail);
    Ok(detail)
}

pub async fn open_folder_by_id_core(
    db: &AppDatabase,
    folder_id: i32,
) -> Result<FolderDetail, AppCommandError> {
    folder_service::set_folder_open(&db.conn, folder_id, true)
        .await
        .map_err(AppCommandError::from)?;
    folder_service::get_folder_by_id(&db.conn, folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found(format!("Folder {folder_id} not found")))
}

pub async fn remove_folder_from_workspace_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    folder_id: i32,
) -> Result<(), AppCommandError> {
    use crate::db::service::tab_service;

    // Capture the folder path before flipping it closed, so we can stop any
    // office watch preview servers rooted under it — belt-and-suspenders over
    // the frontend's per-tab unmount teardown.
    let folder_path = folder_service::get_folder_by_id(&db.conn, folder_id)
        .await
        .ok()
        .flatten()
        .map(|f| f.path);

    folder_service::set_folder_open(&db.conn, folder_id, false)
        .await
        .map_err(AppCommandError::from)?;

    // Atomically drop this folder's open tabs + bump the version (always, as a
    // barrier so a concurrent stale save can't resurrect them) + snapshot, in one
    // transaction. Broadcast the new set only when a persisted tab actually
    // changed (sentinel origin "server" so every client applies it); a zero-row
    // removal just advances the barrier — an in-flight saver reconciles via its
    // rejected CAS.
    let inv = tab_service::delete_folder_tabs_and_bump(&db.conn, folder_id)
        .await
        .map_err(AppCommandError::from)?;
    if let Some(tabs) = inv.emit {
        crate::web::event_bridge::emit_event(
            emitter,
            crate::web::event_bridge::TABS_CHANGED_EVENT,
            crate::web::event_bridge::TabsChanged {
                version: inv.version,
                origin: "server".to_string(),
                tabs,
            },
        );
    }

    if let Some(path) = folder_path {
        crate::office_watch::stop_office_watches_under_root(&path);
    }
    Ok(())
}

pub async fn reorder_folders_core(db: &AppDatabase, ids: Vec<i32>) -> Result<(), AppCommandError> {
    folder_service::reorder_folders(&db.conn, ids)
        .await
        .map_err(AppCommandError::from)
}

pub async fn update_folder_color_core(
    db: &AppDatabase,
    folder_id: i32,
    color: String,
) -> Result<FolderDetail, AppCommandError> {
    folder_service::update_folder_color(&db.conn, folder_id, &color)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found"))
}

pub async fn update_folder_alias_core(
    db: &AppDatabase,
    folder_id: i32,
    alias: Option<String>,
) -> Result<FolderDetail, AppCommandError> {
    // Empty / whitespace-only input clears the alias (stored as NULL).
    let normalized = alias
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    folder_service::update_folder_alias(&db.conn, folder_id, normalized)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found"))
}

pub async fn update_folder_default_agent_core(
    db: &AppDatabase,
    folder_id: i32,
    default_agent_type: Option<crate::models::agent::AgentType>,
) -> Result<FolderDetail, AppCommandError> {
    folder_service::update_folder_default_agent(&db.conn, folder_id, default_agent_type)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found"))
}

// ---------------------------------------------------------------------------
// Tauri command wrappers (thin shims over _core)
// ---------------------------------------------------------------------------

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_folder(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<FolderDetail, DbError> {
    get_folder_core(&db, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn load_folder_history(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<FolderHistoryEntry>, AppCommandError> {
    load_folder_history_core(&db).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn add_folder_to_history(
    db: tauri::State<'_, AppDatabase>,
    path: String,
) -> Result<FolderHistoryEntry, DbError> {
    add_folder_to_history_core(&db, path).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn remove_folder_from_history(
    db: tauri::State<'_, AppDatabase>,
    path: String,
) -> Result<(), AppCommandError> {
    remove_folder_from_history_core(&db, path).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_open_folder_details(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<FolderDetail>, AppCommandError> {
    list_open_folder_details_core(&db).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_all_folder_details(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<FolderDetail>, AppCommandError> {
    list_all_folder_details_core(&db).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_folder(
    db: tauri::State<'_, AppDatabase>,
    path: String,
) -> Result<FolderDetail, AppCommandError> {
    open_folder_core(&db, path).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_worktree_folder(
    db: tauri::State<'_, AppDatabase>,
    path: String,
    source_folder_id: i32,
) -> Result<FolderDetail, AppCommandError> {
    open_worktree_folder_core(&db, path, source_folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_folder_in_workspace(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    path: String,
) -> Result<FolderDetail, AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    open_folder_in_workspace_core(&emitter, &db, path).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_folder_by_id(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<FolderDetail, AppCommandError> {
    open_folder_by_id_core(&db, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn remove_folder_from_workspace(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<(), AppCommandError> {
    remove_folder_from_workspace_core(&EventEmitter::Tauri(app), &db, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn reorder_folders(
    db: tauri::State<'_, AppDatabase>,
    ids: Vec<i32>,
) -> Result<(), AppCommandError> {
    reorder_folders_core(&db, ids).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_folder_color(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    color: String,
) -> Result<FolderDetail, AppCommandError> {
    update_folder_color_core(&db, folder_id, color).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_folder_alias(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    alias: Option<String>,
) -> Result<FolderDetail, AppCommandError> {
    update_folder_alias_core(&db, folder_id, alias).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_folder_default_agent(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    default_agent_type: Option<crate::models::agent::AgentType>,
) -> Result<FolderDetail, AppCommandError> {
    update_folder_default_agent_core(&db, folder_id, default_agent_type).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_folder_directory(path: String) -> Result<(), AppCommandError> {
    std::fs::create_dir_all(&path).map_err(AppCommandError::io)
}

pub(crate) async fn clone_repository_core(
    url: &str,
    target_dir: &str,
    credentials: Option<&GitCredentials>,
    db: &AppDatabase,
    data_dir: &std::path::Path,
) -> Result<(), AppCommandError> {
    if url.trim().is_empty() || target_dir.trim().is_empty() {
        return Err(AppCommandError::invalid_input(
            "Repository URL and target directory are required",
        ));
    }

    let mut cmd = crate::process::tokio_command("git");
    cmd.args(["clone", url, target_dir]);
    prepare_remote_git_cmd_for_url(&mut cmd, url, credentials, db, data_dir).await;

    let output = cmd.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppCommandError::dependency_missing("Git is not installed. Please install Git first.")
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

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn clone_repository(
    url: String,
    target_dir: String,
    credentials: Option<GitCredentials>,
    db: tauri::State<'_, AppDatabase>,
    app_handle: tauri::AppHandle,
) -> Result<(), AppCommandError> {
    let data_dir = app_handle.path().app_data_dir().map_err(|e| {
        AppCommandError::external_command("Failed to resolve app data dir", e.to_string())
    })?;
    // Resolve through the effective data dir so a custom
    // `CODEG_DATA_DIR` reaches the git credential helper invoked by
    // this subprocess.
    let data_dir = crate::paths::resolve_effective_data_dir(&data_dir);
    clone_repository_core(&url, &target_dir, credentials.as_ref(), &db, &data_dir).await
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
        || normalized.contains("could not read password")
        || normalized.contains("logon failed")
        || normalized.contains("terminal prompts disabled")
        || normalized.contains("the requested url returned error: 401")
        || normalized.contains("the requested url returned error: 403")
        || normalized.contains("http basic: access denied")
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

/// The state of a working tree's `HEAD`. The legacy `get_git_branch` contract
/// collapsed every non-branch state into `None`, so a detached HEAD looked
/// identical to a non-git folder and the UI hid all git operations (issue
/// #279). This distinguishes the four states the UI actually needs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitHeadInfo {
    /// Whether git recognizes this path as a work tree.
    pub is_repo: bool,
    /// Branch name when `HEAD` points at a branch — including an unborn branch
    /// right after `git init`. `None` when detached or not a repo.
    pub branch: Option<String>,
    /// Whether `HEAD` is detached (points directly at a commit, not a branch).
    pub detached: bool,
    /// Short commit hash; present when detached and at least one commit exists.
    pub short_sha: Option<String>,
}

async fn git_output(
    path: &str,
    args: &[&str],
) -> Result<std::process::Output, AppCommandError> {
    crate::process::tokio_command("git")
        .args(args)
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)
}

/// Resolve the current `HEAD` state. The common cases (on a branch / detached)
/// cost a single git invocation; the rarer unborn-branch and non-repository
/// cases fall through to `symbolic-ref` and `--is-inside-work-tree`.
async fn resolve_git_head(path: &str) -> Result<GitHeadInfo, AppCommandError> {
    // `rev-parse --abbrev-ref HEAD` prints the branch name, the literal "HEAD"
    // when detached, and fails on an unborn branch or a non-repository.
    let head = git_output(path, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    if head.status.success() {
        let name = String::from_utf8_lossy(&head.stdout).trim().to_string();
        if !name.is_empty() && name != "HEAD" {
            return Ok(GitHeadInfo {
                is_repo: true,
                branch: Some(name),
                detached: false,
                short_sha: None,
            });
        }
        // Detached HEAD — surface the short commit so the UI can label it.
        let short_sha = git_output(path, &["rev-parse", "--short", "HEAD"])
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        return Ok(GitHeadInfo {
            is_repo: true,
            branch: None,
            detached: true,
            short_sha,
        });
    }

    // Unborn branch (after `git init`, before the first commit): `symbolic-ref`
    // still resolves the branch name where `rev-parse` cannot.
    let sym = git_output(path, &["symbolic-ref", "--short", "HEAD"]).await?;
    if sym.status.success() {
        let name = String::from_utf8_lossy(&sym.stdout).trim().to_string();
        if !name.is_empty() {
            return Ok(GitHeadInfo {
                is_repo: true,
                branch: Some(name),
                detached: false,
                short_sha: None,
            });
        }
    }

    // Neither resolved a branch — authoritatively decide repo-ness. An unusual
    // work tree with no resolvable branch still counts as a repo so the UI
    // offers git operations rather than "initialize".
    let inside = git_output(path, &["rev-parse", "--is-inside-work-tree"]).await?;
    let is_repo =
        inside.status.success() && String::from_utf8_lossy(&inside.stdout).trim() == "true";
    Ok(GitHeadInfo {
        is_repo,
        branch: None,
        detached: false,
        short_sha: None,
    })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_git_head(path: String) -> Result<GitHeadInfo, AppCommandError> {
    resolve_git_head(&path).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_git_branch(path: String) -> Result<Option<String>, AppCommandError> {
    Ok(resolve_git_head(&path).await?.branch)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

pub(crate) async fn git_pull_core(
    path: &str,
    credentials: Option<&GitCredentials>,
    db: &AppDatabase,
    data_dir: &std::path::Path,
) -> Result<GitPullResult, AppCommandError> {
    let head_before = get_head_hash(path).await?;

    // Step 1: fetch from remote
    let mut fetch_cmd = crate::process::tokio_command("git");
    fetch_cmd.args(["fetch"]).current_dir(path);
    prepare_remote_git_cmd(&mut fetch_cmd, path, credentials, db, data_dir).await;

    let fetch_output = fetch_cmd.output().await.map_err(AppCommandError::io)?;

    if !fetch_output.status.success() {
        return Err(classify_remote_git_error("fetch", &fetch_output.stderr));
    }

    // Step 2: check if upstream exists
    let upstream_check = crate::process::tokio_command("git")
        .args(["rev-parse", "@{u}"])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !upstream_check.status.success() {
        return Ok(GitPullResult {
            updated_files: 0,
            conflict: None,
        });
    }
    let upstream_commit = String::from_utf8_lossy(&upstream_check.stdout)
        .trim()
        .to_string();

    // Step 3: check if we can fast-forward
    let merge_base = crate::process::tokio_command("git")
        .args(["merge-base", "HEAD", "@{u}"])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;
    let head_hash = crate::process::tokio_command("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    let base_hash = String::from_utf8_lossy(&merge_base.stdout)
        .trim()
        .to_string();
    let current_head = String::from_utf8_lossy(&head_hash.stdout)
        .trim()
        .to_string();

    if base_hash == current_head {
        let ff_output = crate::process::tokio_command("git")
            .args(["merge", "--ff-only", "@{u}"])
            .current_dir(path)
            .output()
            .await
            .map_err(AppCommandError::io)?;

        if !ff_output.status.success() {
            return Err(git_command_error("merge --ff-only", &ff_output.stderr));
        }
    } else {
        let merge_output = crate::process::tokio_command("git")
            .args(["merge", "--no-commit", "@{u}"])
            .current_dir(path)
            .output()
            .await
            .map_err(AppCommandError::io)?;

        if !merge_output.status.success() {
            let conflicted_files = detect_conflicts(path).await?;
            if !conflicted_files.is_empty() {
                let _ = crate::process::tokio_command("git")
                    .args(["merge", "--abort"])
                    .current_dir(path)
                    .output()
                    .await;

                return Ok(GitPullResult {
                    updated_files: 0,
                    conflict: Some(GitConflictInfo {
                        has_conflicts: true,
                        conflicted_files,
                        operation: "pull".to_string(),
                        upstream_commit: Some(upstream_commit),
                    }),
                });
            }
            return Err(git_command_error("merge", &merge_output.stderr));
        }

        let commit_output = crate::process::tokio_command("git")
            .args(["commit", "--no-edit"])
            .current_dir(path)
            .output()
            .await
            .map_err(AppCommandError::io)?;

        if !commit_output.status.success() {
            let stderr = String::from_utf8_lossy(&commit_output.stderr);
            let stdout = String::from_utf8_lossy(&commit_output.stdout);
            if !stderr.contains("nothing to commit") && !stdout.contains("nothing to commit") {
                return Err(git_command_error("commit", &commit_output.stderr));
            }
        }
    }

    let head_after = get_head_hash(path).await?;
    let updated_files = match (head_before.as_deref(), head_after.as_deref()) {
        (Some(before), Some(after)) if before != after => {
            count_changed_files_between(path, before, after).await?
        }
        (None, Some(after)) => count_files_in_commit(path, after).await?,
        _ => 0,
    };

    Ok(GitPullResult {
        updated_files,
        conflict: None,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_pull(
    path: String,
    credentials: Option<GitCredentials>,
    db: tauri::State<'_, AppDatabase>,
    app_handle: tauri::AppHandle,
) -> Result<GitPullResult, AppCommandError> {
    let data_dir = app_handle.path().app_data_dir().map_err(|e| {
        AppCommandError::external_command("Failed to resolve app data dir", e.to_string())
    })?;
    // Resolve through the effective data dir so a custom
    // `CODEG_DATA_DIR` reaches the git credential helper invoked by
    // this subprocess.
    let data_dir = crate::paths::resolve_effective_data_dir(&data_dir);
    git_pull_core(&path, credentials.as_ref(), &db, &data_dir).await
}

/// Start a merge with the upstream branch (used by merge workspace after pull conflict detection).
/// This recreates the conflict state so that :1:, :2:, :3: stage entries are available.
/// If `upstream_commit` is provided, merge against that specific commit instead of `@{u}`.
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_start_pull_merge(
    path: String,
    upstream_commit: Option<String>,
) -> Result<(), AppCommandError> {
    let target = upstream_commit.as_deref().unwrap_or("@{u}");
    let output = crate::process::tokio_command("git")
        .args(["merge", "--no-commit", target])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    // It's expected to fail with conflicts — that's the point.
    // We just need the merge state to be active so stage entries exist.
    if !output.status.success() {
        let conflicted_files = detect_conflicts(&path).await?;
        if !conflicted_files.is_empty() {
            return Ok(()); // Conflict state is now active — merge workspace can proceed
        }
        return Err(git_command_error("merge", &output.stderr));
    }

    Ok(())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_has_merge_head(path: String) -> Result<bool, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["rev-parse", "--verify", "MERGE_HEAD"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;
    Ok(output.status.success())
}

pub(crate) async fn git_fetch_core(
    path: &str,
    credentials: Option<&GitCredentials>,
    db: &AppDatabase,
    data_dir: &std::path::Path,
) -> Result<String, AppCommandError> {
    let mut cmd = crate::process::tokio_command("git");
    cmd.args(["fetch", "--all"]).current_dir(path);
    prepare_remote_git_cmd(&mut cmd, path, credentials, db, data_dir).await;

    let output = cmd.output().await.map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(classify_remote_git_error("fetch --all", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_fetch(
    path: String,
    credentials: Option<GitCredentials>,
    db: tauri::State<'_, AppDatabase>,
    app_handle: tauri::AppHandle,
) -> Result<String, AppCommandError> {
    let data_dir = app_handle.path().app_data_dir().map_err(|e| {
        AppCommandError::external_command("Failed to resolve app data dir", e.to_string())
    })?;
    // Resolve through the effective data dir so a custom
    // `CODEG_DATA_DIR` reaches the git credential helper invoked by
    // this subprocess.
    let data_dir = crate::paths::resolve_effective_data_dir(&data_dir);
    git_fetch_core(&path, credentials.as_ref(), &db, &data_dir).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_push_info(path: String) -> Result<GitPushInfo, AppCommandError> {
    ensure_git_repo(&path)?;

    // Get current branch name
    let branch_output = crate::process::tokio_command("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;
    let branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    // Get tracking remote for current branch
    let remote_key = format!("branch.{}.remote", branch);
    let remote_output = crate::process::tokio_command("git")
        .args(["config", "--get", &remote_key])
        .current_dir(&path)
        .output()
        .await;
    let tracking_remote = remote_output
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|v| !v.is_empty());

    // Get all remotes
    let remotes = git_list_remotes(path).await?;

    Ok(GitPushInfo {
        branch,
        remotes,
        tracking_remote,
    })
}

pub(crate) async fn git_push_core(
    data_dir: &std::path::Path,
    emitter: &EventEmitter,
    folder_id: Option<i32>,
    path: &str,
    remote: Option<&str>,
    credentials: Option<&GitCredentials>,
    db: &AppDatabase,
) -> Result<GitPushResult, AppCommandError> {
    let pushed_commits = estimate_push_commit_count(path).await;

    let target_remote = remote.filter(|s| !s.is_empty()).unwrap_or("origin");

    let branch_output = crate::process::tokio_command("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;
    let branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    let upstream_check = crate::process::tokio_command("git")
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    let current_upstream = if upstream_check.status.success() {
        Some(
            String::from_utf8_lossy(&upstream_check.stdout)
                .trim()
                .to_string(),
        )
    } else {
        None
    };

    let needs_set_upstream = match &current_upstream {
        None => true,
        Some(upstream) => !upstream.starts_with(&format!("{}/", target_remote)),
    };

    let output = if needs_set_upstream {
        let mut cmd = crate::process::tokio_command("git");
        cmd.args(["push", "--set-upstream", target_remote, &branch])
            .current_dir(path);
        prepare_remote_git_cmd_with_remote(
            &mut cmd,
            path,
            Some(target_remote),
            credentials,
            db,
            data_dir,
        )
        .await;
        cmd.output().await.map_err(AppCommandError::io)?
    } else {
        let mut cmd = crate::process::tokio_command("git");
        cmd.args(["push", target_remote, &branch]).current_dir(path);
        prepare_remote_git_cmd_with_remote(
            &mut cmd,
            path,
            Some(target_remote),
            credentials,
            db,
            data_dir,
        )
        .await;
        cmd.output().await.map_err(AppCommandError::io)?
    };

    if !output.status.success() {
        return Err(classify_remote_git_error("push", &output.stderr));
    }

    let upstream_set = needs_set_upstream;

    if let Some(folder_id) = folder_id {
        crate::web::event_bridge::emit_event(
            emitter,
            "folder://git-push-succeeded",
            GitPushSucceededEvent {
                folder_id,
                pushed_commits,
                upstream_set,
            },
        );
    }

    Ok(GitPushResult {
        pushed_commits,
        upstream_set,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_push(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    path: String,
    remote: Option<String>,
    credentials: Option<GitCredentials>,
    folder_id: Option<i32>,
    db: tauri::State<'_, AppDatabase>,
) -> Result<GitPushResult, AppCommandError> {
    let folder_id = folder_id.or_else(|| {
        window
            .label()
            .strip_prefix("push-")
            .and_then(|value| value.parse::<i32>().ok())
    });
    let data_dir = app.path().app_data_dir().map_err(|e| {
        AppCommandError::external_command("Failed to resolve app data dir", e.to_string())
    })?;
    // Resolve through the effective data dir so a custom
    // `CODEG_DATA_DIR` reaches the git credential helper invoked by
    // this subprocess.
    let data_dir = crate::paths::resolve_effective_data_dir(&data_dir);
    let emitter = EventEmitter::Tauri(app.clone());
    git_push_core(
        &data_dir,
        &emitter,
        folder_id,
        &path,
        remote.as_deref(),
        credentials.as_ref(),
        &db,
    )
    .await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

/// Whether the working tree at `path` has no uncommitted changes to *tracked*
/// files (untracked files are ignored — they survive a branch switch). Used to
/// refuse a `shared_in_root` automation before it `git checkout`s the user's
/// root tree, which would otherwise carry their in-progress edits onto the
/// target branch (or fail outright).
pub async fn git_is_clean(path: String) -> Result<bool, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("status", &output.stderr));
    }
    // Porcelain prints one line per changed tracked path; clean ⇒ no output.
    Ok(output.stdout.iter().all(|b| b.is_ascii_whitespace()))
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_reset(path: String, commit: String, mode: String) -> Result<(), AppCommandError> {
    let mode = mode.trim().to_lowercase();
    let mode_flag = match mode.as_str() {
        "soft" | "mixed" | "hard" | "keep" => format!("--{mode}"),
        _ => {
            return Err(AppCommandError::invalid_input(
                "Reset mode must be one of: soft, mixed, hard, keep",
            ))
        }
    };

    let output = crate::process::tokio_command("git")
        .args(["reset", mode_flag.as_str(), commit.as_str()])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("reset", &output.stderr));
    }

    Ok(())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_list_branches(path: String) -> Result<Vec<String>, AppCommandError> {
    ensure_git_repo(&path)?;

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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_stash_push(
    path: String,
    message: Option<String>,
    keep_index: bool,
) -> Result<String, AppCommandError> {
    let mut args = vec!["stash".to_string(), "push".to_string()];
    if let Some(msg) = message {
        if !msg.is_empty() {
            args.push("-m".to_string());
            args.push(msg);
        }
    }
    if keep_index {
        args.push("--keep-index".to_string());
    }
    let output = crate::process::tokio_command("git")
        .args(&args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("stash push", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_stash_pop(
    path: String,
    stash_ref: Option<String>,
) -> Result<String, AppCommandError> {
    let mut args = vec!["stash", "pop"];
    let stash_ref_val;
    if let Some(ref r) = stash_ref {
        stash_ref_val = r.clone();
        args.push(&stash_ref_val);
    }
    let output = crate::process::tokio_command("git")
        .args(&args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("stash pop", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_stash_list(path: String) -> Result<Vec<GitStashEntry>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["stash", "list", "--format=%gd||%gs||%ci"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("stash list", &output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .enumerate()
        .filter_map(|(i, line)| {
            let parts: Vec<&str> = line.splitn(3, "||").collect();
            if parts.len() < 3 {
                return None;
            }
            let ref_name = parts[0].to_string();
            let subject = parts[1];
            let date = parts[2].to_string();

            // Parse branch and message from subject like "On branch: message" or "WIP on branch: hash"
            let (branch, message) = if let Some(rest) = subject.strip_prefix("On ") {
                if let Some(colon_pos) = rest.find(": ") {
                    let branch = rest[..colon_pos].to_string();
                    let msg = rest[colon_pos + 2..].to_string();
                    (branch, msg)
                } else {
                    (String::new(), subject.to_string())
                }
            } else if let Some(rest) = subject.strip_prefix("WIP on ") {
                if let Some(colon_pos) = rest.find(": ") {
                    let branch = rest[..colon_pos].to_string();
                    let msg = rest[colon_pos + 2..].to_string();
                    (branch, msg)
                } else {
                    (String::new(), subject.to_string())
                }
            } else {
                (String::new(), subject.to_string())
            };

            Some(GitStashEntry {
                index: i,
                message,
                branch,
                date,
                ref_name,
            })
        })
        .collect();

    Ok(entries)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_stash_apply(path: String, stash_ref: String) -> Result<String, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["stash", "apply", &stash_ref])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("stash apply", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_stash_drop(path: String, stash_ref: String) -> Result<String, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["stash", "drop", &stash_ref])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("stash drop", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_stash_clear(path: String) -> Result<String, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["stash", "clear"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("stash clear", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_stash_show(
    path: String,
    stash_ref: String,
) -> Result<Vec<GitStatusEntry>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["-c", "core.quotePath=false"])
        .args(["stash", "show", "--name-status", &stash_ref])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("stash show", &output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let mut parts = line.splitn(2, '\t');
            let status = parts.next()?.trim().to_string();
            let file = unquote_git_path(parts.next()?);
            Some(GitStatusEntry { status, file })
        })
        .collect();

    Ok(entries)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_status(
    path: String,
    show_all_untracked: Option<bool>,
) -> Result<Vec<GitStatusEntry>, AppCommandError> {
    ensure_git_repo(&path)?;

    let untracked_mode = if show_all_untracked.unwrap_or(false) {
        "-uall"
    } else {
        "-unormal"
    };
    // `--no-optional-locks` keeps this read-only query from contending with
    // concurrent agent writes on `.git/index.lock`. See PR #215 follow-up.
    let output = crate::process::tokio_command("git")
        .arg("--no-optional-locks")
        .args(["-c", "core.quotePath=false"])
        .args(["status", "--porcelain=v1", untracked_mode])
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
            let file = unquote_git_path(&line[3..]);
            GitStatusEntry { status, file }
        })
        .collect();
    Ok(entries)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_diff(path: String, file: Option<String>) -> Result<String, AppCommandError> {
    ensure_git_repo(&path)?;

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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_diff_with_branch(
    path: String,
    branch: String,
    file: Option<String>,
) -> Result<String, AppCommandError> {
    ensure_git_repo(&path)?;

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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_show_diff(
    path: String,
    commit: String,
    file: Option<String>,
) -> Result<String, AppCommandError> {
    ensure_git_repo(&path)?;

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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_show_file(
    path: String,
    file: String,
    ref_name: Option<String>,
) -> Result<String, AppCommandError> {
    ensure_git_repo(&path)?;

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

pub(crate) async fn git_commit_core(
    emitter: &EventEmitter,
    folder_id: Option<i32>,
    conn: &sea_orm::DatabaseConnection,
    path: &str,
    message: &str,
    files: &[String],
) -> Result<GitCommitResult, AppCommandError> {
    // Find files already staged for deletion — git add would fail on these
    // because they no longer exist in either the working tree or the index.
    let staged_deletions: std::collections::HashSet<String> = crate::process::tokio_command("git")
        .args(["diff", "--cached", "--name-only", "--diff-filter=D", "-z"])
        .current_dir(path)
        .output()
        .await
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    // Stage only files that aren't already staged deletions
    let files_to_add: Vec<_> = files
        .iter()
        .filter(|f| !staged_deletions.contains(f.as_str()))
        .collect();

    if !files_to_add.is_empty() {
        let mut add_args = vec!["add".to_string(), "--".to_string()];
        add_args.extend(
            files_to_add
                .iter()
                .map(|file| to_git_literal_pathspec(file)),
        );

        let add_output = crate::process::tokio_command("git")
            .args(&add_args)
            .current_dir(path)
            .output()
            .await
            .map_err(AppCommandError::io)?;

        if !add_output.status.success() {
            return Err(git_command_error("add", &add_output.stderr));
        }
    }

    // Resolve commit author from matching account (e.g. GitHub username)
    let author_override = crate::git_credential::resolve_commit_author(path, conn).await;

    // Commit
    let mut commit_cmd = crate::process::tokio_command("git");
    if let Some((ref name, ref email)) = author_override {
        commit_cmd.args([
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
        ]);
    }
    commit_cmd.args(["commit", "-m", message]).current_dir(path);

    let commit_output = commit_cmd.output().await.map_err(AppCommandError::io)?;

    if !commit_output.status.success() {
        return Err(git_command_error("commit", &commit_output.stderr));
    }

    let committed_files = count_files_in_commit(path, "HEAD")
        .await
        .unwrap_or(files.len());

    if let Some(folder_id) = folder_id {
        crate::web::event_bridge::emit_event(
            emitter,
            "folder://git-commit-succeeded",
            GitCommitSucceededEvent {
                folder_id,
                committed_files,
            },
        );
    }

    Ok(GitCommitResult { committed_files })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_commit(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    db: tauri::State<'_, AppDatabase>,
    path: String,
    message: String,
    files: Vec<String>,
    folder_id: Option<i32>,
) -> Result<GitCommitResult, AppCommandError> {
    let folder_id = folder_id.or_else(|| {
        window
            .label()
            .strip_prefix("commit-")
            .and_then(|value| value.parse::<i32>().ok())
    });
    let emitter = EventEmitter::Tauri(app.clone());
    git_commit_core(&emitter, folder_id, &db.conn, &path, &message, &files).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_list_all_branches(path: String) -> Result<GitBranchList, AppCommandError> {
    ensure_git_repo(&path)?;

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

/// Parse `git worktree list --porcelain` into `(worktree_path, branch)` pairs.
/// Each porcelain block starts with a `worktree <path>` line and is terminated
/// by a blank line; a `branch refs/heads/<name>` line carries the checked-out
/// branch (absent for detached/bare worktrees → `None`). Trailing blocks with
/// no terminating blank line are flushed at EOF.
fn parse_worktrees(stdout: &str) -> Vec<(String, Option<String>)> {
    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_branch: Option<String> = None;

    for line in stdout.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            // Defensive flush in case a block wasn't blank-line terminated.
            if let Some(path) = current_path.take() {
                entries.push((path, current_branch.take()));
            }
            current_path = Some(p.trim().to_string());
            current_branch = None;
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(b.trim().to_string());
        } else if line.is_empty() {
            if let Some(path) = current_path.take() {
                entries.push((path, current_branch.take()));
            }
            current_branch = None;
        }
    }
    if let Some(path) = current_path.take() {
        entries.push((path, current_branch.take()));
    }
    entries
}

/// Resolve where `branch` is checked out and which registered folder owns that
/// directory. Canonicalizes both the worktree path (from git) and every folder
/// path (from the DB) so symlinked / non-canonical paths still match — this
/// can only be done on the host that runs git, which is why it lives in the
/// backend rather than the webview.
pub async fn resolve_worktree_folder_core(
    db: &AppDatabase,
    repo_path: String,
    branch: String,
) -> Result<WorktreeResolution, AppCommandError> {
    ensure_git_repo(&repo_path)?;

    let output = crate::process::tokio_command("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&repo_path)
        .output()
        .await
        .map_err(AppCommandError::io)?;
    if !output.status.success() {
        return Err(git_command_error("worktree list", &output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let wt_path = parse_worktrees(&stdout)
        .into_iter()
        .find(|(_, b)| b.as_deref() == Some(branch.as_str()))
        .map(|(p, _)| p);

    let Some(wt_path) = wt_path else {
        // Branch is not checked out in any worktree → caller checks it out in root.
        return Ok(WorktreeResolution {
            path: None,
            folder_id: None,
        });
    };

    let canonical_wt = std::fs::canonicalize(&wt_path).unwrap_or_else(|_| PathBuf::from(&wt_path));

    let folders = folder_service::list_all_folder_details(&db.conn)
        .await
        .map_err(AppCommandError::from)?;
    let folder_id = folders
        .into_iter()
        .find(|f| {
            let canon =
                std::fs::canonicalize(&f.path).unwrap_or_else(|_| PathBuf::from(&f.path));
            canon == canonical_wt
        })
        .map(|f| f.id);

    Ok(WorktreeResolution {
        path: Some(canonical_wt.to_string_lossy().to_string()),
        folder_id,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn resolve_worktree_folder(
    db: tauri::State<'_, AppDatabase>,
    repo_path: String,
    branch: String,
) -> Result<WorktreeResolution, AppCommandError> {
    resolve_worktree_folder_core(&db, repo_path, branch).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_list_remotes(path: String) -> Result<Vec<GitRemote>, AppCommandError> {
    ensure_git_repo(&path)?;

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

pub(crate) async fn git_fetch_remote_core(
    path: &str,
    name: &str,
    credentials: Option<&GitCredentials>,
    db: &AppDatabase,
    data_dir: &std::path::Path,
) -> Result<String, AppCommandError> {
    let mut cmd = crate::process::tokio_command("git");
    cmd.args(["fetch", name]).current_dir(path);
    prepare_remote_git_cmd_with_remote(&mut cmd, path, Some(name), credentials, db, data_dir).await;

    let output = cmd.output().await.map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(classify_remote_git_error("fetch", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_fetch_remote(
    path: String,
    name: String,
    credentials: Option<GitCredentials>,
    db: tauri::State<'_, AppDatabase>,
    app_handle: tauri::AppHandle,
) -> Result<String, AppCommandError> {
    let data_dir = app_handle.path().app_data_dir().map_err(|e| {
        AppCommandError::external_command("Failed to resolve app data dir", e.to_string())
    })?;
    // Resolve through the effective data dir so a custom
    // `CODEG_DATA_DIR` reaches the git credential helper invoked by
    // this subprocess.
    let data_dir = crate::paths::resolve_effective_data_dir(&data_dir);
    git_fetch_remote_core(&path, &name, credentials.as_ref(), &db, &data_dir).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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
        let conflicted_files = detect_conflicts(&path).await?;
        if !conflicted_files.is_empty() {
            return Ok(GitMergeResult {
                merged_commits,
                conflict: Some(GitConflictInfo {
                    has_conflicts: true,
                    conflicted_files,
                    operation: "merge".to_string(),
                    upstream_commit: None,
                }),
            });
        }
        return Err(git_command_error("merge", &output.stderr));
    }
    Ok(GitMergeResult {
        merged_commits,
        conflict: None,
    })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_rebase(
    path: String,
    branch_name: String,
) -> Result<GitRebaseResult, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["rebase", &branch_name])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        let conflicted_files = detect_conflicts(&path).await?;
        if !conflicted_files.is_empty() {
            return Ok(GitRebaseResult {
                message: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                conflict: Some(GitConflictInfo {
                    has_conflicts: true,
                    conflicted_files,
                    operation: "rebase".to_string(),
                    upstream_commit: None,
                }),
            });
        }
        return Err(git_command_error("rebase", &output.stderr));
    }
    Ok(GitRebaseResult {
        message: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        conflict: None,
    })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

pub(crate) async fn git_delete_remote_branch_core(
    path: &str,
    remote: &str,
    branch: &str,
    credentials: Option<&GitCredentials>,
    db: &AppDatabase,
    data_dir: &std::path::Path,
) -> Result<String, AppCommandError> {
    let mut cmd = crate::process::tokio_command("git");
    cmd.args(["push", remote, "--delete", branch])
        .current_dir(path);
    prepare_remote_git_cmd_with_remote(&mut cmd, path, Some(remote), credentials, db, data_dir)
        .await;

    let output = cmd.output().await.map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(classify_remote_git_error("push --delete", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_delete_remote_branch(
    path: String,
    remote: String,
    branch: String,
    credentials: Option<GitCredentials>,
    db: tauri::State<'_, AppDatabase>,
    app_handle: tauri::AppHandle,
) -> Result<String, AppCommandError> {
    let data_dir = app_handle.path().app_data_dir().map_err(|e| {
        AppCommandError::external_command("Failed to resolve app data dir", e.to_string())
    })?;
    // Resolve through the effective data dir so a custom
    // `CODEG_DATA_DIR` reaches the git credential helper invoked by
    // this subprocess.
    let data_dir = crate::paths::resolve_effective_data_dir(&data_dir);
    git_delete_remote_branch_core(
        &path,
        &remote,
        &branch,
        credentials.as_ref(),
        &db,
        &data_dir,
    )
    .await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_list_conflicts(path: String) -> Result<Vec<String>, AppCommandError> {
    detect_conflicts(&path).await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_conflict_file_versions(
    path: String,
    file: String,
) -> Result<GitConflictFileVersions, AppCommandError> {
    // :1: = base (common ancestor), :2: = ours (HEAD), :3: = theirs (incoming)
    let mut versions = Vec::with_capacity(3);
    for stage in ["1", "2", "3"] {
        let file_spec = format!(":{}:{}", stage, file);
        let output = crate::process::tokio_command("git")
            .args(["show", &file_spec])
            .current_dir(&path)
            .output()
            .await
            .map_err(AppCommandError::io)?;

        if !output.status.success() {
            // File may not exist at this stage (e.g. newly added on one side)
            versions.push(String::new());
        } else {
            let bytes = &output.stdout;
            if bytes.iter().take(2048).any(|b| *b == 0) {
                return Err(
                    AppCommandError::invalid_input("Binary files are not supported")
                        .with_detail(file_spec),
                );
            }
            versions.push(String::from_utf8_lossy(bytes).to_string());
        }
    }

    // Read the working tree file (contains conflict markers)
    let file_path = Path::new(&path).join(&file);
    let merged = std::fs::read_to_string(&file_path).unwrap_or_default();

    Ok(GitConflictFileVersions {
        base: versions.remove(0),
        ours: versions.remove(0),
        theirs: versions.remove(0),
        merged,
    })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_resolve_conflict(
    path: String,
    file: String,
    content: String,
) -> Result<(), AppCommandError> {
    let file_path = Path::new(&path).join(&file);

    // Write resolved content
    std::fs::write(&file_path, content)
        .map_err(|e| AppCommandError::io_error(format!("Failed to write resolved file: {}", e)))?;

    // Stage the resolved file
    let output = crate::process::tokio_command("git")
        .args(["add", &file])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("add", &output.stderr));
    }

    Ok(())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_abort_operation(path: String, operation: String) -> Result<(), AppCommandError> {
    let args = match operation.as_str() {
        "merge" | "pull" => vec!["merge", "--abort"],
        "rebase" => vec!["rebase", "--abort"],
        _ => {
            return Err(AppCommandError::invalid_input(format!(
                "Unknown operation: {operation}"
            )));
        }
    };

    let output = crate::process::tokio_command("git")
        .args(&args)
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error(
            &format!("{} --abort", operation),
            &output.stderr,
        ));
    }
    Ok(())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_continue_operation(
    path: String,
    operation: String,
) -> Result<(), AppCommandError> {
    let (program, args): (&str, Vec<&str>) = match operation.as_str() {
        "merge" | "pull" => ("git", vec!["commit", "--no-edit"]),
        "rebase" => ("git", vec!["rebase", "--continue"]),
        _ => {
            return Err(AppCommandError::invalid_input(format!(
                "Unknown operation: {operation}"
            )));
        }
    };

    let output = crate::process::tokio_command(program)
        .args(&args)
        .current_dir(&path)
        .env("GIT_EDITOR", "true")
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error(
            &format!("{} --continue", operation),
            &output.stderr,
        ));
    }
    Ok(())
}

const FILE_TREE_IGNORED_DIRS: &[&str] = &[".git", "__pycache__"];

/// Hard limit: refuse to open files larger than 50 MB in the text editor.
const FILE_OPEN_HARD_LIMIT: usize = 50_000_000;
/// Save limit: refuse to save content larger than 50 MB.
const FILE_SAVE_HARD_LIMIT: usize = 50_000_000;
const FILE_BASE64_DEFAULT_MAX_BYTES: usize = 20_000_000;
const FILE_BASE64_MAX_BYTES: usize = 100_000_000;
const FILE_IO_MAX_CONCURRENT_OPS: usize = 8;

static FILE_IO_SEMAPHORE: LazyLock<Semaphore> =
    LazyLock::new(|| Semaphore::new(FILE_IO_MAX_CONCURRENT_OPS));

fn to_git_literal_pathspec(path: &str) -> String {
    format!(":(literal){path}")
}

/// Remove surrounding quotes from a git output path.
/// Git quotes paths containing non-ASCII or special characters, e.g.
/// `"path/\344\270\255\346\226\207.txt"`.  With `core.quotePath=false`
/// the octal escapes are gone, but the quotes may still appear for paths
/// with spaces, tabs, etc.
fn unquote_git_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn resolve_tree_path(
    root: &Path,
    rel_path: &str,
) -> Result<PathBuf, AppCommandError> {
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

fn read_text_full(target: &Path, hard_limit: usize) -> Result<String, AppCommandError> {
    let metadata = std::fs::metadata(target).map_err(AppCommandError::io)?;
    if metadata.len() > hard_limit as u64 {
        return Err(
            AppCommandError::invalid_input("File is too large to open in editor")
                .with_detail(format!("size={}, limit={}", metadata.len(), hard_limit)),
        );
    }

    let bytes = std::fs::read(target).map_err(AppCommandError::io)?;

    if bytes.iter().take(2_048).any(|b| *b == 0) {
        return Err(AppCommandError::invalid_input(
            "Binary files are not supported in preview",
        ));
    }

    Ok(String::from_utf8_lossy(&bytes).to_string())
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
    let dir = std::fs::File::open(path).map_err(AppCommandError::io)?;
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

// ─── Directory browser helpers (for web/server mode) ───

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_home_directory() -> Result<String, AppCommandError> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| AppCommandError::io_error("Could not determine home directory"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
    pub has_children: bool,
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_directory_entries(path: String) -> Result<Vec<DirectoryEntry>, AppCommandError> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(AppCommandError::io_error("Path is not a directory").with_detail(path));
    }

    let mut entries: Vec<DirectoryEntry> = Vec::new();
    let read_dir = std::fs::read_dir(&root).map_err(|e| {
        AppCommandError::io_error("Failed to read directory").with_detail(e.to_string())
    })?;

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        // Follow symlinks: check if the resolved path is a directory
        let is_dir = if file_type.is_symlink() {
            entry.path().is_dir()
        } else {
            file_type.is_dir()
        };
        if !is_dir {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden directories (starting with '.')
        if name.starts_with('.') {
            continue;
        }
        let abs_path = entry.path().to_string_lossy().to_string();

        // Peek into subdirectory to check if it has child directories
        let has_children = match std::fs::read_dir(entry.path()) {
            Ok(sub) => sub.filter_map(|e| e.ok()).any(|e| {
                let ft = e.file_type().ok();
                let is_sub_dir = ft.is_some_and(|ft| {
                    if ft.is_symlink() {
                        e.path().is_dir()
                    } else {
                        ft.is_dir()
                    }
                });
                if !is_sub_dir {
                    return false;
                }
                let sub_name = e.file_name().to_string_lossy().to_string();
                !sub_name.starts_with('.')
            }),
            Err(_) => false,
        };

        entries.push(DirectoryEntry {
            name,
            path: abs_path,
            has_children,
        });
    }

    // Sort by name, case-insensitive
    entries.sort_by_key(|a| a.name.to_lowercase());

    Ok(entries)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// Only meaningful when `is_dir` is true.
    pub has_children: bool,
    /// File size in bytes; `None` for directories.
    pub size: Option<u64>,
}

/// List immediate children of `path`, returning both directories and files.
/// Mirrors `list_directory_entries` but does not filter out files, used by the
/// "attach server file" picker.
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_directory_with_files(
    path: String,
) -> Result<Vec<DirectoryItem>, AppCommandError> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(AppCommandError::io_error("Path is not a directory").with_detail(path));
    }

    let mut items: Vec<DirectoryItem> = Vec::new();
    let read_dir = std::fs::read_dir(&root).map_err(|e| {
        AppCommandError::io_error("Failed to read directory").with_detail(e.to_string())
    })?;

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        // Follow symlinks for the dir/file classification.
        let is_dir = if file_type.is_symlink() {
            entry.path().is_dir()
        } else {
            file_type.is_dir()
        };
        let abs_path = entry.path().to_string_lossy().to_string();

        let (has_children, size) = if is_dir {
            let has = match std::fs::read_dir(entry.path()) {
                Ok(sub) => sub.filter_map(|e| e.ok()).any(|e| {
                    let sub_name = e.file_name().to_string_lossy().to_string();
                    !sub_name.starts_with('.')
                }),
                Err(_) => false,
            };
            (has, None)
        } else {
            let size = entry.metadata().ok().map(|m| m.len());
            (false, size)
        };

        items.push(DirectoryItem {
            name,
            path: abs_path,
            is_dir,
            has_children,
            size,
        });
    }

    // Sort: directories first, then files; each group by name case-insensitive.
    items.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(items)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

/// Flat, gitignore-aware listing of every file and directory under `path`, for
/// fuzzy file search. Unlike [`get_file_tree`], ignored directories
/// (`node_modules`, `target`, `dist`, …) are pruned *during* the walk, so no
/// depth cap is needed: deep files stay reachable while the heavy trees are
/// never descended and the payload stays small. Gitignore handling that used to
/// run client-side now happens here at native speed in a single pass.
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_workspace_files(
    path: String,
) -> Result<Vec<WorkspaceFileEntry>, AppCommandError> {
    let root = PathBuf::from(&path);

    // Conservative gitignore parity with the previous client-side pass: respect
    // in-tree `.gitignore`/`.ignore`/`.git/info/exclude`, but not the global
    // gitignore or parent-directory ignores (the workspace root is the
    // boundary). `require_git(false)` keeps `.gitignore` effective even outside
    // a git repo. `hidden(false)` keeps dotfiles visible. The `filter_entry`
    // mirrors `get_file_tree`'s hardcoded ignores exactly.
    let walker = WalkBuilder::new(&root)
        .hidden(false)
        .parents(false)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                !FILE_TREE_IGNORED_DIRS.contains(&name.as_ref())
            } else {
                name != ".DS_Store"
            }
        })
        .build();

    let mut entries: Vec<WorkspaceFileEntry> = Vec::new();
    for result in walker {
        // Skip unreadable entries (permission errors, transient races) rather
        // than failing the whole search.
        let Ok(entry) = result else { continue };
        let entry_path = entry.path();

        // Skip the root directory itself.
        if entry_path == root {
            continue;
        }

        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        let rel_path = entry_path
            .strip_prefix(&root)
            .unwrap_or(entry_path)
            .to_string_lossy()
            .replace('\\', "/");

        entries.push(WorkspaceFileEntry {
            name,
            path: rel_path,
            kind: if is_dir {
                WorkspaceEntryKind::Dir
            } else {
                WorkspaceEntryKind::File
            },
        });
    }

    Ok(entries)
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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
        .clamp(4_096, FILE_BASE64_MAX_BYTES);

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

/// Open a file for reading, refusing a final-component symlink (unix) so a
/// path validated by canonicalization cannot be redirected through a symlink
/// swapped in afterward.
#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_FLAG_OPEN_REPARSE_POINT opens the reparse point itself instead of
    // following it, so a symlink/junction swapped in after validation is opened
    // (and then rejected by the is_file() check) rather than followed outside
    // the workspace root.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// Like `read_file_base64`, but confined to a workspace root: the path is
/// relative to `root_path` and is canonicalized (resolving symlinks) so it can
/// never read outside the workspace. Used by the HTML preview to inline local
/// sub-resources without exposing the unconfined `read_file_base64` to crafted
/// markup (e.g. a symlink pointing at `/etc/passwd`).
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn read_workspace_file_base64(
    root_path: String,
    path: String,
    max_bytes: Option<usize>,
) -> Result<String, AppCommandError> {
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
        .unwrap_or(FILE_BASE64_DEFAULT_MAX_BYTES)
        .clamp(4_096, FILE_BASE64_MAX_BYTES);

    run_file_io(move || {
        use std::io::Read;
        // Canonicalize and confine, then open a single handle (O_NOFOLLOW on
        // unix) and do metadata + read on the fd. This closes the check-then-
        // read race: the original `target` symlink can't be re-resolved (we use
        // the canonical path), a final-component symlink swapped in after the
        // check makes the open fail, and metadata/read never re-look-up the path.
        let canonical_root =
            std::fs::canonicalize(&root).map_err(AppCommandError::io)?;
        let canonical_target =
            std::fs::canonicalize(&target).map_err(AppCommandError::io)?;
        if !canonical_target.starts_with(&canonical_root) {
            return Err(AppCommandError::invalid_input(
                "Path is outside workspace root",
            ));
        }
        let mut file =
            open_no_follow(&canonical_target).map_err(AppCommandError::io)?;
        let metadata = file.metadata().map_err(AppCommandError::io)?;
        if !metadata.is_file() {
            return Err(AppCommandError::invalid_input("Path is not a file"));
        }
        if metadata.len() > limit as u64 {
            return Err(
                AppCommandError::invalid_input("File is too large to attach")
                    .with_detail(format!("max_bytes={limit}")),
            );
        }
        // take(limit + 1) bounds the read even if the file grows after fstat.
        let mut bytes = Vec::new();
        Read::take(&mut file, limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(AppCommandError::io)?;
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn read_file_preview(
    root_path: String,
    path: String,
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
    let path_for_response = path.clone();

    run_file_io(move || {
        ensure_path_in_workspace(&root, &target)?;
        let content = read_text_full(&target, FILE_OPEN_HARD_LIMIT)?;
        Ok(FilePreviewContent {
            path: path_for_response,
            content,
        })
    })
    .await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn read_file_for_edit(
    root_path: String,
    path: String,
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

    let path_for_response = path.clone();

    run_file_io(move || {
        ensure_path_in_workspace(&root, &target)?;
        let metadata = std::fs::metadata(&target).map_err(AppCommandError::io)?;
        let content = read_text_full(&target, FILE_OPEN_HARD_LIMIT)?;
        let readonly = metadata.permissions().readonly();
        let mtime_ms = file_mtime_ms(&metadata);
        let etag = compute_etag(content.as_bytes(), &metadata);
        let line_ending = detect_line_ending(content.as_bytes());

        Ok(FileEditContent {
            path: path_for_response,
            content,
            etag,
            mtime_ms,
            readonly,
            line_ending,
        })
    })
    .await
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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
    if content.len() > FILE_SAVE_HARD_LIMIT {
        return Err(
            AppCommandError::invalid_input("File is too large to save in editor")
                .with_detail(format!("max_bytes={FILE_SAVE_HARD_LIMIT}")),
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn save_file_copy(
    root_path: String,
    path: String,
    content: String,
) -> Result<FileSaveResult, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }
    if content.len() > FILE_SAVE_HARD_LIMIT {
        return Err(
            AppCommandError::invalid_input("File is too large to save in editor")
                .with_detail(format!("max_bytes={FILE_SAVE_HARD_LIMIT}")),
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

/// Move a file/directory into a different directory of the same workspace,
/// keeping its name. `source_path` and `dest_dir` are both workspace-relative
/// (forward slashes); `dest_dir` is `""` for the workspace root. Returns the new
/// workspace-relative path of the moved entry.
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn move_file_tree_entry(
    root_path: String,
    source_path: String,
    dest_dir: String,
) -> Result<String, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let source = resolve_tree_path(&root, &source_path)?;
    // `symlink_metadata` doesn't follow the final component, so a (possibly
    // dangling) symlink entry the tree still shows counts as existing — and
    // `fs::rename` moves the link itself rather than whatever it points at.
    match std::fs::symlink_metadata(&source) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppCommandError::not_found("Source file does not exist"));
        }
        Err(e) => return Err(AppCommandError::io(e)),
    }
    if source == root {
        return Err(AppCommandError::invalid_input("Cannot move workspace root"));
    }

    let dest = resolve_tree_path(&root, &dest_dir)?;
    if !dest.is_dir() {
        return Err(AppCommandError::invalid_input(
            "Destination is not a directory",
        ));
    }

    // Reject moving a directory into itself or one of its own descendants —
    // `starts_with` is component-wise, so `src` is not mistaken for `src-utils`.
    if dest == source || dest.starts_with(&source) {
        return Err(AppCommandError::invalid_input(
            "Cannot move a directory into itself",
        ));
    }

    let name = source
        .file_name()
        .ok_or_else(|| AppCommandError::invalid_input("Cannot move path without a name"))?;
    let target = dest.join(name);

    // Dropping onto the current parent is a no-op — report the unchanged path
    // rather than erroring so the UI can simply do nothing.
    if target == source {
        return Ok(source_path);
    }
    // Collision check via `symlink_metadata`: a same-name entry of ANY kind —
    // including a dangling symlink, which `Path::exists()` reports as absent —
    // must block the move so `fs::rename` can't silently clobber it.
    match std::fs::symlink_metadata(&target) {
        Ok(_) => {
            return Err(AppCommandError::already_exists(
                "An entry with this name already exists in the destination",
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(AppCommandError::io(e)),
    }

    std::fs::rename(&source, &target).map_err(AppCommandError::io)?;

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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn delete_file_tree_entry(
    root_path: String,
    path: String,
) -> Result<(), AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let target = resolve_tree_path(&root, &path)?;
    // `Path::exists` follows symlinks and silently returns false on a
    // dangling link or any I/O error along the resolve chain, which gave
    // us "Target file does not exist" toasts even for files that were
    // physically present (case-only mismatches against a case-preserving
    // FS, NFD/NFC mismatches on macOS, files under a non-traversable
    // ancestor). `symlink_metadata` only stats the leaf and surfaces the
    // real OS error code in `detail`, which is what we want to diagnose
    // those reports.
    let meta = match std::fs::symlink_metadata(&target) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppCommandError::not_found("Target file does not exist")
                .with_detail(format!("resolved={} relative={}", target.display(), path)));
        }
        Err(err) => {
            return Err(
                AppCommandError::io_error("Failed to stat target").with_detail(format!(
                    "resolved={} relative={} error={}",
                    target.display(),
                    path,
                    err
                )),
            );
        }
    };
    if target == root {
        return Err(AppCommandError::invalid_input(
            "Cannot delete workspace root",
        ));
    }
    if meta.is_dir() {
        std::fs::remove_dir_all(&target).map_err(AppCommandError::io)?;
    } else {
        std::fs::remove_file(&target).map_err(AppCommandError::io)?;
    }

    Ok(())
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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
            resolved.parent().map(|p| p.to_path_buf()).ok_or_else(|| {
                AppCommandError::invalid_input("Cannot determine parent directory")
            })?
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

// A Tauri command deserializes each named arg from JS, so the query knobs stay
// flat positional params rather than a bundled options struct.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_log(
    path: String,
    limit: Option<u32>,
    branch: Option<String>,
    remote: Option<String>,
    skip: Option<u32>,
    author: Option<String>,
    all_branches: Option<bool>,
    with_files: Option<bool>,
) -> Result<GitLogResult, AppCommandError> {
    ensure_git_repo(&path)?;

    const COMMIT_META_PREFIX: &str = "__COMMIT__\0";
    const MESSAGE_END_MARKER: &str = "__COMMIT_MESSAGE_END__";

    let all_branches = all_branches.unwrap_or(false);
    // Per-commit file changes (`--raw --numstat`) diff every commit against its
    // parent, which dominates the cost of a page. Callers that only need the
    // commit list (the log tab, which lazy-loads a commit's files on expand via
    // git_commit_files) pass with_files=false to skip it. Defaults to true for
    // back-compat (e.g. the push window renders files inline).
    let with_files = with_files.unwrap_or(true);

    // Offset for paginated (infinite-scroll) loading: the frontend requests
    // successive pages by their running commit count.
    let skip = skip.unwrap_or(0);
    let limit_str = format!("-{}", limit.unwrap_or(100));
    let mut args = vec![
        "log".to_string(),
        limit_str,
        format!("--format=__COMMIT__%x00%h%x00%H%x00%an%x00%aI%n%B%n{MESSAGE_END_MARKER}"),
    ];
    if with_files {
        args.push("--raw".to_string());
        args.push("--numstat".to_string());
        args.push("--no-renames".to_string());
    }
    if skip > 0 {
        args.push(format!("--skip={}", skip));
    }
    // Author filter (IDEA-style "User" filter): match the EXACT author name via
    // an anchored, BRE-escaped pattern (see git_author_match_pattern) so e.g.
    // "Alice" does not also match "Alice Smith" or a commit whose email contains
    // "alice". `--basic-regexp` pins the pattern language so a user's
    // grep.patternType config can't reinterpret it. Must precede the
    // revision/pathspec arg below.
    if let Some(ref a) = author {
        if !a.is_empty() {
            args.push("--basic-regexp".to_string());
            args.push(format!("--author={}", git_author_match_pattern(a)));
        }
    }
    // `--all` (all refs) is the default "all branches" view; an explicit branch
    // narrows to that ref. `--all` wins if both are somehow set.
    if all_branches {
        args.push("--all".to_string());
    } else if let Some(ref b) = branch {
        args.push(b.clone());
    }
    let output = crate::process::tokio_command("git")
        .args(["-c", "core.quotePath=false"])
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
            return Ok(GitLogResult {
                entries: Vec::new(),
                has_upstream: false,
            });
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

    // Cover the full fetched range (skip + limit) so pushed status stays correct
    // on deeper pages — get_unpushed_hashes caps its rev-list to this count. The
    // all-branches path passes `author` through so the same filter narrows that
    // window (a sparse match still lands inside it); the single-branch upstream
    // paths below are not author-filtered, so a matched commit deep past the
    // window there can still report an inaccurate badge — an accepted limit for
    // that (rarer) branch+author combination.
    let log_limit = skip.saturating_add(limit.unwrap_or(100));
    let (unpushed_hashes, has_upstream) = get_unpushed_hashes(
        &path,
        log_limit,
        remote.as_deref(),
        branch.as_deref(),
        all_branches,
        author.as_deref(),
    )
    .await
    .unwrap_or((None, false));
    for entry in entries.iter_mut() {
        entry.pushed = unpushed_hashes
            .as_ref()
            .map(|hashes| !hashes.contains(&entry.full_hash));
    }

    Ok(GitLogResult {
        entries,
        has_upstream,
    })
}

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_commit_branches(
    path: String,
    commit: String,
) -> Result<Vec<String>, AppCommandError> {
    ensure_git_repo(&path)?;

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

/// Build a `git log --author` pattern matching EXACTLY the given author name
/// (`%an`) — not a substring, and not a match inside the email. The name is
/// escaped for basic regular expressions (BRE) and anchored to the start of the
/// "Name <email>" author ident with a trailing " <", so "Alice" matches
/// "Alice <…>" but not "Alice Smith <…>" nor "Bob <alice@…>". Pair with
/// `--basic-regexp` so the escaping is interpreted as BRE regardless of the
/// user's grep.patternType config. Note `|`, `+`, `?`, `(`, `)`, `{`, `}` are
/// literal in BRE, so they are intentionally left unescaped. Pure — unit-tested
/// without a repo.
fn git_author_match_pattern(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len() + 4);
    for ch in name.chars() {
        if matches!(ch, '\\' | '.' | '*' | '[' | ']' | '^' | '$') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    format!("^{escaped} <")
}

/// The configured commit author name (`git config user.name`) so the author
/// filter can offer a pinned "me" quick-select. Best-effort: absent/blank →
/// None. Cheap (no history walk), unlike a full repo author scan.
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_current_user(path: String) -> Result<Option<String>, AppCommandError> {
    ensure_git_repo(&path)?;

    let output = crate::process::tokio_command("git")
        .args(["config", "user.name"])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Ok(None);
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(if name.is_empty() { None } else { Some(name) })
}

/// Parse `git shortlog -sne` output into a de-duplicated, frequency-ordered list
/// of author names whose name contains `query` (case-insensitive). shortlog lines
/// look like `"   123\tAlice <alice@example.com>"` (a padded count, a tab, then
/// `Name <email>`); we return just the display names. Pure — unit-tested without
/// a repo.
fn filter_shortlog_authors(stdout: &str, query: &str, limit: usize) -> Vec<String> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in stdout.lines() {
        // Strip the leading count column ("   123\t").
        let rest = match line.split_once('\t') {
            Some((_count, rest)) => rest.trim(),
            None => continue,
        };
        // Drop the trailing " <email>" so we key on the display name only (the
        // author filter matches names, see git_author_match_pattern).
        let name = match rest.rsplit_once(" <") {
            Some((name, _email)) => name.trim(),
            None => rest,
        };
        if name.is_empty() || !name.to_lowercase().contains(&needle) {
            continue;
        }
        // Same author under two emails collapses to one name (first = most
        // frequent, since shortlog is count-sorted).
        if seen.insert(name.to_string()) {
            out.push(name.to_string());
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// On-demand author search for the log filter: real commit authors whose name
/// matches `query` (case-insensitive substring), most-active first. Backed by
/// `git shortlog -s -n -e --all` (dedup + counts across all refs). Unlike a full
/// upfront author scan (removed for perf), this runs only while the user is
/// actively typing in the filter box (debounced client-side), so the history walk
/// is paid on demand.
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_search_authors(
    path: String,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<String>, AppCommandError> {
    ensure_git_repo(&path)?;

    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    // `--all` summarizes every ref; `-s -n -e` = counts only, sorted by count
    // desc, identities distinguished by email. stdin is nulled so shortlog never
    // blocks trying to read a revision range from a (possibly piped) stdin.
    let output = crate::process::tokio_command("git")
        .args(["shortlog", "-s", "-n", "-e", "--all"])
        .current_dir(&path)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("shortlog", &output.stderr));
    }

    let limit = limit.unwrap_or(20).clamp(1, 100) as usize;
    Ok(filter_shortlog_authors(
        &String::from_utf8_lossy(&output.stdout),
        trimmed,
        limit,
    ))
}

/// File changes for a single commit, loaded on demand when a commit row is
/// expanded (git_log with with_files=false omits these to keep the list fast).
/// Same `--raw`/`--numstat` shape as git_log so the same parsing applies.
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn git_commit_files(
    path: String,
    commit: String,
) -> Result<Vec<GitLogFileChange>, AppCommandError> {
    ensure_git_repo(&path)?;

    // `--first-parent` keeps merge commits on a normal (first-parent) diff
    // instead of git show's default combined (`--cc`) output, whose `--raw`
    // section uses a different shape that would mis-parse (files defaulting to
    // "M"). This matches the first-parent diff git_log itself produces.
    let output = crate::process::tokio_command("git")
        .args(["-c", "core.quotePath=false"])
        .args([
            "show",
            "--format=",
            "--first-parent",
            "--raw",
            "--numstat",
            "--no-renames",
            &commit,
        ])
        .current_dir(&path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    if !output.status.success() {
        return Err(git_command_error("show", &output.stderr));
    }

    // `git show` for a single commit emits the same `:`-prefixed raw lines and
    // numstat lines git_log parses; reuse the same builder to merge them.
    let mut builder = GitLogFilesBuilder::default();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') {
            if let Some((status, file_path)) = parse_raw_file_line(line) {
                builder.get_or_insert_file(file_path).status = status;
            }
            continue;
        }
        if let Some((additions, deletions, file_path)) = parse_numstat_file_line(line) {
            let file = builder.get_or_insert_file(file_path);
            file.additions = additions;
            file.deletions = deletions;
        }
    }

    Ok(builder.files)
}

/// Accumulates a commit's file changes, merging the `--raw` (status) and
/// `--numstat` (+/-) lines that describe the same path. Shared by git_log's
/// per-commit builder and the standalone git_commit_files command.
#[derive(Default)]
struct GitLogFilesBuilder {
    files: Vec<GitLogFileChange>,
    index_by_path: HashMap<String, usize>,
}

impl GitLogFilesBuilder {
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
}

struct GitLogEntryBuilder {
    hash: String,
    full_hash: String,
    author: String,
    date: String,
    message: String,
    files: GitLogFilesBuilder,
}

impl GitLogEntryBuilder {
    fn new(parts: Vec<&str>) -> Self {
        Self {
            hash: parts[0].to_string(),
            full_hash: parts[1].to_string(),
            author: parts[2].to_string(),
            date: parts[3].to_string(),
            message: String::new(),
            files: GitLogFilesBuilder::default(),
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
        self.files.get_or_insert_file(path)
    }

    fn finish(self) -> GitLogEntry {
        GitLogEntry {
            hash: self.hash,
            full_hash: self.full_hash,
            author: self.author,
            date: self.date,
            message: self.message,
            files: self.files.files,
            pushed: None,
        }
    }
}

fn parse_raw_file_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.split('\t');
    let meta = parts.next()?;
    let file_path = unquote_git_path(parts.next()?);
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
    let file_path = unquote_git_path(parts.next()?);
    Some((additions, deletions, file_path))
}

fn parse_numstat_count(value: &str) -> u32 {
    if value == "-" {
        return 0;
    }

    value.parse::<u32>().unwrap_or(0)
}

/// Returns (unpushed_hashes, has_upstream).
async fn get_unpushed_hashes(
    path: &str,
    limit: u32,
    remote_override: Option<&str>,
    branch: Option<&str>,
    all_branches: bool,
    author: Option<&str>,
) -> Result<(Option<HashSet<String>>, bool), AppCommandError> {
    let limit_arg = format!("-{}", limit);

    // All-branches view: a commit counts as "pushed" iff it is reachable from any
    // remote-tracking ref. "unpushed" = commits the log (`git log --all`) shows
    // that sit on no remote. Two things must line up with git_log so every
    // *displayed* commit gets a correct, definite badge:
    //  - Walk the SAME `--all` ref universe (not just `--branches`), so tag-/
    //    stash-only commits that appear in the log are classified too (walking
    //    only `--branches` would leave them absent from the set → wrongly
    //    "pushed").
    //  - Apply the SAME `--author` filter (anchored BRE, see
    //    git_author_match_pattern), so a sparse author-filtered page's commits
    //    fall inside the `-{limit}` window instead of being pushed past it by
    //    unrelated commits and mislabeled "pushed".
    // The status is always definite (never "unknown"): with no remote-tracking
    // refs `--not --remotes` is a no-op, so every listed commit reads "not
    // pushed" — accurate, nothing is on a remote — instead of a question mark.
    if all_branches {
        // Drives only the has_upstream flag; push status is resolved regardless.
        let has_remotes = crate::process::tokio_command("git")
            .args(["for-each-ref", "--count=1", "refs/remotes"])
            .current_dir(path)
            .output()
            .await
            .map(|o| {
                o.status.success()
                    && !String::from_utf8_lossy(&o.stdout).trim().is_empty()
            })
            .unwrap_or(false);
        let mut rev_args = vec!["rev-list".to_string(), limit_arg.clone()];
        if let Some(a) = author.filter(|a| !a.is_empty()) {
            rev_args.push("--basic-regexp".to_string());
            rev_args.push(format!("--author={}", git_author_match_pattern(a)));
        }
        rev_args.push("--all".to_string());
        rev_args.push("--not".to_string());
        rev_args.push("--remotes".to_string());
        let output = crate::process::tokio_command("git")
            .args(&rev_args)
            .current_dir(path)
            .output()
            .await
            .map_err(AppCommandError::io)?;
        if !output.status.success() {
            return Ok((None, has_remotes));
        }
        let hashes = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.to_string())
            .collect::<HashSet<_>>();
        return Ok((Some(hashes), has_remotes));
    }

    // If viewing a remote branch (e.g. "origin/main"), all commits are pushed
    if let Some(b) = branch {
        let is_remote = crate::process::tokio_command("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/{}", b),
            ])
            .current_dir(path)
            .output()
            .await
            .is_ok_and(|o| o.status.success());
        if is_remote {
            return Ok((Some(HashSet::new()), true));
        }
    }

    // The local ref to compare: specified branch or HEAD
    let local_ref = branch.unwrap_or("HEAD");

    // Check upstream for the target branch
    let upstream_arg = if branch.is_some() {
        format!("{}@{{upstream}}", local_ref)
    } else {
        "@{upstream}".to_string()
    };

    let upstream_output = crate::process::tokio_command("git")
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            &upstream_arg,
        ])
        .current_dir(path)
        .output()
        .await
        .map_err(AppCommandError::io)?;

    let has_upstream = upstream_output.status.success()
        && !String::from_utf8_lossy(&upstream_output.stdout)
            .trim()
            .is_empty();

    // Determine the comparison target for unpushed commits.
    // We compare against <remote>/<branch> specifically rather than all remote
    // branches, so that commits shared with other remote branches still appear.
    let rev_list_output = if has_upstream && remote_override.is_none() {
        // Fast path: branch has an upstream tracking ref, use it directly
        let upstream = String::from_utf8_lossy(&upstream_output.stdout)
            .trim()
            .to_string();
        let range = format!("{upstream}..{local_ref}");
        crate::process::tokio_command("git")
            .args(["rev-list", &limit_arg, &range])
            .current_dir(path)
            .output()
            .await
            .map_err(AppCommandError::io)?
    } else {
        // Either remote_override is specified or no upstream exists.
        // Resolve the branch name and the target remote.
        let branch_name = if let Some(b) = branch {
            b.to_string()
        } else {
            let branch_output = crate::process::tokio_command("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(path)
                .output()
                .await
                .map_err(AppCommandError::io)?;
            if !branch_output.status.success() {
                return Ok((None, has_upstream));
            }
            let name = String::from_utf8_lossy(&branch_output.stdout)
                .trim()
                .to_string();
            if name.is_empty() || name == "HEAD" {
                return Ok((None, has_upstream));
            }
            name
        };

        let remote = if let Some(r) = remote_override {
            r.to_string()
        } else {
            let remote_key = format!("branch.{}.remote", branch_name);
            let remote_output = crate::process::tokio_command("git")
                .args(["config", "--get", &remote_key])
                .current_dir(path)
                .output()
                .await;
            remote_output
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "origin".to_string())
        };

        // Try comparing against <remote>/<branch> directly
        let remote_branch_ref = format!("refs/remotes/{}/{}", remote, branch_name);
        let verify_output = crate::process::tokio_command("git")
            .args(["rev-parse", "--verify", "--quiet", &remote_branch_ref])
            .current_dir(path)
            .output()
            .await;
        let remote_branch_exists = verify_output.is_ok_and(|o| o.status.success());

        if remote_branch_exists {
            let range = format!("{}/{}..{}", remote, branch_name, local_ref);
            crate::process::tokio_command("git")
                .args(["rev-list", &limit_arg, &range])
                .current_dir(path)
                .output()
                .await
                .map_err(AppCommandError::io)?
        } else {
            // Branch doesn't exist on remote yet (new branch).
            // Try merge-base with the remote's default branch to show
            // the meaningful divergence point.
            let remote_head = format!("{}/HEAD", remote);
            let mb_output = crate::process::tokio_command("git")
                .args(["merge-base", local_ref, &remote_head])
                .current_dir(path)
                .output()
                .await;
            let merge_base = mb_output
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty());

            if let Some(base) = merge_base {
                let range = format!("{}..{}", base, local_ref);
                crate::process::tokio_command("git")
                    .args(["rev-list", &limit_arg, &range])
                    .current_dir(path)
                    .output()
                    .await
                    .map_err(AppCommandError::io)?
            } else {
                // Last resort: compare against all branches on the remote
                let remote_arg = format!("--remotes={}", remote);
                crate::process::tokio_command("git")
                    .args(["rev-list", &limit_arg, local_ref, "--not", &remote_arg])
                    .current_dir(path)
                    .output()
                    .await
                    .map_err(AppCommandError::io)?
            }
        }
    };

    if !rev_list_output.status.success() {
        return Ok((None, has_upstream));
    }

    let hashes = String::from_utf8_lossy(&rev_list_output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect::<HashSet<_>>();

    Ok((Some(hashes), has_upstream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;
    use crate::models::agent::AgentType;

    #[test]
    fn git_author_match_pattern_anchors_and_escapes() {
        // Anchored to the name start + trailing " <" so it can't match a longer
        // name or the email portion.
        assert_eq!(git_author_match_pattern("Alice"), "^Alice <");
        // BRE metacharacters are backslash-escaped...
        assert_eq!(git_author_match_pattern("a.b*c"), "^a\\.b\\*c <");
        assert_eq!(git_author_match_pattern("na[me]"), "^na\\[me\\] <");
        assert_eq!(
            git_author_match_pattern("back\\slash"),
            "^back\\\\slash <"
        );
        // ...but `|` is literal in BRE, so a name like `程相|cx` is left as-is.
        assert_eq!(git_author_match_pattern("程相|cx"), "^程相|cx <");
    }

    #[test]
    fn filter_shortlog_authors_matches_dedupes_and_caps() {
        // Real shortlog rows: a space-padded count, a tab, then `Name <email>`.
        let out = "    10\tAlice <alice@example.com>\n     5\tBob <bob@example.com>\n     3\tAlice <alice@other.com>\n     1\tCarol <carol@example.com>\n";
        // Case-insensitive substring on the name; the same name under two emails
        // collapses to a single entry (first/most-frequent wins).
        assert_eq!(filter_shortlog_authors(out, "ali", 20), vec!["Alice"]);
        assert_eq!(filter_shortlog_authors(out, "B", 20), vec!["Bob"]);
        // Empty / whitespace query → no candidates.
        assert!(filter_shortlog_authors(out, "  ", 20).is_empty());
        // The limit caps the result count.
        let many = "   3\tAaa <a@x>\n   2\tAab <b@x>\n   1\tAac <c@x>\n";
        assert_eq!(filter_shortlog_authors(many, "aa", 2), vec!["Aaa", "Aab"]);
    }

    #[test]
    fn emit_folder_upsert_broadcasts_on_folder_channel() {
        // A headlessly-created folder must reach every client on
        // `folder://changed` carrying its full detail, so the sidebar can place a
        // conversation produced inside it without a re-fetch.
        use crate::db::entities::folder::FolderKind;
        use crate::web::event_bridge::{WebEventBroadcaster, FOLDER_CHANGED_EVENT};
        use std::sync::Arc;

        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let mut rx = broadcaster.subscribe();
        let emitter = EventEmitter::test_web_only(broadcaster.clone());

        emit_folder_upsert(
            &emitter,
            FolderDetail {
                id: 7,
                name: "repo-automation-3-run-9".to_string(),
                path: "/home/me/repo-automation-3-run-9".to_string(),
                git_branch: Some("automation/3/run-9".to_string()),
                default_agent_type: None,
                last_opened_at: chrono::Utc::now(),
                sort_order: 0,
                color: "inherit".to_string(),
                parent_id: Some(1),
                kind: FolderKind::Regular,
                alias: None,
            },
        );

        let evt = rx.try_recv().expect("folder upsert should broadcast");
        let p = &*evt.payload;
        assert_eq!(evt.channel, FOLDER_CHANGED_EVENT);
        assert_eq!(p["kind"], "upsert");
        assert_eq!(p["folder"]["id"], 7);
        assert_eq!(p["folder"]["parent_id"], 1);
    }

    /// Create `rel` (relative to `root`) as a file, making parent dirs.
    fn write_file(root: &std::path::Path, rel: &str, contents: &str) {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(full, contents).expect("write file");
    }

    #[tokio::test]
    async fn list_workspace_files_includes_deep_and_prunes_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // A file nested 12 levels deep — beyond the old hardcoded depth-10 cap
        // this command replaces. It must still be discovered (the core fix).
        let deep_rel = "a/b/c/d/e/f/g/h/i/j/k/deep.txt";
        write_file(root, deep_rel, "deep");

        // Gitignore rules must be honored even without a real git repo
        // (`require_git(false)`), pruning ignored trees during the walk.
        write_file(root, ".gitignore", "node_modules/\nignored.txt\n");
        write_file(root, "node_modules/pkg/index.js", "junk");
        write_file(root, "ignored.txt", "junk");

        // The hardcoded ignores (mirroring FILE_TREE_IGNORED_DIRS) must be pruned.
        write_file(root, ".git/config", "[core]");
        write_file(root, "src/.DS_Store", "junk");

        // A normal file that must survive.
        write_file(root, "src/main.rs", "fn main() {}");

        let entries = list_workspace_files(root.to_string_lossy().to_string())
            .await
            .expect("list_workspace_files");
        let paths: std::collections::HashSet<&str> =
            entries.iter().map(|e| e.path.as_str()).collect();

        // Deep and normal files are reachable.
        assert!(
            paths.contains(deep_rel),
            "deep file must be listed, got {paths:?}"
        );
        assert!(paths.contains("src/main.rs"), "normal file must be listed");

        // Gitignored entries are pruned during traversal.
        assert!(
            !paths.iter().any(|p| p.starts_with("node_modules")),
            "gitignored node_modules must be pruned, got {paths:?}"
        );
        assert!(
            !paths.contains("ignored.txt"),
            "gitignored file must be pruned"
        );

        // Hardcoded ignores are pruned.
        // Precisely the `.git` dir / its contents — not `.gitignore`, which is
        // intentionally listed.
        assert!(
            !paths.iter().any(|p| *p == ".git" || p.starts_with(".git/")),
            ".git dir must be pruned, got {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with(".DS_Store")),
            ".DS_Store must be pruned"
        );

        // Intermediate directory entries are emitted alongside files — the
        // `@`-mention picker relies on directories being present.
        assert!(
            entries.iter().any(
                |e| e.path == "a" && matches!(e.kind, WorkspaceEntryKind::Dir)
            ),
            "directory entries must be present"
        );
    }

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

    #[tokio::test]
    async fn resolve_git_head_reports_branch_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        git_run(dir.path(), &["init", "-q"]);
        git_run(dir.path(), &["commit", "-q", "--allow-empty", "-m", "c1"]);
        git_run(dir.path(), &["checkout", "-q", "-b", "feature"]);

        let info = resolve_git_head(dir.path().to_str().unwrap())
            .await
            .expect("resolve");
        assert_eq!(
            info,
            GitHeadInfo {
                is_repo: true,
                branch: Some("feature".into()),
                detached: false,
                short_sha: None,
            }
        );
    }

    #[tokio::test]
    async fn resolve_git_head_detects_detached_head() {
        let dir = tempfile::tempdir().expect("tempdir");
        git_run(dir.path(), &["init", "-q"]);
        git_run(dir.path(), &["commit", "-q", "--allow-empty", "-m", "c1"]);
        git_run(dir.path(), &["commit", "-q", "--allow-empty", "-m", "c2"]);
        git_run(dir.path(), &["checkout", "-q", "HEAD~1"]);

        let info = resolve_git_head(dir.path().to_str().unwrap())
            .await
            .expect("resolve");
        assert!(info.is_repo, "detached HEAD is still a repo");
        assert!(info.detached, "must be flagged detached");
        assert_eq!(info.branch, None, "no branch when detached");
        assert!(
            info.short_sha.as_deref().is_some_and(|s| !s.is_empty()),
            "detached HEAD should expose a short sha, got {:?}",
            info.short_sha
        );
    }

    #[tokio::test]
    async fn resolve_git_head_handles_non_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let info = resolve_git_head(dir.path().to_str().unwrap())
            .await
            .expect("resolve");
        assert_eq!(
            info,
            GitHeadInfo {
                is_repo: false,
                branch: None,
                detached: false,
                short_sha: None,
            }
        );
    }

    #[tokio::test]
    async fn resolve_git_head_handles_unborn_branch() {
        let dir = tempfile::tempdir().expect("tempdir");
        git_run(dir.path(), &["init", "-q"]);

        let info = resolve_git_head(dir.path().to_str().unwrap())
            .await
            .expect("resolve");
        assert!(info.is_repo, "freshly-initialized repo is a repo");
        assert!(!info.detached, "unborn branch is not detached");
        assert!(
            info.branch.is_some(),
            "unborn branch should resolve a name, got {:?}",
            info.branch
        );
    }

    #[tokio::test]
    async fn get_git_branch_stays_none_on_detached_head() {
        let dir = tempfile::tempdir().expect("tempdir");
        git_run(dir.path(), &["init", "-q"]);
        git_run(dir.path(), &["commit", "-q", "--allow-empty", "-m", "c1"]);
        git_run(dir.path(), &["commit", "-q", "--allow-empty", "-m", "c2"]);
        git_run(dir.path(), &["checkout", "-q", "HEAD~1"]);

        // The legacy `Option<String>` contract intentionally reports no branch
        // for a detached HEAD; git-log default selection and compare rely on it.
        assert_eq!(
            get_git_branch(dir.path().to_str().unwrap().to_string())
                .await
                .expect("branch"),
            None
        );
    }

    /// Like `git_run` but returns the command's trimmed stdout (e.g. a resolved
    /// commit hash), with the same isolated identity/config env.
    fn git_capture(dir: &std::path::Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // All-branches push status: with no remote-tracking refs every listed commit
    // is a definite "not pushed" (never the old unknown/None), and the classified
    // set walks the same `--all` universe as the log so a tag-only commit shown in
    // the log is covered too (a `--branches`-only walk would miss it → wrongly
    // "pushed").
    #[tokio::test]
    async fn git_log_all_branches_no_remote_marks_every_commit_not_pushed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        git_run(p, &["init", "-q"]);
        git_run(p, &["commit", "-q", "--allow-empty", "-m", "c1"]);
        git_run(p, &["commit", "-q", "--allow-empty", "-m", "c2"]);
        // A tag-only commit: create c3, tag it, then move the branch back so c3 is
        // reachable only via refs/tags — shown by `git log --all` but absent from
        // `--branches`.
        git_run(p, &["commit", "-q", "--allow-empty", "-m", "c3"]);
        let c3 = git_capture(p, &["rev-parse", "HEAD"]);
        git_run(p, &["tag", "v1"]);
        git_run(p, &["reset", "-q", "--hard", "HEAD~1"]);

        let result = git_log(
            p.to_string_lossy().to_string(),
            None,        // limit
            None,        // branch
            None,        // remote
            None,        // skip
            None,        // author
            Some(true),  // all_branches
            Some(false), // with_files
        )
        .await
        .expect("git_log");

        assert!(
            result.entries.iter().any(|e| e.full_hash == c3),
            "tag-only commit must be listed (proves the --all universe is walked)"
        );
        for e in &result.entries {
            assert_eq!(
                e.pushed,
                Some(false),
                "commit {} must read not-pushed, got {:?}",
                e.hash,
                e.pushed
            );
        }
        assert!(!result.has_upstream, "no remotes → has_upstream=false");
    }

    // A commit reachable from a remote-tracking ref reads "pushed"; commits ahead
    // of it read "not pushed". The remote-tracking ref is faked with update-ref so
    // the test needs no network.
    #[tokio::test]
    async fn git_log_all_branches_marks_remote_reachable_commits_pushed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        git_run(p, &["init", "-q"]);
        git_run(p, &["commit", "-q", "--allow-empty", "-m", "c1"]);
        let c1 = git_capture(p, &["rev-parse", "HEAD"]);
        git_run(p, &["commit", "-q", "--allow-empty", "-m", "c2"]);
        let c2 = git_capture(p, &["rev-parse", "HEAD"]);
        git_run(p, &["commit", "-q", "--allow-empty", "-m", "c3"]);
        let c3 = git_capture(p, &["rev-parse", "HEAD"]);
        // "Pushed up to c2" without a network: point a remote-tracking ref at c2.
        git_run(p, &["update-ref", "refs/remotes/origin/main", &c2]);

        let result = git_log(
            p.to_string_lossy().to_string(),
            None,
            None,
            None,
            None,
            None,
            Some(true),
            Some(false),
        )
        .await
        .expect("git_log");

        let pushed_of = |hash: &str| {
            result
                .entries
                .iter()
                .find(|e| e.full_hash == hash)
                .unwrap_or_else(|| panic!("commit {hash} missing from log"))
                .pushed
        };
        assert_eq!(pushed_of(&c1), Some(true), "c1 is on the remote → pushed");
        assert_eq!(pushed_of(&c2), Some(true), "c2 is the remote tip → pushed");
        assert_eq!(
            pushed_of(&c3),
            Some(false),
            "c3 is ahead of the remote → not pushed"
        );
        assert!(result.has_upstream, "a remote-tracking ref → has_upstream");
    }

    // An author-filtered match that sits past the status rev-list's `-{limit}`
    // window in unfiltered history must still be classified: the status walk
    // applies the same `--author` filter as the log, so the sparse match stays
    // inside the window. An unfiltered walk would drop it and mislabel it
    // "pushed".
    #[tokio::test]
    async fn git_log_all_branches_author_match_past_window_stays_not_pushed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        git_run(p, &["init", "-q"]);
        git_run(
            p,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "a1",
                "--author=Alice <alice@example.com>",
            ],
        );
        let alice = git_capture(p, &["rev-parse", "HEAD"]);
        // Six Bob commits push Alice (the root) to rank 7, past a `-5` window.
        for i in 0..6 {
            let msg = format!("b{i}");
            git_run(
                p,
                &[
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    &msg,
                    "--author=Bob <bob@example.com>",
                ],
            );
        }

        let result = git_log(
            p.to_string_lossy().to_string(),
            Some(5),                   // limit → status window is `-5`
            None,
            None,
            None,
            Some("Alice".to_string()), // author filter
            Some(true),
            Some(false),
        )
        .await
        .expect("git_log");

        assert_eq!(result.entries.len(), 1, "only the one Alice commit matches");
        let e = &result.entries[0];
        assert_eq!(e.full_hash, alice);
        assert_eq!(
            e.pushed,
            Some(false),
            "author-filtered match past the unfiltered window must read not-pushed"
        );
    }

    #[tokio::test]
    async fn add_folder_to_history_core_derives_name_from_path() {
        let db = fresh_in_memory_db().await;
        let entry = add_folder_to_history_core(&db, "/tmp/codeg-test-project".into())
            .await
            .expect("add folder");
        assert_eq!(entry.name, "codeg-test-project");
        assert_eq!(entry.path, "/tmp/codeg-test-project");
    }

    #[tokio::test]
    async fn add_folder_to_history_core_upserts_on_duplicate_path() {
        let db = fresh_in_memory_db().await;
        let path = "/tmp/codeg-dup-test".to_string();
        let first = add_folder_to_history_core(&db, path.clone())
            .await
            .expect("add 1st");
        let second = add_folder_to_history_core(&db, path.clone())
            .await
            .expect("add 2nd");
        assert_eq!(first.id, second.id, "duplicate path must reuse id");

        let history = load_folder_history_core(&db).await.expect("history");
        assert_eq!(
            history.iter().filter(|f| f.path == path).count(),
            1,
            "no duplicate rows for same path"
        );
    }

    #[tokio::test]
    async fn remove_folder_from_history_core_soft_deletes() {
        let db = fresh_in_memory_db().await;
        let path = "/tmp/codeg-remove-test".to_string();
        add_folder_to_history_core(&db, path.clone())
            .await
            .expect("add");
        remove_folder_from_history_core(&db, path.clone())
            .await
            .expect("remove");
        let history = load_folder_history_core(&db).await.expect("history");
        assert!(
            history.iter().all(|f| f.path != path),
            "soft-deleted folder must not appear in list"
        );
    }

    #[tokio::test]
    async fn open_folder_by_id_core_errors_when_missing() {
        let db = fresh_in_memory_db().await;
        let err = open_folder_by_id_core(&db, 99_999)
            .await
            .expect_err("missing id should error");
        // Either the not_found wrapper (when set_folder_open returns Ok(()) on no-op)
        // or the underlying DbError propagates — both are acceptable for "missing".
        let msg = format!("{err:?}");
        assert!(
            msg.to_lowercase().contains("not found") || msg.to_lowercase().contains("99999"),
            "expected not-found-ish error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn open_worktree_folder_core_records_parent_as_root() {
        let db = fresh_in_memory_db().await;
        let root = open_folder_core(&db, "/tmp/codeg-wt-root".into())
            .await
            .expect("open root");
        assert_eq!(root.parent_id, None, "root folder has no parent");

        let wt = open_worktree_folder_core(&db, "/tmp/codeg-wt-a".into(), root.id)
            .await
            .expect("open worktree");
        assert_eq!(
            wt.parent_id,
            Some(root.id),
            "worktree records its source root folder"
        );
    }

    #[tokio::test]
    async fn open_worktree_folder_core_flattens_nested_worktrees() {
        let db = fresh_in_memory_db().await;
        let root = open_folder_core(&db, "/tmp/codeg-wt-flat-root".into())
            .await
            .expect("open root");
        let child = open_worktree_folder_core(&db, "/tmp/codeg-wt-flat-1".into(), root.id)
            .await
            .expect("open child worktree");
        // A worktree created *from* the child must still point at the root, not
        // the intermediate child.
        let grandchild = open_worktree_folder_core(&db, "/tmp/codeg-wt-flat-2".into(), child.id)
            .await
            .expect("open grandchild worktree");
        assert_eq!(child.parent_id, Some(root.id));
        assert_eq!(
            grandchild.parent_id,
            Some(root.id),
            "worktree of a worktree flattens to the original root"
        );
    }

    #[tokio::test]
    async fn open_worktree_folder_core_unknown_source_is_root() {
        let db = fresh_in_memory_db().await;
        let wt = open_worktree_folder_core(&db, "/tmp/codeg-wt-orphan".into(), 0)
            .await
            .expect("open worktree with no source");
        assert_eq!(
            wt.parent_id, None,
            "non-positive / unknown source degrades to a top-level folder"
        );
    }

    #[test]
    fn parse_worktrees_extracts_path_branch_pairs() {
        // Main tree + a linked worktree + a detached worktree, trailing blank line.
        let stdout = "\
worktree /repo/main
HEAD aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
branch refs/heads/main

worktree /repo/wt-feature
HEAD bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
branch refs/heads/feature-x

worktree /repo/wt-detached
HEAD cccccccccccccccccccccccccccccccccccccccc
detached

";
        let entries = parse_worktrees(stdout);
        assert_eq!(
            entries,
            vec![
                ("/repo/main".to_string(), Some("main".to_string())),
                (
                    "/repo/wt-feature".to_string(),
                    Some("feature-x".to_string())
                ),
                ("/repo/wt-detached".to_string(), None),
            ]
        );
    }

    #[test]
    fn parse_worktrees_flushes_trailing_block_without_blank_line() {
        // No terminating blank line on the last block (git omits it at EOF here).
        let stdout = "\
worktree /repo/main
HEAD aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
branch refs/heads/main";
        let entries = parse_worktrees(stdout);
        assert_eq!(
            entries,
            vec![("/repo/main".to_string(), Some("main".to_string()))]
        );
    }

    #[test]
    fn parse_worktrees_handles_empty_and_bare() {
        assert!(parse_worktrees("").is_empty());
        // A bare repo entry carries no branch.
        let entries = parse_worktrees("worktree /repo/bare\nbare\n\n");
        assert_eq!(entries, vec![("/repo/bare".to_string(), None)]);
    }

    #[tokio::test]
    async fn open_folder_core_preserves_existing_worktree_parent() {
        let db = fresh_in_memory_db().await;
        let root = open_folder_core(&db, "/tmp/codeg-wt-preserve-root".into())
            .await
            .expect("open root");
        let wt = open_worktree_folder_core(&db, "/tmp/codeg-wt-preserve".into(), root.id)
            .await
            .expect("open worktree");
        assert_eq!(wt.parent_id, Some(root.id));
        // A plain reopen of the same path must not clear the recorded parent.
        let reopened = open_folder_core(&db, "/tmp/codeg-wt-preserve".into())
            .await
            .expect("reopen plain");
        assert_eq!(
            reopened.parent_id,
            Some(root.id),
            "plain open_folder must preserve an existing worktree parent"
        );
    }

    #[tokio::test]
    async fn open_worktree_folder_core_unknown_source_demotes_existing_to_root() {
        let db = fresh_in_memory_db().await;
        let root = open_folder_core(&db, "/tmp/codeg-wt-demote-root".into())
            .await
            .expect("open root");
        let path = "/tmp/codeg-wt-demote".to_string();
        let wt = open_worktree_folder_core(&db, path.clone(), root.id)
            .await
            .expect("open worktree");
        assert_eq!(wt.parent_id, Some(root.id));
        // Reopening the same path as a worktree with an unknown source writes the
        // authoritative value (top-level) rather than keeping the stale parent.
        let demoted = open_worktree_folder_core(&db, path, 0)
            .await
            .expect("reopen worktree with no source");
        assert_eq!(
            demoted.parent_id, None,
            "explicit worktree open with unknown source demotes to top-level"
        );
    }

    #[tokio::test]
    async fn update_folder_color_core_roundtrips() {
        let db = fresh_in_memory_db().await;
        let entry = add_folder_to_history_core(&db, "/tmp/codeg-color-test".into())
            .await
            .expect("add");
        let updated = update_folder_color_core(&db, entry.id, "#ff8800".into())
            .await
            .expect("update color");
        assert_eq!(updated.color, "#ff8800");
        let read_back = get_folder_core(&db, entry.id).await.expect("get");
        assert_eq!(read_back.color, "#ff8800");
    }

    #[tokio::test]
    async fn update_folder_default_agent_core_set_then_clear() {
        let db = fresh_in_memory_db().await;
        let entry = add_folder_to_history_core(&db, "/tmp/codeg-agent-test".into())
            .await
            .expect("add");
        let set = update_folder_default_agent_core(&db, entry.id, Some(AgentType::ClaudeCode))
            .await
            .expect("set agent");
        assert_eq!(set.default_agent_type, Some(AgentType::ClaudeCode));
        let cleared = update_folder_default_agent_core(&db, entry.id, None)
            .await
            .expect("clear agent");
        assert_eq!(cleared.default_agent_type, None);
    }

    #[tokio::test]
    async fn move_file_tree_entry_moves_file_into_subdir() {
        let root = tempfile::tempdir().expect("root");
        std::fs::write(root.path().join("a.txt"), b"x").expect("write");
        std::fs::create_dir(root.path().join("sub")).expect("mkdir");

        let new_rel = move_file_tree_entry(
            root.path().to_string_lossy().into_owned(),
            "a.txt".to_string(),
            "sub".to_string(),
        )
        .await
        .expect("move");

        assert_eq!(new_rel.replace('\\', "/"), "sub/a.txt");
        assert!(!root.path().join("a.txt").exists());
        assert!(root.path().join("sub/a.txt").exists());
    }

    #[tokio::test]
    async fn move_file_tree_entry_moves_back_to_root() {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir(root.path().join("sub")).expect("mkdir");
        std::fs::write(root.path().join("sub/a.txt"), b"x").expect("write");

        let new_rel = move_file_tree_entry(
            root.path().to_string_lossy().into_owned(),
            "sub/a.txt".to_string(),
            String::new(),
        )
        .await
        .expect("move to root");

        assert_eq!(new_rel, "a.txt");
        assert!(root.path().join("a.txt").exists());
        assert!(!root.path().join("sub/a.txt").exists());
    }

    #[tokio::test]
    async fn move_file_tree_entry_rejects_move_into_own_descendant() {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir_all(root.path().join("src/inner")).expect("mkdir");

        let res = move_file_tree_entry(
            root.path().to_string_lossy().into_owned(),
            "src".to_string(),
            "src/inner".to_string(),
        )
        .await;
        assert!(res.is_err(), "moving a dir into its own descendant must fail");
        assert!(root.path().join("src/inner").is_dir(), "source untouched");
    }

    #[tokio::test]
    async fn move_file_tree_entry_rejects_existing_target() {
        let root = tempfile::tempdir().expect("root");
        std::fs::write(root.path().join("a.txt"), b"1").expect("write");
        std::fs::create_dir(root.path().join("sub")).expect("mkdir");
        std::fs::write(root.path().join("sub/a.txt"), b"2").expect("write");

        let res = move_file_tree_entry(
            root.path().to_string_lossy().into_owned(),
            "a.txt".to_string(),
            "sub".to_string(),
        )
        .await;
        assert!(res.is_err(), "name collision in destination must fail");
        assert!(root.path().join("a.txt").exists(), "source untouched");
    }

    #[tokio::test]
    async fn move_file_tree_entry_same_parent_is_noop() {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir(root.path().join("sub")).expect("mkdir");
        std::fs::write(root.path().join("sub/a.txt"), b"x").expect("write");

        let rel = move_file_tree_entry(
            root.path().to_string_lossy().into_owned(),
            "sub/a.txt".to_string(),
            "sub".to_string(),
        )
        .await
        .expect("no-op move");
        assert_eq!(rel, "sub/a.txt");
        assert!(root.path().join("sub/a.txt").exists());
    }

    #[tokio::test]
    async fn move_file_tree_entry_rejects_parent_escape() {
        let root = tempfile::tempdir().expect("root");
        std::fs::write(root.path().join("a.txt"), b"x").expect("write");

        let res = move_file_tree_entry(
            root.path().to_string_lossy().into_owned(),
            "a.txt".to_string(),
            "../evil".to_string(),
        )
        .await;
        assert!(res.is_err(), "dest containing '..' must be rejected");
    }
}

// Symlink confinement that `read_workspace_file_base64` relies on. Unix-only
// because it uses real filesystem symlinks.
#[cfg(all(test, unix))]
mod workspace_confinement_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn reads_in_root_file() {
        let root = tempfile::tempdir().expect("root");
        std::fs::write(root.path().join("a.txt"), b"hello").expect("write");
        let b64 = read_workspace_file_base64(
            root.path().to_string_lossy().into_owned(),
            "a.txt".to_string(),
            None,
        )
        .await
        .expect("should read in-root file");
        assert_eq!(b64, "aGVsbG8="); // base64("hello")
    }

    #[tokio::test]
    async fn rejects_symlink_escaping_root() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("secret"), b"top").expect("write");
        symlink(outside.path().join("secret"), root.path().join("link"))
            .expect("symlink");
        // The canonical target resolves outside the root, so the read is denied
        // even though `root/link` is lexically inside the workspace.
        let res = read_workspace_file_base64(
            root.path().to_string_lossy().into_owned(),
            "link".to_string(),
            None,
        )
        .await;
        assert!(res.is_err(), "symlink escaping the workspace must be rejected");
    }

    #[test]
    fn ensure_path_in_workspace_rejects_symlink() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, b"x").expect("write");
        let link = root.path().join("asset.txt");
        symlink(&secret, &link).expect("symlink");
        assert!(ensure_path_in_workspace(root.path(), &link).is_err());
    }

    #[tokio::test]
    async fn move_file_tree_entry_does_not_clobber_dangling_symlink() {
        let root = tempfile::tempdir().expect("root");
        std::fs::write(root.path().join("a.txt"), b"real").expect("write");
        std::fs::create_dir(root.path().join("sub")).expect("mkdir");
        // A dangling symlink occupies the destination name. `Path::exists()`
        // follows it and reports `false`, so the collision check must instead
        // use `symlink_metadata` or `fs::rename` would destroy the link.
        symlink(
            root.path().join("nonexistent-target"),
            root.path().join("sub/a.txt"),
        )
        .expect("symlink");

        let res = move_file_tree_entry(
            root.path().to_string_lossy().into_owned(),
            "a.txt".to_string(),
            "sub".to_string(),
        )
        .await;
        assert!(res.is_err(), "must not clobber a dangling destination symlink");
        assert!(
            std::fs::symlink_metadata(root.path().join("sub/a.txt")).is_ok(),
            "dangling symlink must remain intact",
        );
        assert!(root.path().join("a.txt").exists(), "source untouched");
    }

    #[tokio::test]
    async fn move_file_tree_entry_moves_a_symlink_entry() {
        let root = tempfile::tempdir().expect("root");
        std::fs::write(root.path().join("target.txt"), b"x").expect("write");
        symlink(
            root.path().join("target.txt"),
            root.path().join("link.txt"),
        )
        .expect("symlink");
        std::fs::create_dir(root.path().join("sub")).expect("mkdir");

        let new_rel = move_file_tree_entry(
            root.path().to_string_lossy().into_owned(),
            "link.txt".to_string(),
            "sub".to_string(),
        )
        .await
        .expect("move symlink");
        assert_eq!(new_rel.replace('\\', "/"), "sub/link.txt");
        // The moved entry is still a symlink (the link itself moved, not its target).
        let meta = std::fs::symlink_metadata(root.path().join("sub/link.txt"))
            .expect("moved link exists");
        assert!(meta.file_type().is_symlink(), "entry stays a symlink");
        assert!(!root.path().join("link.txt").exists(), "old link gone");
    }
}
