//! codeg's own **ACP-native transcript** for custom agents.
//!
//! Every built-in agent ships a dedicated parser (`crate::parsers::*`) that
//! reverse-engineers that agent's private session store to rebuild history.
//! A custom ACP agent has no such parser — and writing one per agent is
//! exactly what "add an agent by pasting registry info" is meant to avoid.
//!
//! The observation that makes this unnecessary is the same one
//! [`crate::turn_timings`] already relies on: **every session runs through
//! codeg**, so the connection layer witnesses the entire conversation. Here we
//! take it all the way — instead of recording a derived measurement, we record
//! the ACP wire itself:
//!
//! * each prompt codeg sends (`session/prompt`'s content blocks), and
//! * each `session/update` notification the agent sends back, verbatim.
//!
//! [`crate::parsers::acp_native`] reads that back and projects it into
//! `MessageTurn`s using nothing but ACP semantics — no agent-specific
//! knowledge anywhere in the loop.
//!
//! ## Why raw ACP and not codeg's own `AcpEvent`
//!
//! `AcpEvent` is an internal type that changes with every UI feature; persisting
//! it would silently corrupt old history on refactors. The ACP `SessionUpdate`
//! wire format is an external, versioned specification, so a transcript written
//! today still parses after arbitrary internal churn.
//!
//! ## File layout
//!
//! `<paths::codeg_acp_transcripts_root()>/<registry-id>/<session-id>.jsonl`
//!
//! Line 0 is a [`TranscriptHeader`]; every later line is a [`TranscriptEntry`]:
//!
//! ```jsonc
//! {"v":1,"kind":"header","agent":"custom:goose","session_id":"…","cwd":"/repo","started_at_ms":…}
//! {"t":1750000000000,"k":"prompt","p":[{"type":"text","text":"hi"}]}
//! {"t":1750000000123,"k":"update","p":{"sessionUpdate":"agent_message_chunk", …}}
//! ```
//!
//! Unknown/malformed lines are skipped by the reader, and a missing file simply
//! means "no history" — recording is best-effort by contract and must never
//! block or fail a turn.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Schema version of the transcript file. Bump only for incompatible changes;
/// the reader skips lines whose version it does not understand.
pub const TRANSCRIPT_SCHEMA_VERSION: u32 = 1;

/// First line of a transcript file: everything about the session that is not
/// derivable from the ACP stream itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptHeader {
    pub v: u32,
    /// Always the literal `"header"`, so a reader can tell a header from an
    /// entry without positional assumptions (a file could in principle be
    /// concatenated across reconnects).
    pub kind: String,
    /// Wire form of the agent type, e.g. `custom:goose`.
    pub agent: String,
    pub session_id: String,
    /// Working directory the session was created in. The only source of a
    /// custom conversation's folder path when listing transcripts.
    pub cwd: String,
    pub started_at_ms: u64,
    /// Session id this transcript continues.
    ///
    /// Agents are not required to persist sessions, and many custom ones keep
    /// them in memory only — so after a restart `session/load` fails and codeg
    /// starts a fresh agent session for the SAME conversation. The turns codeg
    /// already recorded are not lost by that; they simply live under the old
    /// session id. Rather than move them (a rename could be interrupted between
    /// the file operation and the `external_id` write, stranding the history),
    /// the new transcript points back and the reader concatenates the chain.
    ///
    /// `None` for a transcript that starts its own conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues_from: Option<String>,
}

impl TranscriptHeader {
    pub fn new(agent: &str, session_id: &str, cwd: &str, started_at_ms: u64) -> Self {
        Self {
            v: TRANSCRIPT_SCHEMA_VERSION,
            kind: "header".to_string(),
            agent: agent.to_string(),
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            started_at_ms,
            continues_from: None,
        }
    }

    /// Mark this transcript as the continuation of `previous_session_id`.
    pub fn continuing(mut self, previous_session_id: &str) -> Self {
        self.continues_from = Some(previous_session_id.to_string());
        self
    }
}

