//! Transcript-tail watcher: surfaces Claude Code's OUT-OF-TURN activity.
//!
//! Claude Code produces activity outside any codeg-driven prompt turn:
//! `<task-notification>` completions of async sub-agents (`Agent` launched in
//! the background) and background shell tasks, the agent's continued work
//! after such a notification (which can run for many minutes), and cron//loop
//! autonomous turns. None of that has a reliable ACP wire representation —
//! out-of-turn `session/update`s are forwarded but carry no turn settlement,
//! and cron turns produce NO wire events at all (claude-agent-acp #270). The
//! session's own JSONL transcript is the only complete source, so this module
//! tails it:
//!
//! * **Accounting** — launch acks are detected from the record-level
//!   `toolUseResult` (`status:"async_launched"` → `agentId`, or
//!   `backgroundTaskId` for background shells; these fields exist ONLY on
//!   disk, never on the wire), settled by any of: a `<task-notification>`
//!   record (matching `<task-id>`), a `TaskOutput` result whose structured
//!   `task.status` reached a terminal state, or a `TaskStop`/`KillShell`
//!   call. Background shells almost never emit a `<task-notification>` (they
//!   are collected inline via `TaskOutput` or just left running), so those
//!   two extra signals are what keep the count from stranding. Entries are
//!   re-armed when the main agent resumes a settled sub-agent via
//!   `SendMessage`, and expired past
//!   [`background_keepalive_max_age`]. The outstanding count is mirrored into
//!   `SessionState` (via `apply_event`) to exempt the connection from both
//!   idle sweeps — disconnecting kills the agent CLI, and the background work
//!   dies with it.
//!
//! * **Rendering** — new transcript records that do NOT belong to a
//!   codeg-sent prompt turn are assembled into turns with the SAME Stage-A/
//!   Stage-B code the detail parser uses ([`ClaudeRecordAccumulator`] +
//!   [`group_into_turns`]) and emitted as `AcpEvent::BackgroundActivity`
//!   upserts for the frontend's overlay slice. Foreground turns are excluded
//!   by the **prompt ledger**: every prompt codeg sends is fingerprinted, and
//!   a transcript turn whose initiating user record matches an unconsumed
//!   fingerprint is the wire-rendered foreground turn (each fingerprint is
//!   consumed exactly once, so a cron//loop re-fire of the SAME text later
//!   correctly classifies as out-of-turn).
//!
//! The watcher is connection-scoped on purpose: background work cannot outlive
//! the agent CLI process, whose lifetime IS the connection's. Poll ticks are
//! mtime-gated (an unchanged file costs one `stat`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::acp::session_state::{background_keepalive_max_age, SessionState};
use crate::acp::types::{AcpEvent, BackgroundSettledInfo, ConnectionStatus};
use crate::models::agent::AgentType;
use crate::models::message::MessageTurn;
use crate::parsers::claude::{
    capture_tag, find_session_file, group_into_turns, is_meta_message, slash_command_display,
    task_notification_result_regex, task_notification_status_regex, task_notification_summary_regex,
    task_notification_task_id_regex, task_notification_tool_use_id_regex, ClaudeRecordAccumulator,
    BACKGROUND_RESULT_MAX_CHARS, CONTEXT_CONTINUATION_PREFIX,
};
use crate::parsers::truncate_str;
use crate::web::event_bridge::{emit_with_state, EventEmitter};

/// Poll cadence while background work is outstanding or the transcript moved
/// recently — tight enough that a completed task surfaces within a beat.
const POLL_ACTIVE: Duration = Duration::from_secs(1);
/// Cadence when nothing is pending: cron//loop turns can still land at any
/// time, so the watch never stops — an unchanged file costs one `stat`.
const POLL_IDLE: Duration = Duration::from_secs(3);
/// How long after the last transcript growth the tight cadence is kept.
/// Sized to cover the agent's reaction to a settled task: after the
/// task-notification lands (last growth) the model may take 10–20s to write
/// its first reply block for a heavy synthesis (observed ~16s for a
/// two-agent digest) — dropping to the idle cadence inside that window adds
/// up to POLL_IDLE of avoidable surfacing latency. An unchanged file costs
/// one `stat` per tick, so the wider window is effectively free.
const RECENT_ACTIVITY_WINDOW: Duration = Duration::from_secs(30);
/// Prompt fingerprints older than this are dropped unconsumed — a rejected /
/// never-persisted prompt must not linger and swallow a later cron re-fire of
/// the same text.
const LEDGER_TTL: Duration = Duration::from_secs(600);
/// Max fingerprints kept (oldest evicted first). Far above any realistic
/// number of prompts in flight between transcript flushes.
const LEDGER_CAP: usize = 32;
/// Rotate the episode accumulator at the next out-of-turn boundary once it
/// holds this many messages, bounding per-tick regroup cost during very long
/// autonomous stretches. Already-emitted turns stay valid in the frontend
/// overlay; rotation only re-bases the id namespace for what follows.
const MAX_EPISODE_MESSAGES: usize = 512;
/// Absolute episode bound: a SINGLE autonomous turn can exceed
/// `MAX_EPISODE_MESSAGES` without ever hitting a boundary (a heavy /loop
/// iteration runs hundreds of tool calls in one turn), and every tick
/// clones + regroups + re-hashes the whole episode — unbounded, that's
/// O(n²) work over the turn. Past this valve the episode is force-rotated
/// mid-turn: the in-progress turn renders split across two overlay cards (a
/// visible seam, corrected by the next detail refetch) in exchange for a
/// hard cap on per-tick work. Double the boundary threshold so normal
/// boundary rotation always wins for multi-turn episodes.
const FORCE_ROTATE_MESSAGES: usize = MAX_EPISODE_MESSAGES * 2;

/// Fingerprints of prompts codeg itself sent on this connection, so the
/// watcher can tell wire-rendered foreground turns apart from out-of-turn
/// activity. Shared between the connection loop (writer, on every
/// `ConnectionCommand::Prompt`) and the watcher tick (consumer). A std mutex
/// is deliberate: both sides take it for microseconds and the watcher locks it
/// from a blocking context.
pub(crate) struct PromptLedger {
    entries: Mutex<VecDeque<LedgerEntry>>,
}

struct LedgerEntry {
    fingerprint: String,
    recorded_at: Instant,
}

impl PromptLedger {
    pub(crate) fn shared() -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(VecDeque::new()),
        })
    }

    /// Record the fingerprint of a prompt codeg is about to send: the first
    /// text block, trimmed. Attachment/resource blocks are excluded on
    /// purpose — the CLI may persist those differently, while the leading
    /// text lands verbatim at the start of the transcript's user record.
    pub(crate) fn record_prompt_blocks(&self, blocks: &[crate::acp::types::PromptInputBlock]) {
        let text = blocks.iter().find_map(|b| match b {
            crate::acp::types::PromptInputBlock::Text { text } => {
                let t = text.trim();
                (!t.is_empty()).then(|| t.to_string())
            }
            _ => None,
        });
        let Some(fingerprint) = text else {
            // A prompt with no text (image-only) can't be fingerprinted; its
            // turn will classify as out-of-turn and reconcile via refetch.
            tracing::debug!("[bg-watch] prompt without text block — no fingerprint recorded");
            return;
        };
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.push_back(LedgerEntry {
            fingerprint,
            recorded_at: Instant::now(),
        });
        while entries.len() > LEDGER_CAP {
            entries.pop_front();
        }
    }

    /// Match `initiator_text` (the transcript turn's initiating user text)
    /// against the unconsumed fingerprints; on match the entry is consumed —
    /// exactly once per sent prompt, so a later same-text autonomous re-fire
    /// finds no entry and classifies as out-of-turn. The record may carry
    /// appended wrapper content after the sent text, hence prefix matching.
    fn consume_matching(&self, initiator_text: &str) -> bool {
        let text = initiator_text.trim();
        if text.is_empty() {
            return false;
        }
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|e| e.recorded_at.elapsed() < LEDGER_TTL);
        if let Some(pos) = entries
            .iter()
            .position(|e| text == e.fingerprint || text.starts_with(e.fingerprint.as_str()))
        {
            entries.remove(pos);
            return true;
        }
        false
    }

    #[cfg(test)]
    fn record_text(&self, text: &str) {
        self.record_prompt_blocks(&[crate::acp::types::PromptInputBlock::Text {
            text: text.to_string(),
        }]);
    }
}

/// Aborts the watcher task when the owning conversation loop exits
/// (disconnect or fork restart — the restarted loop arms a fresh watcher).
pub(crate) struct BackgroundWatchGuard(tokio::task::JoinHandle<()>);

impl Drop for BackgroundWatchGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Arm the transcript watcher for a Claude connection; other agents have no
/// transcript-notification mechanism and get no watcher (returns `None`).
pub(crate) fn spawn_if_claude(
    conn_id: &str,
    agent_type: AgentType,
    state: Arc<RwLock<SessionState>>,
    emitter: EventEmitter,
    cwd: String,
    ledger: Arc<PromptLedger>,
) -> Option<BackgroundWatchGuard> {
    if agent_type != AgentType::ClaudeCode {
        return None;
    }
    let conn_id = conn_id.to_string();
    let handle = tokio::spawn(async move {
        run_watch(conn_id, state, emitter, cwd, ledger).await;
    });
    Some(BackgroundWatchGuard(handle))
}

