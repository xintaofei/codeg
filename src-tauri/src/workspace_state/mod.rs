use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use crate::app_error::AppCommandError;
use crate::commands::folders::{self, FileTreeNode};
use crate::web::event_bridge::{emit_event, EventEmitter};

pub const WORKSPACE_STATE_PROTOCOL_VERSION: u16 = 1;

const WATCH_IGNORED_DIRS: &[&str] = &["__pycache__"];
const WATCH_DEBOUNCE_MS: u64 = 2_000;
const WATCH_MAX_BATCH_WINDOW_MS: u64 = 5_000;
const WATCH_MAX_CHANGED_PATHS: usize = 2_000;
const WATCH_EVENT_CHANNEL_CAPACITY: usize = 2_048;
const RECENT_EVENT_CAPACITY: usize = 24;
const WORKSPACE_TREE_MAX_DEPTH: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceGitEntry {
    pub path: String,
    pub status: String,
    pub additions: i32,
    pub deletions: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceDelta {
    TreeReplace { nodes: Vec<FileTreeNode> },
    GitReplace { entries: Vec<WorkspaceGitEntry> },
    Meta { reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceDeltaEnvelope {
    pub seq: u64,
    pub kind: String,
    pub payload: Vec<WorkspaceDelta>,
    pub requires_resync: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceStateEvent {
    pub root_path: String,
    pub seq: u64,
    pub version: u16,
    pub kind: String,
    pub payload: Vec<WorkspaceDelta>,
    pub requires_resync: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSnapshotResponse {
    pub root_path: String,
    pub seq: u64,
    pub version: u16,
    pub full: bool,
    pub tree_snapshot: Option<Vec<FileTreeNode>>,
    pub git_snapshot: Option<Vec<WorkspaceGitEntry>>,
    pub deltas: Vec<WorkspaceDeltaEnvelope>,
    pub degraded: bool,
    pub is_git_repo: bool,
}

struct WorkspaceStateCore {
    root_path: String,
    seq: u64,
    tree_snapshot: Vec<FileTreeNode>,
    git_snapshot: Vec<WorkspaceGitEntry>,
    recent_events: VecDeque<WorkspaceDeltaEnvelope>,
    recent_capacity: usize,
    degraded: bool,
    is_git_repo: bool,
}

impl WorkspaceStateCore {
    fn new(
        root_path: String,
        tree_snapshot: Vec<FileTreeNode>,
        git_snapshot: Vec<WorkspaceGitEntry>,
        is_git_repo: bool,
    ) -> Self {
        Self {
            root_path,
            seq: 0,
            tree_snapshot,
            git_snapshot,
            recent_events: VecDeque::new(),
            recent_capacity: RECENT_EVENT_CAPACITY,
            degraded: false,
            is_git_repo,
        }
    }

    fn append_event(
        &mut self,
        kind: String,
        payload: Vec<WorkspaceDelta>,
        requires_resync: bool,
    ) -> WorkspaceStateEvent {
        self.seq += 1;

        if !requires_resync {
            self.apply_payload(&payload);
        }

        let envelope = WorkspaceDeltaEnvelope {
            seq: self.seq,
            kind: kind.clone(),
            payload: payload.clone(),
            requires_resync,
        };
        self.push_recent_event(envelope);

        WorkspaceStateEvent {
            root_path: self.root_path.clone(),
            seq: self.seq,
            version: WORKSPACE_STATE_PROTOCOL_VERSION,
            kind,
            payload,
            requires_resync,
        }
    }

    fn snapshot(&self, since_seq: Option<u64>) -> WorkspaceSnapshotResponse {
        if let Some(since) = since_seq {
            if self.can_replay_from(since) {
                let deltas = self
                    .recent_events
                    .iter()
                    .filter(|event| event.seq > since)
                    .cloned()
                    .collect::<Vec<_>>();

                return WorkspaceSnapshotResponse {
                    root_path: self.root_path.clone(),
                    seq: self.seq,
                    version: WORKSPACE_STATE_PROTOCOL_VERSION,
                    full: false,
                    tree_snapshot: None,
                    git_snapshot: None,
                    deltas,
                    degraded: self.degraded,
                    is_git_repo: self.is_git_repo,
                };
            }
        }

        WorkspaceSnapshotResponse {
            root_path: self.root_path.clone(),
            seq: self.seq,
            version: WORKSPACE_STATE_PROTOCOL_VERSION,
            full: true,
            tree_snapshot: Some(self.tree_snapshot.clone()),
            git_snapshot: Some(self.git_snapshot.clone()),
            deltas: Vec::new(),
            degraded: self.degraded,
            is_git_repo: self.is_git_repo,
        }
    }

    fn apply_payload(&mut self, payload: &[WorkspaceDelta]) {
        for delta in payload {
            match delta {
                WorkspaceDelta::TreeReplace { nodes } => {
                    self.tree_snapshot = nodes.clone();
                }
                WorkspaceDelta::GitReplace { entries } => {
                    self.git_snapshot = entries.clone();
                }
                WorkspaceDelta::Meta { .. } => {}
            }
        }
    }

    fn push_recent_event(&mut self, event: WorkspaceDeltaEnvelope) {
        // Tree replace events carry large payloads. Keeping a long history of
        // them can cause unnecessary memory growth on large workspaces.
        let has_tree_replace = event
            .payload
            .iter()
            .any(|delta| matches!(delta, WorkspaceDelta::TreeReplace { .. }));
        if has_tree_replace {
            self.recent_events.clear();
            self.recent_events.push_back(event);
            return;
        }

        self.recent_events.push_back(event);
        while self.recent_events.len() > self.recent_capacity {
            let _ = self.recent_events.pop_front();
        }
    }

    fn can_replay_from(&self, since_seq: u64) -> bool {
        if since_seq == self.seq {
            return true;
        }

        if since_seq > self.seq {
            return false;
        }

        let Some(first) = self.recent_events.front() else {
            return false;
        };

        let min_since = first.seq.saturating_sub(1);
        since_seq >= min_since
    }
}

struct WorkspaceStreamEntry {
    root_canonical: PathBuf,
    root_display: String,
    watcher: Option<RecommendedWatcher>,
    task: Option<tokio::task::JoinHandle<()>>,
    ref_count: usize,
    state: Arc<Mutex<WorkspaceStateCore>>,
}

static WORKSPACE_STREAMS: LazyLock<Mutex<HashMap<String, WorkspaceStreamEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
            if self.changed_paths.len() > WATCH_MAX_CHANGED_PATHS {
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

    fn kind(&self, root_canonical: &Path) -> String {
        let has_missing_path = !self.has_remove
            && !self.overflowed
            && self
                .changed_paths
                .iter()
                .any(|p| !root_canonical.join(p).exists());

        if self.has_remove || has_missing_path {
            "remove".to_string()
        } else if self.has_create {
            "create".to_string()
        } else {
            "modify".to_string()
        }
    }

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

fn canonicalize_watch_root(root: &Path) -> Result<(PathBuf, String), AppCommandError> {
    let canonical = std::fs::canonicalize(root).map_err(|e| {
        AppCommandError::not_found("Unable to resolve workspace root").with_detail(e.to_string())
    })?;
    let key = normalize_slash_path(&canonical);
    Ok((canonical, key))
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

    let mut child = crate::process::std_command("git")
        .args(["check-ignore", "--stdin", "-z"])
        .current_dir(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(AppCommandError::io)?;

    if let Some(mut stdin) = child.stdin.take() {
        for path in paths {
            stdin.write_all(path.as_bytes()).map_err(AppCommandError::io)?;
            stdin.write_all(&[0]).map_err(AppCommandError::io)?;
        }
    }

    let output = child.wait_with_output().map_err(AppCommandError::io)?;

    // Exit code 1 means "no matches", which is expected.
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(AppCommandError::external_command(
            "git check-ignore failed",
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
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

async fn should_refresh_git_status_for_paths(root_display: &str, changed_paths: &[String]) -> bool {
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

    let repo_path = root_display.to_string();
    let candidates_for_check = candidates.clone();
    let ignored = match tokio::task::spawn_blocking(move || {
        git_check_ignored_paths(&repo_path, &candidates_for_check)
    })
    .await
    {
        Ok(Ok(ignored)) => ignored,
        // Fail safe: if detection fails, keep current behavior and refresh status.
        _ => return true,
    };

    candidates
        .iter()
        .any(|path| !ignored.contains(path.as_str()))
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

fn normalize_git_status_path(path: &str) -> String {
    let normalized = path.trim().replace('\\', "/");
    if let Some(index) = normalized.rfind(" -> ") {
        return normalized[index + 4..]
            .trim()
            .trim_end_matches('/')
            .to_string();
    }
    normalized.trim_end_matches('/').to_string()
}

fn normalize_numstat_path(path: &str) -> String {
    let trimmed = path.trim().replace('\\', "/");

    if let (Some(open), Some(close)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if open < close {
            let prefix = &trimmed[..open];
            let suffix = &trimmed[close + 1..];
            let inner = &trimmed[open + 1..close];
            if let Some(idx) = inner.find(" => ") {
                let right = &inner[idx + 4..];
                return format!("{prefix}{right}{suffix}");
            }
        }
    }

    if let Some(index) = trimmed.rfind(" => ") {
        return trimmed[index + 4..].to_string();
    }

    trimmed
}

fn parse_numstat_value(raw: &str) -> i32 {
    raw.trim().parse::<i32>().unwrap_or(0)
}

async fn git_numstat_map(path: &str) -> HashMap<String, (i32, i32)> {
    async fn run_numstat(path: &str, args: &[&str]) -> Option<HashMap<String, (i32, i32)>> {
        let output = crate::process::tokio_command("git")
            .args(args)
            .current_dir(path)
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut map = HashMap::new();
        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let mut parts = line.splitn(3, '\t');
            let Some(add_raw) = parts.next() else {
                continue;
            };
            let Some(del_raw) = parts.next() else {
                continue;
            };
            let Some(path_raw) = parts.next() else {
                continue;
            };
            let parsed_path = normalize_numstat_path(path_raw);
            if parsed_path.is_empty() {
                continue;
            }
            map.insert(
                parsed_path,
                (parse_numstat_value(add_raw), parse_numstat_value(del_raw)),
            );
        }

        Some(map)
    }

    if let Some(map) = run_numstat(path, &["diff", "--numstat", "HEAD"]).await {
        return map;
    }

    run_numstat(path, &["diff", "--numstat", "--cached"])
        .await
        .unwrap_or_default()
}

fn is_git_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

async fn collect_git_snapshot(path: &str) -> Result<Vec<WorkspaceGitEntry>, AppCommandError> {
    let status_entries = folders::git_status(path.to_string(), Some(true)).await?;

    let stats = git_numstat_map(path).await;

    let mut result = status_entries
        .into_iter()
        .filter_map(|entry| {
            let normalized_path = normalize_git_status_path(&entry.file);
            if normalized_path.is_empty() {
                return None;
            }
            let (additions, deletions) = stats.get(&normalized_path).cloned().unwrap_or((0, 0));
            Some(WorkspaceGitEntry {
                path: normalized_path,
                status: entry.status,
                additions,
                deletions,
            })
        })
        .collect::<Vec<_>>();

    result.sort_by(|a, b| {
        a.path
            .to_lowercase()
            .cmp(&b.path.to_lowercase())
            .then(a.path.cmp(&b.path))
    });

    Ok(result)
}

async fn flush_watch_batch(
    state: &Arc<Mutex<WorkspaceStateCore>>,
    emitter: &EventEmitter,
    root_display: &str,
    root_canonical: &Path,
    batch: &WatchEventBatch,
) {
    if batch.is_empty() {
        return;
    }

    let event_kind_hint = batch.kind(root_canonical);
    let changed_paths = if batch.overflowed {
        Vec::new()
    } else {
        let mut paths = batch.changed_paths.iter().cloned().collect::<Vec<_>>();
        paths.sort();
        paths
    };

    let should_refresh_tree = batch.overflowed || event_kind_hint != "modify";
    let is_git = is_git_repo(root_canonical);
    let should_refresh_git = is_git
        && (batch.overflowed
            || should_refresh_git_status_for_paths(root_display, &changed_paths).await);

    let mut payload = Vec::new();
    let mut refreshed_tree: Option<Vec<FileTreeNode>> = None;
    let mut refreshed_git: Option<Vec<WorkspaceGitEntry>> = None;

    // Refresh failures are logged and silently skipped. Emitting a
    // `resync_hint` on every failure creates a feedback loop when the
    // failure is persistent (e.g. tree enum hits a permission-denied
    // subdir, git is unreachable), because the frontend would re-fetch
    // the same stored resync_hint event on every watch tick.
    if should_refresh_tree {
        match folders::get_file_tree(root_display.to_string(), Some(WORKSPACE_TREE_MAX_DEPTH)).await
        {
            Ok(tree) => refreshed_tree = Some(tree),
            Err(err) => eprintln!(
                "[workspace-state-watch] tree refresh failed for {}: {}",
                root_display, err
            ),
        }
    }

    if should_refresh_git {
        match collect_git_snapshot(root_display).await {
            Ok(git_snapshot) => refreshed_git = Some(git_snapshot),
            Err(err) => eprintln!(
                "[workspace-state-watch] git refresh failed for {}: {}",
                root_display, err
            ),
        }
    }

    let event = {
        let mut guard = match state.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        // Keep the cached git-presence flag in sync with the filesystem.
        // When it flips, the snapshot response carries the new value, and the
        // emitted event carries `requires_resync=true` so the frontend re-fetches
        // to align its isGitRepo view.
        let git_presence_changed = guard.is_git_repo != is_git;
        if git_presence_changed {
            guard.is_git_repo = is_git;
        }

        if let Some(tree) = refreshed_tree {
            if tree != guard.tree_snapshot {
                payload.push(WorkspaceDelta::TreeReplace { nodes: tree });
            }
        }
        if let Some(git_snapshot) = refreshed_git {
            if git_snapshot != guard.git_snapshot {
                payload.push(WorkspaceDelta::GitReplace {
                    entries: git_snapshot,
                });
            }
        } else if !is_git && !guard.git_snapshot.is_empty() {
            // .git vanished (or was never there) and we still hold stale git
            // data — emit an empty GitReplace so the UI stops showing tracked
            // files that no longer exist from git's perspective.
            payload.push(WorkspaceDelta::GitReplace {
                entries: Vec::new(),
            });
        }

        // Presence flip with no data delta (e.g. `git init` in a clean folder)
        // still needs to wake the frontend, otherwise the snapshot flag never
        // propagates until an unrelated change happens.
        if git_presence_changed && payload.is_empty() {
            payload.push(WorkspaceDelta::Meta {
                reason: format!("is_git_repo_changed:{is_git}"),
            });
        }

        if payload.is_empty() {
            return;
        }

        let kind = if payload
            .iter()
            .any(|delta| matches!(delta, WorkspaceDelta::TreeReplace { .. }))
        {
            "fs_delta".to_string()
        } else if payload
            .iter()
            .any(|delta| matches!(delta, WorkspaceDelta::GitReplace { .. }))
        {
            "git_delta".to_string()
        } else {
            "meta".to_string()
        };

        guard.append_event(kind, payload, git_presence_changed)
    };

    emit_event(emitter, "folder://workspace-state-event", event);
}

async fn run_workspace_watch_event_loop(
    mut event_rx: mpsc::Receiver<notify::Event>,
    dropped_events: Arc<AtomicBool>,
    state: Arc<Mutex<WorkspaceStateCore>>,
    emitter: EventEmitter,
    root_display: String,
    root_canonical: PathBuf,
) {
    let debounce = Duration::from_millis(WATCH_DEBOUNCE_MS);
    let max_batch_window = Duration::from_millis(WATCH_MAX_BATCH_WINDOW_MS);
    let mut batch = WatchEventBatch::default();
    let mut batch_started_at: Option<Instant> = None;

    loop {
        if dropped_events.swap(false, Ordering::AcqRel) {
            batch.overflowed = true;
            if batch_started_at.is_none() {
                batch_started_at = Some(Instant::now());
            }
        }

        if batch.is_empty() {
            match event_rx.recv().await {
                Some(event) => {
                    batch.ingest_event(&root_canonical, event);
                    if !batch.is_empty() {
                        batch_started_at = Some(Instant::now());
                    }
                }
                None => break,
            }
        } else {
            match tokio::time::timeout(debounce, event_rx.recv()).await {
                Ok(Some(event)) => {
                    batch.ingest_event(&root_canonical, event);
                }
                Ok(None) => {
                    flush_watch_batch(&state, &emitter, &root_display, &root_canonical, &batch)
                        .await;
                    break;
                }
                Err(_) => {
                    flush_watch_batch(&state, &emitter, &root_display, &root_canonical, &batch)
                        .await;
                    batch.clear();
                    batch_started_at = None;
                    continue;
                }
            }
        }

        while let Ok(next_event) = event_rx.try_recv() {
            batch.ingest_event(&root_canonical, next_event);
        }

        if dropped_events.swap(false, Ordering::AcqRel) {
            batch.overflowed = true;
            if batch_started_at.is_none() {
                batch_started_at = Some(Instant::now());
            }
        }

        let should_flush = batch_started_at
            .map(|started| started.elapsed() >= max_batch_window)
            .unwrap_or(false);

        if should_flush {
            flush_watch_batch(&state, &emitter, &root_display, &root_canonical, &batch).await;
            batch.clear();
            batch_started_at = None;
        }
    }

    if !batch.is_empty() {
        flush_watch_batch(&state, &emitter, &root_display, &root_canonical, &batch).await;
    }
}

pub async fn start_workspace_state_stream_core(
    emitter: EventEmitter,
    root_path: String,
) -> Result<WorkspaceSnapshotResponse, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let (root_canonical, key) = canonicalize_watch_root(&root)?;

    {
        let mut streams = WORKSPACE_STREAMS.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock workspace stream registry")
        })?;
        if let Some(entry) = streams.get_mut(&key) {
            entry.ref_count += 1;
            let snapshot = entry.state.lock().map_err(|_| {
                AppCommandError::task_execution_failed("Failed to lock workspace state snapshot")
            })?;
            return Ok(snapshot.snapshot(None));
        }
    }

    let initial_tree = folders::get_file_tree(root_path.clone(), Some(WORKSPACE_TREE_MAX_DEPTH))
        .await
        .unwrap_or_default();
    let initial_is_git_repo = is_git_repo(&root_canonical);
    let initial_git = if initial_is_git_repo {
        collect_git_snapshot(&root_path).await.unwrap_or_default()
    } else {
        Vec::new()
    };

    let state = Arc::new(Mutex::new(WorkspaceStateCore::new(
        root_path.clone(),
        initial_tree,
        initial_git,
        initial_is_git_repo,
    )));

    let (event_tx, event_rx) = mpsc::channel::<notify::Event>(WATCH_EVENT_CHANNEL_CAPACITY);
    let dropped_events = Arc::new(AtomicBool::new(false));

    let state_for_task = Arc::clone(&state);
    let emitter_for_task = emitter.clone();
    let root_display_for_task = root_path.clone();
    let root_canonical_for_task = root_canonical.clone();
    let dropped_events_for_task = Arc::clone(&dropped_events);
    let mut task = Some(tokio::spawn(async move {
        run_workspace_watch_event_loop(
            event_rx,
            dropped_events_for_task,
            state_for_task,
            emitter_for_task,
            root_display_for_task,
            root_canonical_for_task,
        )
        .await;
    }));

    let root_display_for_error = root_path.clone();
    let dropped_events_for_callback = Arc::clone(&dropped_events);
    let mut watcher = Some(
        notify::recommended_watcher(
            move |result: Result<notify::Event, notify::Error>| match result {
                Ok(event) => {
                    match event_tx.try_send(event) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            dropped_events_for_callback.store(true, Ordering::Release);
                        }
                        Err(TrySendError::Closed(_)) => {}
                    }
                }
                Err(err) => {
                    eprintln!(
                        "[workspace-state-watch] failed event for {}: {}",
                        root_display_for_error, err
                    );
                }
            },
        )
        .map_err(|e| {
            AppCommandError::io_error("Failed to create workspace state watcher")
                .with_detail(e.to_string())
        })?,
    );

    let watch_result = watcher
        .as_mut()
        .ok_or_else(|| AppCommandError::task_execution_failed("Failed to create watcher"))?
        .watch(&root_canonical, RecursiveMode::Recursive);

    if let Err(err) = watch_result {
        eprintln!(
            "[workspace-state-watch] degraded (no realtime updates) for {}: {}",
            root_path, err
        );
        if let Some(mut created_watcher) = watcher.take() {
            let _ = created_watcher.unwatch(&root_canonical);
        }
        if let Some(created_task) = task.take() {
            created_task.abort();
        }
        if let Ok(mut guard) = state.lock() {
            guard.degraded = true;
        }
    }

    let (should_cleanup_new_stream, start_snapshot) = {
        let mut streams = WORKSPACE_STREAMS.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock workspace stream registry")
        })?;

        if let Some(entry) = streams.get_mut(&key) {
            entry.ref_count += 1;
            let snapshot = entry.state.lock().map_err(|_| {
                AppCommandError::task_execution_failed("Failed to lock workspace state snapshot")
            })?;
            (true, snapshot.snapshot(None))
        } else {
            let snapshot = state
                .lock()
                .map_err(|_| {
                    AppCommandError::task_execution_failed(
                        "Failed to lock workspace state snapshot",
                    )
                })?
                .snapshot(None);
            streams.insert(
                key,
                WorkspaceStreamEntry {
                    root_canonical: root_canonical.clone(),
                    root_display: root_path,
                    watcher: watcher.take(),
                    task: task.take(),
                    ref_count: 1,
                    state: Arc::clone(&state),
                },
            );
            (false, snapshot)
        }
    };

    if should_cleanup_new_stream {
        if let Some(mut created_watcher) = watcher.take() {
            let _ = created_watcher.unwatch(&root_canonical);
        }
        if let Some(created_task) = task.take() {
            created_task.abort();
        }
    }

    Ok(start_snapshot)
}

pub async fn stop_workspace_state_stream_core(root_path: String) -> Result<(), AppCommandError> {
    let root = PathBuf::from(&root_path);
    let key = canonicalize_watch_root(&root)
        .map(|(_, key)| key)
        .unwrap_or_else(|_| normalize_slash_path(&root));

    let mut streams = WORKSPACE_STREAMS.lock().map_err(|_| {
        AppCommandError::task_execution_failed("Failed to lock workspace stream registry")
    })?;

    let target_key = if streams.contains_key(&key) {
        Some(key)
    } else {
        streams.iter().find_map(|(candidate_key, entry)| {
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

    if let Some(entry) = streams.get_mut(&target_key) {
        if entry.ref_count > 1 {
            entry.ref_count -= 1;
            return Ok(());
        }
    }

    let mut removed_entry = streams.remove(&target_key);
    drop(streams);

    if let Some(mut entry) = removed_entry.take() {
        if let Some(mut watcher) = entry.watcher.take() {
            let _ = watcher.unwatch(&entry.root_canonical);
            drop(watcher);
        }
        if let Some(task) = entry.task.take() {
            task.abort();
        }
    }

    Ok(())
}

pub async fn get_workspace_snapshot_core(
    root_path: String,
    since_seq: Option<u64>,
) -> Result<WorkspaceSnapshotResponse, AppCommandError> {
    let root = PathBuf::from(&root_path);
    let key = canonicalize_watch_root(&root)
        .map(|(_, key)| key)
        .unwrap_or_else(|_| normalize_slash_path(&root));

    let state = {
        let streams = WORKSPACE_STREAMS.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock workspace stream registry")
        })?;

        let by_key = streams.get(&key).map(|entry| Arc::clone(&entry.state));
        if let Some(found) = by_key {
            found
        } else if let Some(found) = streams
            .values()
            .find(|entry| entry.root_display == root_path)
            .map(|entry| Arc::clone(&entry.state))
        {
            found
        } else {
            return Err(AppCommandError::not_found(
                "Workspace stream is not running for this root",
            ));
        }
    };

    let guard = state.lock().map_err(|_| {
        AppCommandError::task_execution_failed("Failed to lock workspace state snapshot")
    })?;

    Ok(guard.snapshot(since_seq))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_state_core_seq_is_monotonic() {
        let mut core = WorkspaceStateCore::new("/tmp/repo".to_string(), Vec::new(), Vec::new(), false);

        let e1 = core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "boot".to_string(),
            }],
            false,
        );

        let e2 = core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "tick".to_string(),
            }],
            false,
        );

        assert!(e2.seq > e1.seq);
    }

    #[test]
    fn workspace_state_core_snapshot_incremental_when_since_available() {
        let mut core = WorkspaceStateCore::new("/tmp/repo".to_string(), Vec::new(), Vec::new(), false);

        let e1 = core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "a".to_string(),
            }],
            false,
        );

        core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "b".to_string(),
            }],
            false,
        );

        let snapshot = core.snapshot(Some(e1.seq));
        assert!(!snapshot.full);
        assert_eq!(snapshot.deltas.len(), 1);
        assert!(snapshot.tree_snapshot.is_none());
        assert!(snapshot.git_snapshot.is_none());
    }

    #[test]
    fn workspace_state_core_snapshot_full_when_since_too_old() {
        let mut core = WorkspaceStateCore::new("/tmp/repo".to_string(), Vec::new(), Vec::new(), false);
        core.recent_capacity = 1;

        core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "a".to_string(),
            }],
            false,
        );
        core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "b".to_string(),
            }],
            false,
        );

        let snapshot = core.snapshot(Some(0));
        assert!(snapshot.full);
        assert!(snapshot.tree_snapshot.is_some());
        assert!(snapshot.git_snapshot.is_some());
    }
}