/// What a transcript line records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A prompt codeg sent to the agent. `payload` is the ACP content-block
    /// array from `session/prompt`.
    Prompt,
    /// A `session/update` notification from the agent. `payload` is the
    /// serialized `SessionUpdate`, verbatim.
    Update,
    /// End of a prompt turn, as reported by the `session/prompt` **response**.
    /// ACP has no turn-end notification, so without this line turn boundaries
    /// could only be inferred; codeg is the party that receives the response,
    /// so it records it. Payload: `{"stopReason":…, "durationMs":…,
    /// "model":…, "usage":{…}}` (all optional but `stopReason`).
    ///
    /// Absent from transcripts hydrated by a `session/load` replay — the agent
    /// replays notifications only — where boundaries fall back to user message
    /// chunks.
    TurnEnd,
}

/// One recorded line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Epoch milliseconds when codeg observed this.
    pub t: u64,
    pub k: EntryKind,
    pub p: serde_json::Value,
}

/// A parsed transcript: its header (when present and readable) plus every
/// well-formed entry, in file order.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub header: Option<TranscriptHeader>,
    pub entries: Vec<TranscriptEntry>,
}

impl Transcript {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Milliseconds since the Unix epoch; saturates rather than panicking on a
/// pre-1970 clock.
pub fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Whether `s` is safe as a single path component. Mirrors
/// `turn_timings::safe_component` — the registry id and the session id both
/// come from outside codeg (user input / the agent's own id generator), so
/// neither may introduce a separator, a parent ref, or a hidden-file prefix.
fn safe_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !s.starts_with('.')
}

/// `<root>/<agent_dir>/<session_id>.jsonl`, or `None` when either component is
/// unsafe as a file name.
pub fn transcript_path_in(root: &Path, agent_dir: &str, session_id: &str) -> Option<PathBuf> {
    if !safe_component(agent_dir) || !safe_component(session_id) {
        return None;
    }
    Some(root.join(agent_dir).join(format!("{session_id}.jsonl")))
}

/// One queued append. `ack` fires once the line is on disk so the connection
/// layer can bound-wait at turn end — without it, a conversation reopened
/// immediately after a turn could miss its tail.
struct TranscriptJob {
    root: PathBuf,
    agent_dir: String,
    session_id: String,
    line: String,
    ack: tokio::sync::oneshot::Sender<()>,
}

/// Bound on queued appends. Transcript lines are far more frequent than turn
/// timings (one per streamed chunk), so this is generous; overflow drops lines
/// with a debug log rather than ever blocking the ACP read loop.
const TRANSCRIPT_QUEUE_CAP: usize = 8192;

static TRANSCRIPT_TX: std::sync::OnceLock<std::sync::mpsc::SyncSender<TranscriptJob>> =
    std::sync::OnceLock::new();

/// Queue one serialized line for a session's transcript, returning a receiver
/// that resolves once it has landed.
///
/// All production appends funnel through a single dedicated OS thread, so file
/// order equals enqueue order — which is the transcript's whole contract, since
/// the parser reconstructs turns positionally. A hung filesystem blocks only
/// that thread (never a tokio pool); the queue then fills and later lines are
/// dropped.
fn enqueue_line(
    root: PathBuf,
    agent_dir: String,
    session_id: String,
    line: String,
) -> tokio::sync::oneshot::Receiver<()> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    let tx = TRANSCRIPT_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<TranscriptJob>(TRANSCRIPT_QUEUE_CAP);
        let spawned = std::thread::Builder::new()
            .name("acp-transcript-writer".into())
            .spawn(move || {
                for job in rx {
                    append_line_in(&job.root, &job.agent_dir, &job.session_id, &job.line);
                    let _ = job.ack.send(());
                }
            });
        if let Err(e) = spawned {
            tracing::debug!("[acp-transcript] failed to spawn writer thread: {e}");
        }
        tx
    });
    if tx
        .try_send(TranscriptJob {
            root,
            agent_dir,
            session_id,
            line,
            ack: ack_tx,
        })
        .is_err()
    {
        tracing::debug!("[acp-transcript] queue full or closed; line dropped");
    }
    ack_rx
}