async fn run_watch(
    conn_id: String,
    state: Arc<RwLock<SessionState>>,
    emitter: EventEmitter,
    cwd: String,
    ledger: Arc<PromptLedger>,
) {
    let mut ws = WatchState::new();
    // Wall-clock boundary between pre-existing transcript history (renders
    // via the detail fetch) and records that belong to THIS watch's lifetime.
    // Captured BEFORE the session is established: a NEW session's file is
    // created strictly after this instant, so every record in it — including
    // a first prompt or launch ack written before the watcher learns the
    // session id (SessionStarted can lag file creation by seconds) — is ours
    // to account, classify, and ledger-consume. A RESUMED session's history
    // predates this instant and is skipped. Baselining blindly at EOF on
    // first discovery used to drop that pre-discovery window: the ack never
    // registered (no keep-alive/chip) and the first prompt's ledger entry
    // lingered, able to swallow a later same-text cron refire.
    let spawn_epoch = std::time::SystemTime::now();
    let mut first_arm_done = false;
    loop {
        tokio::time::sleep(ws.poll_delay()).await;

        let (session_id, session_changed_at, is_prompting, turn_ended_abnormally) = {
            let s = state.read().await;
            (
                s.external_id.clone(),
                s.external_id_changed_at,
                s.status == ConnectionStatus::Prompting,
                s.last_turn_ended_abnormally,
            )
        };
        let Some(session_id) = session_id else {
            continue; // session not established yet
        };
        if ws.session_id.as_deref() != Some(session_id.as_str()) {
            // New/resumed/forked session: re-locate and re-baseline. History
            // up to now renders via the normal detail fetch, not the overlay.
            // The first arm keeps the spawn epoch (see above). A LATER re-arm
            // (fork / re-resume) baselines at the instant the session id
            // actually CHANGED — not at this tick: the frontend fires its
            // follow-up prompt the moment the fork resolves, so records can
            // land in the forked transcript seconds before this poll notices;
            // a tick-time epoch would misclassify them as copied history (the
            // copy carries ORIGINAL, pre-fork timestamps — the changed-at
            // instant cleanly separates the two).
            let epoch = if first_arm_done {
                session_changed_at.unwrap_or_else(std::time::SystemTime::now)
            } else {
                spawn_epoch
            };
            first_arm_done = true;
            ws.rearm(session_id.clone(), epoch);
        }

        // File I/O + JSON parsing + turn grouping are blocking work; a large
        // tail after a long foreground turn must not stall the runtime.
        let ledger_ref = Arc::clone(&ledger);
        let cwd_for_tick = cwd.clone();
        let conn_for_tick = conn_id.clone();
        let joined = tokio::task::spawn_blocking(move || {
            let mut ws = ws;
            let event = ws.tick(
                &ledger_ref,
                &cwd_for_tick,
                &conn_for_tick,
                is_prompting,
                turn_ended_abnormally,
            );
            (ws, event)
        })
        .await;
        let event = match joined {
            Ok((returned, event)) => {
                ws = returned;
                event
            }
            Err(e) => {
                // Tick panicked (never expected — it is written to skip bad
                // input). Start over with a fresh baseline rather than killing
                // the watch for the rest of the connection's life.
                tracing::warn!("[bg-watch] tick panicked, re-arming: {e}");
                ws = WatchState::new();
                continue;
            }
        };

        if let Some(event) = event {
            if let AcpEvent::BackgroundActivity {
                turns,
                settled,
                outstanding,
                watermark,
                ..
            } = &event
            {
                tracing::info!(
                    "[bg-watch] surfacing connection={} turns={} settled={} outstanding={} watermark={}",
                    conn_id,
                    turns.len(),
                    settled.len(),
                    outstanding,
                    watermark
                );
            }
            emit_with_state(&state, &emitter, event).await;
        }
    }
}

/// One launched-but-unresolved background task.
struct TaskEntry {
    kind: &'static str,
    started_at: Instant,
}

/// The current out-of-turn episode: a contiguous run of transcript records
/// not belonging to any codeg-sent prompt turn, assembled into turns via the
/// detail parser's own Stage A/B.
struct Episode {
    /// Byte offset of the episode's initiating record — the stable base of
    /// this episode's overlay turn ids (`bg-<start_offset>-<idx>`).
    start_offset: u64,
    acc: ClaudeRecordAccumulator,
    /// turn id → content hash at last emission, for changed-turn upserts.
    emitted_hashes: HashMap<String, u64>,
    /// The task id of the `<task-notification>` that initiated this episode,
    /// or `None` for any other out-of-turn initiator (a cron prompt, other
    /// injected text). Carried across a force-rotation (same continuous
    /// out-of-turn stretch, just re-based) — see `classify_and_feed`. Used by
    /// `collect_changed_turns` to tag each collected turn for the held-turn
    /// suppression filter in `tick()`.
    origin_task_id: Option<String>,
}

enum Mode {
    /// Records belong to a codeg-sent prompt turn — the wire renders them.
    Foreground,
    /// Records are out-of-turn — the overlay renders them.
    Background,
}

pub(crate) struct WatchState {
    session_id: Option<String>,
    file: Option<PathBuf>,
    /// Bytes consumed through the last complete line — the emitted watermark.
    committed: u64,
    /// Bytes after `committed`: a trailing partial line awaiting its newline.
    carry: Vec<u8>,
    /// Last observed (mtime, len): the cheap "did anything change" gate.
    last_stat: Option<(Option<std::time::SystemTime>, u64)>,
    mode: Mode,
    episode: Option<Episode>,
    tasks: HashMap<String, TaskEntry>,
    /// Task ids that have settled at least once — a later `SendMessage` to
    /// such an id re-arms it (the resumed sub-agent will notify again).
    settled_ids: HashSet<String>,
    /// Task ids launched (an `async_launched`/`backgroundTaskId` ack seen)
    /// while the connection's CURRENTLY (or most recently) active turn was
    /// `Prompting`. An `async_launched` (sub-agent) id is inserted here;
    /// `backgroundTaskId` (shell) ids deliberately are NOT (see `account()`).
    /// Cleared on every Connected→Prompting rising edge (each turn starts
    /// with an empty set) AND, early, the instant a turn is observed to have
    /// ended abnormally (see `last_turn_ended_abnormally` below) — otherwise
    /// it persists UNCHANGED across a normal Prompting→Connected falling
    /// edge, with no time limit. Used to detect an out-of-turn
    /// `<task-notification>` follow-up that belongs to a turn #870
    /// (claude-agent-acp v0.59.0) is holding open for its own spawned
    /// sub-agents: that follow-up's content is already rendering on the wire,
    /// so the OVERLAY turn for it must be suppressed to avoid double-rendering
    /// it (`tick()`'s `changed_turns` filter). The `settled` notification for
    /// the same task is NOT suppressed — the frontend needs it to flip the
    /// launch card, and it patches that card in-memory rather than re-parsing
    /// the transcript, so it can't double-render (see `tick()`). No time window
    /// is needed: the set's own lifetime — cleared only at the next rising
    /// edge — already covers the case where the turn's tail content is read by
    /// a tick strictly AFTER the falling edge (the watcher polls on its own
    /// cadence, independent of exactly when the turn settles), for however long
    /// that takes.
    current_turn_launched_ids: HashSet<String>,
    /// `Prompting` state observed at the previous tick — the edge detector for
    /// `current_turn_launched_ids` above.
    was_prompting: bool,
    /// `Prompting` state for the tick currently being processed. Set once at
    /// `tick()` entry from the caller-supplied snapshot so `account()` (called
    /// per transcript line within the same tick) can read it without an extra
    /// parameter threaded through every call site.
    currently_prompting: bool,
    /// `MessageTurn.id` → the out-of-turn episode's origin task id (`None` if
    /// the episode wasn't initiated by a `<task-notification>`, e.g. a cron
    /// prompt), for turns collected THIS tick by `collect_changed_turns`.
    /// Drained by the suppression filter at the end of `tick()` — entries
    /// never outlive the tick that created them.
    turn_origin_task_ids: HashMap<String, Option<String>>,
    last_disk_activity: Option<Instant>,
    last_emitted_outstanding: Option<u32>,
    armed_logged: bool,
    /// Base of the most recently created episode's id namespace. Episode
    /// bases must be STRICTLY increasing: two episodes created while
    /// processing one tick's batch would otherwise share `committed` (it
    /// advances per batch, not per record) and collide their `bg-<base>-…`
    /// ids — the frontend upserts by id, so a collision conflates turns.
    last_episode_base: u64,
    /// Wall-clock boundary for the arm baseline: records at/after this
    /// instant belong to this watch's lifetime and are processed even when
    /// they were written before the transcript file was first discovered;
    /// records before it are pre-existing history. Set by `rearm`.
    epoch: Option<std::time::SystemTime>,
}

impl WatchState {
    pub(crate) fn new() -> Self {
        Self {
            session_id: None,
            file: None,
            committed: 0,
            carry: Vec::new(),
            last_stat: None,
            mode: Mode::Foreground,
            episode: None,
            tasks: HashMap::new(),
            settled_ids: HashSet::new(),
            current_turn_launched_ids: HashSet::new(),
            was_prompting: false,
            currently_prompting: false,
            turn_origin_task_ids: HashMap::new(),
            last_disk_activity: None,
            // Some(0), not None: consumers assume zero until told otherwise,
            // so the first tick must not emit an accounting-only event for a
            // connection with no background work.
            last_emitted_outstanding: Some(0),
            armed_logged: false,
            last_episode_base: 0,
            epoch: None,
        }
    }

    /// Allocate the id-namespace base for a new episode: the current byte
    /// offset, tie-broken upward so consecutive episodes never collide even
    /// when created within a single tick's batch.
    fn next_episode_base(&mut self) -> u64 {
        let base = self.committed.max(self.last_episode_base + 1);
        self.last_episode_base = base;
        base
    }

    fn rearm(&mut self, session_id: String, epoch: std::time::SystemTime) {
        let tasks = std::mem::take(&mut self.tasks);
        let settled_ids = std::mem::take(&mut self.settled_ids);
        *self = Self::new();
        // Keep the accounting across a fork/resume of the same CLI process —
        // the background work is still running in it; the max-age valve and
        // future notifications (same task-id) still resolve these entries.
        // `settled_ids` must survive too: a post-fork `SendMessage(to: <id>)`
        // resumes a sub-agent that settled BEFORE the fork, and without the
        // set the re-arm is missed — outstanding stays 0 and closing the tab
        // could kill the resumed work.
        self.tasks = tasks;
        self.settled_ids = settled_ids;
        self.session_id = Some(session_id);
        self.epoch = Some(epoch);
    }

    fn poll_delay(&self) -> Duration {
        let recently_active = self
            .last_disk_activity
            .is_some_and(|at| at.elapsed() < RECENT_ACTIVITY_WINDOW);
        if !self.tasks.is_empty() || recently_active {
            POLL_ACTIVE
        } else {
            POLL_IDLE
        }
    }

