use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use crate::app_error::AppCommandError;
use crate::commands::folders::{self, FileTreeNode};
use crate::git_repo::is_git_repo;
use crate::web::event_bridge::{emit_event, EventEmitter};

pub const WORKSPACE_STATE_PROTOCOL_VERSION: u16 = 1;

const WATCH_IGNORED_DIRS: &[&str] = &["__pycache__"];
const WATCH_DEBOUNCE_MS: u64 = 300;
const WATCH_MAX_BATCH_WINDOW_MS: u64 = 1_500;
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
    #[serde(default)]
    pub changed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceStateEvent {
    pub root_path: String,
    pub seq: u64,
    pub version: u16,
    pub kind: String,
    pub payload: Vec<WorkspaceDelta>,
    pub requires_resync: bool,
    #[serde(default)]
    pub changed_paths: Vec<String>,
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
    recent_events: VecDeque<Arc<WorkspaceDeltaEnvelope>>,
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
        changed_paths: Vec<String>,
    ) -> WorkspaceStateEvent {
        self.seq += 1;

        if !requires_resync {
            self.apply_payload(&payload);
        }

        let envelope = Arc::new(WorkspaceDeltaEnvelope {
            seq: self.seq,
            kind: kind.clone(),
            payload: payload.clone(),
            requires_resync,
            changed_paths: changed_paths.clone(),
        });
        self.push_recent_event(envelope);

        WorkspaceStateEvent {
            root_path: self.root_path.clone(),
            seq: self.seq,
            version: WORKSPACE_STATE_PROTOCOL_VERSION,
            kind,
            payload,
            requires_resync,
            changed_paths,
        }
    }

    fn snapshot(&self, since_seq: Option<u64>) -> WorkspaceSnapshotResponse {
        if let Some(since) = since_seq {
            if self.can_replay_from(since) {
                let deltas = self
                    .recent_events
                    .iter()
                    .filter(|event| event.seq > since)
                    .map(|event| (**event).clone())
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

    fn push_recent_event(&mut self, event: Arc<WorkspaceDeltaEnvelope>) {
        // Tree/Git replace deltas are idempotent full snapshots — keeping older
        // copies wastes memory and doesn't change replay outcomes. Strip the
        // same-kind deltas from earlier envelopes but preserve their seq slot
        // and `changed_paths` so strict seq continuity and lazy-load
        // invalidation still work.
        let has_tree_replace = event
            .payload
            .iter()
            .any(|delta| matches!(delta, WorkspaceDelta::TreeReplace { .. }));
        let has_git_replace = event
            .payload
            .iter()
            .any(|delta| matches!(delta, WorkspaceDelta::GitReplace { .. }));

        if has_tree_replace || has_git_replace {
            for slot in self.recent_events.iter_mut() {
                let needs_rewrite = slot.payload.iter().any(|delta| match delta {
                    WorkspaceDelta::TreeReplace { .. } => has_tree_replace,
                    WorkspaceDelta::GitReplace { .. } => has_git_replace,
                    WorkspaceDelta::Meta { .. } => false,
                });
                if !needs_rewrite {
                    continue;
                }
                let remaining: Vec<WorkspaceDelta> = slot
                    .payload
                    .iter()
                    .filter(|delta| match delta {
                        WorkspaceDelta::TreeReplace { .. } => !has_tree_replace,
                        WorkspaceDelta::GitReplace { .. } => !has_git_replace,
                        WorkspaceDelta::Meta { .. } => true,
                    })
                    .cloned()
                    .collect();
                *slot = Arc::new(WorkspaceDeltaEnvelope {
                    seq: slot.seq,
                    kind: slot.kind.clone(),
                    payload: remaining,
                    requires_resync: slot.requires_resync,
                    changed_paths: slot.changed_paths.clone(),
                });
            }
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
    // Number of subscribers that consume tree/git snapshots (aux file tree,
    // git panels). File-tab watchers subscribe paths-only: while this count
    // is zero the watch loop skips the per-batch tree walk and git status
    // scan entirely and emits `changed_paths`-only meta envelopes, so the
    // cost of watching N folders for open tabs scales with FS events, not
    // with repository size. Shared with the flush task via Arc.
    full_subscribers: Arc<AtomicUsize>,
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
        .arg("--no-optional-locks")
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

#[derive(Clone, Copy)]
struct GitignoreCacheEntry {
    ignored: bool,
    expires_at: Instant,
}

const GITIGNORE_CACHE_TTL: Duration = Duration::from_secs(30);
const GITIGNORE_CACHE_MAX_ENTRIES: usize = 4_096;

static GITIGNORE_CACHE: LazyLock<Mutex<HashMap<(String, String), GitignoreCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn gitignore_cache_lookup(root: &str, path: &str) -> Option<bool> {
    let mut cache = GITIGNORE_CACHE.lock().ok()?;
    let key = (root.to_string(), path.to_string());
    let entry = *cache.get(&key)?;
    if entry.expires_at <= Instant::now() {
        cache.remove(&key);
        return None;
    }
    Some(entry.ignored)
}

fn gitignore_cache_put_batch(root: &str, results: impl IntoIterator<Item = (String, bool)>) {
    let Ok(mut cache) = GITIGNORE_CACHE.lock() else {
        return;
    };

    if cache.len() >= GITIGNORE_CACHE_MAX_ENTRIES {
        let mut sorted: Vec<_> = cache
            .iter()
            .map(|(k, v)| (k.clone(), v.expires_at))
            .collect();
        sorted.sort_by_key(|(_, exp)| *exp);
        let drop_count = cache.len() / 4;
        for (k, _) in sorted.into_iter().take(drop_count) {
            cache.remove(&k);
        }
    }

    let expires_at = Instant::now() + GITIGNORE_CACHE_TTL;
    for (path, ignored) in results {
        cache.insert(
            (root.to_string(), path),
            GitignoreCacheEntry {
                ignored,
                expires_at,
            },
        );
    }
}

fn gitignore_cache_invalidate_root(root: &str) {
    let Ok(mut cache) = GITIGNORE_CACHE.lock() else {
        return;
    };
    cache.retain(|(r, _), _| r != root);
}

async fn should_refresh_git_status_for_paths(root_display: &str, changed_paths: &[String]) -> bool {
    if changed_paths.is_empty() {
        return true;
    }

    let mut candidates: Vec<String> = Vec::new();
    for path in changed_paths {
        if is_git_metadata_rel_path(path) || is_gitignore_rel_path(path) {
            // `.gitignore` or `.git/*` changed — our ignore cache is likely
            // stale; drop it before returning.
            gitignore_cache_invalidate_root(root_display);
            return true;
        }
        candidates.push(path.clone());
    }

    if candidates.is_empty() {
        return false;
    }

    let mut missing: Vec<String> = Vec::new();
    for path in &candidates {
        match gitignore_cache_lookup(root_display, path) {
            Some(false) => return true, // cached non-ignored → must refresh
            Some(true) => {}
            None => missing.push(path.clone()),
        }
    }

    if missing.is_empty() {
        // All candidates were cached as ignored — nothing to refresh.
        return false;
    }

    let repo_path = root_display.to_string();
    let missing_for_check = missing.clone();
    let ignored = match tokio::task::spawn_blocking(move || {
        git_check_ignored_paths(&repo_path, &missing_for_check)
    })
    .await
    {
        Ok(Ok(ignored)) => ignored,
        // Fail safe: if detection fails, keep current behavior and refresh status.
        _ => return true,
    };

    let results: Vec<(String, bool)> = missing
        .iter()
        .map(|path| (path.clone(), ignored.contains(path.as_str())))
        .collect();
    let should_refresh = results.iter().any(|(_, is_ignored)| !is_ignored);
    gitignore_cache_put_batch(root_display, results);

    should_refresh
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
            .arg("--no-optional-locks")
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

async fn collect_git_snapshot(path: &str) -> Result<Vec<WorkspaceGitEntry>, AppCommandError> {
    // status + numstat don't depend on each other; run concurrently to cut
    // per-flush latency roughly in half on large repos.
    let (status_entries, stats) = tokio::join!(
        folders::git_status(path.to_string(), Some(true)),
        git_numstat_map(path),
    );
    let status_entries = status_entries?;

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
    full_subscribers: &AtomicUsize,
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

    // Paths-only lite mode: with no tree/git subscriber on this root, the
    // batch costs nothing beyond the (already-debounced) FS events — no
    // tree walk, no `git status`, no check-ignore subprocess. Subscribers
    // that only track open file tabs still receive the `changed_paths`
    // meta envelope below. Read per batch so a full subscriber joining
    // mid-stream upgrades the very next batch.
    let wants_tree_git = full_subscribers.load(Ordering::Acquire) > 0;

    let should_refresh_tree =
        wants_tree_git && (batch.overflowed || event_kind_hint != "modify");
    let is_git = wants_tree_git && is_git_repo(root_canonical);
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
            Err(err) => tracing::error!(
                "[workspace-state-watch] tree refresh failed for {}: {}",
                root_display, err
            ),
        }
    }

    if should_refresh_git {
        match collect_git_snapshot(root_display).await {
            Ok(git_snapshot) => refreshed_git = Some(git_snapshot),
            Err(err) => tracing::error!(
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
        // to align its isGitRepo view. Skipped entirely in paths-only mode:
        // `is_git` was not computed there, and no subscriber consumes the flag.
        let git_presence_changed = wants_tree_git && guard.is_git_repo != is_git;
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
        } else if wants_tree_git && !is_git && !guard.git_snapshot.is_empty() {
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

        // Surface FS activity that doesn't otherwise change tree/git snapshots
        // (e.g. files added/removed in a directory beyond WORKSPACE_TREE_MAX_DEPTH,
        // gitignored / non-git-repo changes, or ANY activity in paths-only
        // mode). The envelope's `changed_paths` lets the frontend invalidate
        // lazy-loaded overrides and reconcile open file tabs. An overflowed
        // batch must also get through with EMPTY changed_paths — consumers
        // treat that as "cannot enumerate, sweep everything".
        if payload.is_empty() && (!changed_paths.is_empty() || batch.overflowed) {
            payload.push(WorkspaceDelta::Meta {
                reason: "fs_events".to_string(),
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

        guard.append_event(kind, payload, git_presence_changed, changed_paths)
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
    full_subscribers: Arc<AtomicUsize>,
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
                    flush_watch_batch(
                        &state,
                        &emitter,
                        &root_display,
                        &root_canonical,
                        &full_subscribers,
                        &batch,
                    )
                    .await;
                    break;
                }
                Err(_) => {
                    flush_watch_batch(
                        &state,
                        &emitter,
                        &root_display,
                        &root_canonical,
                        &full_subscribers,
                        &batch,
                    )
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
            flush_watch_batch(
                &state,
                &emitter,
                &root_display,
                &root_canonical,
                &full_subscribers,
                &batch,
            )
            .await;
            batch.clear();
            batch_started_at = None;
        }
    }

    if !batch.is_empty() {
        flush_watch_batch(
            &state,
            &emitter,
            &root_display,
            &root_canonical,
            &full_subscribers,
            &batch,
        )
        .await;
    }
}

// Refresh the tree/git snapshots of an already-running stream and broadcast
// the resulting deltas. Used when the first full (tree/git) subscriber joins
// a stream that was seeded paths-only: its cached snapshots are empty/stale
// because every batch so far skipped scanning.
async fn refresh_tree_git_snapshots(
    state: &Arc<Mutex<WorkspaceStateCore>>,
    emitter: &EventEmitter,
    root_display: &str,
    root_canonical: &Path,
) {
    let refreshed_tree =
        match folders::get_file_tree(root_display.to_string(), Some(WORKSPACE_TREE_MAX_DEPTH))
            .await
        {
            Ok(tree) => Some(tree),
            Err(err) => {
                tracing::error!(
                    "[workspace-state-watch] upgrade tree refresh failed for {}: {}",
                    root_display, err
                );
                None
            }
        };
    let is_git = is_git_repo(root_canonical);
    let refreshed_git = if is_git {
        collect_git_snapshot(root_display).await.ok()
    } else {
        Some(Vec::new())
    };

    let event = {
        let mut guard = match state.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        let mut payload = Vec::new();
        if guard.is_git_repo != is_git {
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
        }
        if payload.is_empty() {
            return;
        }
        let kind = if payload
            .iter()
            .any(|delta| matches!(delta, WorkspaceDelta::TreeReplace { .. }))
        {
            "fs_delta".to_string()
        } else {
            "git_delta".to_string()
        };
        guard.append_event(kind, payload, false, Vec::new())
    };

    emit_event(emitter, "folder://workspace-state-event", event);
}

pub async fn start_workspace_state_stream_core(
    emitter: EventEmitter,
    root_path: String,
    wants_tree_git: bool,
) -> Result<WorkspaceSnapshotResponse, AppCommandError> {
    let root = PathBuf::from(&root_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let (root_canonical, key) = canonicalize_watch_root(&root)?;

    // Existing stream: bump refcounts; the FIRST full subscriber on a
    // paths-seeded stream triggers a one-time tree/git snapshot refresh
    // (outside the registry lock) so its returned snapshot is fresh.
    let existing_upgrade = {
        let mut streams = WORKSPACE_STREAMS.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock workspace stream registry")
        })?;
        if let Some(entry) = streams.get_mut(&key) {
            entry.ref_count += 1;
            let became_full = wants_tree_git
                && entry.full_subscribers.fetch_add(1, Ordering::AcqRel) == 0;
            if !became_full {
                let snapshot = entry.state.lock().map_err(|_| {
                    AppCommandError::task_execution_failed(
                        "Failed to lock workspace state snapshot",
                    )
                })?;
                return Ok(snapshot.snapshot(None));
            }
            Some((Arc::clone(&entry.state), entry.root_display.clone()))
        } else {
            None
        }
    };

    if let Some((existing_state, root_display)) = existing_upgrade {
        refresh_tree_git_snapshots(&existing_state, &emitter, &root_display, &root_canonical)
            .await;
        let snapshot = existing_state.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock workspace state snapshot")
        })?;
        return Ok(snapshot.snapshot(None));
    }

    // Seeding scans are what make cold start expensive on big repos — a
    // paths-only stream (file-tab watching) skips them entirely and holds
    // empty tree/git snapshots until a full subscriber upgrades it.
    let initial_is_git_repo = is_git_repo(&root_canonical);
    let initial_tree = if wants_tree_git {
        folders::get_file_tree(root_path.clone(), Some(WORKSPACE_TREE_MAX_DEPTH))
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let initial_git = if wants_tree_git && initial_is_git_repo {
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
    let full_subscribers = Arc::new(AtomicUsize::new(usize::from(wants_tree_git)));

    let state_for_task = Arc::clone(&state);
    let emitter_for_task = emitter.clone();
    let root_display_for_task = root_path.clone();
    let root_canonical_for_task = root_canonical.clone();
    let dropped_events_for_task = Arc::clone(&dropped_events);
    let full_subscribers_for_task = Arc::clone(&full_subscribers);
    let mut task = Some(tokio::spawn(async move {
        run_workspace_watch_event_loop(
            event_rx,
            dropped_events_for_task,
            state_for_task,
            emitter_for_task,
            root_display_for_task,
            root_canonical_for_task,
            full_subscribers_for_task,
        )
        .await;
    }));

    let root_display_for_error = root_path.clone();
    let dropped_events_for_callback = Arc::clone(&dropped_events);
    let mut watcher = Some(
        notify::recommended_watcher(
            move |result: Result<notify::Event, notify::Error>| match result {
                Ok(event) => match event_tx.try_send(event) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        dropped_events_for_callback.store(true, Ordering::Release);
                    }
                    Err(TrySendError::Closed(_)) => {}
                },
                Err(err) => {
                    tracing::error!(
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
        tracing::info!(
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

    let (should_cleanup_new_stream, start_snapshot, lost_race_upgrade) = {
        let mut streams = WORKSPACE_STREAMS.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock workspace stream registry")
        })?;

        if let Some(entry) = streams.get_mut(&key) {
            // Lost an insert race. Fold this subscription into the winner —
            // including its full-subscriber count. If the winner was seeded
            // paths-only and WE are its first full subscriber, the same
            // upgrade contract as the fast path applies: refresh tree/git
            // (outside the lock, below) so this response carries fresh
            // snapshots instead of the winner's empty paths-only seed.
            entry.ref_count += 1;
            let became_full = wants_tree_git
                && entry.full_subscribers.fetch_add(1, Ordering::AcqRel) == 0;
            let upgrade = if became_full {
                Some((Arc::clone(&entry.state), entry.root_display.clone()))
            } else {
                None
            };
            let snapshot = entry.state.lock().map_err(|_| {
                AppCommandError::task_execution_failed("Failed to lock workspace state snapshot")
            })?;
            (true, snapshot.snapshot(None), upgrade)
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
                    full_subscribers: Arc::clone(&full_subscribers),
                    state: Arc::clone(&state),
                },
            );
            (false, snapshot, None)
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

    if let Some((winner_state, winner_display)) = lost_race_upgrade {
        refresh_tree_git_snapshots(&winner_state, &emitter, &winner_display, &root_canonical)
            .await;
        let snapshot = winner_state
            .lock()
            .map_err(|_| {
                AppCommandError::task_execution_failed("Failed to lock workspace state snapshot")
            })?
            .snapshot(None);
        return Ok(snapshot);
    }

    Ok(start_snapshot)
}

pub async fn stop_workspace_state_stream_core(
    root_path: String,
    wants_tree_git: bool,
) -> Result<(), AppCommandError> {
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
        // Floor at zero: a mismatched stop (e.g. a client that crashed
        // between start and stop bookkeeping) must not underflow and wedge
        // the stream in permanent full-scan mode.
        if wants_tree_git {
            let _ = entry.full_subscribers.fetch_update(
                Ordering::AcqRel,
                Ordering::Acquire,
                |count| count.checked_sub(1),
            );
        }
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

    let (state, root_display, root_canonical, full_subscribers) = {
        let streams = WORKSPACE_STREAMS.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock workspace stream registry")
        })?;

        let by_key = streams.get(&key).map(|entry| {
            (
                Arc::clone(&entry.state),
                entry.root_display.clone(),
                entry.root_canonical.clone(),
                Arc::clone(&entry.full_subscribers),
            )
        });
        if let Some(found) = by_key {
            found
        } else if let Some(found) = streams
            .values()
            .find(|entry| entry.root_display == root_path)
            .map(|entry| {
                (
                    Arc::clone(&entry.state),
                    entry.root_display.clone(),
                    entry.root_canonical.clone(),
                    Arc::clone(&entry.full_subscribers),
                )
            })
        {
            found
        } else {
            return Err(AppCommandError::not_found(
                "Workspace stream is not running for this root",
            ));
        }
    };

    // The frontend calls this endpoint after every write (delete / upload /
    // create / rename) via `fetchTree`. The FS watcher debounces 300ms after
    // each event, so the cached snapshots are reliably stale the moment we're
    // polled — and if a notify event is ever missed (macOS FSEvents drops
    // under load, SSE reconnects, host suspends/resumes), the cache stays stale
    // until the next unrelated FS change comes in. Re-scan disk here and
    // append deltas if either tree or git diverges, so the client request
    // itself catches up the state instead of waiting on the watcher.
    //
    // Paths-only streams skip the whole re-scan: nobody consumes tree/git
    // there, and a resync (e.g. the store re-acquiring within its shutdown
    // grace) must stay as cheap as the stream itself.
    let wants_tree_git = full_subscribers.load(Ordering::Acquire) > 0;
    let is_git = wants_tree_git && is_git_repo(&root_canonical);
    let (refreshed_tree, refreshed_git) = if wants_tree_git {
        let tree_fut =
            folders::get_file_tree(root_display.clone(), Some(WORKSPACE_TREE_MAX_DEPTH));
        let git_fut = async {
            if is_git {
                collect_git_snapshot(&root_display).await.ok()
            } else {
                None
            }
        };
        let (tree_result, git_result) = tokio::join!(tree_fut, git_fut);
        (tree_result.ok(), git_result)
    } else {
        (None, None)
    };

    let guard_snapshot = {
        let mut guard = state.lock().map_err(|_| {
            AppCommandError::task_execution_failed("Failed to lock workspace state snapshot")
        })?;

        let mut payload: Vec<WorkspaceDelta> = Vec::new();
        if let Some(tree) = refreshed_tree {
            if tree != guard.tree_snapshot {
                payload.push(WorkspaceDelta::TreeReplace { nodes: tree });
            }
        }
        if let Some(git) = refreshed_git {
            if git != guard.git_snapshot {
                payload.push(WorkspaceDelta::GitReplace { entries: git });
            }
        } else if wants_tree_git && !is_git && !guard.git_snapshot.is_empty() {
            // .git vanished while we weren't watching — clear the stale entries
            // so the UI stops showing tracked files that no longer exist from
            // git's perspective.
            payload.push(WorkspaceDelta::GitReplace {
                entries: Vec::new(),
            });
        }

        let git_presence_changed = wants_tree_git && guard.is_git_repo != is_git;
        if git_presence_changed {
            guard.is_git_repo = is_git;
            if payload.is_empty() {
                payload.push(WorkspaceDelta::Meta {
                    reason: format!("is_git_repo_changed:{is_git}"),
                });
            }
        }

        if !payload.is_empty() {
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
            guard.append_event(kind, payload, false, Vec::new());
        }

        guard.snapshot(since_seq)
    };

    Ok(guard_snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests assert against the state core (seq / recent events /
    // snapshots), not the transport, so the silent emitter suffices.
    fn test_emitter() -> EventEmitter {
        EventEmitter::Noop
    }

    fn batch_with_paths(paths: &[&str]) -> WatchEventBatch {
        let mut batch = WatchEventBatch::default();
        for path in paths {
            batch.changed_paths.insert((*path).to_string());
        }
        batch
    }

    #[tokio::test]
    async fn flush_watch_batch_paths_only_skips_scans_but_emits_changed_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"hello").expect("write file");
        let root_display = dir.path().to_string_lossy().to_string();

        let state = Arc::new(Mutex::new(WorkspaceStateCore::new(
            root_display.clone(),
            Vec::new(),
            Vec::new(),
            false,
        )));
        let full_subscribers = AtomicUsize::new(0);
        let batch = batch_with_paths(&["a.txt"]);

        flush_watch_batch(
            &state,
            &test_emitter(),
            &root_display,
            dir.path(),
            &full_subscribers,
            &batch,
        )
        .await;

        let guard = state.lock().expect("state lock");
        assert_eq!(guard.seq, 1, "changed_paths envelope must still be issued");
        let event = guard.recent_events.back().expect("recent event");
        assert_eq!(event.kind, "meta");
        assert_eq!(event.changed_paths, vec!["a.txt".to_string()]);
        assert!(matches!(
            event.payload.as_slice(),
            [WorkspaceDelta::Meta { .. }]
        ));
        // The load-bearing assertion: the on-disk file exists, but with no
        // full subscriber the tree walk never ran, so the snapshot stays
        // empty instead of picking it up.
        assert!(
            guard.tree_snapshot.is_empty(),
            "paths-only flush must not run the tree scan"
        );
    }

    #[tokio::test]
    async fn flush_watch_batch_with_full_subscriber_refreshes_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"hello").expect("write file");
        let root_display = dir.path().to_string_lossy().to_string();

        let state = Arc::new(Mutex::new(WorkspaceStateCore::new(
            root_display.clone(),
            Vec::new(),
            Vec::new(),
            false,
        )));
        let full_subscribers = AtomicUsize::new(1);
        // A create event forces the tree-refresh path (kind != "modify").
        let mut batch = batch_with_paths(&["a.txt"]);
        batch.has_create = true;

        flush_watch_batch(
            &state,
            &test_emitter(),
            &root_display,
            dir.path(),
            &full_subscribers,
            &batch,
        )
        .await;

        let guard = state.lock().expect("state lock");
        assert!(
            !guard.tree_snapshot.is_empty(),
            "full mode must refresh the tree snapshot"
        );
        let event = guard.recent_events.back().expect("recent event");
        assert_eq!(event.changed_paths, vec!["a.txt".to_string()]);
        assert!(event
            .payload
            .iter()
            .any(|delta| matches!(delta, WorkspaceDelta::TreeReplace { .. })));
    }

    #[tokio::test]
    async fn flush_watch_batch_paths_only_overflow_still_emits_sweep_envelope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root_display = dir.path().to_string_lossy().to_string();

        let state = Arc::new(Mutex::new(WorkspaceStateCore::new(
            root_display.clone(),
            Vec::new(),
            Vec::new(),
            false,
        )));
        let full_subscribers = AtomicUsize::new(0);
        let batch = WatchEventBatch {
            overflowed: true,
            ..Default::default()
        };

        flush_watch_batch(
            &state,
            &test_emitter(),
            &root_display,
            dir.path(),
            &full_subscribers,
            &batch,
        )
        .await;

        let guard = state.lock().expect("state lock");
        assert_eq!(guard.seq, 1);
        let event = guard.recent_events.back().expect("recent event");
        // Empty changed_paths = "cannot enumerate, sweep everything" for
        // tab-watching consumers; the envelope must not be swallowed.
        assert!(event.changed_paths.is_empty());
        assert!(matches!(
            event.payload.as_slice(),
            [WorkspaceDelta::Meta { .. }]
        ));
    }

    #[tokio::test]
    async fn start_stream_paths_then_full_returns_freshly_scanned_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"hello").expect("write file");
        let root = dir.path().to_string_lossy().to_string();

        // Paths-only cold start: seeding scans are skipped entirely.
        let paths_snapshot =
            start_workspace_state_stream_core(test_emitter(), root.clone(), false)
                .await
                .expect("paths start");
        assert!(
            paths_snapshot.tree_snapshot.unwrap_or_default().is_empty(),
            "paths-only seed must not scan the tree"
        );

        // First full subscriber: the upgrade contract guarantees a freshly
        // scanned tree in the start response (not the empty paths seed).
        let full_snapshot =
            start_workspace_state_stream_core(test_emitter(), root.clone(), true)
                .await
                .expect("full start");
        assert!(
            !full_snapshot.tree_snapshot.unwrap_or_default().is_empty(),
            "first full subscriber must receive a refreshed tree snapshot"
        );

        stop_workspace_state_stream_core(root.clone(), true)
            .await
            .expect("stop full");
        stop_workspace_state_stream_core(root, false)
            .await
            .expect("stop paths");
    }

    #[test]
    fn workspace_state_core_seq_is_monotonic() {
        let mut core =
            WorkspaceStateCore::new("/tmp/repo".to_string(), Vec::new(), Vec::new(), false);

        let e1 = core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "boot".to_string(),
            }],
            false,
            Vec::new(),
        );

        let e2 = core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "tick".to_string(),
            }],
            false,
            Vec::new(),
        );

        assert!(e2.seq > e1.seq);
    }

    #[test]
    fn workspace_state_core_snapshot_incremental_when_since_available() {
        let mut core =
            WorkspaceStateCore::new("/tmp/repo".to_string(), Vec::new(), Vec::new(), false);

        let e1 = core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "a".to_string(),
            }],
            false,
            Vec::new(),
        );

        core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "b".to_string(),
            }],
            false,
            Vec::new(),
        );

        let snapshot = core.snapshot(Some(e1.seq));
        assert!(!snapshot.full);
        assert_eq!(snapshot.deltas.len(), 1);
        assert!(snapshot.tree_snapshot.is_none());
        assert!(snapshot.git_snapshot.is_none());
    }

    #[test]
    fn workspace_state_core_snapshot_full_when_since_too_old() {
        let mut core =
            WorkspaceStateCore::new("/tmp/repo".to_string(), Vec::new(), Vec::new(), false);
        core.recent_capacity = 1;

        core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "a".to_string(),
            }],
            false,
            Vec::new(),
        );
        core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "b".to_string(),
            }],
            false,
            Vec::new(),
        );

        let snapshot = core.snapshot(Some(0));
        assert!(snapshot.full);
        assert!(snapshot.tree_snapshot.is_some());
        assert!(snapshot.git_snapshot.is_some());
    }

    #[test]
    fn workspace_state_core_same_kind_replace_is_compressed() {
        let mut core =
            WorkspaceStateCore::new("/tmp/repo".to_string(), Vec::new(), Vec::new(), false);

        let e1 = core.append_event(
            "git_delta".to_string(),
            vec![WorkspaceDelta::GitReplace {
                entries: vec![WorkspaceGitEntry {
                    path: "a.txt".to_string(),
                    status: "M".to_string(),
                    additions: 1,
                    deletions: 0,
                }],
            }],
            false,
            vec!["a.txt".to_string()],
        );

        let e2 = core.append_event(
            "meta".to_string(),
            vec![WorkspaceDelta::Meta {
                reason: "tick".to_string(),
            }],
            false,
            Vec::new(),
        );

        core.append_event(
            "git_delta".to_string(),
            vec![WorkspaceDelta::GitReplace { entries: vec![] }],
            false,
            vec!["a.txt".to_string()],
        );

        let snapshot = core.snapshot(Some(0));
        assert!(!snapshot.full);
        assert_eq!(snapshot.deltas.len(), 3);

        let first = snapshot
            .deltas
            .iter()
            .find(|d| d.seq == e1.seq)
            .expect("e1 still present");
        assert!(
            first.payload.is_empty(),
            "older GitReplace payload should be dropped after compression"
        );
        assert_eq!(first.changed_paths, vec!["a.txt".to_string()]);

        let meta = snapshot
            .deltas
            .iter()
            .find(|d| d.seq == e2.seq)
            .expect("meta still present");
        assert_eq!(meta.payload.len(), 1);
    }

    #[test]
    fn workspace_state_core_tree_replace_compresses_older_tree_but_keeps_git() {
        let mut core =
            WorkspaceStateCore::new("/tmp/repo".to_string(), Vec::new(), Vec::new(), false);

        core.append_event(
            "fs_delta".to_string(),
            vec![WorkspaceDelta::TreeReplace { nodes: Vec::new() }],
            false,
            Vec::new(),
        );
        let git_event = core.append_event(
            "git_delta".to_string(),
            vec![WorkspaceDelta::GitReplace {
                entries: vec![WorkspaceGitEntry {
                    path: "b.txt".to_string(),
                    status: "??".to_string(),
                    additions: 0,
                    deletions: 0,
                }],
            }],
            false,
            Vec::new(),
        );
        core.append_event(
            "fs_delta".to_string(),
            vec![WorkspaceDelta::TreeReplace { nodes: Vec::new() }],
            false,
            Vec::new(),
        );

        let snapshot = core.snapshot(Some(0));
        assert!(!snapshot.full);
        assert_eq!(snapshot.deltas.len(), 3);

        let git_slot = snapshot
            .deltas
            .iter()
            .find(|d| d.seq == git_event.seq)
            .expect("git delta still present");
        assert!(matches!(
            git_slot.payload.as_slice(),
            [WorkspaceDelta::GitReplace { .. }]
        ));
    }
}