/// Record the session header, creating the transcript file if needed.
///
/// Idempotent per file: if the file already has content (a reconnect to the
/// same session id), nothing is written — the original header stands.
pub fn record_header(
    agent_dir: &str,
    header: &TranscriptHeader,
) -> tokio::sync::oneshot::Receiver<()> {
    record_header_in(&crate::paths::codeg_acp_transcripts_root(), agent_dir, header)
}

/// Root-injectable core of [`record_header`].
pub fn record_header_in(
    root: &Path,
    agent_dir: &str,
    header: &TranscriptHeader,
) -> tokio::sync::oneshot::Receiver<()> {
    // The emptiness probe is racy in principle, but a session id has exactly
    // one live connection, so the only writer is this process's writer thread.
    if let Some(path) = transcript_path_in(root, agent_dir, &header.session_id) {
        if std::fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false) {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(());
            return rx;
        }
    }
    let line = serde_json::to_string(header).unwrap_or_default();
    enqueue_line(
        root.to_path_buf(),
        agent_dir.to_string(),
        header.session_id.clone(),
        line,
    )
}

/// Record one prompt or update.
pub fn record_entry(
    agent_dir: &str,
    session_id: &str,
    kind: EntryKind,
    payload: serde_json::Value,
) -> tokio::sync::oneshot::Receiver<()> {
    record_entry_in(
        &crate::paths::codeg_acp_transcripts_root(),
        agent_dir,
        session_id,
        kind,
        payload,
    )
}

/// Root-injectable core of [`record_entry`].
pub fn record_entry_in(
    root: &Path,
    agent_dir: &str,
    session_id: &str,
    kind: EntryKind,
    payload: serde_json::Value,
) -> tokio::sync::oneshot::Receiver<()> {
    let entry = TranscriptEntry {
        t: now_epoch_ms(),
        k: kind,
        p: payload,
    };
    let line = serde_json::to_string(&entry).unwrap_or_default();
    enqueue_line(
        root.to_path_buf(),
        agent_dir.to_string(),
        session_id.to_string(),
        line,
    )
}

/// Append one already-serialized line. Best-effort: every failure is logged at
/// debug and swallowed. Called by the writer thread (production) and directly
/// by tests building fixtures.
pub fn append_line_in(root: &Path, agent_dir: &str, session_id: &str, line: &str) {
    let Some(path) = transcript_path_in(root, agent_dir, session_id) else {
        tracing::debug!(
            "[acp-transcript] skipping append: unsafe id agent={agent_dir} session={session_id}"
        );
        return;
    };
    if line.is_empty() {
        return;
    }
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        f.write_all(format!("{line}\n").as_bytes())
    };
    if let Err(e) = write() {
        tracing::debug!(
            "[acp-transcript] append failed for {}: {e}",
            path.display()
        );
    }
}

/// Read one session's transcript. Missing file → empty transcript. Malformed
/// lines are skipped so a single truncated write (e.g. a hard kill mid-append)
/// cannot poison the rest of the history.
pub fn read_transcript(agent_dir: &str, session_id: &str) -> Transcript {
    read_transcript_in(
        &crate::paths::codeg_acp_transcripts_root(),
        agent_dir,
        session_id,
    )
}

/// Root-injectable core of [`read_transcript`].
pub fn read_transcript_in(root: &Path, agent_dir: &str, session_id: &str) -> Transcript {
    let Some(path) = transcript_path_in(root, agent_dir, session_id) else {
        return Transcript::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Transcript::default();
    };
    // Decoded lossily rather than read as a `String`: `append_line_in` writes a
    // line with one `write_all`, so a hard kill can tear it in the middle of a
    // multi-byte character. Rejecting the file for that would throw away every
    // turn recorded before it; lossily, the torn line alone fails to parse and
    // is skipped like any other malformed line. It also keeps this in step with
    // the streaming readers below, which cannot see a later byte before
    // returning an earlier answer.
    parse_transcript(&String::from_utf8_lossy(&bytes))
}