    /// One poll tick: stat-gate, tail-read complete lines, account + classify
    /// each record, regroup the episode, and decide what (if anything) to
    /// emit. Never panics on malformed input — bad lines are skipped.
    ///
    /// `is_prompting` is a snapshot of the connection's `Prompting` status
    /// taken by the async caller right before this (blocking) tick runs — see
    /// `current_turn_launched_ids`'s doc comment for why the watcher needs it.
    /// `turn_ended_abnormally` is a snapshot of `SessionState::
    /// last_turn_ended_abnormally` taken at the same instant — meaningful only
    /// on the tick that observes the falling edge (see below).
    pub(crate) fn tick(
        &mut self,
        ledger: &PromptLedger,
        cwd: &str,
        conn_id: &str,
        is_prompting: bool,
        turn_ended_abnormally: bool,
    ) -> Option<AcpEvent> {
        let session_id = self.session_id.clone()?;

        // Rising edge (a fresh turn started prompting): ids a PAST turn
        // launched must not suppress an out-of-turn follow-up that has
        // nowhere else to render. Falling edge: if the turn ended abnormally
        // (cancelled/refused/etc — its content never reached the wire),
        // release its launched ids NOW rather than waiting for the next
        // rising edge — there is no live view left for a late notification to
        // duplicate, so the overlay is correctly the only place left for it
        // to render. A NORMAL falling edge leaves the set untouched: it stays
        // suppression-eligible, with no time limit, until the next rising
        // edge (see `current_turn_launched_ids`'s doc comment).
        // `account()` reads `currently_prompting` per-line below without its
        // own parameter.
        if is_prompting && !self.was_prompting {
            self.current_turn_launched_ids.clear();
        }
        if !is_prompting && self.was_prompting && turn_ended_abnormally {
            self.current_turn_launched_ids.clear();
        }
        self.was_prompting = is_prompting;
        self.currently_prompting = is_prompting;

        // Expire tasks past the keep-alive max age so a lost completion can't
        // pin the connection alive forever; the emitted outstanding drop also
        // releases the frontend's sweep exemption mirror.
        let max_age = background_keepalive_max_age()
            .to_std()
            .unwrap_or(Duration::from_secs(3600));
        let before = self.tasks.len();
        self.tasks.retain(|id, t| {
            let keep = t.started_at.elapsed() < max_age;
            if !keep {
                tracing::info!(
                    "[bg-watch] expiring {} task={id} after max-age (completion never observed)",
                    t.kind
                );
            }
            keep
        });
        let expired_any = self.tasks.len() != before;

        // Locate the transcript (it may not exist yet for a brand-new
        // session; retry every tick until it does).
        if self.file.is_none() {
            if let Some(f) = find_session_file(&session_id) {
                self.adopt_file(f);
            }
            if let Some(f) = &self.file {
                if !self.armed_logged {
                    self.armed_logged = true;
                    tracing::info!(
                        "[bg-watch] armed connection={} session={} baseline={} file={}",
                        conn_id,
                        session_id,
                        self.committed,
                        f.display()
                    );
                }
            }
        }
        let path = self.file.clone()?;

        // Cheap gate: unchanged (mtime, len) and no pending partial line means
        // nothing to read this tick.
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[bg-watch] stat failed for {}: {e}", path.display());
                self.file = None; // session file may have moved; re-locate
                return None;
            }
        };
        let stat = (meta.modified().ok(), meta.len());
        let unchanged = self.last_stat.as_ref() == Some(&stat);
        self.last_stat = Some(stat);

        let mut changed_turns: Vec<MessageTurn> = Vec::new();
        let mut settled: Vec<BackgroundSettledInfo> = Vec::new();

        if meta.len() < self.committed {
            // Truncated/rewritten out from under us: re-baseline at EOF. The
            // frontend overlay reconciles on its next detail refetch.
            tracing::warn!(
                "[bg-watch] transcript shrank ({} -> {}), re-baselining",
                self.committed,
                meta.len()
            );
            self.committed = meta.len();
            self.carry.clear();
            self.episode = None;
            self.mode = Mode::Foreground;
        } else if !unchanged {
            match self.read_new_lines(&path) {
                Ok(lines) => {
                    if !lines.is_empty() {
                        self.last_disk_activity = Some(Instant::now());
                    }
                    for line in &lines {
                        let value: serde_json::Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        self.account(&value, &mut settled);
                        self.classify_and_feed(&value, ledger, cwd, &mut changed_turns);
                    }
                    if !lines.is_empty() {
                        // Regroup once per tick (not per record): collect the
                        // episode's turns whose content changed since the last
                        // emission.
                        self.collect_changed_turns(cwd, &mut changed_turns);
                    }
                }
                Err(e) => {
                    tracing::warn!("[bg-watch] read failed for {}: {e}", path.display());
                    return None;
                }
            }
        }

        // Held-turn OVERLAY suppression: a turn #870 (claude-agent-acp v0.59.0)
        // is holding open for its own spawned sub-agents renders their follow-up
        // content on the wire
        // already, so the OVERLAY copy of that content (a `changed_turns` entry)
        // must NOT also render — that's the double-render this drop prevents. No
        // time window is needed: an id's membership in `current_turn_launched_ids`
        // alone closes the TOCTOU race a naive "is_prompting right now" check
        // would miss (the turn's own tail content can be read by THIS tick
        // strictly after the falling edge, even though it was genuinely
        // wire-rendered a beat earlier while still `Prompting`) — the set simply
        // isn't cleared until the next rising edge (or immediately, for an
        // abnormal ending — see `tick()`'s entry). Every other out-of-turn turn
        // (cron//loop autonomous turns have no originating task id at all; a
        // notification for a task some OTHER, already-superseded turn launched
        // isn't in THIS turn's set; background shells are never inserted into the
        // set at all — see `account()`) passes through unaffected.
        //
        // `settled` is deliberately NOT filtered the same way. It carries the
        // task's terminal state + `<result>` + launching `tool_use_id`, which the
        // frontend needs to flip the launch CARD (`AgentToolCallPart`) from
        // "running in background" to its completed/result form — the ONLY trigger
        // for that flip. Filtering it (as an earlier iteration did) left the card
        // frozen forever, because `settled.push` fires exactly once per
        // notification record and the bytes are never re-read. Un-filtering it
        // does NOT re-introduce a double-render: the frontend patches the
        // existing card in-memory from this payload (`resolveBackgroundTask`)
        // rather than issuing the `refetchDetail` it used to — see §3.2.
        // `outstanding`/`watermark` are computed independently and untouched by
        // this filter, so the sweep-exemption/chip accounting stays accurate.
        //
        // Instead of dropping a held-turn settle we TAG it: `wire_visible` marks
        // a settle whose task belongs to a turn #870 is holding open (its id is
        // still in `current_turn_launched_ids`), so its reply is already on the
        // wire. The frontend reads this to skip arming the "syncing results"
        // hint for such a settle (there's no gap to bridge) — a backend-derived
        // classification, correct even when this tick reads the settlement after
        // the turn already fell back to `Connected` (the set isn't cleared until
        // the next rising edge).
        changed_turns.retain(|t| {
            let origin = self.turn_origin_task_ids.remove(&t.id).flatten();
            !matches!(origin, Some(task_id) if self.current_turn_launched_ids.contains(&task_id))
        });
        for s in settled.iter_mut() {
            s.wire_visible = self.current_turn_launched_ids.contains(&s.task_id);
        }

        let outstanding = self.tasks.len() as u32;
        let accounting_changed =
            expired_any || self.last_emitted_outstanding != Some(outstanding);
        if changed_turns.is_empty() && settled.is_empty() && !accounting_changed {
            return None;
        }
        self.last_emitted_outstanding = Some(outstanding);
        Some(AcpEvent::BackgroundActivity {
            session_id,
            turns: changed_turns,
            outstanding,
            settled,
            watermark: self.committed,
        })
    }

    /// Adopt a just-discovered transcript file, choosing the arm baseline.
    /// History (records before `epoch`) renders via the detail fetch and is
    /// skipped; records at/after `epoch` — a first prompt or launch ack
    /// written between session creation and this discovery — are processed
    /// like any other appended record. Without an epoch (never armed via
    /// `rearm`), falls back to EOF, the pure-history behavior.
    fn adopt_file(&mut self, f: PathBuf) {
        if let Ok(meta) = std::fs::metadata(&f) {
            // `baseline_offset_since` never lands inside a trailing partial
            // line; the EOF fallback covers only an unreadable file (where
            // the tail reader will re-locate anyway).
            self.committed = self
                .epoch
                .and_then(|e| baseline_offset_since(&f, e))
                .unwrap_or(meta.len());
            // Deliberately do NOT pre-seed `last_stat`: the stat gate below
            // must see this tick as changed so a baseline that landed BEFORE
            // EOF (pre-discovery records to process) is read immediately, not
            // on the next unrelated append.
        }
        self.file = Some(f);
    }

    /// Read bytes appended since `committed`, returning COMPLETE lines only;
    /// a trailing partial line stays in `carry` until its newline arrives.
    fn read_new_lines(&mut self, path: &PathBuf) -> std::io::Result<Vec<String>> {
        let mut f = std::fs::File::open(path)?;
        f.seek(SeekFrom::Start(self.committed + self.carry.len() as u64))?;
        let mut fresh = Vec::new();
        f.read_to_end(&mut fresh)?;
        if fresh.is_empty() {
            return Ok(Vec::new());
        }
        self.carry.extend_from_slice(&fresh);

        let mut lines = Vec::new();
        while let Some(nl) = self.carry.iter().position(|b| *b == b'\n') {
            let rest = self.carry.split_off(nl + 1);
            let mut line_bytes = std::mem::replace(&mut self.carry, rest);
            line_bytes.pop(); // the '\n'
            self.committed += nl as u64 + 1;
            // Mirror the detail parser: a non-UTF-8 line is skipped, but its
            // bytes still count toward the watermark.
            if let Ok(line) = String::from_utf8(line_bytes) {
                lines.push(line);
            }
        }
        Ok(lines)
    }

    /// Task accounting for one record: launch acks; settlements via a
    /// `<task-notification>` record, a `TaskOutput` result reaching a terminal
    /// `task.status`, or a `TaskStop`/`KillShell` call; and `SendMessage`
    /// re-arms of settled sub-agents.
    fn account(&mut self, value: &serde_json::Value, settled: &mut Vec<BackgroundSettledInfo>) {
        let record_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match record_type {
            "user" => {
                if let Some(tur) = value.get("toolUseResult") {
                    if tur.get("status").and_then(|s| s.as_str()) == Some("async_launched") {
                        if let Some(id) = tur
                            .get("agentId")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            // `entry().or_insert_with()`, not a blind `insert`:
                            // a re-observed id (e.g. a resumed sub-agent's ack
                            // repeating) must not reset `started_at` to now —
                            // that would restart the max-age clock from
                            // whatever tick last saw it instead of counting
                            // from first launch, silently extending how long a
                            // truly-abandoned task can pin the connection
                            // alive. The log fires only on first registration.
                            self.tasks.entry(id.to_string()).or_insert_with(|| {
                                tracing::info!("[bg-watch] registered async agent task={id}");
                                TaskEntry {
                                    kind: "agent",
                                    started_at: Instant::now(),
                                }
                            });
                            // This turn is still `Prompting` at launch time — a
                            // later out-of-turn follow-up for this SAME id is
                            // therefore held-open content already rendering on
                            // the wire (see `current_turn_launched_ids`'s doc
                            // comment); mark it so `tick()`'s suppression
                            // filter can catch it.
                            if self.currently_prompting {
                                self.current_turn_launched_ids.insert(id.to_string());
                            }
                        }
                    } else if let Some(id) = tur
                        .get("backgroundTaskId")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        // Same first-seen rationale as the agent branch above —
                        // doubly important here since a still-running shell is
                        // typically observed via REPEATED `BashOutput`-style
                        // reads of this identical shape.
                        self.tasks.entry(id.to_string()).or_insert_with(|| {
                            tracing::info!("[bg-watch] registered background shell task={id}");
                            TaskEntry {
                                kind: "shell",
                                started_at: Instant::now(),
                            }
                        });
                        // Deliberately NOT inserted into `current_turn_launched_ids`:
                        // #870 never holds a turn open for a shell (this
                        // module's own top-of-file doc comment — "a hold must
                        // NEVER wait on a shell"), so a shell's owning turn
                        // always ends via an ordinary `end_turn` while the
                        // shell keeps running. If shells were suppression-
                        // eligible, the unbounded (until-next-turn) lifetime
                        // of that set would silently swallow a shell's
                        // eventual completion for its entire realistic
                        // runtime — content that was never on the wire in the
                        // first place, with nothing to fall back on.
                    }

                    // Settle a task the agent collected via `TaskOutput`: its
                    // structured `task.status` reaching a terminal state means
                    // the task finished (exit recorded), even though no
                    // `<task-notification>` was ever written — the agent
                    // awaited it inline. This is the DOMINANT settle path for
                    // background shells, which almost never notify. No `settled`
                    // push on purpose: an inline-awaited collection must not
                    // raise an out-of-turn OS notification (the agent is
                    // mid-turn and already holds the result); only the
                    // outstanding count drops. `settled_ids` still records it so
                    // a later `SendMessage` resume can re-arm.
                    if let Some(task) = tur.get("task") {
                        if let Some(id) = task
                            .get("task_id")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            let status =
                                task.get("status").and_then(|s| s.as_str()).unwrap_or("");
                            if is_terminal_task_status(status) && self.tasks.remove(id).is_some() {
                                self.settled_ids.insert(id.to_string());
                                tracing::info!(
                                    "[bg-watch] settled task={id} via TaskOutput status={status}"
                                );
                            }
                        }
                    }
                }
                if let Some(text) = user_record_text(value) {
                    let trimmed = text.trim_start();
                    if trimmed.starts_with("<task-notification>") {
                        let task_id = capture_tag(task_notification_task_id_regex(), trimmed);
                        let status = capture_tag(task_notification_status_regex(), trimmed)
                            .unwrap_or_else(|| "completed".into());
                        let summary = capture_tag(task_notification_summary_regex(), trimmed);
                        // The notification is self-contained: its `<tool-use-id>`
                        // is the launching tool call's id and `<result>` is the
                        // sub-agent's report. Carrying both lets the frontend flip
                        // the launch card in-memory (rewriting its marker) with no
                        // `refetchDetail` — see `BackgroundSettledInfo`'s doc.
                        // Absent for a background shell (no such tags → `None`).
                        let tool_use_id =
                            capture_tag(task_notification_tool_use_id_regex(), trimmed);
                        // Same cap the cold-parse fold applies, so the live card
                        // matches and a pathological report can't blow the
                        // event-stream size budget.
                        let result = capture_tag(task_notification_result_regex(), trimmed)
                            .map(|r| truncate_str(&r, BACKGROUND_RESULT_MAX_CHARS));
                        if let Some(id) = task_id {
                            let known = self.tasks.remove(&id).is_some();
                            self.settled_ids.insert(id.clone());
                            tracing::info!(
                                "[bg-watch] settled task={id} status={status} known={known}"
                            );
                            settled.push(BackgroundSettledInfo {
                                task_id: id,
                                status,
                                summary,
                                tool_use_id,
                                result,
                                // Set in `tick()` from `current_turn_launched_ids`
                                // once the whole batch has been read.
                                wire_visible: false,
                            });
                        }
                    }
                }
            }
            "assistant" => {
                let Some(blocks) = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                else {
                    return;
                };
                for block in blocks {
                    if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let input = block.get("input");
                    match name {
                        // A settled sub-agent can be resumed: `SendMessage(to:
                        // <id>)` re-arms its accounting entry (it will notify
                        // again — the notification's own <note> documents
                        // multi-notify).
                        "SendMessage" => {
                            let Some(to) =
                                input.and_then(|i| i.get("to")).and_then(|t| t.as_str())
                            else {
                                continue;
                            };
                            if self.settled_ids.remove(to) {
                                tracing::info!("[bg-watch] re-armed resumed task={to}");
                                self.tasks.insert(
                                    to.to_string(),
                                    TaskEntry {
                                        kind: "agent",
                                        started_at: Instant::now(),
                                    },
                                );
                                // Mirrors the launch-time insert in the
                                // `async_launched` branch above: if THIS turn
                                // (the one issuing the resume) is itself held
                                // open by #870 for the resumed sub-agent, its
                                // second notification must be suppression-
                                // eligible the same way a freshly-launched
                                // one is — otherwise a resume-then-hold
                                // reproduces the same double-render.
                                if self.currently_prompting {
                                    self.current_turn_launched_ids.insert(to.to_string());
                                }
                            }
                        }
                        // Explicit kill: the background task's process is gone,
                        // so it must leave the outstanding count now — no
                        // completion notification will follow. `TaskStop` names
                        // it via `task_id`, `KillShell` via `shell_id`.
                        "TaskStop" | "KillShell" => {
                            if let Some(id) = input
                                .and_then(|i| i.get("task_id").or_else(|| i.get("shell_id")))
                                .and_then(|t| t.as_str())
                                .filter(|s| !s.is_empty())
                            {
                                if self.tasks.remove(id).is_some() {
                                    self.settled_ids.insert(id.to_string());
                                    tracing::info!("[bg-watch] settled task={id} via {name}");
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Classify a record against the prompt ledger and feed out-of-turn ones
    /// into the current episode.
    fn classify_and_feed(
        &mut self,
        value: &serde_json::Value,
        ledger: &PromptLedger,
        cwd: &str,
        changed_turns: &mut Vec<MessageTurn>,
    ) {
        if let Some(initiator_text) = turn_initiator_text(value) {
            if ledger.consume_matching(&initiator_text) {
                // A codeg-sent prompt: the wire renders this turn. Close any
                // open episode first (flush its final state) and go silent.
                tracing::debug!("[bg-watch] foreground turn matched ledger");
                self.collect_changed_turns(cwd, changed_turns);
                self.episode = None;
                self.mode = Mode::Foreground;
                return;
            }
            tracing::debug!(
                "[bg-watch] out-of-turn initiator: {:?}",
                initiator_text.chars().take(60).collect::<String>()
            );
            let rotate = self
                .episode
                .as_ref()
                .is_some_and(|e| e.acc.messages.len() >= MAX_EPISODE_MESSAGES);
            if matches!(self.mode, Mode::Foreground) || self.episode.is_none() || rotate {
                if rotate {
                    self.collect_changed_turns(cwd, changed_turns);
                }
                // Any stable, strictly-increasing base works — turn ids only
                // need to be unique and stable within the watch.
                self.episode = Some(Episode {
                    start_offset: self.next_episode_base(),
                    acc: ClaudeRecordAccumulator::new(
                        self.file.clone().unwrap_or_else(|| PathBuf::from("")),
                    ),
                    emitted_hashes: HashMap::new(),
                    origin_task_id: task_notification_origin_id(&initiator_text),
                });
            }
            self.mode = Mode::Background;
        }

        if matches!(self.mode, Mode::Background) {
            // Mid-turn safety valve: one giant turn with no boundary would
            // otherwise grow the episode — and the per-tick regroup over it —
            // without bound. Flush and re-base; the seam is cosmetic and the
            // next detail refetch renders the turn whole.
            let force_rotate = self
                .episode
                .as_ref()
                .is_some_and(|e| e.acc.messages.len() >= FORCE_ROTATE_MESSAGES);
            if force_rotate {
                tracing::warn!(
                    "[bg-watch] episode reached {FORCE_ROTATE_MESSAGES} messages without a turn \
                     boundary — force-rotating (the in-progress turn renders split until the \
                     next detail refetch)"
                );
                self.collect_changed_turns(cwd, changed_turns);
                // Cosmetic re-basing of the SAME continuous out-of-turn
                // stretch (not a new initiator record) — the origin carries
                // over unchanged.
                let inherited_origin =
                    self.episode.as_ref().and_then(|e| e.origin_task_id.clone());
                self.episode = Some(Episode {
                    start_offset: self.next_episode_base(),
                    acc: ClaudeRecordAccumulator::new(
                        self.file.clone().unwrap_or_else(|| PathBuf::from("")),
                    ),
                    emitted_hashes: HashMap::new(),
                    origin_task_id: inherited_origin,
                });
            }
            if let Some(episode) = self.episode.as_mut() {
                episode.acc.feed_value(value.clone());
            }
        }
    }

    /// Regroup the open episode with the detail parser's Stage B + post-
    /// processing and append turns whose content changed since last emission.
    fn collect_changed_turns(&mut self, cwd: &str, out: &mut Vec<MessageTurn>) {
        let Some(episode) = self.episode.as_mut() else {
            return;
        };
        if episode.acc.messages.is_empty() {
            return;
        }
        let origin_task_id = episode.origin_task_id.clone();
        let mut messages = episode.acc.messages.clone();
        // An autonomous turn can itself launch background work; fold any
        // ack+notification pairs seen within this episode, same as the
        // detail parse does.
        episode.acc.apply_background_lifecycle(&mut messages);
        let mut turns = group_into_turns(messages);
        crate::parsers::relocate_orphaned_tool_results(&mut turns);
        crate::parsers::structurize_read_tool_output(&mut turns);
        crate::parsers::resolve_patch_line_numbers(&mut turns, Some(cwd));
        for (idx, mut turn) in turns.into_iter().enumerate() {
            turn.id = format!("bg-{}-{}", episode.start_offset, idx);
            let hash = hash_turn(&turn);
            if episode.emitted_hashes.get(&turn.id) == Some(&hash) {
                continue;
            }
            episode.emitted_hashes.insert(turn.id.clone(), hash);
            // Recorded for `tick()`'s held-turn suppression filter, drained
            // there the same tick it's populated — never outlives one tick.
            self.turn_origin_task_ids
                .insert(turn.id.clone(), origin_task_id.clone());
            out.push(turn);
        }
    }

    #[cfg(test)]
    fn with_file_for_test(session_id: &str, file: PathBuf) -> Self {
        let mut ws = Self::new();
        ws.session_id = Some(session_id.to_string());
        ws.file = Some(file);
        ws
    }
}

/// Extract the text of a user record's content: bare string form, or the
/// concatenated text blocks of the array form. `None` for non-user records.
fn user_record_text(value: &serde_json::Value) -> Option<String> {
    if value.get("type").and_then(|t| t.as_str()) != Some("user") {
        return None;
    }
    let content = value.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let arr = content.as_array()?;
    let texts: Vec<&str> = arr
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect();
    if texts.is_empty() {
        return None;
    }
    Some(texts.join("\n"))
}

/// The text of a record that STARTS a new logical turn, or `None` for records
/// that continue the current one. Ground rules (from real transcripts):
///
/// * tool-result-only user records continue the in-progress turn;
/// * `isMeta` ARRAY records are slash-command expansions — part of the
///   foreground turn that issued the command (a cron prompt is `isMeta` too,
///   but always STRING content, so it still initiates);
/// * auto-compaction continuation summaries land MID-turn while the wire is
///   still rendering it — never a boundary;
/// * everything else user-typed/injected (real prompts, `<task-notification>`
///   records, cron prompts) initiates.
fn turn_initiator_text(value: &serde_json::Value) -> Option<String> {
    if value.get("type").and_then(|t| t.as_str()) != Some("user") {
        return None;
    }
    let content = value.get("message")?.get("content")?;

    if let Some(s) = content.as_str() {
        if s.starts_with(CONTEXT_CONTINUATION_PREFIX) {
            return None;
        }
        // A slash command persists as command tags; codeg sent the display
        // form ("/name args"), so match the ledger against that.
        if let Some(display) = slash_command_display(s) {
            return Some(display);
        }
        return Some(s.to_string());
    }

    let arr = content.as_array()?;
    if !arr.is_empty()
        && arr
            .iter()
            .all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
    {
        return None;
    }
    if is_meta_message(value) {
        return None;
    }
    let text = user_record_text(value)?;
    if text.starts_with(CONTEXT_CONTINUATION_PREFIX) {
        return None;
    }
    Some(text)
}

/// If `text` (an out-of-turn initiator from `turn_initiator_text`) is a
/// `<task-notification>` record, its `<task-id>` — mirroring `account()`'s
/// exact gate so the two never diverge on what counts as a task-notification.
/// `None` for any other out-of-turn initiator (a cron prompt, other injected
/// text), which has no originating task to attribute an episode to.
fn task_notification_origin_id(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    if !trimmed.starts_with("<task-notification>") {
        return None;
    }
    capture_tag(task_notification_task_id_regex(), trimmed)
}

/// The arm baseline separating pre-existing history from records written
/// during this watch's lifetime: the byte offset of the first COMPLETE
/// CONVERSATION line (a `user`/`assistant` record) whose timestamp is at or
/// after `epoch`, or — when no such record exists yet — the end of the last
/// COMPLETE line. The fallback deliberately excludes a trailing partial line:
/// a fragment present at arm time is a record being flushed RIGHT NOW
/// (post-epoch by definition), and baselining past it (EOF) would leave only
/// its unparseable suffix for the tail reader, silently dropping the very
/// record the epoch baseline exists to preserve.
///
/// Only `user`/`assistant` records delimit the boundary; every other record
/// type (and any line without a parseable timestamp) is skipped. This is
/// load-bearing for FORK: a fork copies the parent transcript into the new
/// session file preserving each record's ORIGINAL, pre-fork timestamp, then
/// writes fresh metadata records (`queue-operation`, `mode`, …) at the FILE
/// HEAD stamped at fork time. Those head records are AHEAD of the copied
/// history by byte offset but AFTER it by timestamp, so keying the boundary on
/// "first record at/after epoch" of ANY type would return the head metadata's
/// offset and drag the entire copied history (which renders via the detail
/// fetch) into the out-of-turn overlay — duplicating it. Restricting the
/// boundary to conversation records lands it on the first genuinely-new turn,
/// with the copied history (older timestamps) correctly on the history side.
/// The skipped metadata carries no turn or accounting the watcher consumes.
/// `None` only when the file can't be read. One-shot cost at arm time (runs
/// inside the tick's `spawn_blocking`).
fn baseline_offset_since(path: &PathBuf, epoch: std::time::SystemTime) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let epoch: chrono::DateTime<chrono::Utc> = epoch.into();
    let mut offset = 0u64;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        let start = offset;
        offset += line.len() as u64;
        let Ok(text) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        // Only real conversation records anchor the boundary; fork-time head
        // metadata (see the doc comment) must never be the boundary.
        let record_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if record_type != "user" && record_type != "assistant" {
            continue;
        }
        let Some(ts) = value.get("timestamp").and_then(|t| t.as_str()) else {
            continue;
        };
        let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(ts) else {
            continue;
        };
        if parsed.with_timezone(&chrono::Utc) >= epoch {
            return Some(start);
        }
    }
    Some(offset)
}

/// Whether a `TaskOutput` `task.status` is terminal — the task has stopped and
/// must leave the outstanding count. `"running"` (a non-blocking poll of a
/// still-live task) is deliberately excluded so a status check doesn't clear a
/// task that is genuinely still working.
fn is_terminal_task_status(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "failed"
            | "canceled"
            | "cancelled"
            | "killed"
            | "stopped"
            | "timeout"
            | "timed_out"
            | "error"
    )
}

fn hash_turn(turn: &MessageTurn) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match serde_json::to_string(turn) {
        Ok(s) => s.hash(&mut hasher),
        Err(_) => turn.blocks.len().hash(&mut hasher),
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_session(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("session-1.jsonl")
    }

    fn write_lines(path: &PathBuf, lines: &[&str]) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    /// Append raw bytes WITHOUT a newline — simulates a mid-flush fragment.
    fn append_raw(path: &PathBuf, chunk: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        write!(f, "{chunk}").unwrap();
    }

    /// Real-shape async sub-agent launch ack (structured `toolUseResult`
    /// sibling as captured from a live transcript on 2026-07-07).
    fn agent_ack(agent_id: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:46:14.514Z","uuid":"u-ack-{agent_id}","message":{{"role":"user","content":[{{"tool_use_id":"toolu_01","type":"tool_result","content":[{{"type":"text","text":"Async agent launched successfully. agentId: {agent_id}"}}]}}]}},"toolUseResult":{{"isAsync":true,"status":"async_launched","agentId":"{agent_id}","description":"Run pnpm build"}}}}"#
        )
    }

    /// Real-shape background shell ack (`toolUseResult.backgroundTaskId`).
    fn bash_ack(task_id: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:46:15.000Z","uuid":"u-bash-{task_id}","message":{{"role":"user","content":[{{"tool_use_id":"toolu_02","type":"tool_result","content":"Command running in background with ID: {task_id}."}}]}},"toolUseResult":{{"stdout":"","stderr":"","interrupted":false,"backgroundTaskId":"{task_id}"}}}}"#
        )
    }

    /// Real-shape `<task-notification>` completion record (string content).
    fn notification(task_id: &str, status: &str) -> String {
        let inner = format!(
            "<task-notification>\\n<task-id>{task_id}</task-id>\\n<tool-use-id>toolu_01</tool-use-id>\\n<status>{status}</status>\\n<summary>Agent \\\"Run pnpm build\\\" finished</summary>\\n<result>Build OK</result>\\n</task-notification>"
        );
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:47:00.000Z","uuid":"u-note-{task_id}","isSidechain":false,"message":{{"role":"user","content":"{inner}"}}}}"#
        )
    }

    fn assistant_text(uuid: &str, text: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-07-07T03:47:08.000Z","uuid":"{uuid}","message":{{"role":"assistant","model":"claude-sonnet-5","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    /// Real-shape `TaskOutput` result: a tool-result user record whose
    /// structured `toolUseResult.task` carries `task_id` + `status` (shape
    /// captured from a live transcript on 2026-07-08).
    fn taskoutput_result(task_id: &str, status: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:47:30.000Z","uuid":"u-to-{task_id}-{status}","message":{{"role":"user","content":[{{"tool_use_id":"toolu_out_{task_id}","type":"tool_result","content":[{{"type":"text","text":"<task_id>{task_id}</task_id> <status>{status}</status>"}}]}}]}},"toolUseResult":{{"retrieval_status":"success","task":{{"task_id":"{task_id}","task_type":"local_bash","status":"{status}","exitCode":0}}}}}}"#
        )
    }

    /// Real-shape `TaskStop` call (assistant tool_use naming the task via
    /// `task_id`).
    fn taskstop(task_id: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-07-07T03:47:31.000Z","uuid":"a-stop-{task_id}","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_stop_{task_id}","name":"TaskStop","input":{{"task_id":"{task_id}"}}}}]}}}}"#
        )
    }

    fn user_prompt_array(uuid: &str, text: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:48:00.000Z","uuid":"{uuid}","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    /// Real-shape cron-fired prompt: `isMeta:true` with bare STRING content.
    fn cron_prompt(text: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"2026-07-07T03:49:00.000Z","uuid":"u-cron","isMeta":true,"userType":"external","message":{{"role":"user","content":"{text}"}}}}"#
        )
    }

    fn tick_now(ws: &mut WatchState, ledger: &PromptLedger) -> Option<AcpEvent> {
        ws.tick(ledger, "/tmp", "conn-test", false, false)
    }

    /// Like `tick_now`, but with the connection snapshotted as `Prompting` —
    /// for tests of the held-turn suppression filter (§3 of the 0.59 upgrade
    /// plan), which engages while the connection is prompting and, with no
    /// time limit, for as long afterward as no new turn has started.
    fn tick_prompting(ws: &mut WatchState, ledger: &PromptLedger) -> Option<AcpEvent> {
        ws.tick(ledger, "/tmp", "conn-test", true, false)
    }

    /// Like `tick_now`, but reporting the just-ended turn as having stopped
    /// abnormally (cancelled/refused/etc) — for tests of the early-release
    /// path that lets a held turn's launched ids stop being suppression-
    /// eligible immediately instead of waiting for the next turn.
    fn tick_abnormal_end(ws: &mut WatchState, ledger: &PromptLedger) -> Option<AcpEvent> {
        ws.tick(ledger, "/tmp", "conn-test", false, true)
    }

    fn unpack(
        event: AcpEvent,
    ) -> (Vec<MessageTurn>, u32, Vec<BackgroundSettledInfo>, u64) {
        match event {
            AcpEvent::BackgroundActivity {
                turns,
                outstanding,
                settled,
                watermark,
                ..
            } => (turns, outstanding, settled, watermark),
            other => panic!("expected BackgroundActivity, got {other:?}"),
        }
    }

    #[test]
    fn force_rotates_a_single_giant_turn_and_bounds_the_episode() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        // One out-of-turn initiator, then one giant turn: FORCE + 3 assistant
        // records with NO further boundary. Written and read in a single tick
        // batch to also exercise the same-batch episode-base tie-break.
        let mut lines: Vec<String> = vec![cron_prompt("iterate forever")];
        for i in 0..(FORCE_ROTATE_MESSAGES + 3) {
            lines.push(assistant_text(&format!("a-{i}"), "chunk"));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        write_lines(&path, &refs);

        let (turns, ..) = unpack(tick_now(&mut ws, &ledger).expect("turns event"));

        // The episode was re-based mid-turn: what remains buffered is only the
        // post-rotation fragment, never the whole giant turn.
        let buffered = ws.episode.as_ref().map(|e| e.acc.messages.len()).unwrap();
        assert!(
            buffered < FORCE_ROTATE_MESSAGES,
            "episode must be bounded after force-rotation, still holds {buffered}"
        );
        // Both namespaces surfaced this tick, under distinct (non-colliding)
        // bases even though both episodes were created within one batch.
        let bases: std::collections::HashSet<&str> = turns
            .iter()
            .map(|t| t.id.rsplit_once('-').expect("bg id shape").0)
            .collect();
        assert!(
            bases.len() >= 2,
            "expected pre- and post-rotation id namespaces, got {bases:?}"
        );
        assert_eq!(
            turns.len(),
            turns
                .iter()
                .map(|t| t.id.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            "turn ids must be unique across the forced rotation"
        );
    }

    fn epoch(ts: &str) -> std::time::SystemTime {
        chrono::DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&chrono::Utc)
            .into()
    }

    #[test]
    fn baseline_offset_since_finds_first_record_at_or_after_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        let first = agent_ack("agent1"); // timestamp 03:46:14.514Z
        write_lines(&path, &[&first, &notification("agent1", "completed")]); // 03:47:00Z

        // Epoch between the two records: boundary at the second line.
        assert_eq!(
            baseline_offset_since(&path, epoch("2026-07-07T03:46:30.000Z")),
            Some(first.len() as u64 + 1)
        );
        // Epoch before everything: whole file is ours.
        assert_eq!(
            baseline_offset_since(&path, epoch("2020-01-01T00:00:00Z")),
            Some(0)
        );
        // Epoch after everything: pure history — baseline after the last
        // COMPLETE line (== EOF here, the file ends with a newline).
        let full = std::fs::metadata(&path).unwrap().len();
        assert_eq!(
            baseline_offset_since(&path, epoch("2030-01-01T00:00:00Z")),
            Some(full)
        );
        // A trailing partial flush is a record being written NOW: the
        // fallback baseline must sit BEFORE it so it reconstructs.
        append_raw(&path, r#"{"type":"user","half"#);
        assert_eq!(
            baseline_offset_since(&path, epoch("2030-01-01T00:00:00Z")),
            Some(full)
        );
    }

    /// FORK layout regression. A fork copies the parent transcript (records keep
    /// their ORIGINAL, pre-fork timestamps) and writes fresh `queue-operation`
    /// metadata at the FILE HEAD stamped at fork time. That head record is
    /// post-epoch by timestamp but sits BEFORE the copied history by byte
    /// offset, so a boundary keyed on "first record at/after epoch" of ANY type
    /// would land at offset 0 and pull the entire copied history into the
    /// out-of-turn overlay — duplicating what the detail fetch already renders.
    /// The boundary must skip non-conversation records and land on the first
    /// genuinely-new turn.
    #[test]
    fn baseline_skips_fork_head_metadata_and_lands_on_the_first_new_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        // Fork-time metadata at the head (post-epoch stamp), then the copied
        // history (original pre-fork stamps), then the genuinely-new prompt.
        let queue_op = r#"{"type":"queue-operation","timestamp":"2026-07-07T03:50:00.100Z","uuid":"q-1"}"#;
        let copied_user = r#"{"type":"user","timestamp":"2026-07-07T03:46:00.000Z","uuid":"u-hi","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#;
        let copied_asst = r#"{"type":"assistant","timestamp":"2026-07-07T03:46:05.000Z","uuid":"a-hi","message":{"role":"assistant","content":[{"type":"text","text":"Hi!"}]}}"#;
        let new_user = r#"{"type":"user","timestamp":"2026-07-07T03:50:10.000Z","uuid":"u-hello","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#;
        write_lines(&path, &[queue_op, copied_user, copied_asst, new_user]);

        // Fork epoch sits after the copied history but before the new turn. The
        // boundary must be the new turn's byte offset (past the head metadata
        // AND the copied history), never offset 0.
        let new_turn_offset = (queue_op.len() + 1) as u64
            + (copied_user.len() + 1) as u64
            + (copied_asst.len() + 1) as u64;
        assert_eq!(
            baseline_offset_since(&path, epoch("2026-07-07T03:50:00.000Z")),
            Some(new_turn_offset),
            "fork-time head metadata must not drag the copied history past the baseline"
        );
    }

    /// A new session's file can be discovered mid-flush of its FIRST record:
    /// no complete post-epoch line exists yet, and an EOF fallback would
    /// baseline past the fragment — its completing suffix then reads as
    /// unparseable garbage and the record (a launch ack here) is lost.
    #[test]
    fn adopt_with_trailing_partial_line_keeps_it_ahead_of_the_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        let ledger = PromptLedger::shared();
        // Complete pre-epoch history line, then half of an ack record.
        write_lines(&path, &[&notification("older", "completed")]);
        let ack = agent_ack("agentY");
        let (head, tail) = ack.split_at(ack.len() / 2);
        append_raw(&path, head);

        let mut ws = WatchState::new();
        ws.session_id = Some("s1".into());
        ws.epoch = Some(epoch("2030-01-01T00:00:00Z"));
        ws.adopt_file(path.clone());

        // First tick buffers the fragment (no complete line — no event).
        assert!(tick_now(&mut ws, &ledger).is_none());

        // The flush completes: the reconstructed ack must account.
        append_raw(&path, tail);
        append_raw(&path, "\n");
        let (_, outstanding, ..) =
            unpack(tick_now(&mut ws, &ledger).expect("ack event"));
        assert_eq!(outstanding, 1, "mid-flush ack must survive discovery");
    }

    /// Production fork timing: the frontend fires its follow-up prompt the
    /// moment the fork resolves, so post-fork records land in the forked
    /// transcript BEFORE the polling watcher's next tick notices the session
    /// change. The re-arm epoch is therefore the instant the session id
    /// CHANGED (stamped by SessionStarted in session state), not the tick
    /// time — records written in that gap must still process, while the
    /// fork-copied history (original, pre-fork timestamps) stays skipped.
    #[test]
    fn post_fork_records_written_before_the_watcher_notices_still_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agentX")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger);
        write_lines(&path, &[&notification("agentX", "completed")]);
        let _ = tick_now(&mut ws, &ledger);

        // Fork at 03:50 (all copied history predates it), and the resume
        // record (timestamp 03:52) lands BEFORE the watcher re-arms.
        let forked = dir.path().join("session-2.jsonl");
        std::fs::copy(&path, &forked).unwrap();
        let send = r#"{"type":"assistant","timestamp":"2026-07-07T03:52:00.000Z","uuid":"a-send3","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_05","name":"SendMessage","input":{"to":"agentX","summary":"continue","message":"go on"}}]}}"#;
        write_lines(&forked, &[send]);

        // The watcher's delayed tick finally observes the fork: epoch is the
        // session-change instant, so the already-written resume record is
        // ahead of the baseline and re-arms the accounting.
        ws.rearm("s2".into(), epoch("2026-07-07T03:50:00.000Z"));
        ws.adopt_file(forked.clone());
        let (_, outstanding, ..) =
            unpack(tick_now(&mut ws, &ledger).expect("resume event"));
        assert_eq!(
            outstanding, 1,
            "a resume written before the watcher noticed the fork must re-arm"
        );
    }

    /// `settled_ids` must survive a fork//re-resume re-arm: a post-fork
    /// `SendMessage(to: <id>)` resumes a sub-agent that settled BEFORE the
    /// fork, and missing the re-arm leaves outstanding at 0 — closing the
    /// tab could then kill the resumed background work.
    #[test]
    fn settled_ids_survive_rearm_so_post_fork_resume_rearms() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agentX")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger);
        write_lines(&path, &[&notification("agentX", "completed")]);
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 0);

        // Fork: new session id, new transcript file (history copied with
        // ORIGINAL timestamps — all before the re-arm epoch).
        let forked = dir.path().join("session-2.jsonl");
        std::fs::copy(&path, &forked).unwrap();
        ws.rearm("s2".into(), epoch("2030-01-01T00:00:00Z"));
        ws.adopt_file(forked.clone());
        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "copied history must not re-account"
        );

        let send = r#"{"type":"assistant","timestamp":"2026-07-07T03:52:00.000Z","uuid":"a-send2","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_04","name":"SendMessage","input":{"to":"agentX","summary":"continue","message":"go on"}}]}}"#;
        write_lines(&forked, &[send]);
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 1, "post-fork resume must re-arm");

        write_lines(&forked, &[&notification("agentX", "completed")]);
        let (_, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 0);
        assert_eq!(settled.len(), 1);
    }

    /// The Critical arm-gap regression: a brand-new session's file (and its
    /// first prompt + launch ack) can exist BEFORE the watcher's first
    /// successful discovery — SessionStarted lags file creation by seconds.
    /// Those records must still be accounted and ledger-consumed; blindly
    /// baselining at EOF dropped them (outstanding never armed, and the
    /// unconsumed fingerprint could swallow a later same-text cron refire).
    #[test]
    fn pre_discovery_records_are_accounted_and_ledger_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        let ledger = PromptLedger::shared();
        ledger.record_text("do the thing");

        // On disk before discovery: codeg's first prompt, the reply, an ack.
        write_lines(
            &path,
            &[
                &user_prompt_array("u1", "do the thing"),
                &assistant_text("a1", "launching"),
                &agent_ack("agentX"),
            ],
        );

        let mut ws = WatchState::new();
        ws.session_id = Some("s1".into());
        // Spawn predates session creation for a new session.
        ws.epoch = Some(epoch("2020-01-01T00:00:00Z"));
        ws.adopt_file(path.clone());

        let (turns, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).expect("accounting event"));
        assert_eq!(outstanding, 1, "pre-discovery ack must register");
        assert!(settled.is_empty());
        assert!(
            turns.is_empty(),
            "the codeg-sent prompt classifies foreground — the wire renders it"
        );

        // Its fingerprint was consumed, so a same-text out-of-turn refire
        // (cron//loop) classifies as background and surfaces.
        write_lines(
            &path,
            &[&cron_prompt("do the thing"), &assistant_text("a2", "pass")],
        );
        let (turns, ..) = unpack(tick_now(&mut ws, &ledger).expect("turns event"));
        assert!(
            !turns.is_empty(),
            "same-text refire must surface — a stale ledger entry would swallow it"
        );
    }

    #[test]
    fn resume_baselines_at_eof_and_skips_historical_acks() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        let ledger = PromptLedger::shared();
        // Historical, never-settled ack from a previous run of this session.
        write_lines(&path, &[&agent_ack("stale-old")]);

        let mut ws = WatchState::new();
        ws.session_id = Some("s1".into());
        // Resume: the watch armed long after that history was written.
        ws.epoch = Some(epoch("2030-01-01T00:00:00Z"));
        ws.adopt_file(path.clone());

        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "pure history yields no event"
        );

        // Only appended records are processed; the stale ack never registers.
        write_lines(
            &path,
            &[&cron_prompt("new pass"), &assistant_text("a9", "hi")],
        );
        let (turns, outstanding, ..) =
            unpack(tick_now(&mut ws, &ledger).expect("turns event"));
        assert_eq!(outstanding, 0, "historical ack must NOT register");
        assert_eq!(turns.len(), 1);
    }

    #[test]
    fn acks_register_outstanding_without_rendering_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        write_lines(&path, &[&agent_ack("agent1"), &bash_ack("bash1")]);
        let (turns, outstanding, settled, watermark) =
            unpack(tick_now(&mut ws, &ledger).expect("accounting event"));
        assert!(turns.is_empty(), "acks are tool-result records, not turns");
        assert_eq!(outstanding, 2);
        assert!(settled.is_empty());
        assert!(watermark > 0);

        // Unchanged file → stat-gated, no event.
        assert!(tick_now(&mut ws, &ledger).is_none());
    }

    /// A background shell re-observed via a repeat `BashOutput`-style poll
    /// (the identical `backgroundTaskId` shape appearing again) must not
    /// reset its `started_at` — a blind `insert` would restart the max-age
    /// clock on every poll, letting an actively-polled-but-actually-finished
    /// shell pin the connection alive indefinitely. `entry().or_insert_with()`
    /// only sets `started_at` on the FIRST observation.
    #[test]
    fn repeat_shell_observation_does_not_reset_started_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&bash_ack("shellA")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger);
        let first_seen = ws.tasks.get("shellA").expect("registered").started_at;

        write_lines(&path, &[&bash_ack("shellA")]); // a repeat poll of the same shell
        let _ = tick_now(&mut ws, &ledger);
        let second_seen = ws.tasks.get("shellA").expect("still tracked").started_at;

        assert_eq!(
            first_seen, second_seen,
            "started_at must reflect first-seen (launch), not reset on a repeat observation"
        );
    }

    #[test]
    fn notification_settles_and_surfaces_the_response_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agent1")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger); // consume the ack

        write_lines(
            &path,
            &[
                &notification("agent1", "completed"),
                &assistant_text("a1", "Build finished cleanly."),
            ],
        );
        let (turns, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).expect("settle event"));
        assert_eq!(outstanding, 0);
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].task_id, "agent1");
        assert_eq!(settled[0].status, "completed");
        assert_eq!(
            settled[0].summary.as_deref(),
            Some("Agent \"Run pnpm build\" finished")
        );
        // The notification record itself strips to nothing; the assistant
        // response is the rendered out-of-turn content.
        assert_eq!(turns.len(), 1);
        assert!(turns[0].id.starts_with("bg-"));
    }

    /// A `<task-notification>`
    /// follow-up for an id THIS turn launched, arriving while the connection
    /// is still `Prompting` (claude-agent-acp v0.59.0's #870 holds the turn
    /// open for its own spawned sub-agents), is already rendering on the
    /// wire — so the OVERLAY turn for it must be suppressed. The `settled`
    /// entry, by contrast, MUST still flow: the frontend needs it to flip the
    /// launch card (which it does in-memory, so it can't double-render), and it
    /// carries the launching `tool_use_id` + `<result>` for exactly that.
    #[test]
    fn held_turn_followup_for_this_turns_launched_agent_is_suppressed() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agent1")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        // Launched while prompting: agent1 enters this turn's launched set.
        let _ = tick_prompting(&mut ws, &ledger);

        write_lines(
            &path,
            &[
                &notification("agent1", "completed"),
                &assistant_text("a1", "Build finished cleanly."),
            ],
        );
        // Still prompting: #870 is holding the turn open for agent1.
        let (turns, outstanding, settled, _) =
            unpack(tick_prompting(&mut ws, &ledger).expect("settle event"));
        assert!(
            turns.is_empty(),
            "held-turn overlay follow-up must be suppressed (already on the wire), got {turns:?}"
        );
        assert_eq!(outstanding, 0, "accounting must still reflect settlement");
        // The settle notification is NOT suppressed — it flips the launch card.
        assert_eq!(settled.len(), 1, "settle must flow to flip the card");
        assert_eq!(settled[0].task_id, "agent1");
        assert_eq!(
            settled[0].tool_use_id.as_deref(),
            Some("toolu_01"),
            "settle must carry the launching tool_use_id for the in-memory flip"
        );
        assert_eq!(settled[0].result.as_deref(), Some("Build OK"));
        assert!(
            settled[0].wire_visible,
            "a held-turn task's settle is wire-visible → frontend must not arm the syncing hint"
        );
    }

    /// The exact real-world race that broke a naive "is_prompting right now"
    /// check: the turn settles (Prompting→Connected) BEFORE the watcher's own
    /// tick gets around to reading the follow-up's tail content — the content
    /// was genuinely wire-rendered a beat earlier, while still `Prompting`,
    /// but this tick observes `is_prompting == false`. There is no grace
    /// window anymore: `current_turn_launched_ids` simply isn't cleared until
    /// the NEXT turn starts (or an abnormal ending releases it early), so
    /// OVERLAY suppression tolerates an arbitrarily-delayed read — several idle
    /// ticks pass with no new content before the follow-up finally lands, and
    /// the overlay turn must still be suppressed. The `settled` entry still
    /// flows regardless (it flips the launch card).
    #[test]
    fn held_turn_followup_still_suppressed_when_settlement_races_ahead_of_the_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agent1")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_prompting(&mut ws, &ledger); // agent1 launched while prompting

        // The turn settles with no new transcript content — several idle
        // ticks pass (simulating the watcher's own poll lag) with nothing
        // clearing the launched-set: no new turn has started.
        let _ = tick_now(&mut ws, &ledger);
        let _ = tick_now(&mut ws, &ledger);
        let _ = tick_now(&mut ws, &ledger);

        // The notification + follow-up land well after the falling edge —
        // is_prompting is `false` here, matching the real race exactly.
        write_lines(
            &path,
            &[
                &notification("agent1", "completed"),
                &assistant_text("a1", "Build finished cleanly."),
            ],
        );
        let (turns, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).expect("settle event"));
        assert!(
            turns.is_empty(),
            "must still suppress the overlay for an arbitrarily-delayed read, got {turns:?}"
        );
        assert_eq!(outstanding, 0);
        // Settle still flows (un-suppressed) so the card can flip; wire_visible
        // holds even though this tick read it after the falling edge (the set
        // isn't cleared until the next rising edge).
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].tool_use_id.as_deref(), Some("toolu_01"));
        assert!(settled[0].wire_visible);
    }

    /// A turn that ends ABNORMALLY (cancelled, refused, etc — the same
    /// `stop_reason != "end_turn"` bucket `connection.rs` already treats
    /// uniformly elsewhere) must release its launched ids immediately: that
    /// content never reached the wire (the ACP call was torn down before the
    /// real background work settled), so unlike a normal completion there is
    /// no live view left for a later notification to duplicate — the overlay
    /// is correctly the only place left to render it, and must not wait for
    /// the next turn to start.
    #[test]
    fn abnormal_turn_ending_releases_launched_ids_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agent1")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_prompting(&mut ws, &ledger); // agent1 launched while prompting

        // The turn ends abnormally (e.g. cancelled) instead of a normal end_turn.
        let _ = tick_abnormal_end(&mut ws, &ledger);

        // The notification lands afterward, with no new turn having started —
        // under a NORMAL ending this would still be suppressed (see the
        // settlement-races test above), but the abnormal ending must have
        // already released it.
        write_lines(
            &path,
            &[
                &notification("agent1", "completed"),
                &assistant_text("a1", "Build finished cleanly."),
            ],
        );
        let (turns, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).expect("settle event"));
        assert_eq!(
            turns.len(),
            1,
            "an abandoned held turn's follow-up has nowhere else to render"
        );
        assert_eq!(outstanding, 0);
        assert_eq!(
            settled.len(),
            1,
            "the notification must fire — nothing else will tell the user"
        );
        assert!(
            !settled[0].wire_visible,
            "an abnormally-ended turn released the id → reply not wire-visible, overlay shows it"
        );
    }

    /// A background shell launched while `Prompting` must NOT enter
    /// `current_turn_launched_ids`: #870 never holds a turn open for a shell,
    /// so a shell's owning turn ends via an ordinary `end_turn` while the
    /// shell keeps running. If the shell's id were suppression-eligible, its
    /// eventual completion would be silently swallowed for the shell's entire
    /// realistic runtime — content that was never on the wire to begin with.
    #[test]
    fn background_shell_launched_while_prompting_is_never_suppression_eligible() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&bash_ack("shell1")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        // Registered while prompting — same moment an async agent ack would
        // have entered `current_turn_launched_ids`.
        let _ = tick_prompting(&mut ws, &ledger);
        assert!(
            !ws.current_turn_launched_ids.contains("shell1"),
            "a background shell must never be suppression-eligible"
        );

        // The turn ends normally (a shell's owning turn always does, per
        // #870 never holding for shells) with no new turn since — under the
        // agent case this would still suppress (see the settlement-races
        // test above), but a shell's notification must always surface.
        let _ = tick_now(&mut ws, &ledger);
        write_lines(
            &path,
            &[&notification("shell1", "completed"), &assistant_text("a1", "Done.")],
        );
        let (turns, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).expect("settle event"));
        assert_eq!(turns.len(), 1, "a shell follow-up has nowhere else to render");
        assert_eq!(outstanding, 0);
        assert_eq!(
            settled.len(),
            1,
            "a shell's notification must never be suppressed"
        );
        assert!(
            !settled[0].wire_visible,
            "a shell is never in the launched set → not wire-visible"
        );
    }

    /// A `SendMessage`-resumed sub-agent must be suppression-eligible again if
    /// the RESUMING turn is itself held open by #870 for it — mirroring the
    /// launch-time insert. Without this, a resume-then-hold reproduces the
    /// same double-render the original launch-time tracking exists to
    /// prevent.
    #[test]
    fn resumed_agent_held_by_the_resuming_turn_is_suppressed() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agent1")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_prompting(&mut ws, &ledger); // agent1 launched while prompting (turn A)

        write_lines(&path, &[&notification("agent1", "completed")]);
        let _ = tick_prompting(&mut ws, &ledger); // settles within turn A — already suppressed

        // Turn A ends normally; no new turn yet, so the set still holds
        // agent1 (unbounded persistence, per the new design).
        let _ = tick_now(&mut ws, &ledger);

        // Turn B starts and, in its very first tick, resumes agent1 via
        // SendMessage — itself held open by #870 for the resumed work. The
        // rising edge clears the set BEFORE this line is processed; the
        // resume must re-insert agent1 within the same tick.
        let resume = r#"{"type":"assistant","timestamp":"2026-07-07T03:53:00.000Z","uuid":"a-send-resume","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_09","name":"SendMessage","input":{"to":"agent1","summary":"continue","message":"go on"}}]}}"#;
        write_lines(&path, &[resume]);
        let _ = tick_prompting(&mut ws, &ledger);
        assert!(
            ws.current_turn_launched_ids.contains("agent1"),
            "a resume issued by a held-open turn must re-enter the launched set"
        );

        write_lines(
            &path,
            &[
                &notification("agent1", "completed"),
                &assistant_text("a2", "Continued and finished."),
            ],
        );
        let (turns, outstanding, settled, _) =
            unpack(tick_prompting(&mut ws, &ledger).expect("settle event"));
        assert!(
            turns.is_empty(),
            "the resumed agent's second overlay notification must be suppressed too, got {turns:?}"
        );
        assert_eq!(outstanding, 0);
        // The settle still flows to re-flip the card for the resumed run.
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].tool_use_id.as_deref(), Some("toolu_01"));
        assert!(
            settled[0].wire_visible,
            "the resuming turn holds it open → wire-visible"
        );
    }

    /// A cron//loop autonomous turn has no originating task id at all (its
    /// initiator is plain injected text, not a `<task-notification>`), so it
    /// must never be caught by the held-turn suppression filter — even if,
    /// coincidentally, some OTHER turn happens to be `Prompting` when it
    /// fires.
    #[test]
    fn cron_followup_is_never_suppressed_even_while_prompting() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        write_lines(
            &path,
            &[
                &cron_prompt("iterate forever"),
                &assistant_text("a1", "Working on it."),
            ],
        );
        let (turns, ..) =
            unpack(tick_prompting(&mut ws, &ledger).expect("turns event"));
        assert_eq!(
            turns.len(),
            1,
            "a cron-originated turn has no task id to suppress on"
        );
    }

    /// A `<task-notification>` can name a task id that was launched (and
    /// settled) by a PAST, already-ended turn — not the turn currently
    /// `Prompting`. Only ids launched by the CURRENTLY active turn are
    /// suppression-eligible (`current_turn_launched_ids` clears on every
    /// rising edge), so this must render normally.
    #[test]
    fn notification_for_a_past_turns_task_is_not_suppressed_by_a_new_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agentA")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        // Turn A launches agentA while prompting...
        let _ = tick_prompting(&mut ws, &ledger);
        // ...then turn A ends (falls back to Connected) with no new lines.
        let _ = tick_now(&mut ws, &ledger);

        // Turn B starts (rising edge clears the launched-set) and, within its
        // own held-open window, agentA's late notification from turn A
        // arrives — it belongs to no id turn B itself launched.
        write_lines(
            &path,
            &[
                &notification("agentA", "completed"),
                &assistant_text("a1", "Build finished cleanly."),
            ],
        );
        let (turns, ..) =
            unpack(tick_prompting(&mut ws, &ledger).expect("settle event"));
        assert_eq!(
            turns.len(),
            1,
            "a foreign (past-turn) task id must not be suppressed by a different turn"
        );
    }

    /// The dominant real-world shell path: a background shell is launched, the
    /// agent awaits it with `TaskOutput{block:true}`, and the result's
    /// `task.status` goes terminal — with NO `<task-notification>` ever
    /// written. That collection must clear the outstanding count (the bug:
    /// only `<task-notification>` used to settle, so these stranded for the
    /// full keep-alive max-age). A non-terminal poll must NOT clear it.
    #[test]
    fn taskoutput_terminal_status_settles_background_shell() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&bash_ack("bash1")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).expect("ack event"));
        assert_eq!(outstanding, 1);

        // A non-blocking poll while still running must not touch the count.
        write_lines(&path, &[&taskoutput_result("bash1", "running")]);
        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "a running TaskOutput poll must not change accounting"
        );

        // The collected completion settles it — no notification involved.
        write_lines(&path, &[&taskoutput_result("bash1", "completed")]);
        let (_, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).expect("settle event"));
        assert_eq!(outstanding, 0, "TaskOutput completion must clear the count");
        assert!(
            settled.is_empty(),
            "inline-awaited collection must not raise an OS notification"
        );
    }

    /// An explicit `TaskStop` settles the task immediately — the process is
    /// gone and no completion notification will follow.
    #[test]
    fn taskstop_settles_background_task() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&bash_ack("bash9")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).expect("ack event"));
        assert_eq!(outstanding, 1);

        write_lines(&path, &[&taskstop("bash9")]);
        let (_, outstanding, settled, _) =
            unpack(tick_now(&mut ws, &ledger).expect("settle event"));
        assert_eq!(outstanding, 0, "TaskStop must clear the count");
        assert!(settled.is_empty());
    }

    #[test]
    fn codeg_sent_prompt_is_foreground_and_not_surfaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        ledger.record_text("修复登录 bug");
        write_lines(
            &path,
            &[
                &user_prompt_array("u1", "修复登录 bug"),
                &assistant_text("a1", "On it."),
            ],
        );
        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "foreground turn must not surface as overlay"
        );
    }

    #[test]
    fn same_text_refire_without_ledger_entry_is_background() {
        // The /loop case: codeg sent the text once (consumed), the scheduler
        // re-fires the SAME text later — second occurrence must surface.
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        ledger.record_text("查询武汉当前天气");
        write_lines(
            &path,
            &[
                &user_prompt_array("u1", "查询武汉当前天气"),
                &assistant_text("a1", "24°C 多云"),
            ],
        );
        assert!(tick_now(&mut ws, &ledger).is_none());

        write_lines(
            &path,
            &[
                &cron_prompt("查询武汉当前天气"),
                &assistant_text("a2", "25°C 晴"),
            ],
        );
        let (turns, ..) = unpack(tick_now(&mut ws, &ledger).expect("cron turn surfaces"));
        assert_eq!(turns.len(), 1, "cron assistant response renders as overlay");
    }

    #[test]
    fn meta_array_expansion_does_not_flip_mode() {
        // A slash-command expansion (isMeta + ARRAY content) belongs to the
        // foreground turn that issued the command.
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        ledger.record_text("/init");
        let command_record = r#"{"type":"user","timestamp":"2026-07-07T03:50:00.000Z","uuid":"u-cmd","message":{"role":"user","content":"<command-name>/init</command-name><command-message>init</command-message><command-args></command-args>"}}"#;
        let expansion = r#"{"type":"user","timestamp":"2026-07-07T03:50:00.100Z","uuid":"u-exp","isMeta":true,"message":{"role":"user","content":[{"type":"text","text":"Please analyze this codebase..."}]}}"#;
        write_lines(
            &path,
            &[
                command_record,
                expansion,
                &assistant_text("a1", "Analyzing..."),
            ],
        );
        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "slash command turn is foreground end-to-end"
        );
    }

    #[test]
    fn growing_turn_reemits_with_same_id_and_partial_lines_carry() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());

        write_lines(&path, &[&notification("t1", "completed")]);
        let _ = tick_now(&mut ws, &ledger); // settle-only event

        write_lines(&path, &[&assistant_text("a1", "step one")]);
        let (turns1, ..) = unpack(tick_now(&mut ws, &ledger).expect("first turn"));
        assert_eq!(turns1.len(), 1);
        let id1 = turns1[0].id.clone();

        // Append a PARTIAL line (no newline yet): nothing must surface.
        let more = assistant_text("a2", "step two");
        let (head, tail) = more.split_at(more.len() / 2);
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(head.as_bytes()).unwrap();
        }
        assert!(
            tick_now(&mut ws, &ledger).is_none(),
            "partial line must not parse"
        );

        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(tail.as_bytes()).unwrap();
            f.write_all(b"\n").unwrap();
        }
        let (turns2, ..) = unpack(tick_now(&mut ws, &ledger).expect("completed line surfaces"));
        // Same episode: a NEW assistant message is a NEW turn (bg-…-1); the
        // first turn's content didn't change so it is not re-emitted.
        assert_eq!(turns2.len(), 1);
        assert_ne!(turns2[0].id, id1);
        assert!(turns2[0].id.starts_with("bg-"));
    }

    #[test]
    fn truncation_rebaselines_without_stale_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&notification("t1", "completed")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger);

        std::fs::write(&path, b"").unwrap();
        // Shrink triggers re-baseline; no panic, no stale content.
        let event = tick_now(&mut ws, &ledger);
        if let Some(e) = event {
            let (turns, _, settled, watermark) = unpack(e);
            assert!(turns.is_empty());
            assert!(settled.is_empty());
            assert_eq!(watermark, 0);
        }
        write_lines(&path, &[&assistant_text("a9", "after rewrite")]);
        // Post-truncation content is foreground by default (no initiator seen).
        assert!(tick_now(&mut ws, &ledger).is_none());
    }

    #[test]
    fn send_message_rearms_settled_task() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_session(&dir);
        write_lines(&path, &[&agent_ack("agentX")]);
        let ledger = PromptLedger::shared();
        let mut ws = WatchState::with_file_for_test("s1", path.clone());
        let _ = tick_now(&mut ws, &ledger);

        write_lines(&path, &[&notification("agentX", "completed")]);
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 0);

        let send = r#"{"type":"assistant","timestamp":"2026-07-07T03:52:00.000Z","uuid":"a-send","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_03","name":"SendMessage","input":{"to":"agentX","summary":"continue","message":"go on"}}]}}"#;
        write_lines(&path, &[send]);
        let (_, outstanding, ..) = unpack(tick_now(&mut ws, &ledger).unwrap());
        assert_eq!(outstanding, 1, "resumed sub-agent re-arms the keep-alive");
    }

    #[test]
    fn ledger_prefix_matches_and_consumes_once() {
        let ledger = PromptLedger::shared();
        ledger.record_text("deploy the app");
        assert!(ledger.consume_matching("deploy the app\n<system-hint>extra</system-hint>"));
        assert!(
            !ledger.consume_matching("deploy the app"),
            "an entry is consumed exactly once"
        );
    }

    #[test]
    fn initiator_classification_ground_rules() {
        // tool-result-only user record: continues the turn.
        let ack: serde_json::Value = serde_json::from_str(&agent_ack("x")).unwrap();
        assert!(turn_initiator_text(&ack).is_none());

        // task-notification string record: initiates (raw text, no ledger hit).
        let note: serde_json::Value =
            serde_json::from_str(&notification("x", "completed")).unwrap();
        assert!(turn_initiator_text(&note)
            .unwrap()
            .starts_with("<task-notification>"));

        // cron prompt (isMeta + string): initiates with the prompt text.
        let cron: serde_json::Value = serde_json::from_str(&cron_prompt("check weather")).unwrap();
        assert_eq!(turn_initiator_text(&cron).as_deref(), Some("check weather"));

        // context-continuation summary: never a boundary.
        let cont = format!(
            r#"{{"type":"user","uuid":"u-cont","message":{{"role":"user","content":"{}..."}}}}"#,
            CONTEXT_CONTINUATION_PREFIX
        );
        let cont: serde_json::Value = serde_json::from_str(&cont).unwrap();
        assert!(turn_initiator_text(&cont).is_none());

        // slash command record matches via its display form.
        let cmd = r#"{"type":"user","uuid":"u-cmd","message":{"role":"user","content":"<command-name>/init</command-name><command-args>now</command-args>"}}"#;
        let cmd: serde_json::Value = serde_json::from_str(cmd).unwrap();
        assert_eq!(turn_initiator_text(&cmd).as_deref(), Some("/init now"));
    }
}