/// Parse transcript file content. Split out so tests (and the reader) share one
/// definition of "what a valid line is".
pub fn parse_transcript(content: &str) -> Transcript {
    let mut out = Transcript::default();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // A header is distinguished by its `kind` field, not its position.
        if out.header.is_none() {
            if let Some(header) = parse_header_line(line) {
                out.header = Some(header);
                continue;
            }
        }
        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
            out.entries.push(entry);
        }
    }
    out
}

/// A header line, or `None` when this line is something else. The one
/// definition of "is this a header", shared by the whole-file parser and the
/// streaming readers below so the two can never disagree.
fn parse_header_line(line: &str) -> Option<TranscriptHeader> {
    let header = serde_json::from_str::<TranscriptHeader>(line).ok()?;
    (header.kind == "header" && header.v == TRANSCRIPT_SCHEMA_VERSION).then_some(header)
}

/// Walk a transcript's non-empty lines, stopping as soon as `visit` returns
/// `false`. Nothing is materialized, so a reader that only needs the head of a
/// file pays for the head of a file.
///
/// Lines are split on `\n` and decoded lossily, exactly as
/// [`read_transcript_in`] does — a stopped-early reader can never look at a
/// later byte, so the two must not disagree about anything a reader can reach.
/// (The split is safe to do before decoding: `\n` cannot occur inside a
/// multi-byte UTF-8 sequence, so per-line lossy decoding and whole-file lossy
/// decoding produce the same lines.)
fn scan_lines(path: &Path, mut visit: impl FnMut(&str) -> bool) {
    use std::io::BufRead as _;
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut reader = std::io::BufReader::new(file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let decoded = String::from_utf8_lossy(&buf);
        let line = decoded.trim();
        if line.is_empty() {
            continue;
        }
        if !visit(line) {
            return;
        }
    }
}

/// A transcript's header WITHOUT parsing its entries.
///
/// Same answer as `read_transcript_in(..).header`, but it stops at the header
/// line instead of deserializing every recorded chunk — which is what
/// [`superseded_session_ids_in`] needs from every file in a directory.
pub fn read_header_in(root: &Path, agent_dir: &str, session_id: &str) -> Option<TranscriptHeader> {
    let path = transcript_path_in(root, agent_dir, session_id)?;
    let mut found = None;
    scan_lines(&path, |line| {
        found = parse_header_line(line);
        // Keep looking until a header parses: the whole-file parser accepts one
        // at any position, so this must too.
        found.is_none()
    });
    found
}

/// True when a transcript already holds at least one entry — the gate that
/// keeps a `session/load` replay from duplicating history codeg already
/// recorded live.
pub fn has_entries(agent_dir: &str, session_id: &str) -> bool {
    has_entries_in(
        &crate::paths::codeg_acp_transcripts_root(),
        agent_dir,
        session_id,
    )
}

/// Root-injectable core of [`has_entries`]. Returns at the first entry rather
/// than parsing the file to answer a yes/no — this runs on every reconnect,
/// where the transcript is at its longest.
pub fn has_entries_in(root: &Path, agent_dir: &str, session_id: &str) -> bool {
    let Some(path) = transcript_path_in(root, agent_dir, session_id) else {
        return false;
    };
    let mut header_seen = false;
    let mut found = false;
    scan_lines(&path, |line| {
        // Mirrors the whole-file parser: the header is consumed, not counted.
        if !header_seen && parse_header_line(line).is_some() {
            header_seen = true;
            return true;
        }
        found = serde_json::from_str::<TranscriptEntry>(line).is_ok();
        !found
    });
    found
}

/// Bound on how far [`read_chain_in`] follows `continues_from`. One link is
/// added per failed `session/load`, i.e. roughly per app restart of a
/// long-lived conversation, so this is far above any real chain — it exists
/// only so a hand-edited or corrupted file cannot make a read unbounded.
pub const MAX_CONTINUATION_DEPTH: usize = 512;

/// Read a session's transcript **together with everything it continues** (see
/// [`TranscriptHeader::continues_from`]), oldest entry first.
///
/// The returned header is the OLDEST one in the chain: it carries the
/// conversation's real start time and working directory, which is what callers
/// summarize from. Cycles and over-long chains stop the walk instead of hanging.
pub fn read_chain_in(root: &Path, agent_dir: &str, session_id: &str) -> Transcript {
    let mut chain: Vec<Transcript> = Vec::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cursor = Some(session_id.to_string());

    while let Some(id) = cursor.take() {
        if !visited.insert(id.clone()) {
            tracing::debug!("[acp-transcript] continuation cycle at {id}; stopping walk");
            break;
        }
        if chain.len() >= MAX_CONTINUATION_DEPTH {
            tracing::debug!(
                "[acp-transcript] continuation chain exceeded {MAX_CONTINUATION_DEPTH}; truncating"
            );
            break;
        }
        let transcript = read_transcript_in(root, agent_dir, &id);
        cursor = transcript
            .header
            .as_ref()
            .and_then(|h| h.continues_from.clone());
        chain.push(transcript);
    }

    // `chain` is newest → oldest; replay it the other way so entry order stays
    // chronological and the oldest header wins.
    let mut merged = Transcript::default();
    for transcript in chain.iter_mut().rev() {
        if merged.header.is_none() {
            merged.header = transcript.header.take();
        }
        merged.entries.append(&mut transcript.entries);
    }
    merged
}

/// Session ids under `<root>/<agent_dir>/` that some other transcript continues.
///
/// A superseded transcript is a prefix of its successor, so listing both would
/// show one conversation twice.
///
/// Reads headers only. This runs over every file in the directory before the
/// listing then reads the surviving chains in full, so parsing entries here
/// would deserialize the agent's whole recorded history twice per listing.
pub fn superseded_session_ids_in(
    root: &Path,
    agent_dir: &str,
) -> std::collections::HashSet<String> {
    list_session_ids_in(root, agent_dir)
        .into_iter()
        .filter_map(|id| read_header_in(root, agent_dir, &id))
        .filter_map(|h| h.continues_from)
        .collect()
}

/// Every session id with a transcript under `<root>/<agent_dir>/`. Used by the
/// generic parser's `list_conversations`.
pub fn list_session_ids_in(root: &Path, agent_dir: &str) -> Vec<String> {
    if !safe_component(agent_dir) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root.join(agent_dir)) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".jsonl").map(str::to_string)
        })
        .collect();
    ids.sort();
    ids
}

/// Delete a custom agent's whole transcript directory. Called when the user
/// removes the agent definition and asks for its data to go with it.
pub fn remove_agent_transcripts_in(root: &Path, agent_dir: &str) -> std::io::Result<()> {
    if !safe_component(agent_dir) {
        return Ok(());
    }
    let dir = root.join(agent_dir);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "codeg-acp-transcript-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn header_line(session: &str) -> String {
        serde_json::to_string(&TranscriptHeader::new(
            "custom:goose",
            session,
            "/repo",
            1_750_000_000_000,
        ))
        .unwrap()
    }

    fn entry_line(kind: EntryKind, payload: serde_json::Value) -> String {
        serde_json::to_string(&TranscriptEntry {
            t: 1_750_000_000_001,
            k: kind,
            p: payload,
        })
        .unwrap()
    }

    #[test]
    fn round_trips_header_and_entries() {
        let root = temp_root();
        append_line_in(&root, "goose", "s1", &header_line("s1"));
        append_line_in(
            &root,
            "goose",
            "s1",
            &entry_line(EntryKind::Prompt, serde_json::json!([{"type":"text","text":"hi"}])),
        );
        append_line_in(
            &root,
            "goose",
            "s1",
            &entry_line(
                EntryKind::Update,
                serde_json::json!({"sessionUpdate":"agent_message_chunk"}),
            ),
        );

        let t = read_transcript_in(&root, "goose", "s1");
        assert!(!t.is_empty());
        let header = t.header.clone().expect("header parsed");
        assert_eq!(header.agent, "custom:goose");
        assert_eq!(header.cwd, "/repo");
        assert_eq!(t.entries.len(), 2);
        assert_eq!(t.entries[0].k, EntryKind::Prompt);
        assert_eq!(t.entries[1].k, EntryKind::Update);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn skips_malformed_lines_without_losing_the_rest() {
        let root = temp_root();
        append_line_in(&root, "goose", "s2", &header_line("s2"));
        // A truncated write (hard kill mid-append) plus outright garbage.
        append_line_in(&root, "goose", "s2", "{\"t\":123,\"k\":\"pro");
        append_line_in(&root, "goose", "s2", "not json at all");
        append_line_in(
            &root,
            "goose",
            "s2",
            &entry_line(EntryKind::Update, serde_json::json!({"a":1})),
        );

        let t = read_transcript_in(&root, "goose", "s2");
        assert!(t.header.is_some());
        assert_eq!(t.entries.len(), 1, "only the well-formed entry survives");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let root = temp_root();
        let t = read_transcript_in(&root, "goose", "never-written");
        assert!(t.header.is_none());
        assert!(t.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_unsafe_path_components() {
        let root = temp_root();
        assert!(transcript_path_in(&root, "..", "s").is_none());
        assert!(transcript_path_in(&root, "a/b", "s").is_none());
        assert!(transcript_path_in(&root, "goose", "../escape").is_none());
        assert!(transcript_path_in(&root, ".hidden", "s").is_none());
        assert!(transcript_path_in(&root, "goose", "s1").is_some());
        // An unsafe component must not create anything on disk.
        append_line_in(&root, "..", "s", &header_line("s"));
        assert!(read_transcript_in(&root, "..", "s").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lists_and_removes_session_transcripts() {
        let root = temp_root();
        append_line_in(&root, "goose", "beta", &header_line("beta"));
        append_line_in(&root, "goose", "alpha", &header_line("alpha"));
        assert_eq!(list_session_ids_in(&root, "goose"), vec!["alpha", "beta"]);

        remove_agent_transcripts_in(&root, "goose").unwrap();
        assert!(list_session_ids_in(&root, "goose").is_empty());
        // Removing a directory that never existed is a no-op, not an error.
        remove_agent_transcripts_in(&root, "goose").unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn continuation_chain_reads_as_one_conversation() {
        let root = temp_root();
        // Two restarts: s0 → s1 → s2, each continuing the previous.
        append_line_in(&root, "goose", "s0", &header_line("s0"));
        append_line_in(
            &root,
            "goose",
            "s0",
            &entry_line(EntryKind::Prompt, serde_json::json!([{"type":"text","text":"first"}])),
        );
        for (id, prev, text) in [("s1", "s0", "second"), ("s2", "s1", "third")] {
            let header = TranscriptHeader::new("custom:goose", id, "/elsewhere", 9_999)
                .continuing(prev);
            append_line_in(&root, "goose", id, &serde_json::to_string(&header).unwrap());
            append_line_in(
                &root,
                "goose",
                id,
                &entry_line(EntryKind::Prompt, serde_json::json!([{"type":"text","text":text}])),
            );
        }

        let merged = read_chain_in(&root, "goose", "s2");
        assert_eq!(merged.entries.len(), 3, "every link contributes its entries");
        let header = merged.header.expect("oldest header survives");
        assert_eq!(header.session_id, "s0", "the root header wins");
        assert_eq!(
            header.cwd, "/repo",
            "start time and cwd come from where the conversation actually began"
        );
        assert_eq!(header.started_at_ms, 1_750_000_000_000);

        // Reading a mid-chain id yields only its own prefix.
        assert_eq!(read_chain_in(&root, "goose", "s1").entries.len(), 2);
        assert_eq!(read_chain_in(&root, "goose", "s0").entries.len(), 1);

        // The superseded links are exactly the ones another transcript continues.
        let superseded = superseded_session_ids_in(&root, "goose");
        assert_eq!(superseded.len(), 2);
        assert!(superseded.contains("s0") && superseded.contains("s1"));
        assert!(!superseded.contains("s2"), "the head is never superseded");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn continuation_walk_survives_a_cycle_and_a_missing_link() {
        let root = temp_root();
        // A hand-edited file pointing at itself must not hang the reader.
        let looped = TranscriptHeader::new("custom:goose", "loop", "/repo", 1).continuing("loop");
        append_line_in(&root, "goose", "loop", &serde_json::to_string(&looped).unwrap());
        append_line_in(
            &root,
            "goose",
            "loop",
            &entry_line(EntryKind::Update, serde_json::json!({"a":1})),
        );
        assert_eq!(read_chain_in(&root, "goose", "loop").entries.len(), 1);

        // Pointing at a transcript the user deleted degrades to "no prefix",
        // never to an error or an empty result.
        let orphan =
            TranscriptHeader::new("custom:goose", "orphan", "/repo", 2).continuing("deleted");
        append_line_in(&root, "goose", "orphan", &serde_json::to_string(&orphan).unwrap());
        append_line_in(
            &root,
            "goose",
            "orphan",
            &entry_line(EntryKind::Update, serde_json::json!({"b":2})),
        );
        let merged = read_chain_in(&root, "goose", "orphan");
        assert_eq!(merged.entries.len(), 1);
        assert_eq!(
            merged.header.expect("own header stands in").session_id,
            "orphan"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn header_only_read_agrees_with_the_whole_file_parser() {
        let root = temp_root();
        // Normal file: header first, then entries.
        append_line_in(&root, "goose", "h1", &header_line("h1"));
        append_line_in(
            &root,
            "goose",
            "h1",
            &entry_line(EntryKind::Update, serde_json::json!({"a": 1})),
        );
        assert_eq!(
            read_header_in(&root, "goose", "h1"),
            read_transcript_in(&root, "goose", "h1").header
        );

        // A header preceded by junk is still found, exactly as the whole-file
        // parser finds it.
        append_line_in(&root, "goose", "h2", "not json at all");
        append_line_in(&root, "goose", "h2", &header_line("h2"));
        assert_eq!(
            read_header_in(&root, "goose", "h2"),
            read_transcript_in(&root, "goose", "h2").header
        );
        assert!(read_header_in(&root, "goose", "h2").is_some());

        // Headerless and missing files both yield nothing, not an error.
        append_line_in(
            &root,
            "goose",
            "h3",
            &entry_line(EntryKind::Prompt, serde_json::json!([])),
        );
        assert_eq!(read_header_in(&root, "goose", "h3"), None);
        assert_eq!(read_header_in(&root, "goose", "never-written"), None);
        // An unsafe component must not read anything.
        assert_eq!(read_header_in(&root, "..", "h1"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn has_entries_sees_past_the_header_and_stops_at_the_first_entry() {
        let root = temp_root();
        // A session whose ACP header was written but which was closed before
        // the user sent anything: no entries.
        append_line_in(&root, "goose", "e1", &header_line("e1"));
        assert!(!has_entries_in(&root, "goose", "e1"));

        append_line_in(
            &root,
            "goose",
            "e1",
            &entry_line(EntryKind::Prompt, serde_json::json!([{"type":"text","text":"hi"}])),
        );
        assert!(has_entries_in(&root, "goose", "e1"));

        // Garbage alone is not an entry, and a missing file is not either.
        append_line_in(&root, "goose", "e2", "not json at all");
        assert!(!has_entries_in(&root, "goose", "e2"));
        assert!(!has_entries_in(&root, "goose", "never-written"));
        assert!(!has_entries_in(&root, "..", "e1"));

        // Agrees with the whole-file parser on every file written above.
        for id in ["e1", "e2", "never-written"] {
            assert_eq!(
                has_entries_in(&root, "goose", id),
                !read_transcript_in(&root, "goose", id).is_empty(),
                "streaming and whole-file emptiness must agree for {id}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_line_torn_mid_character_costs_only_that_line() {
        // `append_line_in` writes a line with a single `write_all`, so a hard
        // kill can cut it inside a multi-byte character — which is easy to hit
        // with CJK content. That must cost the torn line, not the whole
        // history, and every reader must agree on what survives.
        let root = temp_root();
        let path = transcript_path_in(&root, "goose", "torn").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut raw = Vec::new();
        raw.extend_from_slice(header_line("torn").as_bytes());
        raw.push(b'\n');
        raw.extend_from_slice(
            entry_line(EntryKind::Prompt, serde_json::json!([{"type":"text","text":"你好"}]))
                .as_bytes(),
        );
        raw.push(b'\n');
        // A prompt line cut in the middle of a 3-byte character.
        let torn = entry_line(EntryKind::Prompt, serde_json::json!([{"type":"text","text":"世界"}]));
        let cut = torn.find('世').unwrap() + 2;
        raw.extend_from_slice(&torn.as_bytes()[..cut]);
        std::fs::write(&path, &raw).unwrap();

        let transcript = read_transcript_in(&root, "goose", "torn");
        assert!(
            transcript.header.is_some(),
            "the header must survive a torn line later in the file"
        );
        assert_eq!(transcript.entries.len(), 1, "the intact prompt survives");

        // The streaming readers must reach the same conclusions, since they
        // return before ever seeing the torn bytes.
        assert_eq!(read_header_in(&root, "goose", "torn"), transcript.header);
        assert_eq!(
            has_entries_in(&root, "goose", "torn"),
            !transcript.is_empty()
        );

        // The same tear in the FIRST line: no header, and the readers still agree.
        let path2 = transcript_path_in(&root, "goose", "torn2").unwrap();
        let header = header_line("torn2");
        let cut2 = header.find("custom").unwrap() + 3;
        std::fs::write(&path2, &header.as_bytes()[..cut2]).unwrap();
        let t2 = read_transcript_in(&root, "goose", "torn2");
        assert_eq!(read_header_in(&root, "goose", "torn2"), t2.header);
        assert_eq!(has_entries_in(&root, "goose", "torn2"), !t2.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_corrupt_link_cannot_hide_the_conversation_it_continues() {
        // `superseded_session_ids_in` reads headers only. If it could see a
        // header that the full read does not, it would hide `old` while `new`
        // read as empty — losing both from the listing.
        let root = temp_root();
        let path = transcript_path_in(&root, "goose", "new").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let header =
            serde_json::to_string(&TranscriptHeader::new("custom:goose", "new", "/repo", 1).continuing("old"))
                .unwrap();
        let mut raw = Vec::from(header.as_bytes());
        raw.push(b'\n');
        raw.extend_from_slice(&[0xff, 0xfe, b'\n']);
        std::fs::write(&path, &raw).unwrap();

        append_line_in(&root, "goose", "old", &header_line("old"));
        append_line_in(
            &root,
            "goose",
            "old",
            &entry_line(EntryKind::Prompt, serde_json::json!([{"type":"text","text":"hi"}])),
        );

        // `new` is readable despite the garbage line, so `old` is legitimately
        // superseded and its turns are reachable through the chain.
        assert!(superseded_session_ids_in(&root, "goose").contains("old"));
        assert_eq!(read_chain_in(&root, "goose", "new").entries.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn headers_written_before_continuation_existed_still_parse() {
        // A v1 header from before `continues_from` was added: the field is
        // absent, not null, and must not fail the parse.
        let legacy = r#"{"v":1,"kind":"header","agent":"custom:glm-acp-agent","session_id":"8e73","cwd":"/repo","started_at_ms":1785023133723}"#;
        let parsed = parse_transcript(legacy);
        let header = parsed.header.expect("legacy header parses");
        assert_eq!(header.session_id, "8e73");
        assert_eq!(header.continues_from, None);
    }

    #[test]
    fn header_is_written_once_per_session() {
        let root = temp_root();
        let header = TranscriptHeader::new("custom:goose", "s3", "/repo", 1);
        // Direct append (as the writer thread would), then a second attempt
        // through the idempotent front door.
        append_line_in(&root, "goose", "s3", &serde_json::to_string(&header).unwrap());
        let second = TranscriptHeader::new("custom:goose", "s3", "/other", 2);
        // The ack receiver is dropped on purpose: the file-non-empty short
        // circuit means nothing is queued, so there is nothing to await.
        drop(record_header_in(&root, "goose", &second));
        let t = read_transcript_in(&root, "goose", "s3");
        assert_eq!(
            t.header.unwrap().cwd,
            "/repo",
            "a reconnect must not overwrite the original header"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
