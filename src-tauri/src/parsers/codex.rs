use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use walkdir::WalkDir;

use crate::models::*;
use crate::parsers::codex_code_mode::{
    extract_chunk_ids, extract_shell_session_ids, is_code_mode_call, parse_code_mode_script,
    script_card_input,
    split_code_mode_output, with_note, CodeModeCall, CodeModeOutput, CodeModeScript, ScriptStatus,
    Separator, CODEX_SCRIPT_TOOL_NAME,
};
use crate::parsers::{
    folder_name_from_path, title_from_user_text, truncate_str, AgentParser, ParseError,
};

pub struct CodexParser {
    base_dir: PathBuf,
}

impl Default for CodexParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexParser {
    pub fn new() -> Self {
        let base_dir = resolve_codex_home_dir().join("sessions");
        Self { base_dir }
    }

    /// Test-only constructor that lets callers point the parser at a fixture
    /// directory instead of `~/.codex/sessions`.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Load Codex's append-only session title index. The transcript remains the
    /// fallback source, so a missing/unreadable index or a malformed line is
    /// deliberately ignored. Later non-empty records for the same session win.
    pub(crate) fn load_thread_name_index(&self) -> HashMap<String, String> {
        let mut titles = HashMap::new();
        let Some(home_dir) = self.base_dir.parent() else {
            return titles;
        };
        let Ok(file) = fs::File::open(home_dir.join("session_index.jsonl")) else {
            return titles;
        };

        for line in BufReader::new(file).lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let session_id = value.get("id").and_then(serde_json::Value::as_str);
            let thread_name = value
                .get("thread_name")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty());
            if let (Some(id), Some(name)) = (session_id, thread_name) {
                titles.insert(id.to_string(), truncate_str(name, 100));
            }
        }

        titles
    }

    fn parse_jsonl_summary(
        &self,
        path: &PathBuf,
    ) -> Result<Option<ConversationSummary>, ParseError> {
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);

        let mut conversation_id: Option<String> = None;
        let mut cwd: Option<String> = None;
        let mut git_branch: Option<String> = None;
        let mut model: Option<String> = None;
        let mut title: Option<String> = None;
        let mut _cli_version: Option<String> = None;
        let mut first_timestamp: Option<DateTime<Utc>> = None;
        let mut last_timestamp: Option<DateTime<Utc>> = None;
        let mut message_count: u32 = 0;
        // Mirror the detail parser's leading-`/goal` fallback in the lightweight
        // list path: newer codex records `/goal` only as `thread_goal_updated` (no
        // `user_message`), so without this the sidebar/import entry is titleless
        // and under-counted. Decide it POSITIONALLY, exactly like the detail
        // parser: the goal is the opener iff no real user turn preceded it, and it
        // then supplies the title + one synthetic-turn count even when a LATER real
        // reply (e.g. "确认") exists. `has_real_user` tracks the same real-user-turn
        // sources detail uses (an `event_msg.user_message`, or an image-bearing
        // `response_item` user); `goal_objective` latches the first opening goal;
        // `goal_opens_session` snapshots whether it opened the session.
        let mut has_real_user = false;
        let mut goal_objective: Option<String> = None;
        let mut goal_opens_session = false;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }

            let value: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let msg_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");

            if let Some(ts_str) = value.get("timestamp").and_then(|t| t.as_str()) {
                if let Ok(ts) = ts_str.parse::<DateTime<Utc>>() {
                    if first_timestamp.is_none() {
                        first_timestamp = Some(ts);
                    }
                    last_timestamp = Some(ts);
                }
            }

            match msg_type {
                "session_meta" => {
                    if let Some(payload) = value.get("payload") {
                        conversation_id = payload
                            .get("id")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                        cwd = payload
                            .get("cwd")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                        _cli_version = payload
                            .get("cli_version")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                        git_branch = payload
                            .get("git")
                            .and_then(|g| g.get("branch"))
                            .and_then(|b| b.as_str())
                            .map(|s| s.to_string());
                    }
                }
                "turn_context" if model.is_none() => {
                    model = value
                        .get("payload")
                        .and_then(|p| p.get("model"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string());
                }
                "event_msg" => {
                    if let Some(payload) = value.get("payload") {
                        let payload_type =
                            payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match payload_type {
                            "user_message" => {
                                message_count += 1;
                                has_real_user = true;
                                if title.is_none() {
                                    title = payload
                                        .get("message")
                                        .and_then(|m| m.as_str())
                                        .and_then(|text| extract_codex_title_candidate(text, true));
                                }
                            }
                            "agent_message" => {
                                message_count += 1;
                            }
                            "thread_goal_updated" => {
                                // Capture the first OPENING goal for the fallback,
                                // through the SAME shared mapping the detail parser
                                // uses — so the summary keys off exactly the objective
                                // the detail parser would synthesize from: only a
                                // `create_goal` (an active goal with an objective),
                                // never a `goal:null` clear, a blank objective, or a
                                // terminal-status goal.
                                if goal_objective.is_none() {
                                    if let Some(marker) = payload
                                        .get("goal")
                                        .and_then(crate::acp::codex_goal::goal_marker)
                                    {
                                        if marker.tool_name == "create_goal" {
                                            // Positional, mirroring the detail parser:
                                            // the goal opened the session iff no real
                                            // user turn preceded it. Claim the title
                                            // from the objective HERE, in stream order,
                                            // so a later `user_message` can't steal it
                                            // while a native `thread_name_updated` still
                                            // overrides it.
                                            goal_opens_session = !has_real_user;
                                            if goal_opens_session && title.is_none() {
                                                title = extract_codex_title_candidate(
                                                    &marker.objective,
                                                    true,
                                                );
                                            }
                                            goal_objective = Some(marker.objective);
                                        }
                                    }
                                }
                            }
                            "thread_name_updated" => {
                                // Codex native thread name — newest non-empty wins
                                // (parity with the detail parser). Accept both the
                                // rollout `thread_name` and the live `threadName`.
                                if let Some(name) = payload
                                    .get("thread_name")
                                    .or_else(|| payload.get("threadName"))
                                    .or_else(|| payload.get("name"))
                                    .and_then(|n| n.as_str())
                                    .map(str::trim)
                                    .filter(|n| !n.is_empty())
                                {
                                    title = Some(truncate_str(name, 100));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "response_item" => {
                    if let Some(payload) = value.get("payload") {
                        let payload_type =
                            payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if payload_type == "message" {
                            let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                            // The detail parser only turns an IMAGE-bearing
                            // `response_item` user into a real user turn
                            // (`extract_response_item_user_image_blocks`) and only
                            // titles from that same turn. Text-only `response_item`
                            // users are internal envelopes (`<environment_context>`,
                            // `<codex_internal_context>`, `<turn_aborted>`, …) or
                            // duplicates of `event_msg.user_message` — detail ignores
                            // them for BOTH the turn and the title, so the summary
                            // must too. Mirroring it here keeps the pure-`/goal`
                            // fallback (title + count) in exact sync and stops
                            // internal text from leaking into the list title.
                            if role == "user" && response_item_user_has_image(payload) {
                                has_real_user = true;
                                if title.is_none() {
                                    title = extract_codex_text_content(payload)
                                        .and_then(|t| extract_codex_title_candidate(&t, false));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let started_at = match first_timestamp {
            Some(ts) => ts,
            None => return Ok(None),
        };

        // Leading-`/goal` fallback, positional and mirroring the detail parser:
        // when a `/goal` opened the session (before any real user turn), the detail
        // view synthesizes a leading user message from the objective — so count
        // that one turn here, even when a LATER real reply exists, keeping the list
        // entry in sync with the opened conversation. The title was already claimed
        // in-loop (see the goal arm).
        if goal_opens_session {
            message_count += 1;
        }

        let id = conversation_id.unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });

        let folder_path = cwd.clone();
        let folder_name = folder_path.as_ref().map(|p| folder_name_from_path(p));

        Ok(Some(ConversationSummary {
            id,
            agent_type: AgentType::Codex,
            folder_path,
            folder_name,
            title,
            started_at,
            ended_at: last_timestamp,
            message_count,
            model,
            git_branch,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        }))
    }
}

pub(crate) fn resolve_codex_home_dir() -> PathBuf {
    resolve_codex_home_dir_from(std::env::var_os("CODEX_HOME"), dirs::home_dir())
}

fn resolve_codex_home_dir_from(
    codex_home_env: Option<std::ffi::OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    codex_home_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.unwrap_or_default().join(".codex"))
}

impl AgentParser for CodexParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let mut conversations = Vec::new();

        if !self.base_dir.exists() {
            return Ok(conversations);
        }

        // Apply this outside `summary_cache`: changing only session_index.jsonl
        // must refresh a title even when the rollout itself is unchanged.
        let indexed_titles = self.load_thread_name_index();

        for entry in WalkDir::new(&self.base_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path().to_path_buf();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            if !fname.starts_with("rollout-") {
                continue;
            }

            match super::summary_cache::get_or_parse(AgentType::Codex, &path, || {
                self.parse_jsonl_summary(&path)
            }) {
                Ok(Some(mut summary)) => {
                    if let Some(title) = indexed_titles.get(&summary.id) {
                        summary.title = Some(title.clone());
                    }
                    conversations.push(summary);
                }
                _ => continue,
            }
        }

        conversations.sort_by_key(|b| std::cmp::Reverse(b.started_at));
        Ok(conversations)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        if !self.base_dir.exists() {
            return Err(ParseError::ConversationNotFound(
                conversation_id.to_string(),
            ));
        }

        // Find the conversation file by walking the directory tree
        for entry in WalkDir::new(&self.base_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path().to_path_buf();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            if fname.contains(conversation_id) {
                let mut detail = self.parse_conversation_detail(&path, conversation_id)?;
                if let Some(title) = self.load_thread_name_index().get(conversation_id) {
                    detail.summary.title = Some(title.clone());
                }
                return Ok(detail);
            }
        }

        Err(ParseError::ConversationNotFound(
            conversation_id.to_string(),
        ))
    }
}

fn parse_codex_json_arg(payload: &serde_json::Value) -> Option<serde_json::Value> {
    let args = payload.get("arguments").or_else(|| payload.get("input"))?;
    if let Some(s) = args.as_str() {
        serde_json::from_str(s).ok()
    } else if args.is_object() || args.is_array() {
        Some(args.clone())
    } else {
        None
    }
}

fn parse_codex_json_output(payload: &serde_json::Value) -> Option<serde_json::Value> {
    let output = payload.get("output")?;
    if let Some(s) = output.as_str() {
        serde_json::from_str(s).ok()
    } else if output.is_object() || output.is_array() {
        Some(output.clone())
    } else {
        None
    }
}

/// A `tools.exec_command()` result the script printed verbatim
/// (`text(JSON.stringify(r))`) — the object form of the very envelope the
/// string path spells out as `Chunk ID: …` / `Wall time: …` / `Output:`.
/// Reduce it to the output it wraps so the card reads like the terminal it is:
/// 9% of code-mode chunks are this shape, and the ones announcing a background
/// session carry an empty `output`, so today those cards show nothing but the
/// envelope. Callers must read whatever they need from the raw text (the
/// session announcement lives in `session_id`) before cleaning.
fn unwrap_exec_chunk_envelope(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let obj = value.as_object()?;
    // The whole shape, not just `chunk_id`: a command whose own stdout is JSON
    // carrying those two keys would otherwise be truncated to its `output`
    // field, silently dropping the rest. Every envelope in real transcripts
    // carries all four.
    if !ENVELOPE_KEYS.iter().all(|key| obj.contains_key(*key)) {
        return None;
    }
    Some(obj.get("output")?.as_str()?.to_string())
}

/// Keys every `tools.exec_command()` result object carries, in both the
/// `exit_code` (finished) and `session_id` (still running) variants.
const ENVELOPE_KEYS: [&str; 4] = [
    "chunk_id",
    "wall_time_seconds",
    "original_token_count",
    "output",
];

/// The command line and body of a codex exec envelope, or `None` when the text
/// carries neither — i.e. when there is no envelope to strip. Distinct from
/// `clean_codex_exec_output` in exactly one case that matters to a background
/// session: an envelope whose body is empty answers `Some("")` (that poll
/// collected nothing) rather than falling back to the header text.
fn split_codex_exec_output(text: &str) -> Option<String> {
    let mut cmd_line: Option<&str> = None;
    let mut in_output = false;
    let mut output_lines = Vec::new();

    for line in text.lines() {
        if cmd_line.is_none() && line.starts_with("$ ") {
            cmd_line = Some(line);
            continue;
        }
        if line.trim_end() == "Output:" {
            in_output = true;
            continue;
        }
        if in_output {
            output_lines.push(line);
        }
    }

    if cmd_line.is_none() && !in_output {
        return None;
    }

    let mut result = String::new();
    if let Some(cmd) = cmd_line {
        result.push_str(cmd);
    }
    if !output_lines.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&output_lines.join("\n"));
    }
    Some(result)
}

fn clean_codex_exec_output(text: &str) -> String {
    split_codex_exec_output(text)
        .filter(|cleaned| !cleaned.is_empty())
        .unwrap_or_else(|| text.to_string())
}

/// Strip whichever exec envelope `text` is wrapped in — the object form or the
/// `Chunk ID:` / `Wall time:` / `Output:` header — or `None` when it is wrapped
/// in neither. The header form is only stripped when the FIRST line announces
/// it: an `Output:` line in the middle of a build log is a build log, not an
/// envelope, and stripping on that alone would swallow everything above it.
fn strip_exec_envelope(text: &str) -> Option<String> {
    if let Some(unwrapped) = unwrap_exec_chunk_envelope(text) {
        return Some(unwrapped);
    }
    let first = text.lines().next()?.trim_start().to_ascii_lowercase();
    let announced = ["chunk id:", "wall time", "process ", "script "]
        .iter()
        .any(|prefix| first.starts_with(prefix));
    if !announced {
        return None;
    }
    split_codex_exec_output(text)
}

/// Banner codex puts in front of a script's `text()` stream when it re-rendered
/// the whole stream as one blob instead of passing the parts through.
const TRUNCATION_WARNING: &str = "Warning: truncated output";
const TOTAL_OUTPUT_LINES: &str = "Total output lines: ";

/// The per-`text()` chunks hiding inside a collapsed output blob, or `None` when
/// recovering them is not provable.
///
/// Codex re-renders a long script's whole `text()` stream as ONE chunk behind a
/// `Warning: truncated output (original token count: N)` / `Total output lines:
/// M` banner. The boundaries survive in it — one physical line per `text()` —
/// so the per-call split is still there to be had, and without it a
/// `Promise.all` of four commands shows up as a single card whose body is four
/// lines of raw envelope JSON.
///
/// Recovering it is only sound while all M lines are present: past the cap codex
/// drops whole lines, and it drops them from the MIDDLE, so the survivors'
/// positions stop naming their calls. Measured over the newest 400 rollouts: with
/// every line present, line *i* is call *i* in 36 of 36 checkable cases; with
/// lines dropped, 13 of 57 land on the wrong command. So all three counts have to
/// agree — declared, present, and calls — and every line has to be a complete
/// exec envelope, which is also what tells this blob apart from a single
/// command's stdout that merely got truncated (the far more common banner).
fn uncollapse_truncated_chunks(blob: Option<&TruncatedBlob>, calls: usize) -> Option<Vec<String>> {
    if calls == 0 {
        return None;
    }
    let blob = blob?;
    if blob.body.len() != blob.declared || blob.declared != calls {
        return None;
    }
    blob.body
        .iter()
        .all(|line| unwrap_exec_chunk_envelope(line).is_some())
        .then(|| blob.body.iter().map(|line| line.to_string()).collect())
}

/// The line count a collapsed blob declares, and the lines it actually kept.
///
/// Reading it costs a pass over the whole blob, and three of the paths below
/// want it, so `unwrap_code_mode_script` reads it once and hands it down.
struct TruncatedBlob<'a> {
    declared: usize,
    body: Vec<&'a str>,
}

impl TruncatedBlob<'_> {
    /// Codex dropped lines from the middle to fit the cap.
    fn truncated(&self) -> bool {
        self.declared > self.body.len()
    }
}

fn truncated_blob(parsed: &CodeModeOutput) -> Option<TruncatedBlob<'_>> {
    let [collapsed] = parsed.chunks.as_slice() else {
        return None;
    };
    let mut lines = collapsed.lines();
    if !lines.next()?.starts_with(TRUNCATION_WARNING) {
        return None;
    }
    let declared: usize = lines
        .next()?
        .strip_prefix(TOTAL_OUTPUT_LINES)?
        .trim()
        .parse()
        .ok()?;
    if !lines.next()?.is_empty() {
        return None;
    }
    Some(TruncatedBlob {
        declared,
        body: lines.collect(),
    })
}

/// One call's share of a collapsed blob that was split on the separator lines
/// the script printed.
struct SeparatorSlot {
    /// The lines this call owns. `None` means it printed nothing between its
    /// separator and the next — which is not the same as having no separator,
    /// hence `placed`.
    text: Option<String>,
    /// Whether a span of the blob could be attributed to this call at all. False
    /// when truncation removed its separator, leaving nothing to cut on.
    placed: bool,
    /// Calls whose separators were truncated away, leaving their output inside
    /// this slot with no boundary to cut on. Empty in the normal case.
    shares_with: Vec<usize>,
}

/// At least this many separators have to be found, so a split rests on at least
/// one proven boundary. One anchor divides nothing — it would hand the whole
/// blob to the first call and leave the rest empty.
const MIN_ANCHORS: usize = 2;

/// The blob split on the separator lines the script printed before each result,
/// or `None` when it cannot be read that way.
///
/// This is the only handle left once codex re-renders a long `text()` stream as
/// one truncated blob: 89% of collapsed blobs are missing lines, so the counts
/// `uncollapse_truncated_chunks` compares never agree. But scripts of this shape
/// label their results — ``text(`===== ${k} =====\n${r.output}`)`` — and the
/// labels come from the same literal table the commands do. So the separator
/// each call printed can be *predicted* from the source and then looked up in
/// the output, which is a far stronger check than counting: a wrong prediction
/// does not match and the split is simply refused.
///
/// Truncation cannot reorder — it only deletes — so lines between two surviving
/// separators belong to the earlier one and nothing else. When the separator
/// *between* them was dropped, the span covers several calls with no way to
/// tell where each begins; that span stays whole, attributed to the call it
/// provably starts with, and the calls sharing it are named in `shares_with`
/// rather than silently given someone else's output.
///
/// One residual, deliberately accepted: a matched line is only *probably* the
/// separator the script printed. A command whose own stdout contains a line
/// identical to another row's separator — printing this very transcript, say —
/// would be cut there, and the tail of its output would be filed under that
/// row. Uniqueness makes this impossible whenever every separator survived (a
/// look-alike would be a second match and reject the candidate), so the risk
/// only exists in the truncated case, and only when the look-alike stands in
/// for a separator that is genuinely gone. Refusing truncated blobs would avoid
/// it at the cost of the case this exists for — 89% of collapsed blobs are
/// missing lines — so the split is kept and the labels are what it rests on.
fn split_by_separators(
    blob: Option<&TruncatedBlob>,
    calls: &[CodeModeCall],
    separators: &[Separator],
) -> Option<Vec<SeparatorSlot>> {
    if calls.len() < 2 || separators.is_empty() {
        return None;
    }
    let body = &blob?.body;

    let labels: Option<Vec<&str>> = calls.iter().map(|c| c.label.as_deref()).collect();
    let by_index = |offset: usize| (0..calls.len()).map(|i| (i + offset).to_string()).collect();
    // `---RESULT ${i+1}---` is as common as a label in this corpus, and costs
    // nothing to try: the output either contains the line or it does not.
    let candidates: Vec<Vec<String>> = labels
        .map(|names| names.iter().map(|n| n.to_string()).collect())
        .into_iter()
        .chain([by_index(0), by_index(1)])
        .collect();

    let index = index_body_lines(body);
    let mut best: Option<Vec<Option<usize>>> = None;
    for separator in separators {
        for values in &candidates {
            let Some(found) = locate_anchors(&index, separator, values) else {
                continue;
            };
            let hits = found.iter().flatten().count();
            if hits < MIN_ANCHORS {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|b| hits > b.iter().flatten().count())
            {
                best = Some(found);
            }
        }
    }

    Some(slots_from_anchors(body, &best?))
}

/// Every line of the blob, trimmed of trailing whitespace, mapped to where it
/// sits — or to `None` when it occurs more than once.
///
/// Built once and read by every candidate, which is what keeps the split from
/// costing a pass over the blob per command: a run of separators is looked up,
/// not searched for.
fn index_body_lines<'a>(body: &[&'a str]) -> HashMap<&'a str, Option<usize>> {
    let mut index = HashMap::with_capacity(body.len());
    for (at, line) in body.iter().enumerate() {
        index
            .entry(line.trim_end())
            .and_modify(|seen| *seen = None)
            .or_insert(Some(at));
    }
    index
}

/// Where each value's separator line sits in the blob, or `None` for the ones
/// truncation removed. Refuses the whole candidate when a line repeats or the
/// lines run backwards — either means the match is not the separator run.
fn locate_anchors(
    index: &HashMap<&str, Option<usize>>,
    separator: &Separator,
    values: &[String],
) -> Option<Vec<Option<usize>>> {
    let mut found = Vec::with_capacity(values.len());
    let mut previous = None;

    for value in values {
        let anchor = separator.anchor(value);
        let hit = match index.get(anchor.trim_end()) {
            // The line occurs twice, so it is not a boundary to cut on. Only a
            // line some value actually predicts can refuse a candidate; an
            // ordinary output line repeating is nobody's separator.
            Some(None) => return None,
            Some(Some(at)) => Some(*at),
            None => None,
        };
        if let Some(at) = hit {
            if previous.is_some_and(|before| at <= before) {
                return None;
            }
            previous = Some(at);
        }
        found.push(hit);
    }

    Some(found)
}

fn slots_from_anchors(body: &[&str], found: &[Option<usize>]) -> Vec<SeparatorSlot> {
    // A call whose separator survived owns the lines after it. The first call
    // owns the head of the blob even without one: deletion never moves a line,
    // so nothing can precede the first call's output.
    let mut owners: Vec<(usize, Option<usize>)> = Vec::new();
    if found.first().is_some_and(Option::is_none) {
        owners.push((0, None));
    }
    owners.extend(
        found
            .iter()
            .enumerate()
            .filter_map(|(call, at)| at.map(|at| (call, Some(at)))),
    );

    let mut slots: Vec<SeparatorSlot> = (0..found.len())
        .map(|_| SeparatorSlot {
            text: None,
            placed: false,
            shares_with: Vec::new(),
        })
        .collect();

    for (position, (call, anchor)) in owners.iter().enumerate() {
        let start = anchor.map_or(0, |at| at + 1);
        let (end, next_call) = match owners.get(position + 1) {
            // The next owner's separator line is not part of this slot.
            Some((next_call, Some(at))) => (*at, *next_call),
            _ => (body.len(), found.len()),
        };
        let mut text = body.get(start..end).unwrap_or_default().join("\n");
        let kept = text.trim_end().len();
        text.truncate(kept);
        slots[*call] = SeparatorSlot {
            text: (!text.is_empty()).then_some(text),
            placed: true,
            shares_with: (call + 1..next_call).collect(),
        };
    }

    slots
}

/// `text` trimmed with every run of digits collapsed to `#`, so `---RESULT 1---`
/// and `---RESULT 2---` compare equal while `--- dto diff ---` and
/// `--- handler diff ---` do not.
fn digit_blind(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut digits = false;
    for ch in text.trim().chars() {
        if ch.is_ascii_digit() {
            if !digits {
                out.push('#');
                digits = true;
            }
        } else {
            digits = false;
            out.push(ch);
        }
    }
    out
}

/// How many `text()` chunks each call contributed — 1 for the plain
/// `results.forEach(r => text(r.output))` shape — or `None` when the chunks do
/// not provably divide into one group per call.
///
/// Scripts routinely print more than one chunk per call: a `---RESULT n---`
/// header before the output, an `exit_code=0` after it. 268 of the 1297
/// multi-call scripts that fail the 1:1 count in the newest 400 rollouts are
/// exactly this, and they render as one undecomposable script card today.
///
/// The count alone proves nothing — a script that prints every output first and
/// every exit code after hands over the same chunks in the wrong order, and
/// slicing those into pairs would pin each command's output to the previous
/// command's card. The proof is `text_run` (see `CodeModeScript`): exactly
/// `stride` `text()` calls with no loop between the first and the last, so that
/// run emitted its chunks together, `calls` times over. A repeating slot is kept
/// as corroboration — it is only evidence, since a command's own stdout can be
/// `exit_code=0` — and costs 13 of 256 candidates, which stay script cards.
///
/// Measured with commands whose output names its own call (`nl -ba F | sed -n
/// 'A,Bp'` must start at line A): 152 of 153 groups land on the right command,
/// and the one exception is the check's own blind spot — that `nl` failed and
/// printed a usage error, so there was no line number to match.
fn call_chunk_stride(chunks: &[String], calls: usize, text_run: Option<usize>) -> Option<usize> {
    if calls == 0 || chunks.is_empty() || !chunks.len().is_multiple_of(calls) {
        return None;
    }
    let stride = chunks.len() / calls;
    if stride == 1 {
        return Some(1);
    }
    if text_run != Some(stride) {
        return None;
    }
    (0..stride)
        .any(|slot| {
            let marker = digit_blind(&chunks[slot]);
            !marker.is_empty()
                && (1..calls).all(|index| digit_blind(&chunks[index * stride + slot]) == marker)
        })
        .then_some(stride)
}

/// Turn one code-mode script + its output envelope into the tool blocks the
/// pre-code-mode history path produced.
///
/// Returns `(replacement tool_use blocks, tool_result blocks)`. The first is
/// `None` when the script could not be decomposed — the placeholder script card
/// stays and simply receives the whole output.
///
/// Splitting per call needs the chunks to divide into one group per call, in
/// order — see `call_chunk_stride` for what counts as proof. Anything less falls
/// back to the script card rather than misattributing output, including the
/// collapsed blob codex renders in place of the chunks when the stream is long,
/// unless `uncollapse_truncated_chunks` can prove the chunks back out of it.
fn unwrap_code_mode_script(
    call_id: &str,
    script: &CodeModeScript,
    parsed: &CodeModeOutput,
    payload: &serde_json::Value,
    sessions: &mut HashMap<String, ShellSession>,
    poll_origins: &mut HashMap<String, String>,
) -> (Option<Vec<ContentBlock>>, Vec<ContentBlock>) {
    let calls = script.calls.as_deref().unwrap_or_default();

    let result_block = |tool_use_id: Option<String>, text: Option<String>, note: Option<&str>| {
        let output_preview = with_note(text, note);
        let is_error = parsed.is_error()
            || infer_tool_call_output_is_error(payload, None, output_preview.as_deref());
        ContentBlock::ToolResult {
            tool_use_id,
            output_preview,
            is_error,
            agent_stats: None,
            images: Vec::new(),
        }
    };

    let non_empty = |text: String| {
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    };

    // NB: the envelope note (`Script running with cell ID N`) is deliberately
    // NOT registered as a shell session here. That cell is the SCRIPT — it has
    // not finished — so the `wait` collecting it carries this very script's
    // return value and is folded back onto these cards by the caller
    // (`DeferredScript`), never rendered as a session of its own.

    // A collapsed blob stands in for the chunks it was rendered from, when the
    // counts prove which is which. Nothing changes when it does not: the
    // fallback is `parsed.chunks` itself, banner and all.
    // Every path that reads it bails on the call count first, so a script that
    // recovered nothing does not pay to have its blob split into lines.
    let blob = (!calls.is_empty())
        .then(|| truncated_blob(parsed))
        .flatten();
    let uncollapsed = uncollapse_truncated_chunks(blob.as_ref(), calls.len());
    let chunks: &[String] = uncollapsed.as_deref().unwrap_or(&parsed.chunks);

    if calls.len() == 1 {
        let call = &calls[0];
        let joined = chunks.join("\n");
        let uses = vec![ContentBlock::ToolUse {
            tool_use_id: Some(call_id.to_string()),
            tool_name: call.tool_name.clone(),
            input_preview: Some(shell_session_input_preview(
                call,
                call_id,
                sessions,
                poll_origins,
            )),
            status: None,
            meta: codex_script_meta(call.label.as_deref(), false, &[], false),
        }];
        register_shell_sessions(call, call_id, &joined, sessions);
        let results = vec![result_block(
            Some(call_id.to_string()),
            non_empty(exec_chunk_display(call, joined)),
            parsed.note.as_deref(),
        )];
        return (Some(uses), results);
    }

    if let Some(stride) = (calls.len() > 1)
        .then(|| call_chunk_stride(chunks, calls.len(), script.text_run))
        .flatten()
    {
        let last = calls.len() - 1;
        let mut uses = Vec::with_capacity(calls.len());
        let mut results = Vec::with_capacity(calls.len());
        for (index, call) in calls.iter().enumerate() {
            let id = format!("{call_id}#{index}");
            let note = if index == last {
                parsed.note.as_deref()
            } else {
                None
            };
            let group = &chunks[index * stride..(index + 1) * stride];
            uses.push(ContentBlock::ToolUse {
                tool_use_id: Some(id.clone()),
                tool_name: call.tool_name.clone(),
                input_preview: Some(shell_session_input_preview(
                    call,
                    &id,
                    sessions,
                    poll_origins,
                )),
                status: None,
                meta: codex_script_meta(call.label.as_deref(), false, &[], false),
            });
            // Announcements are read from the chunks as written: unwrapping an
            // envelope drops the `session_id` that names the session.
            register_shell_sessions(call, &id, &group.join("\n"), sessions);
            let shown = group
                .iter()
                .map(|chunk| exec_chunk_display(call, chunk.clone()))
                .collect::<Vec<_>>()
                .join("\n");
            results.push(result_block(Some(id), non_empty(shown), note));
        }
        return (Some(uses), results);
    }

    // The chunks did not divide, but the script may have labelled its results
    // on their way out. Only reachable for a collapsed blob — anything with real
    // chunks was already handled above.
    if let Some(slots) = split_by_separators(blob.as_ref(), calls, &script.separators) {
        let truncated = blob.as_ref().is_some_and(TruncatedBlob::truncated);
        let last = calls.len() - 1;
        let mut uses = Vec::with_capacity(calls.len());
        let mut results = Vec::with_capacity(calls.len());
        for (index, (call, slot)) in calls.iter().zip(&slots).enumerate() {
            let id = format!("{call_id}#{index}");
            let note = if index == last {
                parsed.note.as_deref()
            } else {
                None
            };
            let shared: Vec<String> = slot
                .shares_with
                .iter()
                .filter_map(|other| calls.get(*other))
                .map(call_display_name)
                .collect();
            uses.push(ContentBlock::ToolUse {
                tool_use_id: Some(id.clone()),
                tool_name: call.tool_name.clone(),
                input_preview: Some(shell_session_input_preview(
                    call,
                    &id,
                    sessions,
                    poll_origins,
                )),
                status: None,
                meta: codex_script_meta(
                    call.label.as_deref(),
                    // A command that ran and printed nothing is not a command
                    // whose output went missing; only the second is worth a
                    // notice, and saying it of the first would be false.
                    !slot.placed,
                    &shared,
                    truncated,
                ),
            });
            if let Some(text) = &slot.text {
                register_shell_sessions(call, &id, text, sessions);
            }
            let shown = slot
                .text
                .clone()
                .and_then(|text| non_empty(exec_chunk_display(call, text)));
            results.push(result_block(Some(id), shown, note));
        }
        return (Some(uses), results);
    }

    // Undecomposable script: the card stays a script card, and no session it
    // announced is attributed. There is no way to tell which of its calls
    // started one — that is exactly why it did not decompose.
    let joined = chunks.join("\n");
    (
        None,
        vec![result_block(
            Some(call_id.to_string()),
            non_empty(joined),
            parsed.note.as_deref(),
        )],
    )
}

/// What the renderer needs to know about a call recovered from a code-mode
/// script, as facts rather than prose: the backend states them, the frontend
/// words them in the reader's language.
fn codex_script_meta(
    label: Option<&str>,
    output_missing: bool,
    shares_with: &[String],
    truncated: bool,
) -> Option<serde_json::Value> {
    let mut marks = serde_json::Map::new();
    if let Some(label) = label {
        marks.insert("label".to_string(), label.into());
    }
    if output_missing {
        marks.insert("outputMissing".to_string(), true.into());
    }
    if !shares_with.is_empty() {
        marks.insert("sharedWith".to_string(), shares_with.into());
    }
    if truncated {
        marks.insert("truncated".to_string(), true.into());
    }
    (!marks.is_empty()).then(|| serde_json::json!({ "codeg.codexScript": marks }))
}

/// How to name a call when telling the reader whose output shares a card.
fn call_display_name(call: &CodeModeCall) -> String {
    call.label.clone().unwrap_or_else(|| {
        truncate_str(
            call.input_preview.lines().next().unwrap_or_default().trim(),
            60,
        )
    })
}

/// A code-mode script whose output was `Script running with cell ID N`: the
/// script itself has not finished. Remembers where its cards were written so
/// the `wait` that collects cell N — its real return value, arriving any number
/// of turns later — can be folded back onto them instead of rendering as a
/// separate card. Indices are stable because the parse loop only appends.
#[derive(Clone)]
struct DeferredScript {
    call_id: String,
    /// Index of the message holding the script's ToolUse block(s).
    use_index: usize,
    /// Index of the message holding its ToolResult block(s).
    result_index: usize,
    script: CodeModeScript,
    /// Everything the script has printed so far. A `wait` answers with what has
    /// been printed SINCE — measured on real transcripts, the collected answer
    /// shares nothing with the one that parked the script — so the chunks have
    /// to be accumulated and re-decomposed as one sequence. Replacing the cards
    /// from the `wait` alone would drop whatever the script had already printed,
    /// and would decompose against a chunk count that is missing its front.
    chunks: Vec<String>,
}

/// codex's unified-exec session tools. Neither carries a command of its own:
/// both address a background shell started by an earlier `exec_command`, by the
/// id that command's output announced (`wait` calls it `cell_id`,
/// `write_stdin` calls it `session_id`).
const SHELL_SESSION_TOOLS: [&str; 2] = ["wait", "write_stdin"];

/// Key added to a `wait` / `write_stdin` `input_preview` carrying the command
/// that started the session it addresses.
///
/// Deliberately NOT `command`: `inferFromInput` (`tool-call-normalization.ts`)
/// classifies any live input carrying `command`/`cmd`/… as a terminal call, so
/// that spelling would hijack the live classification of these tools.
const SESSION_COMMAND_KEY: &str = "session_command";

/// A background shell an `exec_command` left running: unified-exec answers that
/// command with whatever it printed so far plus `Process running with session
/// ID N`, and the rest of the output has to be collected by later `wait` /
/// `write_stdin` calls addressing N.
#[derive(Clone)]
struct ShellSession {
    /// The command that started it. Titles the calls that address it.
    command: String,
    /// `tool_use_id` of the card holding that command's output, while more of
    /// it may still be appended there. `None` once the session got a card of
    /// its own (keystrokes, termination): output arriving after that card
    /// belongs below it, not folded back above it.
    origin: Option<String>,
}

fn is_shell_session_tool(tool_name: &str) -> bool {
    SHELL_SESSION_TOOLS.contains(&tool_name)
}

/// The text a recovered call's card shows. Only the exec family answers with
/// the chunk envelope, and only there is unwrapping it provably right — another
/// tool returning an object that happens to carry `chunk_id` would be showing
/// its own result, not a terminal's.
fn exec_chunk_display(call: &CodeModeCall, chunk: String) -> String {
    if call.tool_name != "exec_command" && !is_shell_session_tool(&call.tool_name) {
        return chunk;
    }
    strip_exec_envelope(&chunk).unwrap_or(chunk)
}

/// Whether a `wait` / `write_stdin` only collects output — no keystrokes to
/// show, no termination to report. Such a call *is* the earlier command still
/// running, so it gets no card and its output is appended to that command's.
fn is_pure_poll(args: &serde_json::Map<String, serde_json::Value>) -> bool {
    let sends_input = args
        .get("chars")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    let terminates = args
        .get("terminate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    !sends_input && !terminates
}

/// The session id a `wait` / `write_stdin` argument object addresses, as a
/// string — `wait` sends `"cell_id":"7106"`, `write_stdin` sends
/// `"session_id":33067`.
fn shell_session_id(args: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let value = args.get("cell_id").or_else(|| args.get("session_id"))?;
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Annotate a `wait` / `write_stdin` argument JSON with the command that
/// started the session it addresses, so the card can be titled by that command
/// instead of a bare `wait`. Returns `None` when the arguments aren't an
/// object, carry no session id, or the id was never announced — the caller then
/// keeps the arguments untouched rather than inventing a command.
fn annotate_shell_session_args(
    args: &serde_json::Value,
    sessions: &HashMap<String, ShellSession>,
) -> Option<String> {
    let obj = args.as_object()?;
    let session = sessions.get(&shell_session_id(obj)?)?;
    let mut annotated = obj.clone();
    annotated.insert(
        SESSION_COMMAND_KEY.to_string(),
        serde_json::Value::String(session.command.clone()),
    );
    Some(serde_json::Value::Object(annotated).to_string())
}

/// Same annotation for a session tool recovered from a code-mode script, whose
/// `input_preview` is already the argument object's JSON.
fn shell_session_input_preview(
    call: &CodeModeCall,
    tool_use_id: &str,
    sessions: &mut HashMap<String, ShellSession>,
    poll_origins: &mut HashMap<String, String>,
) -> String {
    if !is_shell_session_tool(&call.tool_name) {
        return call.input_preview.clone();
    }
    let args = serde_json::from_str::<serde_json::Value>(&call.input_preview).ok();
    note_shell_session_call(Some(tool_use_id), args.as_ref(), sessions, poll_origins);
    args.and_then(|args| annotate_shell_session_args(&args, sessions))
        .unwrap_or_else(|| call.input_preview.clone())
}

/// Record `session id → command` for every background session this call's
/// output announced. Only `exec_command` starts sessions, and only its own
/// output can name them, so nothing else is registered — a wrong mapping would
/// put someone else's command in a `wait` card's title.
fn register_shell_sessions(
    call: &CodeModeCall,
    origin: &str,
    output: &str,
    sessions: &mut HashMap<String, ShellSession>,
) {
    // `args_inline` is the proof that this command is what actually ran: one
    // call site executes any number of times, and arguments reached through a
    // variable can be reassigned between iterations, leaving the recovered
    // command a guess. Titling — and now folding — a session under a guess is
    // exactly the failure this parser must not have.
    if call.tool_name != "exec_command" || !call.args_inline {
        return;
    }
    // `input_preview` of a recovered `exec_command` is the bare command string.
    register_announced_sessions(&call.input_preview, origin, output, sessions);
}

/// `origin` is the `tool_use_id` of the card this output is being written to —
/// where the session's remaining output gets appended once a later poll
/// collects it.
fn register_announced_sessions(
    command: &str,
    origin: &str,
    output: &str,
    sessions: &mut HashMap<String, ShellSession>,
) {
    if command.trim().is_empty() {
        return;
    }
    let announced = extract_shell_session_ids(output);
    if announced.is_empty() {
        return;
    }
    // The chunk ids of the same output are aliases for the cells it announced —
    // codex addresses them either way. Registered only alongside a real
    // announcement, so a chunk id from a command that already exited stays
    // meaningless.
    for id in announced.into_iter().chain(extract_chunk_ids(output)) {
        sessions.insert(
            id,
            ShellSession {
                command: command.to_string(),
                origin: Some(origin.to_string()),
            },
        );
    }
}

/// Classify a `wait` / `write_stdin` against the session it addresses: either
/// it only collects more of that session's output — in which case it is marked
/// for folding into the card of the command producing it — or it is an action
/// (keystrokes, termination) that earns a card, and everything the session
/// prints from then on belongs below that card rather than folded back above
/// it. Sessions nobody announced fall through both: nothing to fold into.
fn note_shell_session_call(
    tool_use_id: Option<&str>,
    args: Option<&serde_json::Value>,
    sessions: &mut HashMap<String, ShellSession>,
    poll_origins: &mut HashMap<String, String>,
) {
    let Some(args) = args.and_then(|a| a.as_object()) else {
        return;
    };
    let Some(session_id) = shell_session_id(args) else {
        return;
    };
    let Some(origin) = sessions.get(&session_id).and_then(|s| s.origin.clone()) else {
        return;
    };
    if is_pure_poll(args) {
        if let Some(call_id) = tool_use_id {
            poll_origins.insert(call_id.to_string(), origin);
        }
        return;
    }
    // End the folding for every id naming this cell, not just the one this call
    // addressed: the numeric session id and the hex chunk id are aliases, and a
    // poll arriving through the other spelling would otherwise still be folded
    // in above the card that caused it.
    for session in sessions.values_mut() {
        if session.origin.as_deref() == Some(origin.as_str()) {
            session.origin = None;
        }
    }
}

/// Drop every card that only collects more of an earlier command's output,
/// moving what it collected onto that command's card. Runs on the finished
/// message list so a poll codex called directly and one a code-mode script
/// wrote are folded the same way — the script path builds its cards inside
/// `unwrap_code_mode_script`, long after the call itself was read.
fn fold_shell_session_polls(
    messages: &mut Vec<UnifiedMessage>,
    poll_origins: &HashMap<String, String>,
) {
    if poll_origins.is_empty() {
        return;
    }

    // Where every tool block lives, and — in transcript order — the polls to
    // fold. Collected up front: a poll's origin always sits earlier, so folding
    // as we walk would need the map anyway, and both blocks of a poll have to be
    // located before either can be dropped.
    let mut result_at: HashMap<String, (usize, usize)> = HashMap::new();
    let mut use_at: HashMap<String, (usize, usize)> = HashMap::new();
    let mut folds: Vec<(String, String, usize, usize)> = Vec::new();
    for (mi, message) in messages.iter().enumerate() {
        for (bi, block) in message.content.iter().enumerate() {
            match block {
                ContentBlock::ToolUse {
                    tool_use_id: Some(id),
                    ..
                } => {
                    use_at.insert(id.clone(), (mi, bi));
                }
                ContentBlock::ToolResult {
                    tool_use_id: Some(id),
                    ..
                } => match poll_origins.get(id) {
                    Some(origin) => folds.push((id.clone(), origin.clone(), mi, bi)),
                    None => {
                        result_at.insert(id.clone(), (mi, bi));
                    }
                },
                _ => {}
            }
        }
    }

    let mut drop_at: Vec<(usize, usize)> = Vec::new();
    for (poll_id, origin, mi, bi) in folds {
        // No origin block left — a code-mode script re-decomposed under it, say.
        // Leave the poll's own card alone rather than dropping what it collected.
        let Some(&(omi, obi)) = result_at.get(&origin) else {
            continue;
        };
        let ContentBlock::ToolResult {
            output_preview,
            is_error,
            ..
        } = &messages[mi].content[bi]
        else {
            continue;
        };
        let collected = output_preview.clone().unwrap_or_default();
        let collected = collected.trim_end().to_string();
        let collected_error = *is_error;

        let ContentBlock::ToolResult {
            output_preview,
            is_error,
            ..
        } = &mut messages[omi].content[obi]
        else {
            continue;
        };
        if !collected.is_empty() {
            match output_preview {
                Some(existing) if !existing.trim().is_empty() => {
                    while existing.ends_with('\n') {
                        existing.pop();
                    }
                    existing.push('\n');
                    existing.push_str(&collected);
                }
                _ => *output_preview = Some(collected),
            }
        }
        *is_error = *is_error || collected_error;

        drop_at.push((mi, bi));
        if let Some(&at) = use_at.get(&poll_id) {
            drop_at.push(at);
        }
    }

    if drop_at.is_empty() {
        return;
    }
    let emptied: HashSet<usize> = drop_at.iter().map(|(mi, _)| *mi).collect();
    drop_at.sort_unstable();
    // Two polls sharing a `tool_use_id` would name the same use block twice, and
    // removing a block index twice panics. Ids are unique in practice; a broken
    // transcript must not take the whole conversation down with it.
    drop_at.dedup();
    for (mi, bi) in drop_at.into_iter().rev() {
        messages[mi].content.remove(bi);
    }
    let mut index = 0;
    messages.retain(|m| {
        let keep = !m.content.is_empty() || !emptied.contains(&index);
        index += 1;
        keep
    });
}

fn value_to_preview(value: Option<&serde_json::Value>) -> Option<String> {
    let v = value?;
    if v.is_null() {
        return None;
    }
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    serde_json::to_string(v).ok()
}

fn is_failed_status(status: &str) -> bool {
    let status = status.trim();
    status.eq_ignore_ascii_case("error")
        || status.eq_ignore_ascii_case("failed")
        || status.eq_ignore_ascii_case("failure")
        || status.eq_ignore_ascii_case("cancelled")
        || status.eq_ignore_ascii_case("canceled")
}

fn parse_nonzero_exit_code_from_line(line: &str) -> Option<i64> {
    let trimmed = line.trim();
    let (label, rest) = trimmed.split_once(':')?;
    if !label.trim_end().eq_ignore_ascii_case("exit code") {
        return None;
    }
    let number_text = rest.split_whitespace().next()?;
    let code = number_text.parse::<i64>().ok()?;
    if code == 0 {
        None
    } else {
        Some(code)
    }
}

fn infer_output_text_is_error(text: &str) -> bool {
    for line in text.lines().take(16) {
        if parse_nonzero_exit_code_from_line(line).is_some() {
            return true;
        }
    }

    for line in text.lines().take(32) {
        let lower = line.trim().to_ascii_lowercase();
        let shell_prefix =
            lower.starts_with("bash:") || lower.starts_with("zsh:") || lower.starts_with("sh:");
        if shell_prefix
            && (lower.contains("command not found")
                || lower.contains("no such file or directory")
                || lower.contains("permission denied"))
        {
            return true;
        }
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(trimmed)
            .ok()
            .map(|v| infer_output_value_is_error(&v, 0))
            .unwrap_or(false)
    {
        return true;
    }

    trimmed
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("error:"))
}

fn infer_output_value_is_error(value: &serde_json::Value, depth: usize) -> bool {
    if depth > 4 {
        return false;
    }

    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => false,
        serde_json::Value::String(text) => infer_output_text_is_error(text),
        serde_json::Value::Array(items) => items
            .iter()
            .any(|item| infer_output_value_is_error(item, depth + 1)),
        serde_json::Value::Object(map) => {
            if map
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return true;
            }

            if map.get("ok").and_then(|v| v.as_bool()) == Some(false)
                || map.get("success").and_then(|v| v.as_bool()) == Some(false)
            {
                return true;
            }

            if let Some(status) = map.get("status").and_then(|v| v.as_str()) {
                if is_failed_status(status) {
                    return true;
                }
            }

            if let Some(exit_code) = map.get("exit_code").and_then(|v| v.as_i64()) {
                if exit_code != 0 {
                    return true;
                }
            }

            if let Some(stderr) = map.get("stderr").and_then(|v| v.as_str()) {
                if !stderr.trim().is_empty() {
                    return true;
                }
            }

            if let Some(error) = map.get("error") {
                match error {
                    serde_json::Value::Null => {}
                    serde_json::Value::Bool(false) => {}
                    serde_json::Value::String(s) if s.trim().is_empty() => {}
                    _ => return true,
                }
            }

            for key in ["output", "result", "details", "data"] {
                if let Some(child) = map.get(key) {
                    if infer_output_value_is_error(child, depth + 1) {
                        return true;
                    }
                }
            }

            false
        }
    }
}

fn infer_tool_call_output_is_error(
    payload: &serde_json::Value,
    output_value: Option<&serde_json::Value>,
    output_preview: Option<&str>,
) -> bool {
    if payload
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return true;
    }

    if let Some(status) = payload.get("status").and_then(|s| s.as_str()) {
        if is_failed_status(status) {
            return true;
        }
    }

    if let Some(error) = payload.get("error") {
        match error {
            serde_json::Value::Null => {}
            serde_json::Value::Bool(false) => {}
            serde_json::Value::String(s) if s.trim().is_empty() => {}
            _ => return true,
        }
    }

    if let Some(output) = output_value {
        if infer_output_value_is_error(output, 0) {
            return true;
        }
    }

    output_preview
        .map(infer_output_text_is_error)
        .unwrap_or(false)
}

/// Synthetic rawInput key the live input shaper uses to carry the collab op
/// through to the card (see frontend `collab-tool.ts` `COLLAB_OP_KEY`). Kept in
/// sync here so history `wait_agent` capsules render with an op-aware title.
const COLLAB_OP_KEY: &str = "__codegCollabOp";

/// Whether a collab status string is an error (mirrors the frontend
/// `isErrorCollabStatusKind`: only `errored` / `failed` / `notFound`).
fn is_error_collab_status(status: &str) -> bool {
    matches!(status, "errored" | "failed" | "notFound")
}

/// Prefix of codex's encrypted payload envelope — a Fernet token, whose version
/// byte `0x80` always base64s to `gA`. codex uses it for `reasoning`'s
/// `encrypted_content` and, since 0.147, for the inter-agent `message` a
/// `spawn_agent` / `send_message` carries.
const CODEX_ENCRYPTED_PREFIX: &str = "gAAAAA";

/// Shortest blob worth treating as an envelope: the token's own header (version
/// byte + 8-byte timestamp + 16-byte IV + HMAC) is already well past this, so
/// the bound only rules out a short string that merely starts the same way.
const CODEX_ENCRYPTED_MIN_LEN: usize = 64;

/// Whether a payload is one of codex's opaque encrypted envelopes rather than
/// text a human wrote.
///
/// codex 0.147 encrypts every inter-agent message: what reaches the rollout for
/// a `spawn_agent` is `"gAAAAABqgWsi0g7g…"`, ~500 characters of base64 that only
/// codex can open. Rendering it verbatim is what put a wall of base64 in the
/// sub-agent capsule's title and prompt. There is no plaintext to recover — the
/// capsule simply shows no prompt.
///
/// Deliberately narrow: the exact prefix AND no whitespace AND long enough. A
/// prompt that merely *contains* base64, or discusses one, still renders.
fn is_encrypted_envelope(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with(CODEX_ENCRYPTED_PREFIX)
        && trimmed.len() >= CODEX_ENCRYPTED_MIN_LEN
        && !trimmed.chars().any(char::is_whitespace)
}

/// What an encrypted envelope renders as inside a tool card's argument preview.
const CODEX_ENCRYPTED_PLACEHOLDER: &str = "[encrypted]";

/// Synthetic input key marking a sub-agent capsule as ONLY a launch — the card
/// settles when codex acknowledges the spawn, not when the child finishes (see
/// the frontend `agent-tool-call.tsx`, which turns it into a translated note).
///
/// Only codex 0.147's native team-of-agents needs it. In the older collab flow
/// the spawn capsule really did stand for the sub-agent's run, because a
/// `wait_agent` / `close_agent` capsule carried its result; 0.147 emits neither,
/// so an unmarked "completed" would claim a still-running child had finished.
///
/// Public because the LIVE path writes the same key
/// (`acp/connection.rs::classify_codex_subagent_activity`) — streaming and
/// reload must not disagree about what the card means.
pub const CODEX_SUBAGENT_LAUNCH_KEY: &str = "__codegCodexSubagentLaunch";

/// Whether a `spawn_agent`'s arguments are codex 0.147's native team-of-agents
/// shape: `task_name` and no `agent_type` (which that release removed).
fn is_native_team_spawn(args: Option<&serde_json::Value>) -> bool {
    args.is_some_and(|a| a.get("agent_type").is_none() && a.get("task_name").is_some())
}

/// Replace every encrypted envelope inside a parsed argument tree with
/// [`CODEX_ENCRYPTED_PLACEHOLDER`], returning whether anything was replaced.
///
/// `spawn_agent` is not the only carrier: `send_message` (the collaboration call
/// a parent uses to talk to a running sub-agent, and a sub-agent to answer)
/// takes the same sealed `message`, and it has no capsule of its own — it lands
/// on the generic tool card, whose preview is the whole argument JSON. Without
/// this, that card shows ~500 characters of base64. Nothing is lost by
/// replacing it: only codex can open the envelope.
///
/// Recurses so a nested payload is caught too, and reports whether it changed
/// anything so the caller can leave every other tool's preview byte-identical.
fn redact_encrypted_args(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => {
            if is_encrypted_envelope(s) {
                *s = CODEX_ENCRYPTED_PLACEHOLDER.to_string();
                return true;
            }
            false
        }
        serde_json::Value::Array(items) => redact_encrypted_children(items.iter_mut()),
        serde_json::Value::Object(map) => redact_encrypted_children(map.values_mut()),
        _ => false,
    }
}

/// Redact every child of a container and report whether any of them changed.
///
/// Deliberately a loop and not `Iterator::any`: the return value is a
/// by-product, the traversal is the point, and `any` short-circuits — it would
/// leave every envelope after the first one unredacted.
fn redact_encrypted_children<'a>(
    children: impl Iterator<Item = &'a mut serde_json::Value>,
) -> bool {
    let mut changed = false;
    for child in children {
        changed |= redact_encrypted_args(child);
    }
    changed
}

/// Add `agent_id` to a spawn execution capsule's input JSON (the
/// `{subagent_type,prompt,description}` object), so the card can show the
/// sub-agent UUID. Tolerates a missing/!object input by starting fresh.
fn inject_agent_id_into_input(input: Option<&str>, agent_id: &str) -> String {
    let mut obj = input
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    obj.insert(
        "agent_id".to_string(),
        serde_json::Value::String(agent_id.to_string()),
    );
    serde_json::Value::Object(obj).to_string()
}

/// Pull a single sub-agent's `(status, message)` out of one `wait_agent`
/// `output.status` value, e.g. `{ "completed": "<result>" }`. Generalizes over
/// the terminal key: prefer `completed`, else the first string-valued key, so a
/// future `{ "errored": "<msg>" }` maps to `status="errored"`.
fn extract_wait_agent_status(value: &serde_json::Value) -> (String, Option<String>) {
    if let Some(obj) = value.as_object() {
        if let Some(text) = obj.get("completed").and_then(|v| v.as_str()) {
            return ("completed".to_string(), Some(text.to_string()));
        }
        for (key, val) in obj {
            if let Some(text) = val.as_str() {
                return (key.clone(), Some(text.to_string()));
            }
        }
    } else if let Some(text) = value.as_str() {
        return (text.to_string(), None);
    }
    ("completed".to_string(), None)
}

/// Build a synthesized live-shaped collab `rawInput` JSON (and whether any agent
/// errored) for a history `wait_agent` capsule, from that wait's own
/// `output.status` map `{ agent_id: { <terminal-key>: <text> } }`. The result
/// routes through the same `CollabAgentCard` as the live `wait` capsule (matches
/// the shape `collab-tool.ts` `parseCollabToolInput` expects). Caller guarantees
/// `status` is non-empty.
/// Build the history capsule for codex 0.147's `wait_agent`, whose output is
/// `{"message":"Wait completed.","timed_out":false}` — no per-agent map at all.
///
/// It stays worth showing even though it carries nothing: with the native
/// team-of-agents the spawn capsule settles the instant codex acknowledges the
/// launch, so this wait is the ONLY thing in the timeline that spans the child's
/// actual run. Live already renders it (codex-acp forwards the wait as a
/// `collabAgentToolCall`, unlike the spawn), and the reload must agree — a
/// contentless capsule live and nothing at all on reload is the disagreement
/// this exists to remove. Both sides come out as a bare pill: no agents, no
/// prompt, so `AgentCapsule` renders no body.
///
/// The legacy shape is NOT routed here — it carries per-agent results, and a
/// content-free one there means "timed out with nothing to report", which is
/// noise the caller still drops. `timed_out` (a real bool) is the shape gate:
/// only the native-team output has it, and a timeout is a real outcome, so it
/// flags the capsule as failed.
fn native_team_wait_input(output: &serde_json::Value) -> Option<(String, bool)> {
    let timed_out = output.get("timed_out")?.as_bool()?;
    let input = serde_json::json!({
        "senderThreadId": "",
        "receiverThreadIds": [],
        "agentsStates": {},
        "status": if timed_out { "failed" } else { "completed" },
        COLLAB_OP_KEY: "wait",
    });
    Some((input.to_string(), timed_out))
}

fn build_collab_wait_input(status: &serde_json::Map<String, serde_json::Value>) -> (String, bool) {
    let mut receiver_ids: Vec<serde_json::Value> = Vec::new();
    let mut agents_states = serde_json::Map::new();
    let mut any_error = false;
    for (agent_id, value) in status {
        receiver_ids.push(serde_json::Value::String(agent_id.clone()));
        let (st, msg) = extract_wait_agent_status(value);
        if is_error_collab_status(&st) {
            any_error = true;
        }
        agents_states.insert(
            agent_id.clone(),
            serde_json::json!({
                "status": st,
                "message": msg,
            }),
        );
    }
    let input = serde_json::json!({
        "senderThreadId": "",
        "receiverThreadIds": receiver_ids,
        "agentsStates": serde_json::Value::Object(agents_states),
        "status": if any_error { "failed" } else { "completed" },
        COLLAB_OP_KEY: "wait",
    });
    (input.to_string(), any_error)
}

/// Whether a transcript's opening record declares it a forked thread
/// (`session_meta.parent_thread_id`) — codex 0.147's sub-agent shape, where the
/// child is a rollout of its own and `fork_turns` copies the parent's history
/// into its head.
///
/// That copy is the problem: the parent's own tool calls sit at the top of the
/// child's file, indistinguishable at the record level from the child's, so
/// counting the whole file would credit the child with work the PARENT did. The
/// hand-off that separates them (the addressed inter-agent `agent_message`) is
/// not a reliable boundary either — a parent that already collected an earlier
/// sibling's reply carries one inside the forked prefix too.
///
/// So a forked thread yields no stats at all rather than wrong ones. The capsule
/// still names the sub-agent and badges its thread id (both come from the
/// PARENT's rollout); only the nested tool-call list is absent. The legacy
/// `agent-<id>.jsonl` shape has no such prefix and is unaffected.
fn is_forked_thread_header(value: &serde_json::Value) -> bool {
    value.get("type").and_then(|t| t.as_str()) == Some("session_meta")
        && value
            .pointer("/payload/parent_thread_id")
            .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
}

fn parse_codex_subagent_stats(
    session_dir: &std::path::Path,
    agent_id: &str,
) -> Option<AgentExecutionStats> {
    if agent_id.len() > 64 || agent_id.contains("..") || agent_id.contains('/') {
        return None;
    }

    // Try exact filename first (e.g., "agent-{agent_id}.jsonl"), then fall
    // back to files whose stem ends with the agent_id. Collect and sort
    // candidates to ensure deterministic selection across platforms.
    let exact_path = session_dir.join(format!("agent-{}.jsonl", agent_id));
    let session_file = if exact_path.is_file() {
        exact_path
    } else {
        let mut candidates: Vec<_> = fs::read_dir(session_dir)
            .ok()?
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    return None;
                }
                let stem = path.file_stem()?.to_string_lossy().into_owned();
                // Match only if the stem ends with the agent_id after a separator
                // (e.g., "session-abc123" matches agent_id "abc123")
                if stem == agent_id
                    || stem
                        .strip_suffix(agent_id)
                        .is_some_and(|prefix| prefix.ends_with('-') || prefix.ends_with('_'))
                {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();
        candidates.sort();
        candidates.into_iter().next()?
    };

    let file = fs::File::open(&session_file).ok()?;
    let reader = BufReader::new(file);

    let mut tool_calls = Vec::new();
    // A code-mode `exec` call expands to one entry per inner `tools.*` call, so
    // the sub-agent stats count real tools instead of a pile of `exec`.
    let mut pending_calls: HashMap<String, Vec<AgentToolCall>> = HashMap::new();
    let mut first_ts: Option<DateTime<Utc>> = None;
    let mut last_ts: Option<DateTime<Utc>> = None;
    let mut checked_header = false;

    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // The header decides whether this file can be counted at all — see
        // `is_forked_thread_header`. Checked on the first record that parses,
        // and never again.
        if !checked_header {
            checked_header = true;
            if is_forked_thread_header(&value) {
                return None;
            }
        }

        if let Some(ts) = parse_codex_timestamp(&value) {
            if first_ts.is_none() {
                first_ts = Some(ts);
            }
            last_ts = Some(ts);
        }

        if value.get("type").and_then(|t| t.as_str()) != Some("response_item") {
            continue;
        }
        let payload = match value.get("payload") {
            Some(p) => p,
            None => continue,
        };
        let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match payload_type {
            "function_call" | "custom_tool_call" => {
                let call_id = payload
                    .get("call_id")
                    .or_else(|| payload.get("tool_call_id"))
                    .or_else(|| payload.get("id"))
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string());
                let tool_name = payload
                    .get("name")
                    .or_else(|| payload.get("tool_name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                let entries = if is_code_mode_call(&tool_name) {
                    let source = payload
                        .get("input")
                        .or_else(|| payload.get("arguments"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let script = parse_code_mode_script(source);
                    match script.calls {
                        Some(calls) if !calls.is_empty() => calls
                            .into_iter()
                            .map(|call| AgentToolCall {
                                tool_name: call.tool_name,
                                input_preview: Some(truncate_str(&call.input_preview, 500)),
                                output_preview: None,
                                is_error: false,
                            })
                            .collect(),
                        _ => vec![AgentToolCall {
                            tool_name: CODEX_SCRIPT_TOOL_NAME.to_string(),
                            input_preview: script
                                .summary
                                .or_else(|| Some(source.to_string()))
                                .map(|s| truncate_str(&s, 500)),
                            output_preview: None,
                            is_error: false,
                        }],
                    }
                } else {
                    let input_preview = if tool_name == "exec_command" {
                        parse_codex_json_arg(payload)
                            .and_then(|a| {
                                a.get("cmd").and_then(|v| v.as_str()).map(|s| s.to_string())
                            })
                            .or_else(|| {
                                value_to_preview(
                                    payload.get("arguments").or_else(|| payload.get("input")),
                                )
                            })
                    } else {
                        value_to_preview(payload.get("arguments").or_else(|| payload.get("input")))
                    };

                    vec![AgentToolCall {
                        tool_name,
                        input_preview: input_preview.map(|s| truncate_str(&s, 500)),
                        output_preview: None,
                        is_error: false,
                    }]
                };

                if let Some(id) = call_id {
                    pending_calls.insert(id, entries);
                } else {
                    tool_calls.extend(entries);
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = payload
                    .get("call_id")
                    .or_else(|| payload.get("tool_call_id"))
                    .or_else(|| payload.get("id"))
                    .and_then(|id| id.as_str());

                if let Some(id) = call_id {
                    if let Some(entries) = pending_calls.remove(id) {
                        let output_value = payload.get("output");
                        let envelope = split_code_mode_output(output_value);
                        let per_call = entries.len() > 1 && envelope.chunks.len() == entries.len();
                        let joined = if envelope.status != ScriptStatus::Unknown
                            || output_value.is_some_and(|v| v.is_array())
                        {
                            Some(envelope.joined())
                        } else {
                            value_to_preview(output_value)
                        };

                        for (index, mut tc) in entries.into_iter().enumerate() {
                            let raw_output = if per_call {
                                Some(envelope.chunks[index].clone())
                            } else if index == 0 {
                                joined.clone()
                            } else {
                                None
                            };
                            if tc.tool_name == "exec_command" {
                                tc.output_preview = raw_output
                                    .map(|s| truncate_str(&clean_codex_exec_output(&s), 500));
                            } else {
                                tc.output_preview = raw_output.map(|s| truncate_str(&s, 500));
                            }
                            tc.is_error = envelope.is_error()
                                || infer_tool_call_output_is_error(
                                    payload,
                                    output_value,
                                    tc.output_preview.as_deref(),
                                );
                            tool_calls.push(tc);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    tool_calls.extend(pending_calls.into_values().flatten());

    let total_duration_ms = match (first_ts, last_ts) {
        (Some(f), Some(l)) => {
            let dur = (l - f).num_milliseconds();
            if dur > 0 {
                Some(dur as u64)
            } else {
                None
            }
        }
        _ => None,
    };

    let tool_count = tool_calls.len() as u32;
    Some(AgentExecutionStats {
        agent_type: None,
        status: None,
        total_duration_ms,
        total_tokens: None,
        total_tool_use_count: if tool_count > 0 {
            Some(tool_count)
        } else {
            None
        },
        read_count: None,
        search_count: None,
        bash_count: None,
        edit_file_count: None,
        lines_added: None,
        lines_removed: None,
        other_tool_count: None,
        tool_calls,
        // Codex's sub-agent rollout is folded into these stats; there is no
        // separate session for the card to open.
        child_session_id: None,
    })
}

impl CodexParser {
    fn parse_conversation_detail(
        &self,
        path: &PathBuf,
        conversation_id: &str,
    ) -> Result<ConversationDetail, ParseError> {
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);

        let mut messages = Vec::new();
        let mut cwd: Option<String> = None;
        let mut git_branch: Option<String> = None;
        let mut model: Option<String> = None;
        let mut title: Option<String> = None;
        // Objective of the goal run currently open while replaying `/goal`
        // transitions, so a persisted `thread_goal_updated` with `goal: null`
        // closes the run by objective — identical to the live path. See
        // `crate::acp::codex_goal::next_goal_marker`.
        let mut codex_open_goal: Option<String> = None;
        // Objective of the FIRST goal opened, captured for a post-parse fallback:
        // newer codex consumes `/goal <objective>` as a slash command (persists
        // `thread_goal_updated` but no `user_message`), so the typed `/goal …`
        // prompt has no user turn on reload. We surface the objective as the
        // leading user message AFTER the loop — never mid-loop — so the synthetic
        // message can never poison `should_skip_duplicate_user_message` and drop a
        // real same-text user message that arrives later in the file.
        let mut first_goal_objective: Option<String> = None;
        // Whether that first `/goal` OPENED the session — i.e. no real user turn
        // had been recorded when it arrived. This is positional, not "no user turn
        // anywhere": newer codex records the goal first with no `user_message`, so
        // the objective IS the opening prompt and must be surfaced as the leading
        // user turn even when a LATER real reply (e.g. a "确认") exists. Older codex
        // persisted the `/goal` text as the opening `user_message`, which arrives
        // BEFORE the goal — there the flag stays false and nothing is synthesized.
        let mut goal_opens_session = false;
        // Start-of-turn markers, chronological, fed to
        // `backfill_turn_durations` so the first reply of a turn is measured
        // from when codex began working — not from the previous turn's end,
        // which would charge it the user's thinking time.
        //
        // Two lists because the two events are not equally trustworthy.
        // `task_started` fires exactly once per turn. `turn_context` normally
        // follows it within milliseconds, but newer codex re-emits it MID-turn
        // (it carries a `turn_id` and is rewritten when the turn's config
        // changes) — 6 of 495 records across the local rollout corpus. Since a
        // marker only ever moves the boundary forward, a mid-turn one would
        // truncate the reply that follows it and silently drop that slice from
        // the turn's total. So `turn_context` is used only as a fallback, for
        // rollouts old enough to predate `task_started`.
        let mut task_start_markers: Vec<DateTime<Utc>> = Vec::new();
        let mut turn_context_markers: Vec<DateTime<Utc>> = Vec::new();
        let mut context_window_used_tokens: Option<u64> = None;
        let mut context_window_max_tokens: Option<u64> = None;
        let mut latest_total_usage: Option<TurnUsage> = None;
        let mut latest_total_tokens: Option<u64> = None;
        // Cumulative `total_token_usage` as of the previous `token_count`, so
        // each event can be reduced to what its own round-trip added.
        let mut previous_total_usage: Option<TurnUsage> = None;
        // Previous `last_token_usage`, used only by the no-total fallback to
        // recognize a restated event.
        let mut previous_last_usage: Option<TurnUsage> = None;
        // Round-trip spend recorded before any assistant message existed to
        // carry it; flushed onto the first one that appears.
        let mut pending_round_usage: Option<TurnUsage> = None;
        // Everything every round-trip reported, kept independently of which
        // message ended up carrying it — see `reconcile_turn_usage`.
        let mut recorded_round_usage = TurnUsage::default();

        let mut first_timestamp: Option<DateTime<Utc>> = None;
        let mut last_timestamp: Option<DateTime<Utc>> = None;

        // Agent subagent tracking (spawn_agent / wait_agent / close_agent).
        //
        // Capsule model (mirrors the live frontend, see collab-tool.ts):
        //   - spawn_agent → an "Agent" execution capsule (this file + nested
        //     stats from `agent-<id>.jsonl`). Shows the task + process; it does
        //     NOT carry the final result text (that lives in the wait capsule).
        //   - wait_agent  → a synthesized `collab_agent` capsule per wait, built
        //     from THAT wait's own `output.status` (the agents it returned). The
        //     full result text is shown here, via the same `CollabAgentCard` the
        //     live `wait` capsule uses. codex returns each agent's result in
        //     exactly one wait, so wait capsules never overlap.
        //   - close_agent → folded into the execution capsule (no own capsule);
        //     its result is only a fallback for agents never waited on.
        // codex-acp 1.0.1 (#223) maps `collabAgentToolCall` onto live ACP
        // `tool_call`s; 1.1.3+ (#304) additionally emits `subAgentActivity` as a
        // SEPARATE live `tool_call` (`_meta.codex.subagent`), but codeg
        // suppresses it (redundant with the collab capsule — see
        // `is_codex_subagent_activity`) and it carries no transcript content, so
        // the nested `agent-<id>.jsonl` stats still only exist on history reload.
        // Live and reconstructed capsules never double-render (live during
        // streaming, this on reload).
        let mut spawn_agent_call_ids: HashSet<String> = HashSet::new();
        let mut agent_id_to_spawn_call_id: HashMap<String, String> = HashMap::new();
        // Result text used to FILL the execution capsule only as a fallback for
        // agents that were never returned by a wait (keyed by agent_id). Filled
        // from close_agent's `previous_status`.
        let mut agent_fallback_results: HashMap<String, String> = HashMap::new();
        // Agents whose result was already shown in a wait capsule — their
        // execution capsule must NOT also show the result (no duplication).
        let mut agent_waited: HashSet<String> = HashSet::new();
        // Agents that ended in an error state (see `is_error_collab_status`:
        // errored/failed/notFound) in any wait or close — used to mark the
        // execution capsule as failed (live parity).
        let mut agent_errored: HashSet<String> = HashSet::new();
        let mut wait_agent_call_ids: HashSet<String> = HashSet::new();
        let mut close_agent_call_ids: HashSet<String> = HashSet::new();
        let mut close_agent_targets: HashMap<String, String> = HashMap::new();
        let mut active_agent_count: u32 = 0;
        let mut call_id_tool_names: HashMap<String, String> = HashMap::new();
        // Code-mode scripts (`custom_tool_call` named `exec`) awaiting their
        // output. A placeholder script card is pushed when the call is read, so
        // an interrupted turn still shows it in place; the output arm rewrites
        // that message's blocks once it knows how many `text()` chunks came
        // back. See `parsers/codex_code_mode.rs`.
        let mut pending_exec_scripts: HashMap<String, (usize, CodeModeScript)> = HashMap::new();
        // `exec_command` call_id → the command it ran, and the background shell
        // sessions that command's output announced (`session id → command`).
        // A later `wait` / `write_stdin` carries only the session id, so this is
        // what lets its card be titled by the command it is waiting on instead
        // of a bare `wait`. See `extract_shell_session_ids`.
        let mut call_id_commands: HashMap<String, String> = HashMap::new();
        let mut shell_sessions: HashMap<String, ShellSession> = HashMap::new();
        // Calls that only collect more of an earlier command's output: poll
        // `tool_use_id` → `tool_use_id` of the card holding that command. The
        // fold happens once parsing is done (`fold_shell_session_polls`), which
        // is what lets a poll written inside a code-mode script be folded on the
        // same footing as one codex called directly.
        let mut poll_origins: HashMap<String, String> = HashMap::new();
        // Scripts that answered `Script running with cell ID N` — they have NOT
        // finished, and the `wait` that later collects cell N carries their own
        // return value. Keyed by cell id so a parallel batch of waits each folds
        // into the right script no matter what order the records interleave in.
        let mut deferred_scripts: HashMap<String, DeferredScript> = HashMap::new();
        let mut deferred_waits: HashMap<String, String> = HashMap::new();
        // Codex 0.129+ writes a generated image both as `event_msg.image_generation_end`
        // and as `response_item.image_generation_call`, sharing the same call_id/id.
        // Emit at most one ContentBlock::Image per id to avoid duplicate display.
        let mut emitted_image_ids: HashSet<String> = HashSet::new();
        // Streaming reasoning buffer. Codex emits one `event_msg.agent_reasoning`
        // per reasoning section, then groups the same sections into a single
        // `response_item.reasoning.summary`. We buffer the per-section events and
        // let the grouped summary supersede them (one 思考 card per turn, live
        // parity); the buffer is only flushed on its own — as one joined Thinking
        // block — when no grouped summary arrives (interrupted/older rollouts),
        // so streaming reasoning is never lost. `pending_reasoning_ts` stamps the
        // fallback block with the last buffered section's time.
        let mut pending_reasoning: Vec<String> = Vec::new();
        let mut pending_reasoning_ts: Option<DateTime<Utc>> = None;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }

            let value: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let msg_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");

            if let Some(ts_str) = value.get("timestamp").and_then(|t| t.as_str()) {
                if let Ok(ts) = ts_str.parse::<DateTime<Utc>>() {
                    if first_timestamp.is_none() {
                        first_timestamp = Some(ts);
                    }
                    last_timestamp = Some(ts);
                }
            }

            match msg_type {
                "session_meta" => {
                    if let Some(payload) = value.get("payload") {
                        cwd = payload
                            .get("cwd")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                        git_branch = payload
                            .get("git")
                            .and_then(|g| g.get("branch"))
                            .and_then(|b| b.as_str())
                            .map(|s| s.to_string());
                    }
                }
                "turn_context" => {
                    // A new API turn means any prior agent lifecycle is complete.
                    active_agent_count = 0;
                    if model.is_none() {
                        model = value
                            .get("payload")
                            .and_then(|p| p.get("model"))
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string());
                    }
                    if let Some(ts) = parse_codex_timestamp(&value) {
                        push_turn_start(&mut turn_context_markers, ts);
                    }
                }
                "event_msg" => {
                    if let Some(payload) = value.get("payload") {
                        let payload_type =
                            payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

                        let timestamp = parse_codex_timestamp(&value).unwrap_or_else(Utc::now);

                        // A new reasoning section keeps buffering; `token_count` is
                        // metadata with no visible message and never splits a run.
                        // Anything else closes an open reasoning run — flush any
                        // buffered streaming reasoning that never got a grouped
                        // summary so it isn't lost or reordered behind this event.
                        if payload_type != "agent_reasoning" && payload_type != "token_count" {
                            flush_pending_reasoning(
                                &mut messages,
                                &mut pending_reasoning,
                                pending_reasoning_ts,
                            );
                        }

                        match payload_type {
                            // codex 0.147 stopped returning the sub-agent's id
                            // from `spawn_agent` (its output is just
                            // `{"task_name":"/root/pnpm_build"}`). This event is
                            // now the only place the parent's rollout names the
                            // child thread, and it correlates back by carrying
                            // the spawn's own `call_id` as `event_id`.
                            "sub_agent_activity" => {
                                let call_id = payload
                                    .get("event_id")
                                    .and_then(|v| v.as_str())
                                    .filter(|id| spawn_agent_call_ids.contains(*id));
                                let thread_id = payload
                                    .get("agent_thread_id")
                                    .and_then(|v| v.as_str())
                                    .filter(|id| !id.is_empty());
                                if let (Some(call_id), Some(thread_id)) = (call_id, thread_id) {
                                    agent_id_to_spawn_call_id
                                        .entry(thread_id.to_string())
                                        .or_insert_with(|| call_id.to_string());
                                }
                            }
                            "task_started" => {
                                if context_window_max_tokens.is_none() {
                                    context_window_max_tokens = payload
                                        .get("model_context_window")
                                        .and_then(|v| v.as_u64());
                                }
                                // The one marker codex writes exactly once per
                                // turn; it precedes `turn_context` and the
                                // prompt.
                                if let Some(ts) = parse_codex_timestamp(&value) {
                                    push_turn_start(&mut task_start_markers, ts);
                                }
                            }
                            "user_message" => {
                                active_agent_count = 0;
                                let text = payload
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let normalized = strip_blocked_resource_mentions(&text);
                                let mut blocks: Vec<ContentBlock> = Vec::new();
                                if !normalized.is_empty() {
                                    blocks.push(ContentBlock::Text { text: normalized });
                                }

                                if let Some(images) =
                                    payload.get("images").and_then(|v| v.as_array())
                                {
                                    for image in images {
                                        let Some(raw) = image.as_str() else {
                                            continue;
                                        };
                                        let Some((mime_type, data)) = parse_data_uri_image(raw)
                                        else {
                                            continue;
                                        };
                                        blocks.push(ContentBlock::Image {
                                            data,
                                            mime_type,
                                            uri: None,
                                        });
                                    }
                                }

                                if blocks.is_empty() {
                                    blocks.push(ContentBlock::Text {
                                        text: "Attached resources".to_string(),
                                    });
                                }

                                if title.is_none() {
                                    title = extract_codex_title_candidate(&text, true);
                                }

                                if should_skip_duplicate_user_message(&messages, &blocks, timestamp)
                                {
                                    continue;
                                }

                                messages.push(UnifiedMessage {
                                    id: format!("user-{}", messages.len()),
                                    role: MessageRole::User,
                                    content: blocks,
                                    timestamp,
                                    usage: None,
                                    duration_ms: None,
                                    model: None,
                                    completed_at: Some(timestamp),
                                });
                            }
                            "agent_message" => {
                                // Parent narration is emitted even while a
                                // sub-agent is active (active_agent_count > 0).
                                // codex-acp 1.0.x writes the sub-agent's own
                                // transcript to its `agent-<id>.jsonl`, NOT into
                                // the parent rollout, so every agent_message here
                                // is the parent's (verified across 180 real
                                // rollouts: 0 sub-agent leaks). The old
                                // `active_agent_count == 0` guard wrongly dropped
                                // the parent's between-capsule narration — and,
                                // when no close_agent ran (active never returns to
                                // 0), even the final answer. Images keep their own
                                // guard (see image_generation arms).
                                let text = payload
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                messages.push(UnifiedMessage {
                                    id: format!("assistant-{}", messages.len()),
                                    role: MessageRole::Assistant,
                                    content: vec![ContentBlock::Text { text }],
                                    timestamp,
                                    usage: None,
                                    duration_ms: None,
                                    model: None,
                                    completed_at: Some(timestamp),
                                });
                            }
                            "thread_goal_updated" => {
                                // codex-acp v1.1.0 (#263) routes live goals through
                                // `session_info_update`; the CLI has always persisted
                                // each `/goal` transition to the rollout as
                                // `event_msg.thread_goal_updated.goal`. Synthesize the
                                // same canonical create_goal/update_goal
                                // tool_use+tool_result the live path emits (shared
                                // mapping in `crate::acp::codex_goal`) so a reloaded
                                // conversation renders goal cards identical to live —
                                // history that never surfaced goals before.
                                if let Some(marker) = payload.get("goal").and_then(|goal| {
                                    crate::acp::codex_goal::next_goal_marker(
                                        &mut codex_open_goal,
                                        goal,
                                    )
                                }) {
                                    // Remember the first opened goal's objective for
                                    // the post-parse leading-user-message fallback (see
                                    // `first_goal_objective`). Deferred, not synthesized
                                    // here, so it can't interfere with duplicate
                                    // suppression of a later real user message.
                                    if marker.tool_name == "create_goal"
                                        && first_goal_objective.is_none()
                                    {
                                        // Positional: the goal opened the session iff no
                                        // real user turn exists yet.
                                        goal_opens_session = !messages
                                            .iter()
                                            .any(|m| matches!(m.role, MessageRole::User));
                                        // Claim the title from the objective HERE, in
                                        // stream order, when the goal is the opener — so
                                        // a LATER `user_message` (its own guard is
                                        // `title.is_none()`) can't steal it, while an
                                        // EARLIER real user (older codex) or a native
                                        // `thread_name_updated` still wins.
                                        if goal_opens_session && title.is_none() {
                                            title = extract_codex_title_candidate(
                                                &marker.objective,
                                                true,
                                            );
                                        }
                                        first_goal_objective = Some(marker.objective.clone());
                                    }
                                    // Occurrence id from the message index — unique
                                    // per goal event, stable across reparse, and
                                    // shared by this event's ToolUse + ToolResult.
                                    let id = crate::acp::codex_goal::goal_tool_call_id(
                                        messages.len() as u64,
                                    );
                                    messages.push(UnifiedMessage {
                                        id: format!("tool-{}", messages.len()),
                                        role: MessageRole::Assistant,
                                        content: vec![
                                            ContentBlock::ToolUse {
                                                tool_use_id: Some(id.clone()),
                                                tool_name: marker.tool_name.to_string(),
                                                input_preview: Some(marker.input_json),
                                                status: None,
                                                meta: None,
                                            },
                                            ContentBlock::ToolResult {
                                                tool_use_id: Some(id),
                                                output_preview: Some(marker.output_json),
                                                is_error: false,
                                                agent_stats: None,
                                                images: Vec::new(),
                                            },
                                        ],
                                        timestamp,
                                        usage: None,
                                        duration_ms: None,
                                        model: None,
                                        completed_at: Some(timestamp),
                                    });
                                }
                            }
                            "thread_name_updated" => {
                                // Codex's native thread name — adopt it as the
                                // auto-title (parity with Claude `aiTitle`, Gemini
                                // `update_topic`, OpenCode `session.title`). Newest
                                // non-empty wins, overriding the first-prompt
                                // fallback; `refresh_auto_title`'s `title_locked`
                                // guard still respects a manual rename.
                                // Rollout persists `thread_name` (snake_case); the
                                // live ACP notification uses `threadName`. Accept
                                // both so the parser is robust to either source.
                                if let Some(name) = payload
                                    .get("thread_name")
                                    .or_else(|| payload.get("threadName"))
                                    .or_else(|| payload.get("name"))
                                    .and_then(|n| n.as_str())
                                    .map(str::trim)
                                    .filter(|n| !n.is_empty())
                                {
                                    title = Some(truncate_str(name, 100));
                                }
                            }
                            "agent_reasoning" => {
                                // Buffer this streaming reasoning section. The grouped
                                // `response_item.reasoning.summary` (parsed in the
                                // `response_item` match below) normally arrives right
                                // after the section events and supersedes the buffer,
                                // so history shows ONE 思考 card per turn (live parity)
                                // instead of one card per section. If no grouped
                                // summary arrives (interrupted/older rollouts), the
                                // buffer is flushed on its own and nothing is lost.
                                let text = payload
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("");
                                if !text.trim().is_empty() {
                                    pending_reasoning.push(text.to_string());
                                    pending_reasoning_ts = Some(timestamp);
                                }
                            }
                            "image_generation_end" => {
                                if active_agent_count > 0 {
                                    continue;
                                }
                                let call_id = payload
                                    .get("call_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let result =
                                    payload.get("result").and_then(|v| v.as_str()).unwrap_or("");
                                if result.is_empty() {
                                    continue;
                                }
                                if !call_id.is_empty() && emitted_image_ids.contains(&call_id) {
                                    continue;
                                }
                                let mime_type = payload
                                    .get("mime_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("image/png")
                                    .to_string();
                                let uri = payload
                                    .get("saved_path")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                let revised_prompt = payload
                                    .get("revised_prompt")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .filter(|s| !s.trim().is_empty());
                                messages.push(UnifiedMessage {
                                    id: format!("assistant-imagegen-{}", messages.len()),
                                    role: MessageRole::Assistant,
                                    content: vec![ContentBlock::ImageGeneration {
                                        revised_prompt,
                                        image: Some(ImageData {
                                            data: result.to_string(),
                                            mime_type,
                                            uri,
                                        }),
                                    }],
                                    timestamp,
                                    usage: None,
                                    duration_ms: None,
                                    model: None,
                                    completed_at: Some(timestamp),
                                });
                                if !call_id.is_empty() {
                                    emitted_image_ids.insert(call_id);
                                }
                            }
                            "token_count" => {
                                if let Some(info) = payload.get("info") {
                                    if let Some(total_usage_payload) = info.get("total_token_usage")
                                    {
                                        if let Some(total_usage) =
                                            extract_turn_usage_from_codex_usage(total_usage_payload)
                                        {
                                            latest_total_usage = Some(total_usage);
                                        }
                                        if let Some(total_tokens) =
                                            extract_total_tokens_from_usage(total_usage_payload)
                                        {
                                            latest_total_tokens = Some(total_tokens);
                                        }
                                    }

                                    let total_tokens =
                                        extract_context_window_used_tokens_from_token_count_info(
                                            info,
                                        );
                                    if total_tokens.is_some() {
                                        context_window_used_tokens = total_tokens;
                                    }

                                    let context_window =
                                        info.get("model_context_window").and_then(|v| v.as_u64());
                                    if context_window.is_some() {
                                        context_window_max_tokens = context_window;
                                    }

                                    if !info.is_null() {
                                        // What this round-trip spent. Preferred
                                        // as the rise in the session's own
                                        // cumulative counter; `last_token_usage`
                                        // is the fallback for transcripts that
                                        // report no total, and there a repeat of
                                        // the previous event is dropped since it
                                        // restates one call rather than adding
                                        // another.
                                        let round = match info.get("total_token_usage") {
                                            Some(total_payload) => {
                                                let total = codex_usage_counters(total_payload);
                                                let round = codex_round_usage(
                                                    previous_total_usage.as_ref(),
                                                    &total,
                                                );
                                                previous_total_usage = Some(total);
                                                round
                                            }
                                            None => match info.get("last_token_usage") {
                                                Some(last_payload) => {
                                                    let last = codex_usage_counters(last_payload);
                                                    let repeated = previous_last_usage
                                                        .as_ref()
                                                        .is_some_and(|prev| prev == &last);
                                                    previous_last_usage = Some(last.clone());
                                                    if repeated {
                                                        TurnUsage::default()
                                                    } else {
                                                        // Carry this round into the
                                                        // cumulative baseline too.
                                                        // A transcript that mixes
                                                        // both shapes would
                                                        // otherwise count it twice:
                                                        // once here, and again
                                                        // inside the next
                                                        // total-bearing event's
                                                        // delta, which is measured
                                                        // from a baseline that
                                                        // never learned about it.
                                                        let base = previous_total_usage
                                                            .clone()
                                                            .unwrap_or_default();
                                                        previous_total_usage =
                                                            Some(codex_usage_add(&base, &last));
                                                        last
                                                    }
                                                }
                                                None => TurnUsage::default(),
                                            },
                                        };

                                        if !codex_usage_is_zero(&round) {
                                            recorded_round_usage =
                                                codex_usage_add(&recorded_round_usage, &round);
                                            // Every round of a turn belongs to
                                            // the assistant message it worked
                                            // for, so they accumulate onto it
                                            // rather than the first one winning
                                            // and the rest being dropped. A
                                            // round that ran before any
                                            // assistant message (the model went
                                            // straight to a tool call) waits
                                            // here for one to arrive — 3 % of
                                            // all recorded spend, previously
                                            // discarded outright.
                                            pending_round_usage = Some(match pending_round_usage {
                                                Some(ref pending) => {
                                                    codex_usage_add(pending, &round)
                                                }
                                                None => round,
                                            });
                                        }
                                        if let (Some(pending), Some(last_msg)) = (
                                            pending_round_usage.clone(),
                                            messages
                                                .iter_mut()
                                                .rev()
                                                .find(|m| matches!(m.role, MessageRole::Assistant)),
                                        ) {
                                            last_msg.usage = Some(match last_msg.usage {
                                                Some(ref existing) => {
                                                    codex_usage_add(existing, &pending)
                                                }
                                                None => pending,
                                            });
                                            pending_round_usage = None;
                                        }
                                    }
                                }
                                // Durations are NOT derived here. `token_count`
                                // fires once per model request, so measuring
                                // turn_context → token_count restated the whole
                                // elapsed turn on every sub-turn; the UI sums
                                // sub-turns into one card, which multiplied a
                                // reply's reported time several-fold. See
                                // `backfill_turn_durations`, applied after
                                // grouping, which partitions the turn instead.
                            }
                            _ => {}
                        }
                    }
                }
                "response_item" => {
                    if let Some(payload) = value.get("payload") {
                        let payload_type =
                            payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        let timestamp = parse_codex_timestamp(&value).unwrap_or_else(Utc::now);

                        // A `reasoning` item resolves the buffered streaming sections
                        // (handled in its arm). Any other response item closes an open
                        // reasoning run — flush buffered streaming reasoning that never
                        // got a grouped summary so it isn't lost or reordered.
                        if payload_type != "reasoning" {
                            flush_pending_reasoning(
                                &mut messages,
                                &mut pending_reasoning,
                                pending_reasoning_ts,
                            );
                        }

                        match payload_type {
                            "reasoning" => {
                                // Codex records a reasoning turn as a `summary` array
                                // of `{type:"summary_text", text}` parts — one part per
                                // section — grouping the same sections the streaming
                                // `event_msg.agent_reasoning` events carry one-by-one
                                // (buffered in `pending_reasoning`). Join the parts into
                                // ONE Thinking block (live parity: a single 思考 card
                                // per turn) and discard the buffer it supersedes. An
                                // empty summary (encrypted-only reasoning, the common
                                // case) carries no surfaced text, so fall back to any
                                // buffered streaming sections (interrupted/older
                                // rollouts) and otherwise emit nothing.
                                let text = payload
                                    .get("summary")
                                    .and_then(|s| s.as_array())
                                    .map(|parts| {
                                        parts
                                            .iter()
                                            .filter_map(|p| {
                                                p.get("text").and_then(|t| t.as_str())
                                            })
                                            .filter(|t| !t.trim().is_empty())
                                            .collect::<Vec<_>>()
                                            .join("\n\n")
                                    })
                                    .unwrap_or_default();
                                if !text.is_empty() {
                                    pending_reasoning.clear();
                                    messages.push(UnifiedMessage {
                                        id: format!("thinking-{}", messages.len()),
                                        role: MessageRole::Assistant,
                                        content: vec![ContentBlock::Thinking { text }],
                                        timestamp,
                                        usage: None,
                                        duration_ms: None,
                                        model: None,
                                        completed_at: Some(timestamp),
                                    });
                                } else {
                                    flush_pending_reasoning(
                                        &mut messages,
                                        &mut pending_reasoning,
                                        pending_reasoning_ts,
                                    );
                                }
                            }
                            "function_call" | "custom_tool_call" => {
                                let tool_use_id = payload
                                    .get("call_id")
                                    .or_else(|| payload.get("tool_call_id"))
                                    .or_else(|| payload.get("id"))
                                    .and_then(|id| id.as_str())
                                    .map(|s| s.to_string());
                                let raw_tool_name = payload
                                    .get("name")
                                    .or_else(|| payload.get("tool_name"))
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("unknown");

                                // A `wait` collecting a code-mode script that
                                // has not finished. Matched by cell id, never by
                                // adjacency — parallel calls interleave freely.
                                let deferred_cell = if raw_tool_name == "wait" {
                                    parse_codex_json_arg(payload)
                                        .as_ref()
                                        .and_then(|a| a.as_object())
                                        .and_then(shell_session_id)
                                        .filter(|cell| deferred_scripts.contains_key(cell))
                                } else {
                                    None
                                };

                                match raw_tool_name {
                                    // No card of its own: the output it is about
                                    // to collect IS the parked script's return
                                    // value, and the output arm writes it back
                                    // onto that script's cards.
                                    _ if deferred_cell.is_some() => {
                                        if let (Some(id), Some(cell)) = (tool_use_id, deferred_cell)
                                        {
                                            deferred_waits.insert(id, cell);
                                        }
                                    }
                                    "spawn_agent" => {
                                        let args = parse_codex_json_arg(payload);
                                        // codex 0.147's team-of-agents renamed the
                                        // label: the old `agent_type` became
                                        // `task_name` (`pnpm_build`). Reading only
                                        // the old key left every capsule titled
                                        // "agent".
                                        let agent_type = args
                                            .as_ref()
                                            .and_then(|a| {
                                                a.get("agent_type").or_else(|| a.get("task_name"))
                                            })
                                            .and_then(|v| v.as_str())
                                            .filter(|s| !s.trim().is_empty())
                                            .unwrap_or("agent");
                                        // Same release made the hand-off message an
                                        // encrypted envelope; rendering it verbatim
                                        // filled the capsule's title and prompt with
                                        // a wall of base64 (see `is_encrypted_envelope`).
                                        let message = args
                                            .as_ref()
                                            .and_then(|a| a.get("message"))
                                            .and_then(|v| v.as_str())
                                            .filter(|m| !is_encrypted_envelope(m))
                                            .unwrap_or("");
                                        let description =
                                            truncate_str(message.lines().next().unwrap_or(""), 60);

                                        if let Some(ref id) = tool_use_id {
                                            spawn_agent_call_ids.insert(id.clone());
                                        }
                                        active_agent_count += 1;

                                        let mut agent_input = serde_json::json!({
                                            "subagent_type": agent_type,
                                            "prompt": message,
                                            "description": description,
                                        });
                                        // The 0.147 shape (`task_name`, no
                                        // `agent_type`) is the one where this
                                        // capsule is ONLY a launch: there is no
                                        // wait/close capsule to carry the result,
                                        // so the card must not read as "the
                                        // sub-agent finished". Legacy spawns keep
                                        // their old meaning and no marker.
                                        if is_native_team_spawn(args.as_ref()) {
                                            if let Some(obj) = agent_input.as_object_mut() {
                                                obj.insert(
                                                    CODEX_SUBAGENT_LAUNCH_KEY.to_string(),
                                                    serde_json::Value::Bool(true),
                                                );
                                            }
                                        }

                                        messages.push(UnifiedMessage {
                                            id: format!("tool-{}", messages.len()),
                                            role: MessageRole::Assistant,
                                            content: vec![ContentBlock::ToolUse {
                                                tool_use_id,
                                                tool_name: "Agent".to_string(),
                                                input_preview: Some(agent_input.to_string()),
                                                status: None,
                                                meta: None,
                                            }],
                                            timestamp,
                                            usage: None,
                                            duration_ms: None,
                                            model: None,
                                            completed_at: Some(timestamp),
                                        });
                                    }
                                    "wait_agent" => {
                                        if let Some(ref id) = tool_use_id {
                                            wait_agent_call_ids.insert(id.clone());
                                        }
                                    }
                                    "close_agent" => {
                                        if let Some(ref id) = tool_use_id {
                                            close_agent_call_ids.insert(id.clone());
                                            let target =
                                                parse_codex_json_arg(payload).and_then(|a| {
                                                    a.get("target")
                                                        .and_then(|v| v.as_str())
                                                        .map(|s| s.to_string())
                                                });
                                            if let Some(target) = target {
                                                close_agent_targets.insert(id.clone(), target);
                                            }
                                        }
                                    }
                                    // Code mode: the whole turn's tool calls are
                                    // wrapped in one JS script. Park a script
                                    // card here and let the output arm replace
                                    // it with the real per-call cards — the
                                    // split depends on how many `text()` chunks
                                    // came back, which is only known then. An
                                    // interrupted turn never gets an output, so
                                    // the placeholder must be pushed now to keep
                                    // its position.
                                    _ if is_code_mode_call(raw_tool_name) => {
                                        let source = payload
                                            .get("input")
                                            .or_else(|| payload.get("arguments"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let script = parse_code_mode_script(source);
                                        if let Some(ref id) = tool_use_id {
                                            pending_exec_scripts.insert(
                                                id.clone(),
                                                (messages.len(), script.clone()),
                                            );
                                        }
                                        messages.push(UnifiedMessage {
                                            id: format!("tool-{}", messages.len()),
                                            role: MessageRole::Assistant,
                                            content: vec![ContentBlock::ToolUse {
                                                tool_use_id,
                                                tool_name: CODEX_SCRIPT_TOOL_NAME.to_string(),
                                                input_preview: Some(script_card_input(
                                                    source,
                                                    script.summary.as_deref(),
                                                    script.call_sites,
                                                )),
                                                status: None,
                                                meta: None,
                                            }],
                                            timestamp,
                                            usage: None,
                                            duration_ms: None,
                                            model: None,
                                            completed_at: Some(timestamp),
                                        });
                                    }
                                    _ => {
                                        if let Some(ref id) = tool_use_id {
                                            call_id_tool_names
                                                .insert(id.clone(), raw_tool_name.to_string());
                                        }
                                        let raw_args = || {
                                            // `send_message` and friends carry a
                                            // sealed inter-agent `message`; show a
                                            // marker instead of half a kilobyte of
                                            // base64 (see `redact_encrypted_args`).
                                            // Every other tool takes the original
                                            // path and renders byte-identically.
                                            if let Some(mut args) = parse_codex_json_arg(payload) {
                                                if redact_encrypted_args(&mut args) {
                                                    return value_to_preview(Some(&args));
                                                }
                                            }
                                            value_to_preview(
                                                payload
                                                    .get("arguments")
                                                    .or_else(|| payload.get("input")),
                                            )
                                        };
                                        let input_preview = if raw_tool_name == "exec_command" {
                                            let cmd = parse_codex_json_arg(payload).and_then(|a| {
                                                a.get("cmd")
                                                    .and_then(|v| v.as_str())
                                                    .map(|s| s.to_string())
                                            });
                                            // Remembered so this command can be
                                            // attached to whatever background
                                            // session its output announces.
                                            if let (Some(id), Some(cmd)) =
                                                (tool_use_id.as_ref(), cmd.as_ref())
                                            {
                                                call_id_commands.insert(id.clone(), cmd.clone());
                                            }
                                            cmd.or_else(raw_args)
                                        } else if is_shell_session_tool(raw_tool_name) {
                                            // `wait` / `write_stdin`: carry the
                                            // command that started the session
                                            // they address, when it is known.
                                            let args = parse_codex_json_arg(payload);
                                            note_shell_session_call(
                                                tool_use_id.as_deref(),
                                                args.as_ref(),
                                                &mut shell_sessions,
                                                &mut poll_origins,
                                            );
                                            args.and_then(|args| {
                                                annotate_shell_session_args(&args, &shell_sessions)
                                            })
                                            .or_else(raw_args)
                                        } else {
                                            raw_args()
                                        };
                                        messages.push(UnifiedMessage {
                                            id: format!("tool-{}", messages.len()),
                                            role: MessageRole::Assistant,
                                            content: vec![ContentBlock::ToolUse {
                                                tool_use_id,
                                                tool_name: raw_tool_name.to_string(),
                                                input_preview,
                                                status: None,
                                                meta: None,
                                            }],
                                            timestamp,
                                            usage: None,
                                            duration_ms: None,
                                            model: None,
                                            completed_at: Some(timestamp),
                                        });
                                    }
                                }
                            }
                            "function_call_output" | "custom_tool_call_output" => {
                                let tool_use_id = payload
                                    .get("call_id")
                                    .or_else(|| payload.get("tool_call_id"))
                                    .or_else(|| payload.get("id"))
                                    .and_then(|id| id.as_str())
                                    .map(|s| s.to_string());

                                let is_spawn = tool_use_id
                                    .as_ref()
                                    .is_some_and(|id| spawn_agent_call_ids.contains(id));
                                let is_wait = tool_use_id
                                    .as_ref()
                                    .is_some_and(|id| wait_agent_call_ids.contains(id));
                                let is_close = tool_use_id
                                    .as_ref()
                                    .is_some_and(|id| close_agent_call_ids.contains(id));
                                let pending_script = tool_use_id
                                    .as_ref()
                                    .and_then(|id| pending_exec_scripts.remove(id));
                                let deferred = tool_use_id
                                    .as_ref()
                                    .and_then(|id| deferred_waits.remove(id))
                                    .and_then(|cell| deferred_scripts.remove(&cell));

                                if let Some(deferred) = deferred {
                                    // The parked script's real return value, at
                                    // last. Re-decompose it against that script
                                    // and rewrite the cards it already owns —
                                    // this `wait` contributes no card of its own.
                                    let collected = split_code_mode_output(payload.get("output"));
                                    // The whole run, not just this instalment:
                                    // see `DeferredScript::chunks`.
                                    let parsed = CodeModeOutput {
                                        status: collected.status,
                                        chunks: deferred
                                            .chunks
                                            .iter()
                                            .cloned()
                                            .chain(collected.chunks)
                                            .collect(),
                                        note: collected.note,
                                    };
                                    let (uses, results) = unwrap_code_mode_script(
                                        &deferred.call_id,
                                        &deferred.script,
                                        &parsed,
                                        payload,
                                        &mut shell_sessions,
                                        &mut poll_origins,
                                    );
                                    if let Some(uses) = uses {
                                        messages[deferred.use_index].content = uses;
                                    }
                                    messages[deferred.result_index].content = results;
                                    // Still not done: whatever cell it reports
                                    // now is what the next `wait` collects.
                                    if parsed.status == ScriptStatus::Running {
                                        let deferred = DeferredScript {
                                            chunks: parsed.chunks,
                                            ..deferred
                                        };
                                        for cell in extract_shell_session_ids(
                                            parsed.note.as_deref().unwrap_or_default(),
                                        ) {
                                            deferred_scripts.insert(cell, deferred.clone());
                                        }
                                    }
                                } else if let Some((message_index, script)) = pending_script {
                                    let call_id = tool_use_id.unwrap_or_default();
                                    let parsed = split_code_mode_output(payload.get("output"));
                                    let (uses, results) = unwrap_code_mode_script(
                                        &call_id,
                                        &script,
                                        &parsed,
                                        payload,
                                        &mut shell_sessions,
                                        &mut poll_origins,
                                    );
                                    if let Some(uses) = uses {
                                        messages[message_index].content = uses;
                                    }
                                    if parsed.status == ScriptStatus::Running {
                                        let deferred = DeferredScript {
                                            call_id: call_id.clone(),
                                            use_index: message_index,
                                            result_index: messages.len(),
                                            script: script.clone(),
                                            chunks: parsed.chunks.clone(),
                                        };
                                        for cell in extract_shell_session_ids(
                                            parsed.note.as_deref().unwrap_or_default(),
                                        ) {
                                            deferred_scripts.insert(cell, deferred.clone());
                                        }
                                    }
                                    messages.push(UnifiedMessage {
                                        id: format!("tool-result-{}", messages.len()),
                                        role: MessageRole::Tool,
                                        content: results,
                                        timestamp,
                                        usage: None,
                                        duration_ms: None,
                                        model: None,
                                        completed_at: Some(timestamp),
                                    });
                                } else if is_spawn {
                                    if let Some(output_obj) = parse_codex_json_output(payload) {
                                        if let (Some(agent_id), Some(call_id)) = (
                                            output_obj.get("agent_id").and_then(|v| v.as_str()),
                                            tool_use_id.as_ref(),
                                        ) {
                                            agent_id_to_spawn_call_id
                                                .insert(agent_id.to_string(), call_id.clone());
                                        }
                                    }
                                    messages.push(UnifiedMessage {
                                        id: format!("tool-result-{}", messages.len()),
                                        role: MessageRole::Tool,
                                        content: vec![ContentBlock::ToolResult {
                                            tool_use_id,
                                            output_preview: None,
                                            is_error: false,
                                            agent_stats: None,
                                            images: Vec::new(),
                                        }],
                                        timestamp,
                                        usage: None,
                                        duration_ms: None,
                                        model: None,
                                        completed_at: Some(timestamp),
                                    });
                                } else if is_wait {
                                    // Emit one `collab_agent` capsule per wait,
                                    // routed through the same CollabAgentCard as
                                    // the live wait capsule. Two output shapes —
                                    // see `native_team_wait_input`.
                                    let capsule = parse_codex_json_output(payload).and_then(
                                        |output_obj| match output_obj
                                            .get("status")
                                            .and_then(|s| s.as_object())
                                        {
                                            Some(status) => {
                                                // Mark returned agents so the spawn
                                                // capsule won't also show their
                                                // result, and record per-agent error
                                                // state so the execution capsule can
                                                // render failed (live parity).
                                                for (agent_id, value) in status {
                                                    agent_waited.insert(agent_id.clone());
                                                    let (st, _) = extract_wait_agent_status(value);
                                                    if is_error_collab_status(&st) {
                                                        agent_errored.insert(agent_id.clone());
                                                    }
                                                }
                                                (!status.is_empty())
                                                    .then(|| build_collab_wait_input(status))
                                            }
                                            None => native_team_wait_input(&output_obj),
                                        },
                                    );
                                    if let Some((collab_input, is_error)) = capsule {
                                        messages.push(UnifiedMessage {
                                            id: format!("tool-{}", messages.len()),
                                            role: MessageRole::Assistant,
                                            content: vec![ContentBlock::ToolUse {
                                                tool_use_id: tool_use_id.clone(),
                                                tool_name: "collab_agent".to_string(),
                                                input_preview: Some(collab_input),
                                                status: None,
                                                meta: None,
                                            }],
                                            timestamp,
                                            usage: None,
                                            duration_ms: None,
                                            model: None,
                                            completed_at: Some(timestamp),
                                        });
                                        messages.push(UnifiedMessage {
                                            id: format!("tool-result-{}", messages.len()),
                                            role: MessageRole::Tool,
                                            content: vec![ContentBlock::ToolResult {
                                                tool_use_id,
                                                output_preview: None,
                                                is_error,
                                                agent_stats: None,
                                                images: Vec::new(),
                                            }],
                                            timestamp,
                                            usage: None,
                                            duration_ms: None,
                                            model: None,
                                            completed_at: Some(timestamp),
                                        });
                                    }
                                } else if is_close {
                                    active_agent_count = active_agent_count.saturating_sub(1);
                                    if let Some(output_obj) = parse_codex_json_output(payload) {
                                        if let Some(agent_id) = tool_use_id
                                            .as_ref()
                                            .and_then(|id| close_agent_targets.get(id))
                                        {
                                            // Generalize over the terminal key (not
                                            // just `completed`): an errored/notFound
                                            // close with no wait must not lose its
                                            // message or its error state.
                                            if let Some(prev) =
                                                output_obj.get("previous_status")
                                            {
                                                let (st, msg) =
                                                    extract_wait_agent_status(prev);
                                                if let Some(text) = msg {
                                                    agent_fallback_results
                                                        .entry(agent_id.clone())
                                                        .or_insert(text);
                                                }
                                                if is_error_collab_status(&st) {
                                                    agent_errored.insert(agent_id.clone());
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    let is_exec = tool_use_id.as_ref().is_some_and(|id| {
                                        call_id_tool_names
                                            .get(id)
                                            .is_some_and(|n| n == "exec_command")
                                    });
                                    let output_value = payload.get("output");
                                    // Unified-exec tools (`wait`, …) share the
                                    // code-mode output shape: an ARRAY of
                                    // `{type:"input_text", text}` parts behind a
                                    // `Script …/Wall time …/Output:` header.
                                    // `value_to_preview` would dump that array as
                                    // raw JSON into the result panel.
                                    let envelope = split_code_mode_output(output_value);
                                    // An `exec_command` that left a background
                                    // shell running announces its id here; bind
                                    // it to the command so the `wait` /
                                    // `write_stdin` calls that address it later
                                    // can be titled by that command. Read before
                                    // `clean_codex_exec_output`, which drops
                                    // everything above the `Output:` line — the
                                    // announcement included.
                                    if let Some((origin, command)) =
                                        tool_use_id.as_ref().and_then(|id| {
                                            call_id_commands.get(id).map(|cmd| (id.clone(), cmd))
                                        })
                                    {
                                        let mut announced = envelope.joined();
                                        if let Some(note) = envelope.note.as_deref() {
                                            announced.push('\n');
                                            announced.push_str(note);
                                        }
                                        register_announced_sessions(
                                            command,
                                            &origin,
                                            &announced,
                                            &mut shell_sessions,
                                        );
                                    }
                                    let (raw_output, envelope_error) =
                                        if envelope.status != ScriptStatus::Unknown
                                            || output_value.is_some_and(|v| v.is_array())
                                        {
                                            (
                                                with_note(
                                                    Some(envelope.joined()),
                                                    envelope.note.as_deref(),
                                                )
                                                .filter(|s| !s.is_empty()),
                                                envelope.is_error(),
                                            )
                                        } else {
                                            (value_to_preview(output_value), false)
                                        };
                                    // A poll about to be folded into the card of
                                    // the command it is collecting for: its
                                    // envelope has to go, and an envelope that
                                    // wrapped nothing has to end up empty rather
                                    // than falling back to its own header.
                                    let is_folded_poll = tool_use_id
                                        .as_ref()
                                        .is_some_and(|id| poll_origins.contains_key(id));
                                    let output = if is_folded_poll {
                                        raw_output
                                            .map(|s| strip_exec_envelope(&s).unwrap_or(s))
                                            .filter(|s| !s.is_empty())
                                    } else if is_exec {
                                        raw_output.map(|s| clean_codex_exec_output(&s))
                                    } else {
                                        raw_output
                                    };
                                    let is_error = envelope_error
                                        || infer_tool_call_output_is_error(
                                            payload,
                                            output_value,
                                            output.as_deref(),
                                        );
                                    messages.push(UnifiedMessage {
                                        id: format!("tool-result-{}", messages.len()),
                                        role: MessageRole::Tool,
                                        content: vec![ContentBlock::ToolResult {
                                            tool_use_id,
                                            output_preview: output,
                                            is_error,
                                            agent_stats: None,
                                            images: Vec::new(),
                                        }],
                                        timestamp,
                                        usage: None,
                                        duration_ms: None,
                                        model: None,
                                        completed_at: Some(timestamp),
                                    });
                                }
                            }
                            "message" => {
                                let role =
                                    payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                                if role == "user" {
                                    active_agent_count = 0;
                                    if let Some(blocks) =
                                        extract_response_item_user_image_blocks(payload)
                                    {
                                        if should_skip_duplicate_user_message(
                                            &messages, &blocks, timestamp,
                                        ) {
                                            continue;
                                        }

                                        if title.is_none() {
                                            if let Some(text) = first_text_block(&blocks) {
                                                title = extract_codex_title_candidate(
                                                    text.as_str(),
                                                    true,
                                                );
                                            }
                                        }

                                        messages.push(UnifiedMessage {
                                            id: format!("user-{}", messages.len()),
                                            role: MessageRole::User,
                                            content: blocks,
                                            timestamp,
                                            usage: None,
                                            duration_ms: None,
                                            model: None,
                                            completed_at: Some(timestamp),
                                        });
                                    }
                                }
                            }
                            "image_generation_call" => {
                                // codex 0.129+ writes the same generated image as both an
                                // `event_msg.image_generation_end` (earlier in the file) and
                                // a `response_item.image_generation_call` (here). They share
                                // the same id; emit at most once via emitted_image_ids.
                                // Subagent suppression mirrors the event_msg arm: parent
                                // timeline must not host children's generated images.
                                if active_agent_count > 0 {
                                    continue;
                                }
                                let id = payload
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                if !id.is_empty() && emitted_image_ids.contains(&id) {
                                    continue;
                                }
                                let result =
                                    payload.get("result").and_then(|v| v.as_str()).unwrap_or("");
                                if result.is_empty() {
                                    continue;
                                }
                                let mime_type = payload
                                    .get("mime_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("image/png")
                                    .to_string();
                                let revised_prompt = payload
                                    .get("revised_prompt")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .filter(|s| !s.trim().is_empty());
                                messages.push(UnifiedMessage {
                                    id: format!("assistant-imagegen-{}", messages.len()),
                                    role: MessageRole::Assistant,
                                    content: vec![ContentBlock::ImageGeneration {
                                        revised_prompt,
                                        image: Some(ImageData {
                                            data: result.to_string(),
                                            mime_type,
                                            // response_item.image_generation_call has no
                                            // saved_path; event_msg.image_generation_end is
                                            // the only carrier of the on-disk file URI.
                                            uri: None,
                                        }),
                                    }],
                                    timestamp,
                                    usage: None,
                                    duration_ms: None,
                                    model: None,
                                    completed_at: Some(timestamp),
                                });
                                if !id.is_empty() {
                                    emitted_image_ids.insert(id);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        // Streaming reasoning at the very end of a truncated/interrupted rollout
        // (the `agent_reasoning` events were written but the file ended before the
        // grouped `response_item.reasoning` summary) — flush it so it isn't lost.
        flush_pending_reasoning(&mut messages, &mut pending_reasoning, pending_reasoning_ts);

        // Fill in subagent tool call stats (and, only as a fallback, the result)
        // on each spawn execution capsule.
        if !agent_id_to_spawn_call_id.is_empty() {
            let spawn_call_to_agent: HashMap<&str, &str> = agent_id_to_spawn_call_id
                .iter()
                .map(|(agent_id, call_id)| (call_id.as_str(), agent_id.as_str()))
                .collect();

            let session_dir = path.parent();
            let mut agent_stats_cache: HashMap<String, Option<AgentExecutionStats>> =
                HashMap::new();

            for msg in &mut messages {
                for block in &mut msg.content {
                    match block {
                        ContentBlock::ToolResult {
                            tool_use_id: Some(ref id),
                            ref mut output_preview,
                            ref mut is_error,
                            ref mut agent_stats,
                            ..
                        } => {
                            if let Some(&agent_id) = spawn_call_to_agent.get(id.as_str()) {
                                // The result text normally lives in the wait
                                // capsule; only show it on the execution capsule
                                // when this agent was never returned by a wait
                                // (else duplicate).
                                if !agent_waited.contains(agent_id) {
                                    if let Some(result) = agent_fallback_results.get(agent_id) {
                                        *output_preview = Some(result.clone());
                                    }
                                }
                                // Mark the execution capsule failed when the agent
                                // ended in error (in a wait or close) — live parity.
                                if agent_errored.contains(agent_id) {
                                    *is_error = true;
                                }
                                if let Some(dir) = session_dir {
                                    let stats = agent_stats_cache.entry(agent_id.to_string())
                                        .or_insert_with(|| {
                                            parse_codex_subagent_stats(dir, agent_id)
                                        });
                                    if stats.is_some() {
                                        *agent_stats = stats.clone();
                                    }
                                }
                            }
                        }
                        // Stamp the sub-agent's id onto the spawn execution capsule
                        // input so the card can render it (parity with the wait
                        // capsule, whose agentsStates already carry the id).
                        ContentBlock::ToolUse {
                            tool_use_id: Some(ref id),
                            ref tool_name,
                            ref mut input_preview,
                            ..
                        } if tool_name == "Agent" => {
                            if let Some(&agent_id) = spawn_call_to_agent.get(id.as_str()) {
                                *input_preview = Some(inject_agent_id_into_input(
                                    input_preview.as_deref(),
                                    agent_id,
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Leading-`/goal` fallback: when a `/goal` opened the session (before any
        // real user turn), newer codex recorded only `thread_goal_updated` — no
        // `user_message` — so the typed `/goal <objective>` prompt would be missing
        // on reload (headless, or, when a later reply like "确认" exists, starting
        // mid-conversation). Surface it as the leading user message, prefixed with
        // `/goal ` to match what the user actually typed and the live optimistic
        // bubble. The title was already claimed in-loop (see the goal-capture
        // block). Applied here, after parsing, so the synthetic turn never
        // participates in `should_skip_duplicate_user_message`.
        if let Some(objective) = first_goal_objective {
            if goal_opens_session {
                messages.insert(
                    0,
                    UnifiedMessage {
                        id: "codex-goal-user".to_string(),
                        role: MessageRole::User,
                        content: vec![ContentBlock::Text {
                            text: format!("/goal {objective}"),
                        }],
                        // Earliest event time so it sorts ahead of the goal card.
                        timestamp: first_timestamp.unwrap_or_else(Utc::now),
                        usage: None,
                        duration_ms: None,
                        model: None,
                        completed_at: first_timestamp,
                    },
                );
            }
        }

        let folder_path = cwd.clone();
        let folder_name = folder_path.as_ref().map(|p| folder_name_from_path(p));

        fold_shell_session_polls(&mut messages, &poll_origins);
        let mut turns = group_into_turns(messages);
        reconcile_turn_usage(&mut turns, &recorded_round_usage);
        super::relocate_orphaned_tool_results(&mut turns);
        super::structurize_read_tool_output(&mut turns);
        super::resolve_patch_line_numbers(&mut turns, cwd.as_deref());
        // After relocation every turn's `completed_at` is final — tile the
        // timeline into per-reply durations before stats aggregate them.
        let turn_starts = if task_start_markers.is_empty() {
            &turn_context_markers
        } else {
            &task_start_markers
        };
        super::backfill_turn_durations(&mut turns, turn_starts);
        let mut session_stats = super::compute_session_stats(&turns);
        session_stats =
            merge_codex_total_usage_stats(session_stats, latest_total_usage, latest_total_tokens);
        session_stats = merge_codex_context_window_stats(
            session_stats,
            context_window_used_tokens,
            context_window_max_tokens,
        );

        let summary = ConversationSummary {
            id: conversation_id.to_string(),
            agent_type: AgentType::Codex,
            folder_path,
            folder_name,
            title,
            started_at: first_timestamp.unwrap_or_else(Utc::now),
            ended_at: last_timestamp,
            message_count: turns.len() as u32,
            model,
            git_branch,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        };

        Ok(ConversationDetail {
            summary,
            turns,
            session_stats,
            transcript_watermark: None,
        })
    }
}

fn extract_total_tokens_from_usage(usage: &serde_json::Value) -> Option<u64> {
    if let Some(total_tokens) = usage.get("total_tokens").and_then(|v| v.as_u64()) {
        return Some(total_tokens);
    }

    let input_tokens = usage
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cached_input_tokens = usage
        .get("cached_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reasoning_output_tokens = usage
        .get("reasoning_output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // Codex payloads use `input_tokens` as the full input (cache read included),
    // so fallback totals should not double-count cached tokens.
    let total = if cached_input_tokens <= input_tokens {
        input_tokens + output_tokens + reasoning_output_tokens
    } else {
        input_tokens + cached_input_tokens + output_tokens + reasoning_output_tokens
    };
    if total > 0 {
        Some(total)
    } else {
        None
    }
}

fn extract_turn_usage_from_codex_usage(usage: &serde_json::Value) -> Option<TurnUsage> {
    let input_tokens = usage
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read_input_tokens = usage
        .get("cached_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if input_tokens == 0 && output_tokens == 0 && cache_read_input_tokens == 0 {
        return None;
    }

    Some(TurnUsage {
        input_tokens: input_tokens.saturating_sub(cache_read_input_tokens),
        output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens,
    })
}

/// A codex usage payload read as raw counters, keeping zeros.
///
/// [`extract_turn_usage_from_codex_usage`] answers "is there anything to show",
/// so it collapses an all-zero payload to `None`. The running-total arithmetic
/// below needs the opposite: a cumulative counter that legitimately still reads
/// zero is a real datapoint, not an absent one.
fn codex_usage_counters(usage: &serde_json::Value) -> TurnUsage {
    let field = |name: &str| usage.get(name).and_then(|v| v.as_u64()).unwrap_or(0);
    let cache_read = field("cached_input_tokens");
    TurnUsage {
        // Codex reports `input_tokens` *inclusive* of the cached prefix, so the
        // cached part is split out rather than counted twice.
        input_tokens: field("input_tokens").saturating_sub(cache_read),
        output_tokens: field("output_tokens"),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read,
    }
}

fn codex_usage_is_zero(usage: &TurnUsage) -> bool {
    usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_creation_input_tokens == 0
        && usage.cache_read_input_tokens == 0
}

fn codex_usage_add(base: &TurnUsage, extra: &TurnUsage) -> TurnUsage {
    TurnUsage {
        input_tokens: base.input_tokens.saturating_add(extra.input_tokens),
        output_tokens: base.output_tokens.saturating_add(extra.output_tokens),
        cache_creation_input_tokens: base
            .cache_creation_input_tokens
            .saturating_add(extra.cache_creation_input_tokens),
        cache_read_input_tokens: base
            .cache_read_input_tokens
            .saturating_add(extra.cache_read_input_tokens),
    }
}

/// What one model round-trip added, as the rise in the session's cumulative
/// counter.
///
/// Codex emits a `token_count` event after **every** model call — including the
/// tool-calling rounds inside a turn — and each one restates
/// `total_token_usage` for the whole session. Differencing that counter is what
/// makes the accounting exact: it cannot double-count a `token_count` that is
/// emitted twice for the same call (7 380 of 31 846 events in a real session
/// tree are such repeats), and it cannot lose a round whose event went
/// unrecorded, because the next event's total absorbs it.
///
/// A total that moves *backwards* means the counter restarted (a fresh context
/// after compaction), so the new value is the round's own spend.
fn codex_round_usage(previous_total: Option<&TurnUsage>, total: &TurnUsage) -> TurnUsage {
    let grand = |u: &TurnUsage| {
        u.input_tokens
            .saturating_add(u.output_tokens)
            .saturating_add(u.cache_creation_input_tokens)
            .saturating_add(u.cache_read_input_tokens)
    };
    // No baseline, or the counter restarted below it: the whole figure is this
    // round's own spend. Differencing per field instead would clamp every field
    // to zero and silently drop the rest of the session.
    let Some(previous) = previous_total.filter(|prev| grand(prev) <= grand(total)) else {
        return total.clone();
    };
    TurnUsage {
        input_tokens: total.input_tokens.saturating_sub(previous.input_tokens),
        output_tokens: total.output_tokens.saturating_sub(previous.output_tokens),
        cache_creation_input_tokens: total
            .cache_creation_input_tokens
            .saturating_sub(previous.cache_creation_input_tokens),
        cache_read_input_tokens: total
            .cache_read_input_tokens
            .saturating_sub(previous.cache_read_input_tokens),
    }
}

/// Make the turns account for every token the transcript reported.
///
/// Round-trip usage is attached to the assistant message that was current when
/// the `token_count` arrived, which is right whenever that message survives —
/// but presentation is allowed to drop or fold messages (a tool call absorbed
/// into a capsule, a duplicate agent message collapsed away), and a message
/// that disappears takes its usage with it. Across a real session tree that
/// silently lost 7 % of Codex spend, concentrated in exactly the sessions that
/// worked hardest.
///
/// So the recorded rounds are also summed independently, and any shortfall is
/// put back on the last turn that can hold it. The invariant this establishes
/// is worth stating plainly: **the per-turn usage of a Codex session always
/// sums to what its own counter reported**, whatever the renderer did to the
/// turns in between.
///
/// A surplus is left alone. It would mean the turns claim more than the
/// transcript ever reported, which no path here can produce, and inventing a
/// correction for an impossible state would only hide the bug that caused it.
fn reconcile_turn_usage(turns: &mut [MessageTurn], recorded: &TurnUsage) {
    if codex_usage_is_zero(recorded) {
        return;
    }
    let attributed = turns
        .iter()
        .filter_map(|t| t.usage.as_ref())
        .fold(TurnUsage::default(), |acc, u| codex_usage_add(&acc, u));

    let missing = TurnUsage {
        input_tokens: recorded.input_tokens.saturating_sub(attributed.input_tokens),
        output_tokens: recorded
            .output_tokens
            .saturating_sub(attributed.output_tokens),
        cache_creation_input_tokens: recorded
            .cache_creation_input_tokens
            .saturating_sub(attributed.cache_creation_input_tokens),
        cache_read_input_tokens: recorded
            .cache_read_input_tokens
            .saturating_sub(attributed.cache_read_input_tokens),
    };
    if codex_usage_is_zero(&missing) {
        return;
    }

    // Prefer a turn that already reports usage — it is one the transcript
    // itself tied to a model call, so the recovered tokens land beside spend
    // that really happened rather than on an unrelated bubble.
    let target = turns
        .iter()
        .rposition(|t| t.usage.is_some())
        .or_else(|| turns.iter().rposition(|t| matches!(t.role, TurnRole::Assistant)));
    if let Some(turn) = target.and_then(|i| turns.get_mut(i)) {
        turn.usage = Some(match turn.usage {
            Some(ref existing) => codex_usage_add(existing, &missing),
            None => missing,
        });
    }
}

fn extract_context_window_used_tokens_from_token_count_info(
    info: &serde_json::Value,
) -> Option<u64> {
    // `last_token_usage` is the current turn usage and best matches context window occupancy.
    if let Some(last_usage) = info.get("last_token_usage") {
        if let Some(total) = extract_total_tokens_from_usage(last_usage) {
            return Some(total);
        }
    }

    // Fallback: some payloads may only have cumulative totals.
    info.get("total_token_usage")
        .and_then(extract_total_tokens_from_usage)
}

fn merge_codex_context_window_stats(
    stats: Option<SessionStats>,
    used_tokens: Option<u64>,
    max_tokens: Option<u64>,
) -> Option<SessionStats> {
    if used_tokens.is_none() && max_tokens.is_none() {
        return stats;
    }

    let usage_percent = match (used_tokens, max_tokens) {
        (Some(used), Some(max)) if max > 0 => Some((used as f64 / max as f64) * 100.0),
        _ => None,
    };

    match stats {
        Some(mut s) => {
            s.context_window_used_tokens = used_tokens;
            s.context_window_max_tokens = max_tokens;
            s.context_window_usage_percent = usage_percent;
            Some(s)
        }
        None => Some(SessionStats {
            total_usage: None,
            total_tokens: None,
            total_duration_ms: 0,
            context_window_used_tokens: used_tokens,
            context_window_max_tokens: max_tokens,
            context_window_usage_percent: usage_percent,
        }),
    }
}

fn merge_codex_total_usage_stats(
    stats: Option<SessionStats>,
    total_usage: Option<TurnUsage>,
    total_tokens: Option<u64>,
) -> Option<SessionStats> {
    match stats {
        Some(mut s) => {
            if let Some(total) = total_usage {
                s.total_usage = Some(total);
            }
            if total_tokens.is_some() {
                s.total_tokens = total_tokens;
            }
            Some(s)
        }
        None if total_usage.is_some() || total_tokens.is_some() => Some(SessionStats {
            total_usage,
            total_tokens,
            total_duration_ms: 0,
            context_window_used_tokens: None,
            context_window_max_tokens: None,
            context_window_usage_percent: None,
        }),
        None => None,
    }
}

fn parse_codex_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
}

/// Append a start-of-turn marker to one of the two marker lists (see their
/// declaration for why `task_started` and `turn_context` are kept apart).
///
/// An out-of-order arrival (skewed clocks in a rollout) is dropped rather than
/// inserted: the backfill scans the list once, in order.
fn push_turn_start(turn_starts: &mut Vec<DateTime<Utc>>, ts: DateTime<Utc>) {
    match turn_starts.last() {
        Some(last) if ts <= *last => {}
        _ => turn_starts.push(ts),
    }
}

/// Emit any buffered streaming `agent_reasoning` sections as a single Thinking
/// message and clear the buffer. No-op when the buffer is empty. Used only as a
/// fallback when the grouped `response_item.reasoning.summary` (which normally
/// supersedes and clears the buffer) is absent — e.g. an interrupted rollout —
/// so streaming reasoning is preserved as one 思考 card instead of being lost.
fn flush_pending_reasoning(
    messages: &mut Vec<UnifiedMessage>,
    pending: &mut Vec<String>,
    ts: Option<DateTime<Utc>>,
) {
    if pending.is_empty() {
        return;
    }
    let text = pending.join("\n\n");
    pending.clear();
    let timestamp = ts.unwrap_or_else(Utc::now);
    messages.push(UnifiedMessage {
        id: format!("thinking-{}", messages.len()),
        role: MessageRole::Assistant,
        content: vec![ContentBlock::Thinking { text }],
        timestamp,
        usage: None,
        duration_ms: None,
        model: None,
        completed_at: Some(timestamp),
    });
}

fn agents_instructions_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?s)\A# AGENTS\.md instructions for [^\n]+\n\s*\n<INSTRUCTIONS>.*?</INSTRUCTIONS>\s*",
        )
        .expect("valid agents instructions regex")
    })
}

fn strip_agents_instructions_block(input: &str) -> String {
    let text = agents_instructions_regex().replace(input, "");
    text.trim().to_string()
}

fn is_agents_instruction_message(input: &str) -> bool {
    input
        .trim_start()
        .starts_with("# AGENTS.md instructions for ")
}

fn is_environment_context_message(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.starts_with("<environment_context>") && trimmed.ends_with("</environment_context>")
}

/// codex re-injects `<codex_internal_context source="goal">Continue working …`
/// user turns while a `/goal` is active. These are machine context, never a real
/// prompt, so they must never become a conversation title (they otherwise leak in
/// on the summary path, whose title fallback doesn't gate on image blocks).
fn is_codex_internal_context_message(input: &str) -> bool {
    input.trim_start().starts_with("<codex_internal_context")
}

fn extract_codex_title_candidate(input: &str, fallback_attached: bool) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty()
        || is_agents_instruction_message(trimmed)
        || is_environment_context_message(trimmed)
        || is_codex_internal_context_message(trimmed)
    {
        return None;
    }

    let without_agents = strip_agents_instructions_block(trimmed);
    if without_agents.is_empty()
        || is_agents_instruction_message(&without_agents)
        || is_environment_context_message(&without_agents)
        || is_codex_internal_context_message(&without_agents)
    {
        return None;
    }

    let cleaned = strip_blocked_resource_mentions(&without_agents);
    if cleaned.is_empty() {
        if fallback_attached {
            Some("Attached resources".to_string())
        } else {
            None
        }
    } else {
        Some(title_from_user_text(&cleaned))
    }
}

fn extract_codex_text_content(payload: &serde_json::Value) -> Option<String> {
    let content = payload.get("content")?;
    if let Some(arr) = content.as_array() {
        for item in arr {
            let t = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if t == "input_text" {
                return item
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(|t| t.to_string());
            }
        }
    }
    None
}

fn parse_data_uri_image(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim();
    if !trimmed.starts_with("data:") {
        return None;
    }
    let marker = ";base64,";
    let marker_idx = trimmed.find(marker)?;
    let mime_type = trimmed.get(5..marker_idx)?.trim();
    if !mime_type.starts_with("image/") {
        return None;
    }
    let data = trimmed.get(marker_idx + marker.len()..)?.trim();
    if data.is_empty() {
        return None;
    }
    Some((mime_type.to_string(), data.to_string()))
}

fn parse_input_image_data_uri(item: &serde_json::Value) -> Option<(String, String)> {
    let data_uri = item
        .get("image_url")
        .and_then(|v| v.as_str())
        .or_else(|| {
            item.get("image_url")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| item.get("url").and_then(|v| v.as_str()))?;
    parse_data_uri_image(data_uri)
}

fn first_text_block(blocks: &[ContentBlock]) -> Option<String> {
    blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } => Some(text.clone()),
        _ => None,
    })
}

fn blocks_equal(a: &[ContentBlock], b: &[ContentBlock]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

fn should_skip_duplicate_user_message(
    messages: &[UnifiedMessage],
    blocks: &[ContentBlock],
    timestamp: DateTime<Utc>,
) -> bool {
    // Some Codex logs emit the same user message through both `response_item`
    // and `event_msg`, sometimes with a non-trivial delay. Deduplicate by
    // content in a bounded recent time window.
    const DUP_WINDOW_MS: i64 = 120_000;

    for msg in messages.iter().rev() {
        if !matches!(msg.role, MessageRole::User) {
            continue;
        }
        let delta_ms = (timestamp - msg.timestamp).num_milliseconds().abs();
        if delta_ms > DUP_WINDOW_MS {
            break;
        }
        if blocks_equal(&msg.content, blocks) {
            return true;
        }
    }

    false
}

/// Whether a `response_item` user message carries an `input_image` — the exact
/// condition under which [`extract_response_item_user_image_blocks`] yields a
/// real user turn in the detail parser. The lightweight summary parser uses this
/// to detect the same real-user-turn so its pure-`/goal` fallback stays in sync.
fn response_item_user_has_image(payload: &serde_json::Value) -> bool {
    payload
        .get("content")
        .and_then(|c| c.as_array())
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(|v| v.as_str()) == Some("input_image")
            })
        })
}

fn extract_response_item_user_image_blocks(
    payload: &serde_json::Value,
) -> Option<Vec<ContentBlock>> {
    let content = payload.get("content")?.as_array()?;
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let mut text_parts: Vec<String> = Vec::new();
    let mut has_input_image = false;

    for item in content {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match item_type {
            "input_text" => {
                let Some(text) = item.get("text").and_then(|v| v.as_str()) else {
                    continue;
                };
                if text.trim() == "<image>" {
                    continue;
                }
                if !text.is_empty() {
                    text_parts.push(text.to_string());
                }
            }
            "input_image" => {
                has_input_image = true;
                let Some((mime_type, data)) = parse_input_image_data_uri(item) else {
                    continue;
                };
                blocks.push(ContentBlock::Image {
                    data,
                    mime_type,
                    uri: None,
                });
            }
            _ => {}
        }
    }

    if !has_input_image {
        return None;
    }

    let text = strip_blocked_resource_mentions(&text_parts.join("\n"));
    if !text.is_empty() {
        blocks.insert(0, ContentBlock::Text { text });
    }

    if blocks.is_empty() {
        blocks.push(ContentBlock::Text {
            text: "Attached resources".to_string(),
        });
    }

    Some(blocks)
}

fn strip_blocked_resource_mentions(input: &str) -> String {
    let blocked_re = Regex::new(r"@([^\s@]+)\s*\[blocked[^\]]*\]").expect("valid blocked regex");
    let image_tag_re = Regex::new(r"(?i)</?image\s*/?>").expect("valid image tag regex");
    let collapsed_ws_re = Regex::new(r"[ \t]{2,}").expect("valid whitespace regex");
    let text = blocked_re.replace_all(input, "").to_string();
    let text = image_tag_re.replace_all(&text, "").to_string();
    let text = collapsed_ws_re.replace_all(&text, " ").to_string();
    text.trim().to_string()
}

/// Group flat messages into conversation turns.
/// Codex rule: consecutive Assistant + Tool messages merge into one Assistant turn.
fn group_into_turns(messages: Vec<UnifiedMessage>) -> Vec<MessageTurn> {
    let mut turns = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        let msg = &messages[i];

        if matches!(msg.role, MessageRole::User) {
            turns.push(MessageTurn {
                id: format!("turn-{}", turns.len()),
                role: TurnRole::User,
                blocks: msg.content.clone(),
                timestamp: msg.timestamp,
                usage: None,
                duration_ms: None,
                model: None,
                completed_at: msg.completed_at,
            });
            i += 1;
        } else if matches!(msg.role, MessageRole::System) {
            turns.push(MessageTurn {
                id: format!("turn-{}", turns.len()),
                role: TurnRole::System,
                blocks: msg.content.clone(),
                timestamp: msg.timestamp,
                usage: None,
                duration_ms: None,
                model: None,
                completed_at: msg.completed_at,
            });
            i += 1;
        } else {
            // Assistant or Tool — start a group
            let mut blocks: Vec<ContentBlock> = msg.content.clone();
            let mut usage = msg.usage.clone();
            let mut duration_ms = msg.duration_ms;
            let mut turn_model = msg.model.clone();
            let timestamp = msg.timestamp;
            let mut completed_at = msg.completed_at;
            i += 1;

            // Only absorb immediately following Tool messages
            // (stop at the next assistant message to keep turns small for virtualization)
            while i < messages.len() && matches!(messages[i].role, MessageRole::Tool) {
                blocks.extend(messages[i].content.clone());
                if usage.is_none() {
                    usage = messages[i].usage.clone();
                }
                if duration_ms.is_none() {
                    duration_ms = messages[i].duration_ms;
                }
                if turn_model.is_none() {
                    turn_model = messages[i].model.clone();
                }
                if messages[i].completed_at.is_some() {
                    completed_at = messages[i].completed_at;
                }
                i += 1;
            }

            turns.push(MessageTurn {
                id: format!("turn-{}", turns.len()),
                role: TurnRole::Assistant,
                blocks,
                timestamp,
                usage,
                duration_ms,
                model: turn_model,
                completed_at,
            });
        }
    }

    turns
}

#[cfg(test)]
mod tests {

    use std::collections::HashMap;

    use super::extract_codex_title_candidate;
    use super::extract_context_window_used_tokens_from_token_count_info;
    use super::extract_response_item_user_image_blocks;
    use super::extract_turn_usage_from_codex_usage;
    use super::is_encrypted_envelope;
    use super::merge_codex_context_window_stats;
    use super::native_team_wait_input;
    use super::merge_codex_total_usage_stats;
    use super::parse_codex_subagent_stats;
    use super::redact_encrypted_args;
    use super::resolve_codex_home_dir_from;
    use super::CODEX_SUBAGENT_LAUNCH_KEY;
    use super::COLLAB_OP_KEY;
    use super::should_skip_duplicate_user_message;
    use super::strip_blocked_resource_mentions;
    use super::AgentParser;
    use super::CodexParser;
    use super::CODEX_SCRIPT_TOOL_NAME;
    use crate::models::{
        ContentBlock, MessageRole, MessageTurn, SessionStats, TurnRole, TurnUsage, UnifiedMessage,
    };
    use chrono::{DateTime, Duration, Utc};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_index_title_fixture(
        conversation_id: &str,
    ) -> (tempfile::TempDir, CodexParser, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let codex_home = temp_dir.path().join(".codex");
        let sessions_dir = codex_home.join("sessions");
        let rollout_dir = sessions_dir.join("2026").join("08").join("15");
        fs::create_dir_all(&rollout_dir).expect("create rollout dir");

        let rollout_path = rollout_dir.join(format!(
            "rollout-2026-08-15T16-00-00-{conversation_id}.jsonl"
        ));
        let lines = [
            serde_json::json!({
                "timestamp": "2026-08-15T08:00:00Z",
                "type": "session_meta",
                "payload": {"id": conversation_id, "cwd": "/tmp/Temp"}
            })
            .to_string(),
            serde_json::json!({
                "timestamp": "2026-08-15T08:00:01Z",
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "Makefile 文件的作用"}
            })
            .to_string(),
        ];
        fs::write(&rollout_path, format!("{}\n", lines.join("\n"))).expect("write rollout");

        let index_path = codex_home.join("session_index.jsonl");
        let parser = CodexParser::with_base_dir(sessions_dir);
        (temp_dir, parser, index_path)
    }

    #[test]
    fn session_index_title_wins_for_list_and_detail() {
        let conversation_id = "01a00496-1418-7273-a06f-dc4fae5cfa64";
        let (_temp_dir, parser, index_path) = write_index_title_fixture(conversation_id);
        let index_lines = [
            serde_json::json!({
                "id": conversation_id,
                "thread_name": "旧标题",
                "updated_at": "2026-08-15T08:01:00Z"
            })
            .to_string(),
            "{malformed json".to_string(),
            serde_json::json!({
                "id": conversation_id,
                "thread_name": "  解释 Makefile 文件作用  ",
                "updated_at": "2026-08-15T08:02:00Z"
            })
            .to_string(),
            serde_json::json!({
                "id": conversation_id,
                "thread_name": "   ",
                "updated_at": "2026-08-15T08:03:00Z"
            })
            .to_string(),
        ];
        fs::write(&index_path, format!("{}\n", index_lines.join("\n"))).expect("write index");

        let summaries = parser.list_conversations().expect("list conversations");
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].title.as_deref(),
            Some("解释 Makefile 文件作用")
        );

        let detail = parser
            .get_conversation(conversation_id)
            .expect("get conversation");
        assert_eq!(
            detail.summary.title.as_deref(),
            Some("解释 Makefile 文件作用")
        );
    }

    #[test]
    fn session_index_title_wins_over_rollout_thread_name_update() {
        let conversation_id = "index-vs-rollout-title";
        let (_temp_dir, parser, index_path) = write_index_title_fixture(conversation_id);
        let rollout_path = parser
            .base_dir
            .join("2026")
            .join("08")
            .join("15")
            .join(format!(
                "rollout-2026-08-15T16-00-00-{conversation_id}.jsonl"
            ));
        let mut rollout = fs::read_to_string(&rollout_path).expect("read rollout");
        rollout.push_str(
            &serde_json::json!({
                "timestamp": "2026-08-15T08:00:02Z",
                "type": "event_msg",
                "payload": {
                    "type": "thread_name_updated",
                    "thread_name": "rollout thread title"
                }
            })
            .to_string(),
        );
        rollout.push('\n');
        fs::write(&rollout_path, rollout).expect("append rollout title");
        fs::write(
            &index_path,
            format!(
                "{}\n",
                serde_json::json!({
                    "id": conversation_id,
                    "thread_name": "session index title",
                    "updated_at": "2026-08-15T08:01:00Z"
                })
            ),
        )
        .expect("write index title");

        let summaries = parser.list_conversations().expect("list conversations");
        assert_eq!(summaries[0].title.as_deref(), Some("session index title"));
        let detail = parser
            .get_conversation(conversation_id)
            .expect("get conversation");
        assert_eq!(detail.summary.title.as_deref(), Some("session index title"));
    }

    #[test]
    fn session_index_title_refreshes_after_summary_cache_hit() {
        let conversation_id = "cache-title-session";
        let (_temp_dir, parser, index_path) = write_index_title_fixture(conversation_id);
        fs::write(
            &index_path,
            format!(
                "{}\n",
                serde_json::json!({"id": conversation_id, "thread_name": "索引标题甲"})
            ),
        )
        .expect("write first index title");

        let first = parser.list_conversations().expect("first list");
        assert_eq!(first[0].title.as_deref(), Some("索引标题甲"));

        // The rollout file is unchanged, so its summary is served from cache.
        // The independently-read index must still replace the cached title.
        fs::write(
            &index_path,
            format!(
                "{}\n",
                serde_json::json!({"id": conversation_id, "thread_name": "索引标题乙"})
            ),
        )
        .expect("update index title");
        let second = parser.list_conversations().expect("second list");
        assert_eq!(second[0].title.as_deref(), Some("索引标题乙"));
    }

    #[test]
    fn unavailable_session_index_preserves_rollout_title_fallback() {
        let conversation_id = "index-fallback-session";
        let (_temp_dir, parser, index_path) = write_index_title_fixture(conversation_id);

        let missing = parser.list_conversations().expect("list without index");
        assert_eq!(missing[0].title.as_deref(), Some("Makefile 文件的作用"));

        fs::write(
            &index_path,
            format!(
                "not json\n{}\n",
                serde_json::json!({"id": conversation_id, "thread_name": "  "})
            ),
        )
        .expect("write unusable index records");
        let malformed = parser.list_conversations().expect("list malformed index");
        assert_eq!(malformed[0].title.as_deref(), Some("Makefile 文件的作用"));

        fs::remove_file(&index_path).expect("remove index file");
        fs::create_dir(&index_path).expect("make index path unreadable as a file");
        let unreadable = parser.list_conversations().expect("list unreadable index");
        assert_eq!(unreadable[0].title.as_deref(), Some("Makefile 文件的作用"));
        let detail = parser
            .get_conversation(conversation_id)
            .expect("detail unreadable index");
        assert_eq!(detail.summary.title.as_deref(), Some("Makefile 文件的作用"));
    }

    #[test]
    fn skips_agents_instructions_title_candidate() {
        let input =
            "# AGENTS.md instructions for /tmp/demo\n\n<INSTRUCTIONS>\nhello\n</INSTRUCTIONS>";
        let got = extract_codex_title_candidate(input, true);
        assert!(got.is_none());
    }

    #[test]
    fn skips_environment_context_title_candidate() {
        let input = "<environment_context>\n  <cwd>/tmp/demo</cwd>\n</environment_context>";
        let got = extract_codex_title_candidate(input, true);
        assert!(got.is_none());
    }

    #[test]
    fn keeps_real_user_prompt_as_title_candidate() {
        let input = "修复 codex 会话标题";
        let got = extract_codex_title_candidate(input, true);
        assert_eq!(got.as_deref(), Some("修复 codex 会话标题"));
    }

    #[test]
    fn strips_image_placeholders_from_user_text() {
        let input = "这个图片里面是什么\n</image>\n<image>\n";
        let got = strip_blocked_resource_mentions(input);
        assert_eq!(got, "这个图片里面是什么");
    }

    #[test]
    fn extracts_response_item_input_image_blocks() {
        let payload = serde_json::json!({
            "content": [
                {"type": "input_text", "text": "这是什么东西"},
                {"type": "input_text", "text": "<image>"},
                {"type": "input_image", "image_url": "data:image/png;base64,QUJD"}
            ]
        });

        let blocks = extract_response_item_user_image_blocks(&payload).expect("blocks");
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            ContentBlock::Text { text } => assert_eq!(text, "这是什么东西"),
            _ => panic!("expected text block"),
        }
        match &blocks[1] {
            ContentBlock::Image {
                data, mime_type, ..
            } => {
                assert_eq!(mime_type, "image/png");
                assert_eq!(data, "QUJD");
            }
            _ => panic!("expected image block"),
        }
    }

    #[test]
    fn skips_duplicate_user_message_within_short_window() {
        let now = Utc::now();
        let blocks = vec![
            ContentBlock::Text {
                text: "hello".to_string(),
            },
            ContentBlock::Image {
                data: "QUJD".to_string(),
                mime_type: "image/png".to_string(),
                uri: None,
            },
        ];
        let messages = vec![UnifiedMessage {
            id: "user-0".to_string(),
            role: MessageRole::User,
            content: blocks.clone(),
            timestamp: now,
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: Some(now),
        }];

        assert!(should_skip_duplicate_user_message(
            &messages,
            &blocks,
            now + Duration::milliseconds(1200),
        ));
        assert!(!should_skip_duplicate_user_message(
            &messages,
            &blocks,
            now + Duration::seconds(180),
        ));
    }

    #[test]
    fn summary_title_skips_injected_messages_and_uses_real_prompt() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-test-{nanos}.jsonl"));

        // Injected/duplicate context arrives as text-only `response_item` user
        // messages (AGENTS.md, environment_context); the real prompt is delivered
        // by `event_msg.user_message` — the canonical prompt channel in real codex
        // rollouts. The summary titles from the real prompt and, like the detail
        // parser, never from a text-only `response_item` (which detail does not
        // render as a user turn at all).
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"test-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"# AGENTS.md instructions for /tmp/demo\\n\\n<INSTRUCTIONS>\\nhello\\n</INSTRUCTIONS>\"}]}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"<environment_context>\\n  <cwd>/tmp/demo</cwd>\\n</environment_context>\"}]}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"真实用户标题\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary ok")
            .expect("summary exists");
        assert_eq!(summary.title.as_deref(), Some("真实用户标题"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn extracts_context_window_used_tokens_from_last_usage_total() {
        let info = serde_json::json!({
            "total_token_usage": {
                "total_tokens": 1234,
                "input_tokens": 1000,
                "cached_input_tokens": 100,
                "output_tokens": 100,
                "reasoning_output_tokens": 34
            },
            "last_token_usage": {
                "total_tokens": 321,
                "input_tokens": 300,
                "cached_input_tokens": 10,
                "output_tokens": 11
            }
        });
        assert_eq!(
            extract_context_window_used_tokens_from_token_count_info(&info),
            Some(321)
        );
    }

    #[test]
    fn extracts_context_window_used_tokens_from_last_usage_sum_when_total_missing() {
        let info = serde_json::json!({
            "total_token_usage": {
                "input_tokens": 1000,
                "cached_input_tokens": 100,
                "output_tokens": 100,
                "reasoning_output_tokens": 34
            },
            "last_token_usage": {
                "input_tokens": 200,
                "cached_input_tokens": 20,
                "output_tokens": 2
            }
        });
        assert_eq!(
            extract_context_window_used_tokens_from_token_count_info(&info),
            Some(202)
        );
    }

    #[test]
    fn falls_back_to_total_usage_when_last_usage_missing() {
        let info = serde_json::json!({
            "total_token_usage": {
                "total_tokens": 1234
            }
        });
        assert_eq!(
            extract_context_window_used_tokens_from_token_count_info(&info),
            Some(1234)
        );
    }

    #[test]
    fn extracts_turn_usage_from_codex_usage_payload() {
        let usage = serde_json::json!({
            "input_tokens": 120,
            "cached_input_tokens": 80,
            "output_tokens": 16
        });
        let parsed = extract_turn_usage_from_codex_usage(&usage).expect("usage");
        assert_eq!(parsed.input_tokens, 40);
        assert_eq!(parsed.output_tokens, 16);
        assert_eq!(parsed.cache_creation_input_tokens, 0);
        assert_eq!(parsed.cache_read_input_tokens, 80);
    }

    #[test]
    fn merge_total_usage_overrides_aggregated_usage() {
        let aggregated = SessionStats {
            total_usage: Some(TurnUsage {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 3,
            }),
            total_tokens: Some(6),
            total_duration_ms: 100,
            context_window_used_tokens: None,
            context_window_max_tokens: None,
            context_window_usage_percent: None,
        };
        let total = TurnUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 20,
        };
        let merged =
            merge_codex_total_usage_stats(Some(aggregated), Some(total.clone()), Some(170))
                .expect("stats");
        assert_eq!(
            merged.total_usage.expect("usage").input_tokens,
            total.input_tokens
        );
        assert_eq!(merged.total_tokens, Some(170));
        assert_eq!(merged.total_duration_ms, 100);
    }

    #[test]
    fn merges_context_window_stats_without_turn_usage() {
        let merged = merge_codex_context_window_stats(None, Some(1200), Some(4000))
            .expect("stats should be present");
        assert!(merged.total_usage.is_none());
        assert!(merged.total_tokens.is_none());
        assert_eq!(merged.context_window_used_tokens, Some(1200));
        assert_eq!(merged.context_window_max_tokens, Some(4000));
        let pct = merged
            .context_window_usage_percent
            .expect("context window percent present");
        assert!((pct - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_detail_sets_context_window_stats_from_token_count() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-ctx-{nanos}.jsonl"));

        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"ctx-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"done\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"total_tokens\":129200,\"input_tokens\":120000,\"cached_input_tokens\":8000,\"output_tokens\":1200},\"last_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":50,\"output_tokens\":20,\"total_tokens\":170},\"model_context_window\":258400}}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "ctx-1")
            .expect("parse detail ok");

        let stats: SessionStats = detail.session_stats.expect("session stats should exist");
        assert_eq!(stats.context_window_used_tokens, Some(170));
        assert_eq!(stats.context_window_max_tokens, Some(258400));
        let total_usage = stats.total_usage.expect("total usage should exist");
        assert_eq!(total_usage.input_tokens, 112000);
        assert_eq!(total_usage.cache_read_input_tokens, 8000);
        assert_eq!(total_usage.output_tokens, 1200);
        assert_eq!(stats.total_tokens, Some(129200));
        let pct = stats
            .context_window_usage_percent
            .expect("context window percent present");
        assert!((pct - ((170.0 / 258400.0) * 100.0)).abs() < 0.0001);

        let _ = fs::remove_file(path);
    }

    /// Sum the per-turn usage a parse produced — what the usage dashboard
    /// materializes and what the session panel adds up.
    fn turn_usage_total(detail: &crate::models::ConversationDetail) -> u64 {
        detail
            .turns
            .iter()
            .filter_map(|t| t.usage.as_ref())
            .map(|u| {
                u.input_tokens + u.output_tokens + u.cache_creation_input_tokens
                    + u.cache_read_input_tokens
            })
            .sum()
    }

    fn parse_rollout(label: &str, content: &str, session_id: &str) -> crate::models::ConversationDetail {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-{label}-{nanos}.jsonl"));
        fs::write(&path, content).expect("write test jsonl");
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, session_id)
            .expect("parse detail ok");
        let _ = fs::remove_file(path);
        detail
    }

    #[test]
    fn every_model_round_trip_of_a_turn_is_counted() {
        // Codex emits a `token_count` after each model call, so one turn that
        // calls tools four times reports four times. Only the first was kept
        // (`if last_msg.usage.is_none()`), which lost most of a working turn's
        // spend — measured at 61 % of all Codex tokens in a real session tree.
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"rounds-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"working\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":20224,\"cached_input_tokens\":0,\"output_tokens\":458},\"last_token_usage\":{\"input_tokens\":20224,\"cached_input_tokens\":0,\"output_tokens\":458}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:13Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":51186,\"cached_input_tokens\":18000,\"output_tokens\":886},\"last_token_usage\":{\"input_tokens\":30962,\"cached_input_tokens\":18000,\"output_tokens\":428}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:29Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":92652,\"cached_input_tokens\":45000,\"output_tokens\":1420},\"last_token_usage\":{\"input_tokens\":41466,\"cached_input_tokens\":27000,\"output_tokens\":534}}}}\n"
        );
        let detail = parse_rollout("rounds", content, "rounds-1");

        // The session's own cumulative counter is the ground truth, and the
        // per-turn rows now reconstruct it exactly.
        assert_eq!(turn_usage_total(&detail), 92_652 + 1_420);
        let stats = detail.session_stats.expect("session stats");
        let total = stats.total_usage.expect("total usage");
        assert_eq!(
            total.input_tokens + total.output_tokens + total.cache_read_input_tokens,
            92_652 + 1_420
        );
    }

    #[test]
    fn a_restated_token_count_is_not_counted_twice() {
        // Codex sometimes reports the same call twice (7 380 of 31 846 events
        // in a real tree). Differencing the cumulative counter makes the repeat
        // contribute nothing, where summing `last_token_usage` would inflate it.
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"repeat-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"hi\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":0,\"output_tokens\":50},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":0,\"output_tokens\":50}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":0,\"output_tokens\":50},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":0,\"output_tokens\":50}}}}\n"
        );
        let detail = parse_rollout("repeat", content, "repeat-1");
        assert_eq!(turn_usage_total(&detail), 1_050);
    }

    #[test]
    fn a_round_that_ran_before_any_assistant_message_is_not_lost() {
        // When the model opens a turn by calling a tool, its first round-trips
        // finish before it ever speaks. Those had no assistant message to
        // attach to and were dropped outright — 3 % of all recorded spend.
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"early-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"go\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":800,\"cached_input_tokens\":0,\"output_tokens\":40},\"last_token_usage\":{\"input_tokens\":800,\"cached_input_tokens\":0,\"output_tokens\":40}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"done\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1900,\"cached_input_tokens\":0,\"output_tokens\":110},\"last_token_usage\":{\"input_tokens\":1100,\"cached_input_tokens\":0,\"output_tokens\":70}}}}\n"
        );
        let detail = parse_rollout("early", content, "early-1");
        assert_eq!(turn_usage_total(&detail), 1_900 + 110);
    }

    #[test]
    fn a_transcript_without_cumulative_totals_still_counts_each_call_once() {
        // Fallback path: no `total_token_usage` to difference, so a `token_count`
        // that merely restates the previous one is recognized by its payload.
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"nototal-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"hi\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":500,\"cached_input_tokens\":100,\"output_tokens\":25}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":500,\"cached_input_tokens\":100,\"output_tokens\":25}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":700,\"cached_input_tokens\":200,\"output_tokens\":30}}}}\n"
        );
        let detail = parse_rollout("nototal", content, "nototal-1");
        assert_eq!(turn_usage_total(&detail), 525 + 730);
    }

    #[test]
    fn a_transcript_that_mixes_both_shapes_counts_each_round_exactly_once() {
        // A no-total event contributes its own `last_token_usage`, so the
        // cumulative baseline has to learn about it. Otherwise the next
        // total-bearing event differences against a stale figure and bills that
        // round a second time.
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"mixed-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"hi\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":0,\"output_tokens\":0},\"last_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":0,\"output_tokens\":0}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":50,\"cached_input_tokens\":0,\"output_tokens\":0}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":180,\"cached_input_tokens\":0,\"output_tokens\":0},\"last_token_usage\":{\"input_tokens\":30,\"cached_input_tokens\":0,\"output_tokens\":0}}}}\n"
        );
        let detail = parse_rollout("mixed", content, "mixed-1");
        // 100 + 50 + (180 - 150) — the session's own counter says 180 total.
        assert_eq!(turn_usage_total(&detail), 180);
    }

    #[test]
    fn a_rollout_with_no_assistant_message_still_reports_its_spend() {
        // A turn that only ran tools and was interrupted leaves `token_count`
        // events with no assistant turn to carry them, so `reconcile_turn_usage`
        // has no target. The tokens are not lost: the session-level total is
        // populated, and that is the fallback `facts_from_detail` records as a
        // single fact row (covered by `a_session_level_total_is_recorded_when_
        // no_turn_reports_usage` in commands::token_usage).
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"toolonly-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"go\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":900,\"cached_input_tokens\":100,\"output_tokens\":40},\"last_token_usage\":{\"input_tokens\":900,\"cached_input_tokens\":100,\"output_tokens\":40}}}}\n"
        );
        let detail = parse_rollout("toolonly", content, "toolonly-1");
        assert!(
            !detail.turns.iter().any(|t| matches!(t.role, TurnRole::Assistant)),
            "precondition: this rollout has no assistant turn"
        );
        let total = detail
            .session_stats
            .as_ref()
            .and_then(|s| s.total_usage.as_ref())
            .expect("session-level total must survive as the fallback fact source");
        assert_eq!(
            total.input_tokens + total.output_tokens + total.cache_read_input_tokens,
            940
        );
    }

    #[test]
    fn a_counter_that_restarts_after_compaction_does_not_wrap_to_zero() {
        // A cumulative counter that moves backwards means a fresh context, so
        // the new value is that round's own spend rather than a negative delta
        // silently clamped away.
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"compact-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"hi\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":90000,\"cached_input_tokens\":0,\"output_tokens\":2000}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":5000,\"cached_input_tokens\":0,\"output_tokens\":100}}}}\n"
        );
        let detail = parse_rollout("compact", content, "compact-1");
        assert_eq!(turn_usage_total(&detail), 92_000 + 5_100);
    }

    #[test]
    fn parse_detail_durations_partition_a_multi_message_turn() {
        // Regression: durations came from `turn_context → token_count`, and
        // codex fires `token_count` once per model request — so every reply in
        // a turn restated the elapsed time SINCE THE PROMPT. The UI merges a
        // turn's replies into one card by summing their durations, so a 30s
        // turn reported 10+22+30 = 62s (on a real 4-prompt rollout: 26 minutes
        // of work shown as 109). Each reply must instead carry only its own
        // slice, and the slices must add up to the turn.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-spans-{nanos}.jsonl"));

        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"spans-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:00.100Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:00.200Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:00.300Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"go\"}}\n",
            // Reply 1 at +10s, then two more model requests inside the SAME turn.
            "{\"timestamp\":\"2026-03-01T10:00:10.300Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"one\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:11.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":10,\"cached_input_tokens\":0,\"output_tokens\":2,\"total_tokens\":12}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:22.300Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"two\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:23.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":10,\"cached_input_tokens\":0,\"output_tokens\":2,\"total_tokens\":12}}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:30.300Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"three\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:30.400Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":10,\"cached_input_tokens\":0,\"output_tokens\":2,\"total_tokens\":12}}}}\n",
            // A second prompt arrives 10 minutes later; that idle gap belongs
            // to nobody, so its reply must report 4s and not 10m4s.
            "{\"timestamp\":\"2026-03-01T10:10:30.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:10:30.100Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:10:30.300Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"again\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:10:34.300Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"four\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "spans-1")
            .expect("parse detail ok");

        let assistant_durations: Vec<Option<u64>> = detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::Assistant))
            .map(|t| t.duration_ms)
            .collect();
        assert_eq!(
            assistant_durations,
            vec![
                Some(10_000), // prompt → "one"
                Some(12_000), // "one" → "two"
                Some(8_000),  // "two" → "three"
                Some(4_000),  // second prompt → "four"
            ]
        );

        // Turn 1's replies sum to its wall clock, and the session total is the
        // two turns' work — never the idle stretch between them.
        let turn_one: u64 = assistant_durations[..3].iter().flatten().sum();
        assert_eq!(turn_one, 30_000);
        let stats = detail.session_stats.expect("session stats");
        assert_eq!(stats.total_duration_ms, 34_000);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_mid_turn_turn_context_does_not_truncate_the_reply_after_it() {
        // Newer codex re-emits `turn_context` in the MIDDLE of a turn — it
        // carries a `turn_id` and is rewritten when the turn's config changes
        // (6 of 495 records across the local rollout corpus). A start marker
        // only ever moves the boundary forward, so treating that one as a turn
        // start would charge the following reply just the time since it and
        // silently drop the rest of the span from the turn's total. Only
        // `task_started` — exactly one per turn — anchors the measurement;
        // `turn_context` is a fallback for rollouts that predate it.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-midctx-{nanos}.jsonl"));

        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"midctx-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:00.100Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:00.200Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:00.300Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"go\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:10.300Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"one\"}}\n",
            // Re-emitted mid-turn, 1s before the next reply. If this counted as
            // a turn start, "two" would report 1s instead of its real 20s.
            "{\"timestamp\":\"2026-03-01T10:00:29.300Z\",\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"t-1\",\"model\":\"gpt-5-codex\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:30.300Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"two\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "midctx-1")
            .expect("parse detail ok");

        let assistant_durations: Vec<Option<u64>> = detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::Assistant))
            .map(|t| t.duration_ms)
            .collect();
        assert_eq!(assistant_durations, vec![Some(10_000), Some(20_000)]);
        // Still tiles: 10s + 20s is the whole prompt→last-reply span.
        let total: u64 = assistant_durations.iter().flatten().sum();
        assert_eq!(total, 30_000);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_detail_completion_time_uses_agent_message_timestamp_not_added_turn_span() {
        // Regression: `duration_ms` is a *span* and `timestamp` on the
        // assistant `UnifiedMessage` is the agent_message event time (already
        // at the end of that span, not its start), so adding the two
        // double-counts. completed_at must reflect the agent_message arrival.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-completed-{nanos}.jsonl"));

        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"completed-1\",\"cwd\":\"/tmp/demo\"}}\n",
            // Turn starts here.
            "{\"timestamp\":\"2026-03-01T10:00:00.522Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            // Assistant message arrives ~9.5s into the turn.
            "{\"timestamp\":\"2026-03-01T10:00:10.081Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"done\"}}\n",
            // token_count fires shortly after, bringing duration_ms = 9.7s.
            "{\"timestamp\":\"2026-03-01T10:00:10.268Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"total_tokens\":100,\"input_tokens\":80,\"cached_input_tokens\":0,\"output_tokens\":20},\"last_token_usage\":{\"input_tokens\":80,\"cached_input_tokens\":0,\"output_tokens\":20,\"total_tokens\":100},\"model_context_window\":258400}}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "completed-1")
            .expect("parse detail ok");

        let assistant = detail
            .turns
            .iter()
            .find(|t| matches!(t.role, TurnRole::Assistant))
            .expect("assistant turn");
        let completed_at = assistant.completed_at.expect("completed_at populated");
        let expected = "2026-03-01T10:00:10.081Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(completed_at, expected);
        // The naive `timestamp + duration_ms` would produce ~10.00:19.827Z.
        let wrong = "2026-03-01T10:00:19.827Z".parse::<DateTime<Utc>>().unwrap();
        assert_ne!(completed_at, wrong);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_detail_reconstructs_goal_cards_and_native_title() {
        // codex-acp v1.1.0 (#263): `/goal` transitions persist to the rollout as
        // `event_msg.thread_goal_updated`. The parser must reconstruct the same
        // create_goal/update_goal tool call the live path emits (history goal
        // parity — which never existed before), normalizing the camelCase
        // `ThreadGoalStatus` to the snake_case bucket the goal card expects.
        // `thread_name_updated` must win over the first-prompt title.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-goal-{nanos}.jsonl"));

        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"goal-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"hi\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Refactor the auth module\",\"status\":\"active\",\"tokensUsed\":0,\"timeUsedSeconds\":0}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"working\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Refactor the auth module\",\"status\":\"budgetLimited\",\"tokensUsed\":5200,\"timeUsedSeconds\":19}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:06Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_name_updated\",\"thread_id\":\"goal-1\",\"thread_name\":\"Refactor auth module\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "goal-1")
            .expect("parse detail ok");

        // Native thread name wins over the first-prompt fallback ("hi").
        assert_eq!(
            detail.summary.title.as_deref(),
            Some("Refactor auth module")
        );

        // Correlate synthesized goal tool_use/tool_result blocks by id.
        let mut names: HashMap<String, String> = HashMap::new();
        let mut inputs: HashMap<String, String> = HashMap::new();
        let mut outputs: HashMap<String, String> = HashMap::new();
        for turn in &detail.turns {
            for block in &turn.blocks {
                match block {
                    ContentBlock::ToolUse {
                        tool_use_id: Some(id),
                        tool_name,
                        input_preview,
                        ..
                    } => {
                        names.insert(id.clone(), tool_name.clone());
                        if let Some(i) = input_preview {
                            inputs.insert(id.clone(), i.clone());
                        }
                    }
                    ContentBlock::ToolResult {
                        tool_use_id: Some(id),
                        output_preview: Some(o),
                        ..
                    } => {
                        outputs.insert(id.clone(), o.clone());
                    }
                    _ => {}
                }
            }
        }

        let find = |target: &str| -> String {
            names
                .iter()
                .find(|(_, n)| n.as_str() == target)
                .map(|(id, _)| id.clone())
                .unwrap_or_else(|| panic!("{target} tool call present"))
        };

        // active → create_goal, objective + status carried in the tool_result.
        let create_id = find("create_goal");
        let create_out: serde_json::Value =
            serde_json::from_str(&outputs[&create_id]).unwrap();
        assert_eq!(create_out["goal"]["status"], "active");
        assert_eq!(create_out["goal"]["objective"], "Refactor the auth module");
        // Distinct goal events get distinct (occurrence-addressed) ids.
        assert_ne!(create_id, find("update_goal"));

        // budgetLimited → update_goal with the status normalized to snake_case.
        let update_id = find("update_goal");
        let update_out: serde_json::Value =
            serde_json::from_str(&outputs[&update_id]).unwrap();
        assert_eq!(update_out["goal"]["status"], "budget_limited");
        assert_eq!(update_out["goal"]["tokensUsed"], 5200);
        let update_in: serde_json::Value = serde_json::from_str(&inputs[&update_id]).unwrap();
        assert_eq!(update_in["status"], "budget_limited");
        assert_eq!(update_in["objective"], "Refactor the auth module");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_detail_closes_goal_on_persisted_null_clear() {
        // A persisted `thread_goal_updated` with `goal: null` must close the open
        // run (create_goal → update_goal/complete inheriting the objective),
        // identical to the live path — not leave it perpetually active.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-goalnull-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"gc-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Ship it\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":null}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "gc-1")
            .expect("parse detail ok");

        let mut names: HashMap<String, String> = HashMap::new();
        let mut outputs: HashMap<String, String> = HashMap::new();
        for turn in &detail.turns {
            for block in &turn.blocks {
                match block {
                    ContentBlock::ToolUse {
                        tool_use_id: Some(id),
                        tool_name,
                        ..
                    } => {
                        names.insert(id.clone(), tool_name.clone());
                    }
                    ContentBlock::ToolResult {
                        tool_use_id: Some(id),
                        output_preview: Some(o),
                        ..
                    } => {
                        outputs.insert(id.clone(), o.clone());
                    }
                    _ => {}
                }
            }
        }
        // The null clear produced a closing update_goal (not dropped).
        let close_id = names
            .iter()
            .find(|(_, n)| n.as_str() == "update_goal")
            .map(|(id, _)| id.clone())
            .expect("null clear closes the run with an update_goal");
        assert!(names.values().any(|n| n == "create_goal"));
        let close_out: serde_json::Value = serde_json::from_str(&outputs[&close_id]).unwrap();
        assert_eq!(close_out["goal"]["status"], "complete");
        assert_eq!(close_out["goal"]["objective"], "Ship it");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_detail_synthesizes_user_message_for_pure_goal_session() {
        // Newer codex consumes `/goal <objective>` as a slash command: it persists
        // `thread_goal_updated` but NO `user_message`, so a pure-`/goal` session
        // (the user only set a goal) reloads with no user turn and no title — the
        // live view showed the typed prompt but reload lost it. The parser must
        // surface the objective as the leading user message + title so the
        // conversation isn't headless. The goal card must still render, in order.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-puregoal-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"pg-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Build a static test page\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"<environment_context>\\n  <cwd>/tmp/demo</cwd>\\n</environment_context>\"}]}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Build a static test page\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"On it.\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "pg-1")
            .expect("parse detail ok");

        // Title falls back to the goal objective (no thread_name in this session).
        assert_eq!(
            detail.summary.title.as_deref(),
            Some("Build a static test page")
        );

        // Exactly one user turn, synthesized from the objective — the internal
        // `<environment_context>` message stays filtered, and the second active
        // re-emit does NOT add a second user message. The message carries the
        // `/goal ` prefix the user actually typed (the title above stays clean).
        let user_turns: Vec<&MessageTurn> = detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::User))
            .collect();
        assert_eq!(user_turns.len(), 1, "one synthesized user turn");
        let user_text = user_turns[0].blocks.iter().find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(user_text, Some("/goal Build a static test page"));

        // The synthesized user turn precedes the goal card.
        let first_user_idx = detail
            .turns
            .iter()
            .position(|t| matches!(t.role, TurnRole::User))
            .expect("user turn present");
        let first_goal_idx = detail
            .turns
            .iter()
            .position(|t| {
                t.blocks.iter().any(|b| {
                    matches!(
                        b,
                        ContentBlock::ToolUse { tool_name, .. } if tool_name == "create_goal"
                    )
                })
            })
            .expect("goal card present");
        assert!(
            first_user_idx < first_goal_idx,
            "synthesized user message precedes the goal card"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_detail_keeps_single_user_turn_when_goal_text_persisted() {
        // Older codex persisted the `/goal` text as a real `user_message`. The
        // synthesis guard must NOT add a second user turn there (no duplicate).
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf =
            env::temp_dir().join(format!("codeg-codex-goaltext-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"gt-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"/goal Analyze the README\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Analyze the README\",\"status\":\"active\"}}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "gt-1")
            .expect("parse detail ok");

        let user_turns = detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::User))
            .count();
        assert_eq!(user_turns, 1, "real user_message not duplicated by synthesis");
        let user_text = detail
            .turns
            .iter()
            .find(|t| matches!(t.role, TurnRole::User))
            .and_then(|t| {
                t.blocks.iter().find_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            });
        assert_eq!(user_text.as_deref(), Some("/goal Analyze the README"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_detail_goal_opener_and_later_same_text_user_are_both_kept() {
        // The leading `/goal` opener is synthesized AFTER parsing, so it can never
        // poison `should_skip_duplicate_user_message`. A goal objective
        // "Investigate auth" (which OPENED the session) followed by a REAL
        // `user_message` of the same text within the dup window must yield BOTH:
        // the synthetic "/goal Investigate auth" opener AND the real "Investigate
        // auth" reply (no data loss, no false dedup — the prefix also keeps them
        // textually distinct).
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf =
            env::temp_dir().join(format!("codeg-codex-goaldup-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"gd-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Investigate auth\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Investigate auth\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "gd-1")
            .expect("parse detail ok");

        let user_texts: Vec<String> = detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::User))
            .filter_map(|t| {
                t.blocks.iter().find_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            })
            .collect();
        assert_eq!(
            user_texts,
            vec![
                "/goal Investigate auth".to_string(),
                "Investigate auth".to_string()
            ],
            "the synthetic /goal opener leads and the real reply survives"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_detail_goal_opener_then_confirm_reply_keeps_prompt_and_titles_from_goal() {
        // The real bug repro: a `/goal` opens the session (goal first, no
        // `user_message`); the agent asks for confirmation; the user replies
        // "确认" (a REAL `user_message`). Reopening must NOT start mid-conversation
        // at "确认" — the leading "/goal <objective>" prompt is restored as the
        // opener, the "确认" reply is kept in place, and the title comes from the
        // goal objective (the opening prompt), NOT from the later "确认".
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf =
            env::temp_dir().join(format!("codeg-codex-goalconfirm-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"gc-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Build a static page\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"Please confirm the plan: reply 批准 or 换主题.\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:20Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"确认\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:21Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"收到确认。\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "gc-1")
            .expect("parse detail ok");

        // Title is the goal objective, NOT the later "确认" reply.
        assert_eq!(detail.summary.title.as_deref(), Some("Build a static page"));

        // Two user turns, in order: the synthetic "/goal …" opener then "确认".
        let user_texts: Vec<String> = detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::User))
            .filter_map(|t| {
                t.blocks.iter().find_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            })
            .collect();
        assert_eq!(
            user_texts,
            vec!["/goal Build a static page".to_string(), "确认".to_string()],
            "leading /goal prompt restored ahead of the 确认 reply"
        );

        // The synthetic opener sorts ahead of the goal card.
        let first_user_idx = detail
            .turns
            .iter()
            .position(|t| matches!(t.role, TurnRole::User))
            .expect("user turn present");
        let first_goal_idx = detail
            .turns
            .iter()
            .position(|t| {
                t.blocks.iter().any(|b| {
                    matches!(
                        b,
                        ContentBlock::ToolUse { tool_name, .. } if tool_name == "create_goal"
                    )
                })
            })
            .expect("goal card present");
        assert!(first_user_idx < first_goal_idx);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_summary_goal_opener_then_confirm_reply_titles_from_goal_and_counts_opener() {
        // Summary parity with the detail repro above: a `/goal` opener followed by
        // a later "确认" reply must title the list entry from the goal objective
        // (not "确认") and count the synthetic opener turn.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf =
            env::temp_dir().join(format!("codeg-codex-sumconfirm-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sc-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Build a static page\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"Please confirm.\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:20Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"确认\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:21Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"收到确认。\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary ok")
            .expect("summary present");

        // Title from the goal objective, not the "确认" reply.
        assert_eq!(summary.title.as_deref(), Some("Build a static page"));
        // "确认" (+1) + two agent_messages (+2) + synthetic /goal opener (+1).
        assert_eq!(summary.message_count, 4);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_summary_titles_pure_goal_session_from_objective() {
        // The lightweight list parser must mirror the detail fallback: a
        // pure-`/goal` session (no `user_message`) gets its title from the goal
        // objective — set before the `<codex_internal_context>` re-injection so
        // that internal text never leaks into the title — and the synthesized
        // leading user turn is counted so the entry isn't reported empty.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf =
            env::temp_dir().join(format!("codeg-codex-sumgoal-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sg-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Build a static test page\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"<codex_internal_context source=\\\"goal\\\">\\nContinue working toward the active thread goal.\\n</codex_internal_context>\"}]}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"On it.\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary ok")
            .expect("summary present");

        // Objective wins as title; the internal-context text never leaks in.
        assert_eq!(
            summary.title.as_deref(),
            Some("Build a static test page")
        );
        // The synthesized user turn (+1) plus the agent_message (+1).
        assert_eq!(summary.message_count, 2);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_summary_prefers_native_thread_name_over_goal_objective() {
        // A native `thread_name_updated` wins over the goal-objective fallback
        // (newest non-empty), matching the detail parser.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf =
            env::temp_dir().join(format!("codeg-codex-sumname-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sn-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Build a static test page\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_name_updated\",\"thread_name\":\"Static test page\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary ok")
            .expect("summary present");

        assert_eq!(summary.title.as_deref(), Some("Static test page"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_summary_goal_opener_then_image_user_still_synthesizes_opener() {
        // A `/goal` that OPENED the session (goal first, no preceding user) followed
        // by an image-bearing `response_item` user is the positional case: the goal
        // objective is the opening prompt (its own turn) and the image is a later,
        // separate turn. The detail parser synthesizes the leading "/goal …" opener
        // AND keeps the image turn, titling from the objective. The summary must
        // match: title = objective, and it counts the synthetic opener.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf =
            env::temp_dir().join(format!("codeg-codex-sumimg-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"si-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Do the thing\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_image\",\"image_url\":\"data:image/png;base64,AAAA\"}]}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"ok\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary ok")
            .expect("summary present");

        // Summary: agent_message (+1) + synthetic /goal opener (+1); title from the
        // objective (the image-only user contributes no title).
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.title.as_deref(), Some("Do the thing"));

        // Detail parity: title = objective, the leading "/goal …" opener precedes
        // the image turn (two user turns total).
        let detail = parser
            .parse_conversation_detail(&path, "si-1")
            .expect("parse detail ok");
        assert_eq!(detail.summary.title.as_deref(), Some("Do the thing"));
        let user_turns: Vec<&MessageTurn> = detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::User))
            .collect();
        assert_eq!(user_turns.len(), 2, "synthetic /goal opener + image turn");
        let opener_text = user_turns[0].blocks.iter().find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(opener_text, Some("/goal Do the thing"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_summary_goal_null_clear_adds_no_synthetic_count() {
        // A `thread_goal_updated` with `goal: null` (or a blank objective) carries
        // no usable objective, so the detail parser never captures
        // `first_goal_objective` and never synthesizes a user. The summary must
        // likewise add no synthetic count and no title.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf =
            env::temp_dir().join(format!("codeg-codex-sumnull-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"snl-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":null}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"   \",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"hmm\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary ok")
            .expect("summary present");

        // Only the agent_message counts — no synthetic user for a null/blank goal.
        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.title, None);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn goal_with_text_only_response_item_titles_from_objective_in_both_paths() {
        // A text-only `response_item` user is not a real user turn in the detail
        // parser, so a `/goal` session carrying one still falls back to the goal
        // objective for BOTH title and the synthesized leading user. The summary
        // must match exactly — it must NOT title from the text-only response_item.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf =
            env::temp_dir().join(format!("codeg-codex-gtxt-{nanos}.jsonl"));
        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"gt2-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Do X\",\"status\":\"active\"}}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"hello\"}]}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"...\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();

        // Summary titles from the objective, not from the text-only "hello".
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary ok")
            .expect("summary present");
        assert_eq!(summary.title.as_deref(), Some("Do X"));

        // Detail matches: title = objective (clean), and the synthesized leading
        // user carries the `/goal ` prefix (the text-only response_item never
        // becomes a turn).
        let detail = parser
            .parse_conversation_detail(&path, "gt2-1")
            .expect("parse detail ok");
        assert_eq!(detail.summary.title.as_deref(), Some("Do X"));
        let user_turns: Vec<&MessageTurn> = detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::User))
            .collect();
        assert_eq!(user_turns.len(), 1, "one synthesized user turn");
        let user_text = user_turns[0].blocks.iter().find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(user_text, Some("/goal Do X"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn summary_ignores_non_opening_goal_for_synthesis() {
        // Only a `create_goal` (active) opening captures an objective for the
        // synthetic-user fallback — via the shared `goal_marker`, exactly like the
        // detail parser. A terminal-status goal alone must not synthesize a user
        // or title; a later active goal is what gets captured.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();

        // (a) terminal-only goal → no capture, no synthetic count/title.
        let path_a: PathBuf =
            env::temp_dir().join(format!("codeg-codex-term-{nanos}.jsonl"));
        fs::write(
            &path_a,
            concat!(
                "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"tm-1\",\"cwd\":\"/tmp/demo\"}}\n",
                "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Terminal only\",\"status\":\"complete\"}}}\n",
                "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"done\"}}\n"
            ),
        )
        .expect("write");
        let parser = CodexParser::new();
        let summary_a = parser
            .parse_jsonl_summary(&path_a)
            .expect("ok")
            .expect("present");
        assert_eq!(summary_a.title, None, "terminal goal is not a title");
        assert_eq!(summary_a.message_count, 1, "no synthetic user for terminal goal");
        let _ = fs::remove_file(&path_a);

        // (b) terminal THEN active → the active objective is captured (not the
        // terminal one), matching the detail parser's first-create_goal capture.
        let path_b: PathBuf =
            env::temp_dir().join(format!("codeg-codex-termact-{nanos}.jsonl"));
        fs::write(
            &path_b,
            concat!(
                "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"ta-1\",\"cwd\":\"/tmp/demo\"}}\n",
                "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Old terminal\",\"status\":\"complete\"}}}\n",
                "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_goal_updated\",\"goal\":{\"objective\":\"Fresh active\",\"status\":\"active\"}}}\n"
            ),
        )
        .expect("write");
        let summary_b = parser
            .parse_jsonl_summary(&path_b)
            .expect("ok")
            .expect("present");
        assert_eq!(summary_b.title.as_deref(), Some("Fresh active"));
        let _ = fs::remove_file(&path_b);
    }

    #[test]
    fn codex_home_env_overrides_default_home() {
        let resolved = resolve_codex_home_dir_from(
            Some(std::ffi::OsString::from("/tmp/custom-codex-home")),
            Some(PathBuf::from("/Users/default")),
        );
        assert_eq!(resolved, PathBuf::from("/tmp/custom-codex-home"));
    }

    #[test]
    fn codex_home_defaults_to_home_dot_codex() {
        let resolved = resolve_codex_home_dir_from(None, Some(PathBuf::from("/Users/default")));
        assert_eq!(resolved, PathBuf::from("/Users/default/.codex"));
    }

    /// codex 0.129+ writes a generated image both as `event_msg.image_generation_end`
    /// and `response_item.image_generation_call`, sharing the same call_id/id.
    /// The parser must surface exactly one ContentBlock::ImageGeneration per id.
    #[test]
    fn image_generation_end_and_call_dedupe_by_id() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-img-dedupe-{nanos}.jsonl"));

        let content = concat!(
            "{\"timestamp\":\"2026-05-05T12:35:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"ig-test\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:35:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"draw a cat\"}]}}\n",
            "{\"timestamp\":\"2026-05-05T12:35:17Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"image_generation_end\",\"call_id\":\"ig_abc\",\"status\":\"generating\",\"revised_prompt\":\"a fluffy ginger kitten\",\"result\":\"AAAA_BASE64\",\"saved_path\":\"/tmp/cat.png\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:35:18Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"image_generation_call\",\"id\":\"ig_abc\",\"status\":\"generating\",\"revised_prompt\":\"a fluffy ginger kitten\",\"result\":\"AAAA_BASE64\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "ig-test")
            .expect("parse ok");

        let imagegen_blocks: Vec<&ContentBlock> = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .filter(|b| matches!(b, ContentBlock::ImageGeneration { .. }))
            .collect();
        assert_eq!(
            imagegen_blocks.len(),
            1,
            "Same image must appear once across event_msg.image_generation_end + response_item.image_generation_call (got {} image-generation blocks)",
            imagegen_blocks.len()
        );
        // The first emit (event_msg.image_generation_end) wins, so saved_path
        // and revised_prompt are preserved.
        match imagegen_blocks[0] {
            ContentBlock::ImageGeneration {
                revised_prompt,
                image,
            } => {
                assert_eq!(revised_prompt.as_deref(), Some("a fluffy ginger kitten"));
                let image = image.as_ref().expect("image present on completed event");
                assert_eq!(image.data, "AAAA_BASE64");
                assert_eq!(image.mime_type, "image/png");
                assert_eq!(image.uri.as_deref(), Some("/tmp/cat.png"));
            }
            other => panic!("expected ContentBlock::ImageGeneration, got {other:?}"),
        }

        let _ = fs::remove_file(path);
    }

    /// `event_msg.image_generation_end` ought to honor an explicit `mime_type`
    /// field when codex writes one (defensive fallback to image/png otherwise).
    #[test]
    fn image_generation_end_honors_explicit_mime_type() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-img-mime-{nanos}.jsonl"));

        let content = concat!(
            "{\"timestamp\":\"2026-05-05T12:35:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"ig-mime\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:35:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"hi\"}]}}\n",
            "{\"timestamp\":\"2026-05-05T12:35:17Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"image_generation_end\",\"call_id\":\"ig_jpeg\",\"status\":\"generating\",\"mime_type\":\"image/jpeg\",\"result\":\"JPEGDATA\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "ig-mime")
            .expect("parse ok");

        let mime = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .find_map(|b| match b {
                ContentBlock::ImageGeneration {
                    image: Some(img), ..
                } if img.data == "JPEGDATA" => Some(img.mime_type.clone()),
                _ => None,
            })
            .expect("jpeg image should be present");
        assert_eq!(mime, "image/jpeg");

        let _ = fs::remove_file(path);
    }

    /// Manual smoke check against a real codex JSONL captured locally.
    /// `#[ignore]` so it doesn't run in CI; activate with
    /// `cargo test image_generation_smoke_real_session -- --ignored --nocapture`
    /// while iterating on the parser locally.
    #[test]
    #[ignore]
    fn image_generation_smoke_real_session() {
        let path = PathBuf::from(
            "/Users/xggz/.codex/sessions/2026/05/10/rollout-2026-05-10T06-13-43-019e0ecd-b954-7e33-8011-053d08baa62e.jsonl"
        );
        if !path.exists() {
            eprintln!("session not found at {}; skipping", path.display());
            return;
        }
        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "smoke")
            .expect("parse ok");

        let mut imagegen_count = 0usize;
        let mut prompt_chars = 0usize;
        let mut total_bytes = 0usize;
        for turn in &detail.turns {
            for b in &turn.blocks {
                if let ContentBlock::ImageGeneration {
                    revised_prompt,
                    image,
                } = b
                {
                    imagegen_count += 1;
                    if let Some(p) = revised_prompt {
                        prompt_chars += p.chars().count();
                    }
                    if let Some(img) = image {
                        total_bytes += img.data.len();
                    }
                }
            }
        }
        eprintln!("image_generation_blocks={imagegen_count}");
        eprintln!("revised_prompt_total_chars={prompt_chars}");
        eprintln!("total_image_base64_bytes={total_bytes}");
        assert!(
            imagegen_count >= 1,
            "expected at least 1 ContentBlock::ImageGeneration in the smoke session"
        );
    }

    /// When `revised_prompt` is absent in the payload, the parser must emit
    /// `revised_prompt: None` (codex's `imagegen` skill does not always echo
    /// the prompt back, e.g. when status="failed").
    #[test]
    fn image_generation_end_omits_revised_prompt_when_missing() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-img-noprompt-{nanos}.jsonl"));

        let content = concat!(
            "{\"timestamp\":\"2026-05-05T12:35:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"ig-noprompt\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:35:17Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"image_generation_end\",\"call_id\":\"ig_np\",\"status\":\"generating\",\"result\":\"NOPROMPT_DATA\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "ig-noprompt")
            .expect("parse ok");

        let block = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .find(|b| matches!(b, ContentBlock::ImageGeneration { .. }))
            .expect("image generation block present");
        match block {
            ContentBlock::ImageGeneration {
                revised_prompt,
                image,
            } => {
                assert!(revised_prompt.is_none());
                let image = image.as_ref().expect("image present on completed event");
                assert_eq!(image.data, "NOPROMPT_DATA");
            }
            _ => unreachable!(),
        }

        let _ = fs::remove_file(path);
    }

    /// Subagents in codex run inside the parent's JSONL, but their own
    /// transcripts are written to a separate `agent-<id>.jsonl`, so parent
    /// narration (messages / reasoning) is never gated on `active_agent_count`.
    /// image_generation is the exception: a generated image carries no agent
    /// attribution, so one emitted inside a subagent window must be suppressed
    /// (`active_agent_count > 0`), otherwise the subagent's image leaks into the
    /// parent timeline as an inline ContentBlock::ImageGeneration.
    #[test]
    fn image_generation_inside_subagent_is_suppressed_in_parent() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path: PathBuf = env::temp_dir().join(format!("codeg-codex-img-subagent-{nanos}.jsonl"));

        // Sequence:
        //   1. user msg
        //   2. parent calls spawn_agent           → active_agent_count = 1
        //   3. spawn_agent output (assigns agent_id)
        //   4. SUBAGENT generates an image (event_msg + response_item)
        //   5. parent calls close_agent
        //   6. close_agent output                  → active_agent_count = 0
        //   7. PARENT generates an image after the subagent finished
        //
        // Only step 7's image must surface; step 4 is the subagent's and
        // belongs to the subagent's own transcript, not the parent timeline.
        let content = concat!(
            "{\"timestamp\":\"2026-05-05T12:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"ig-subagent\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"go\"}]}}\n",
            "{\"timestamp\":\"2026-05-05T12:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"call_id\":\"spawn_call_1\",\"name\":\"spawn_agent\",\"arguments\":\"{\\\"agent_type\\\":\\\"researcher\\\",\\\"message\\\":\\\"do work\\\"}\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"spawn_call_1\",\"output\":\"{\\\"agent_id\\\":\\\"agent_a\\\"}\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"image_generation_end\",\"call_id\":\"ig_subagent_x\",\"status\":\"generating\",\"revised_prompt\":\"subagent painted this\",\"result\":\"SUBAGENT_BYTES\",\"saved_path\":\"/tmp/sub.png\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:00:05Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"image_generation_call\",\"id\":\"ig_subagent_y\",\"status\":\"generating\",\"revised_prompt\":\"subagent painted this too\",\"result\":\"SUBAGENT_BYTES_2\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:00:06Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"call_id\":\"close_call_1\",\"name\":\"close_agent\",\"arguments\":\"{\\\"target\\\":\\\"agent_a\\\"}\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:00:07Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"close_call_1\",\"output\":\"{}\"}}\n",
            "{\"timestamp\":\"2026-05-05T12:00:08Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"image_generation_end\",\"call_id\":\"ig_parent\",\"status\":\"generating\",\"revised_prompt\":\"parent painted this\",\"result\":\"PARENT_BYTES\",\"saved_path\":\"/tmp/parent.png\"}}\n"
        );
        fs::write(&path, content).expect("write test jsonl");

        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "ig-subagent")
            .expect("parse ok");

        let imagegen_blocks: Vec<&ContentBlock> = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .filter(|b| matches!(b, ContentBlock::ImageGeneration { .. }))
            .collect();

        assert_eq!(
            imagegen_blocks.len(),
            1,
            "only the parent's post-subagent image must surface ({} blocks)",
            imagegen_blocks.len()
        );
        match imagegen_blocks[0] {
            ContentBlock::ImageGeneration {
                revised_prompt,
                image,
            } => {
                assert_eq!(revised_prompt.as_deref(), Some("parent painted this"));
                let image = image.as_ref().expect("parent image present");
                assert_eq!(image.data, "PARENT_BYTES");
                assert_eq!(image.uri.as_deref(), Some("/tmp/parent.png"));
            }
            other => panic!("expected ContentBlock::ImageGeneration, got {other:?}"),
        }

        let _ = fs::remove_file(path);
    }

    /// Write JSONL lines to a unique temp file and return its path.
    fn write_temp_rollout(tag: &str, lines: &[String]) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let path = env::temp_dir().join(format!("codeg-codex-{tag}-{nanos}.jsonl"));
        let mut content = lines.join("\n");
        content.push('\n');
        fs::write(&path, content).expect("write test jsonl");
        path
    }

    fn rollout_line(ts: &str, msg_type: &str, payload: serde_json::Value) -> String {
        serde_json::json!({ "timestamp": ts, "type": msg_type, "payload": payload }).to_string()
    }

    /// A directory of its own, for the tests that exercise the sibling-file
    /// lookup: `parse_codex_subagent_stats` scans the whole session directory,
    /// so it must not see other tests' rollouts (or the rest of `/tmp`).
    fn temp_session_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let dir = env::temp_dir().join(format!("codeg-codex-{tag}-{nanos}"));
        fs::create_dir_all(&dir).expect("create temp session dir");
        dir
    }

    fn thinking_texts(detail: &crate::models::ConversationDetail) -> Vec<String> {
        detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .filter_map(|b| match b {
                ContentBlock::Thinking { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Codex surfaces one reasoning turn twice: as per-section
    /// `event_msg.agent_reasoning` events (one per `**Header**` section) AND as a
    /// single `response_item.reasoning` whose `summary` array groups the same
    /// sections. History must render ONE 思考 card per turn (live parity), so the
    /// grouped summary is parsed and the split events are ignored — never one card
    /// per section.
    #[test]
    fn reasoning_summary_groups_sections_into_single_thinking_block() {
        let lines = vec![
            rollout_line(
                "2026-06-29T08:40:00Z",
                "event_msg",
                serde_json::json!({"type": "user_message", "message": "继续"}),
            ),
            // Streaming per-section events (must NOT each become a card).
            rollout_line(
                "2026-06-29T08:42:33.517Z",
                "event_msg",
                serde_json::json!({
                    "type": "agent_reasoning",
                    "text": "**Creating curl command**\n\nFirst section body."
                }),
            ),
            rollout_line(
                "2026-06-29T08:42:33.529Z",
                "event_msg",
                serde_json::json!({
                    "type": "agent_reasoning",
                    "text": "**Crafting the command**\n\nSecond section body."
                }),
            ),
            // Grouped summary written at the end of the reasoning turn.
            rollout_line(
                "2026-06-29T08:42:33.530Z",
                "response_item",
                serde_json::json!({
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [
                        {"type": "summary_text", "text": "**Creating curl command**\n\nFirst section body."},
                        {"type": "summary_text", "text": "**Crafting the command**\n\nSecond section body."}
                    ]
                }),
            ),
            rollout_line(
                "2026-06-29T08:42:33.943Z",
                "event_msg",
                serde_json::json!({"type": "agent_message", "message": "done"}),
            ),
        ];
        let path = write_temp_rollout("reasoning-group", &lines);
        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "reasoning-group")
            .expect("parse ok");

        let thinking = thinking_texts(&detail);
        assert_eq!(
            thinking.len(),
            1,
            "consecutive reasoning sections must render as ONE thinking block, got {}",
            thinking.len()
        );
        assert_eq!(
            thinking[0],
            "**Creating curl command**\n\nFirst section body.\n\n**Crafting the command**\n\nSecond section body."
        );

        let _ = fs::remove_file(path);
    }

    /// A reasoning item with an empty (encrypted-only) summary carries no
    /// surfaced text — the common case in real rollouts — and must produce no
    /// thinking card.
    #[test]
    fn empty_reasoning_summary_emits_no_thinking_block() {
        let lines = vec![
            rollout_line(
                "2026-06-29T08:40:00Z",
                "event_msg",
                serde_json::json!({"type": "user_message", "message": "hi"}),
            ),
            rollout_line(
                "2026-06-29T08:41:00Z",
                "response_item",
                serde_json::json!({
                    "type": "reasoning",
                    "id": "rs_empty",
                    "summary": [],
                    "encrypted_content": "gAAAredacted"
                }),
            ),
            rollout_line(
                "2026-06-29T08:41:01Z",
                "event_msg",
                serde_json::json!({"type": "agent_message", "message": "hello"}),
            ),
        ];
        let path = write_temp_rollout("reasoning-empty", &lines);
        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "reasoning-empty")
            .expect("parse ok");

        assert!(
            thinking_texts(&detail).is_empty(),
            "empty reasoning summary must not emit a thinking block"
        );

        let _ = fs::remove_file(path);
    }

    /// Defensive fallback: an interrupted rollout whose `agent_reasoning` events
    /// were written but that ended before the grouped `response_item.reasoning`
    /// summary must still surface the streaming reasoning — flushed at EOF as ONE
    /// joined Thinking block, not lost.
    #[test]
    fn streaming_reasoning_without_summary_flushes_as_one_block() {
        let lines = vec![
            rollout_line(
                "2026-06-29T08:40:00Z",
                "event_msg",
                serde_json::json!({"type": "user_message", "message": "go"}),
            ),
            rollout_line(
                "2026-06-29T08:42:00Z",
                "event_msg",
                serde_json::json!({"type": "agent_reasoning", "text": "**One**\n\nbody A"}),
            ),
            rollout_line(
                "2026-06-29T08:42:01Z",
                "event_msg",
                serde_json::json!({"type": "agent_reasoning", "text": "**Two**\n\nbody B"}),
            ),
            // No response_item/reasoning — the file ends here (interruption).
        ];
        let path = write_temp_rollout("reasoning-nosummary", &lines);
        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "reasoning-nosummary")
            .expect("parse ok");

        let thinking = thinking_texts(&detail);
        assert_eq!(
            thinking.len(),
            1,
            "buffered streaming reasoning must flush as ONE block, got {}",
            thinking.len()
        );
        assert_eq!(thinking[0], "**One**\n\nbody A\n\n**Two**\n\nbody B");

        let _ = fs::remove_file(path);
    }

    /// Same fallback, but the reasoning is followed by more content with no
    /// grouped summary (schema drift): the flushed Thinking block must stay in
    /// order — before the assistant message that follows it, never appended last.
    #[test]
    fn streaming_reasoning_without_summary_keeps_order_before_next_message() {
        let lines = vec![
            rollout_line(
                "2026-06-29T08:40:00Z",
                "event_msg",
                serde_json::json!({"type": "user_message", "message": "go"}),
            ),
            rollout_line(
                "2026-06-29T08:42:00Z",
                "event_msg",
                serde_json::json!({"type": "agent_reasoning", "text": "**Plan**\n\nthinking"}),
            ),
            rollout_line(
                "2026-06-29T08:42:01Z",
                "event_msg",
                serde_json::json!({"type": "agent_message", "message": "the answer"}),
            ),
        ];
        let path = write_temp_rollout("reasoning-order", &lines);
        let parser = CodexParser::new();
        let detail = parser
            .parse_conversation_detail(&path, "reasoning-order")
            .expect("parse ok");

        // Flatten assistant-side blocks in document order: the buffered reasoning
        // must be flushed as a Thinking block BEFORE the agent_message answer.
        let ordered: Vec<&str> = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .filter_map(|b| match b {
                ContentBlock::Thinking { .. } => Some("thinking"),
                ContentBlock::Text { text } if text == "the answer" => Some("answer"),
                _ => None,
            })
            .collect();
        assert_eq!(
            ordered,
            vec!["thinking", "answer"],
            "buffered reasoning must flush before the following assistant message"
        );
        assert_eq!(
            thinking_texts(&detail),
            vec!["**Plan**\n\nthinking".to_string()]
        );

        let _ = fs::remove_file(path);
    }

    /// Multi-wait, no close (the real codex polling pattern): every parent
    /// narration in the active window must survive (incl. the final answer with
    /// no close), each `wait_agent` becomes its own `collab_agent` capsule built
    /// from only the agents IT returned, and the result text moves off the spawn
    /// execution capsule into those wait capsules.
    #[test]
    fn subagent_waits_emit_independent_collab_capsules_and_keep_narration() {
        let spawn = |ts: &str, call: &str, msg: &str| {
            rollout_line(
                ts,
                "response_item",
                serde_json::json!({
                    "type": "function_call", "call_id": call, "name": "spawn_agent",
                    "arguments": serde_json::json!({"agent_type":"worker","message":msg}).to_string(),
                }),
            )
        };
        let spawn_out = |ts: &str, call: &str, agent_id: &str| {
            rollout_line(
                ts,
                "response_item",
                serde_json::json!({
                    "type": "function_call_output", "call_id": call,
                    "output": serde_json::json!({"agent_id":agent_id}).to_string(),
                }),
            )
        };
        let wait = |ts: &str, call: &str, targets: serde_json::Value| {
            rollout_line(
                ts,
                "response_item",
                serde_json::json!({
                    "type": "function_call", "call_id": call, "name": "wait_agent",
                    "arguments": serde_json::json!({"targets":targets}).to_string(),
                }),
            )
        };
        let wait_out = |ts: &str, call: &str, status: serde_json::Value| {
            rollout_line(
                ts,
                "response_item",
                serde_json::json!({
                    "type": "function_call_output", "call_id": call,
                    "output": serde_json::json!({"status":status}).to_string(),
                }),
            )
        };
        let narration = |ts: &str, text: &str| {
            rollout_line(
                ts,
                "event_msg",
                serde_json::json!({"type":"agent_message","message":text}),
            )
        };

        let lines = vec![
            rollout_line(
                "2026-06-27T10:00:00Z",
                "session_meta",
                serde_json::json!({"id":"mw","cwd":"/tmp/demo"}),
            ),
            rollout_line(
                "2026-06-27T10:00:01Z",
                "response_item",
                serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}),
            ),
            spawn("2026-06-27T10:00:02Z", "spawn_a", "task A"),
            spawn_out("2026-06-27T10:00:03Z", "spawn_a", "agent_a"),
            spawn("2026-06-27T10:00:04Z", "spawn_b", "task B"),
            spawn_out("2026-06-27T10:00:05Z", "spawn_b", "agent_b"),
            // active_agent_count == 2 here — old code dropped these three.
            narration("2026-06-27T10:00:06Z", "NARRATION_STARTED both started"),
            wait(
                "2026-06-27T10:00:07Z",
                "wait_1",
                serde_json::json!(["agent_a", "agent_b"]),
            ),
            // wait #1 returned ONLY agent_b (agent_a still running).
            wait_out(
                "2026-06-27T10:00:08Z",
                "wait_1",
                serde_json::json!({"agent_b":{"completed":"B_RESULT_TOKEN"}}),
            ),
            narration("2026-06-27T10:00:09Z", "NARRATION_MID B back waiting A"),
            wait("2026-06-27T10:00:10Z", "wait_2", serde_json::json!(["agent_a"])),
            wait_out(
                "2026-06-27T10:00:11Z",
                "wait_2",
                serde_json::json!({"agent_a":{"completed":"A_RESULT_TOKEN"}}),
            ),
            // Final answer with NO close → active never returns to 0.
            narration("2026-06-27T10:00:12Z", "NARRATION_FINAL summary"),
        ];

        let path = write_temp_rollout("multiwait", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "mw")
            .expect("parse ok");
        let blocks: Vec<&ContentBlock> =
            detail.turns.iter().flat_map(|t| t.blocks.iter()).collect();

        // Part A: every parent narration survives the active window.
        let all_text: String = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        for token in ["NARRATION_STARTED", "NARRATION_MID", "NARRATION_FINAL"] {
            assert!(all_text.contains(token), "missing narration {token}");
        }

        // Part B: exactly two wait capsules, each with its own returned agent.
        let collab_inputs: Vec<&str> = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_name,
                    input_preview,
                    ..
                } if tool_name == "collab_agent" => input_preview.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(collab_inputs.len(), 2, "one collab_agent capsule per wait");
        let b_cap = collab_inputs
            .iter()
            .find(|s| s.contains("B_RESULT_TOKEN"))
            .expect("wait capsule carrying B's result");
        assert!(b_cap.contains("agent_b"));
        assert!(
            !b_cap.contains("A_RESULT_TOKEN") && !b_cap.contains("agent_a"),
            "wait capsules must not overlap"
        );
        let a_cap = collab_inputs
            .iter()
            .find(|s| s.contains("A_RESULT_TOKEN"))
            .expect("wait capsule carrying A's result");
        assert!(a_cap.contains("agent_a"));
        // op-aware title source is present.
        assert!(a_cap.contains("__codegCollabOp"));

        // The result text must NOT remain on the spawn execution capsules or any
        // tool result (it lives only in the wait capsules now).
        for b in &blocks {
            match b {
                ContentBlock::ToolResult {
                    output_preview: Some(o),
                    ..
                } => {
                    assert!(
                        !o.contains("A_RESULT_TOKEN") && !o.contains("B_RESULT_TOKEN"),
                        "result leaked into a tool result"
                    );
                }
                ContentBlock::ToolUse {
                    tool_name,
                    input_preview: Some(i),
                    ..
                } if tool_name == "Agent" => {
                    assert!(
                        !i.contains("A_RESULT_TOKEN") && !i.contains("B_RESULT_TOKEN"),
                        "result leaked into the execution capsule"
                    );
                }
                _ => {}
            }
        }

        let _ = fs::remove_file(path);
    }

    /// A sub-agent closed without ever being waited on: there is no wait capsule
    /// to host the result, so the execution capsule falls back to showing the
    /// close `previous_status` result (no data loss).
    #[test]
    fn subagent_close_without_wait_falls_back_to_execution_capsule() {
        let lines = vec![
            rollout_line(
                "2026-06-27T11:00:00Z",
                "session_meta",
                serde_json::json!({"id":"cf","cwd":"/tmp/demo"}),
            ),
            rollout_line(
                "2026-06-27T11:00:01Z",
                "response_item",
                serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}),
            ),
            rollout_line(
                "2026-06-27T11:00:02Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"spawn_c","name":"spawn_agent",
                    "arguments": serde_json::json!({"agent_type":"worker","message":"task C"}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T11:00:03Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call_output","call_id":"spawn_c",
                    "output": serde_json::json!({"agent_id":"agent_c"}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T11:00:04Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"close_c","name":"close_agent",
                    "arguments": serde_json::json!({"target":"agent_c"}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T11:00:05Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call_output","call_id":"close_c",
                    "output": serde_json::json!({"previous_status":{"completed":"C_RESULT_TOKEN"}}).to_string(),
                }),
            ),
        ];

        let path = write_temp_rollout("closefallback", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "cf")
            .expect("parse ok");
        let blocks: Vec<&ContentBlock> =
            detail.turns.iter().flat_map(|t| t.blocks.iter()).collect();

        let collab_count = blocks
            .iter()
            .filter(|b| matches!(b, ContentBlock::ToolUse { tool_name, .. } if tool_name == "collab_agent"))
            .count();
        assert_eq!(collab_count, 0, "no wait → no collab capsule");

        let spawn_c_result = blocks
            .iter()
            .find_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id: Some(id),
                    output_preview,
                    ..
                } if id == "spawn_c" => Some(output_preview.clone()),
                _ => None,
            })
            .expect("spawn_c result block present");
        assert_eq!(
            spawn_c_result.as_deref(),
            Some("C_RESULT_TOKEN"),
            "execution capsule must show the close fallback result"
        );

        let _ = fs::remove_file(path);
    }

    /// A sub-agent closed (no wait) with a non-`completed` terminal result must
    /// keep that result AND mark the execution capsule failed — no data loss and
    /// live/history parity for errored no-wait closes.
    #[test]
    fn subagent_errored_close_without_wait_marks_execution_error() {
        let lines = vec![
            rollout_line(
                "2026-06-27T12:00:00Z",
                "session_meta",
                serde_json::json!({"id":"ce","cwd":"/tmp/demo"}),
            ),
            rollout_line(
                "2026-06-27T12:00:01Z",
                "response_item",
                serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}),
            ),
            rollout_line(
                "2026-06-27T12:00:02Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"spawn_e","name":"spawn_agent",
                    "arguments": serde_json::json!({"agent_type":"worker","message":"risky"}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T12:00:03Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call_output","call_id":"spawn_e",
                    "output": serde_json::json!({"agent_id":"agent_e"}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T12:00:04Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"close_e","name":"close_agent",
                    "arguments": serde_json::json!({"target":"agent_e"}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T12:00:05Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call_output","call_id":"close_e",
                    "output": serde_json::json!({"previous_status":{"errored":"BOOM_TOKEN"}}).to_string(),
                }),
            ),
        ];

        let path = write_temp_rollout("closeerr", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "ce")
            .expect("parse ok");
        let blocks: Vec<&ContentBlock> =
            detail.turns.iter().flat_map(|t| t.blocks.iter()).collect();

        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { tool_name, .. } if tool_name == "collab_agent")),
            "no wait → no collab capsule"
        );
        let (output, is_error) = blocks
            .iter()
            .find_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id: Some(id),
                    output_preview,
                    is_error,
                    ..
                } if id == "spawn_e" => Some((output_preview.clone(), *is_error)),
                _ => None,
            })
            .expect("spawn_e result block present");
        assert_eq!(output.as_deref(), Some("BOOM_TOKEN"), "errored result kept");
        assert!(is_error, "errored no-wait close → execution capsule failed");

        let _ = fs::remove_file(path);
    }

    /// An errored wait marks BOTH its own wait capsule and the execution capsule
    /// as failed (the result text still lives only on the wait capsule).
    #[test]
    fn subagent_errored_wait_marks_execution_error() {
        let lines = vec![
            rollout_line(
                "2026-06-27T13:00:00Z",
                "session_meta",
                serde_json::json!({"id":"we","cwd":"/tmp/demo"}),
            ),
            rollout_line(
                "2026-06-27T13:00:01Z",
                "response_item",
                serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}),
            ),
            rollout_line(
                "2026-06-27T13:00:02Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"spawn_w","name":"spawn_agent",
                    "arguments": serde_json::json!({"agent_type":"worker","message":"risky"}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T13:00:03Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call_output","call_id":"spawn_w",
                    "output": serde_json::json!({"agent_id":"agent_w"}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T13:00:04Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"wait_w","name":"wait_agent",
                    "arguments": serde_json::json!({"targets":["agent_w"]}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T13:00:05Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call_output","call_id":"wait_w",
                    "output": serde_json::json!({"status":{"agent_w":{"errored":"WAIT_BOOM"}}}).to_string(),
                }),
            ),
        ];

        let path = write_temp_rollout("waiterr", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "we")
            .expect("parse ok");
        let blocks: Vec<&ContentBlock> =
            detail.turns.iter().flat_map(|t| t.blocks.iter()).collect();

        // The wait capsule exists and carries the errored result text.
        let wait_input = blocks
            .iter()
            .find_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_name,
                    input_preview,
                    ..
                } if tool_name == "collab_agent" => input_preview.as_deref(),
                _ => None,
            })
            .expect("wait capsule present");
        assert!(wait_input.contains("WAIT_BOOM") && wait_input.contains("errored"));

        // The execution capsule (spawn) is marked failed, with no result text on it.
        let (output, is_error) = blocks
            .iter()
            .find_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id: Some(id),
                    output_preview,
                    is_error,
                    ..
                } if id == "spawn_w" => Some((output_preview.clone(), *is_error)),
                _ => None,
            })
            .expect("spawn_w result block present");
        assert_eq!(output, None, "result stays on the wait capsule");
        assert!(is_error, "errored wait → execution capsule failed");

        let _ = fs::remove_file(path);
    }

    /// The spawn execution capsule's input carries the sub-agent's `agent_id`
    /// (UUID), so the card can badge it uniformly with the wait capsule.
    #[test]
    fn subagent_spawn_capsule_input_carries_agent_id() {
        let lines = vec![
            rollout_line(
                "2026-06-27T14:00:00Z",
                "session_meta",
                serde_json::json!({"id":"ai","cwd":"/tmp/demo"}),
            ),
            rollout_line(
                "2026-06-27T14:00:01Z",
                "response_item",
                serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}),
            ),
            rollout_line(
                "2026-06-27T14:00:02Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"spawn_x","name":"spawn_agent",
                    "arguments": serde_json::json!({"agent_type":"worker","message":"do it"}).to_string(),
                }),
            ),
            rollout_line(
                "2026-06-27T14:00:03Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call_output","call_id":"spawn_x",
                    "output": serde_json::json!({"agent_id":"AGENT_UUID_X"}).to_string(),
                }),
            ),
        ];

        let path = write_temp_rollout("spawnid", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "ai")
            .expect("parse ok");
        let input = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .find_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_use_id: Some(id),
                    tool_name,
                    input_preview,
                    ..
                } if id == "spawn_x" && tool_name == "Agent" => input_preview.as_deref(),
                _ => None,
            })
            .expect("spawn Agent capsule present");
        let parsed: serde_json::Value =
            serde_json::from_str(input).expect("spawn input is JSON");
        assert_eq!(
            parsed.get("agent_id").and_then(|v| v.as_str()),
            Some("AGENT_UUID_X"),
            "spawn capsule input must carry the agent_id"
        );
        // Original fields preserved.
        assert_eq!(
            parsed.get("subagent_type").and_then(|v| v.as_str()),
            Some("worker")
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn native_team_spawn_names_the_task_and_hides_the_encrypted_message() {
        // codex 0.147's team-of-agents: `agent_type` became `task_name`, the
        // hand-off `message` became an encrypted envelope, and the spawn output
        // no longer returns an id — the `sub_agent_activity` event carries it,
        // correlated by the spawn's own call_id.
        let sealed = format!("gAAAAAB{}", "qgWsi0g7gOInVU3UTzqL".repeat(30));
        let lines = vec![
            rollout_line(
                "2026-08-16T07:47:37Z",
                "session_meta",
                serde_json::json!({"id":"parent","cwd":"/tmp/demo"}),
            ),
            rollout_line(
                "2026-08-16T07:47:41Z",
                "response_item",
                serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"跑一下构建"}]}),
            ),
            rollout_line(
                "2026-08-16T07:47:45Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"call_y7sk","name":"spawn_agent",
                    "namespace":"collaboration",
                    "arguments": serde_json::json!({
                        "task_name":"pnpm_build","fork_turns":"all","message": sealed,
                    }).to_string(),
                }),
            ),
            rollout_line(
                "2026-08-16T07:47:46Z",
                "event_msg",
                serde_json::json!({
                    "type":"sub_agent_activity","event_id":"call_y7sk",
                    "agent_thread_id":"01a0098a-7e8a-72d3-b7c0-2df130c84063",
                    "agent_path":"/root/pnpm_build","kind":"started",
                }),
            ),
            rollout_line(
                "2026-08-16T07:47:47Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call_output","call_id":"call_y7sk",
                    "output": serde_json::json!({"task_name":"/root/pnpm_build"}).to_string(),
                }),
            ),
        ];

        let path = write_temp_rollout("nativeteam", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "parent")
            .expect("parse ok");
        let input = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .find_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_use_id: Some(id),
                    tool_name,
                    input_preview,
                    ..
                } if id == "call_y7sk" && tool_name == "Agent" => input_preview.as_deref(),
                _ => None,
            })
            .expect("spawn Agent capsule present");
        let parsed: serde_json::Value = serde_json::from_str(input).expect("spawn input is JSON");
        assert_eq!(
            parsed.get("subagent_type").and_then(|v| v.as_str()),
            Some("pnpm_build"),
            "the capsule must be named after the task, not the removed agent_type"
        );
        // The envelope is unreadable, so the capsule shows no prompt at all
        // rather than a wall of base64 in its title and prompt panel.
        assert_eq!(parsed.get("prompt").and_then(|v| v.as_str()), Some(""));
        assert_eq!(parsed.get("description").and_then(|v| v.as_str()), Some(""));
        assert_eq!(
            parsed.get("agent_id").and_then(|v| v.as_str()),
            Some("01a0098a-7e8a-72d3-b7c0-2df130c84063"),
            "the sub_agent_activity event is the only source of the thread id"
        );
        // 0.147 emits no wait/close capsule, so this card stands for the LAUNCH
        // only and must say so rather than read as "the sub-agent finished".
        assert_eq!(
            parsed.get(CODEX_SUBAGENT_LAUNCH_KEY).and_then(|v| v.as_bool()),
            Some(true)
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn legacy_collab_spawn_keeps_its_run_semantics() {
        // The pre-0.147 shape DOES get a wait/close capsule carrying the
        // result, so its capsule really does stand for the run — it must not
        // pick up the launch-only caveat.
        let lines = vec![
            rollout_line(
                "2026-06-27T14:00:00Z",
                "session_meta",
                serde_json::json!({"id":"ai","cwd":"/tmp/demo"}),
            ),
            rollout_line(
                "2026-06-27T14:00:02Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"spawn_x","name":"spawn_agent",
                    "arguments": serde_json::json!({"agent_type":"worker","message":"do it"}).to_string(),
                }),
            ),
        ];
        let path = write_temp_rollout("legacyspawn", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "ai")
            .expect("parse ok");
        let input = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .find_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_name,
                    input_preview,
                    ..
                } if tool_name == "Agent" => input_preview.as_deref(),
                _ => None,
            })
            .expect("spawn Agent capsule present");
        let parsed: serde_json::Value = serde_json::from_str(input).expect("JSON");
        assert_eq!(parsed.get("subagent_type").and_then(|v| v.as_str()), Some("worker"));
        assert_eq!(parsed.get("prompt").and_then(|v| v.as_str()), Some("do it"));
        assert!(parsed.get(CODEX_SUBAGENT_LAUNCH_KEY).is_none());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn send_message_shows_a_marker_instead_of_the_sealed_payload() {
        // `send_message` has no capsule of its own — it renders on the generic
        // tool card, whose preview is the whole argument JSON. Its `message` is
        // the same sealed envelope `spawn_agent` carries.
        let sealed = format!("gAAAAAB{}", "0g7gOInVU3UTzqL".repeat(30));
        let lines = vec![
            rollout_line(
                "2026-08-16T07:47:37Z",
                "session_meta",
                serde_json::json!({"id":"parent","cwd":"/tmp/demo"}),
            ),
            rollout_line(
                "2026-08-16T07:47:50Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"call_send","name":"send_message",
                    "namespace":"collaboration",
                    "arguments": serde_json::json!({
                        "target":"/root/pnpm_build","message": sealed,
                    }).to_string(),
                }),
            ),
        ];
        let path = write_temp_rollout("sendmessage", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "parent")
            .expect("parse ok");
        let input = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .find_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_name,
                    input_preview,
                    ..
                } if tool_name == "send_message" => input_preview.as_deref(),
                _ => None,
            })
            .expect("send_message card present");
        assert!(
            !input.contains("gAAAAAB"),
            "the sealed payload must not reach the card: {input}"
        );
        assert!(input.contains("[encrypted]"), "{input}");
        // The readable arguments survive.
        assert!(input.contains("/root/pnpm_build"), "{input}");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn redaction_leaves_ordinary_arguments_untouched() {
        let mut args = serde_json::json!({"cmd":"pnpm build","timeout_ms":3600000});
        assert!(!redact_encrypted_args(&mut args));
        assert_eq!(args, serde_json::json!({"cmd":"pnpm build","timeout_ms":3600000}));
        // Nested and array positions are reached.
        let sealed = format!("gAAAAAB{}", "0g7gOInVU3UTzqL".repeat(10));
        let mut nested = serde_json::json!({"outer":{"list":[sealed.clone(),"keep me"]}});
        assert!(redact_encrypted_args(&mut nested));
        assert_eq!(
            nested,
            serde_json::json!({"outer":{"list":["[encrypted]","keep me"]}})
        );
    }

    #[test]
    fn redaction_visits_every_sibling_not_just_the_first() {
        // Guards the reason `redact_encrypted_children` is a loop: the obvious
        // `Iterator::any` (which clippy's `unnecessary_fold` even suggests)
        // stops at the first hit, so a second sealed sibling would reach the
        // card as half a kilobyte of base64.
        let sealed = format!("gAAAAAB{}", "0g7gOInVU3UTzqL".repeat(10));
        let mut args = serde_json::json!({
            "list": [sealed.clone(), sealed.clone()],
            "first": sealed.clone(),
            "second": sealed.clone(),
        });
        assert!(redact_encrypted_args(&mut args));
        assert_eq!(
            args,
            serde_json::json!({
                "list": ["[encrypted]", "[encrypted]"],
                "first": "[encrypted]",
                "second": "[encrypted]",
            })
        );
    }

    #[test]
    fn native_team_wait_renders_the_same_bare_capsule_history_and_live() {
        // codex-acp forwards `wait_agent` (unlike the spawn), so live shows a
        // 「fetch the sub-agent's result」 pill. Reload must show it too — it is
        // the only span in the timeline covering the child's actual run.
        let lines = vec![
            rollout_line(
                "2026-08-16T09:40:59Z",
                "session_meta",
                serde_json::json!({"id":"parent","cwd":"/tmp/demo"}),
            ),
            rollout_line(
                "2026-08-16T09:41:00Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call","call_id":"call_wait","name":"wait_agent",
                    "namespace":"collaboration",
                    "arguments": serde_json::json!({"timeout_ms":3600000}).to_string(),
                }),
            ),
            rollout_line(
                "2026-08-16T09:41:20Z",
                "response_item",
                serde_json::json!({
                    "type":"function_call_output","call_id":"call_wait",
                    "output": serde_json::json!({
                        "message":"Wait completed.","timed_out":false,
                    }).to_string(),
                }),
            ),
        ];
        let path = write_temp_rollout("nativewait", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "parent")
            .expect("parse ok");
        let input = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .find_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_name,
                    input_preview,
                    ..
                } if tool_name == "collab_agent" => input_preview.as_deref(),
                _ => None,
            })
            .expect("wait capsule present");
        let parsed: serde_json::Value = serde_json::from_str(input).expect("JSON");
        assert_eq!(parsed.get(COLLAB_OP_KEY).and_then(|v| v.as_str()), Some("wait"));
        assert_eq!(parsed.get("status").and_then(|v| v.as_str()), Some("completed"));
        // No agents and no prompt — the card renders as a bare pill, exactly
        // what the live `collabAgentToolCall` produces for this output.
        assert_eq!(
            parsed.get("agentsStates"),
            Some(&serde_json::json!({}))
        );
        let errored = detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .any(|b| matches!(b, ContentBlock::ToolResult { is_error: true, .. }));
        assert!(!errored, "a completed wait is not an error");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn native_team_wait_shape_gate_and_timeout() {
        // `timed_out` is the shape gate: only the native-team output has it.
        assert!(native_team_wait_input(&serde_json::json!({"message":"Wait completed."})).is_none());
        assert!(native_team_wait_input(&serde_json::json!({})).is_none());
        // A timeout is a real outcome — flag the capsule failed.
        let (input, is_error) =
            native_team_wait_input(&serde_json::json!({"message":"x","timed_out":true}))
                .expect("native shape");
        assert!(is_error);
        let parsed: serde_json::Value = serde_json::from_str(&input).expect("JSON");
        assert_eq!(parsed.get("status").and_then(|v| v.as_str()), Some("failed"));
    }

    #[test]
    fn encrypted_envelope_detection_is_narrow() {
        let sealed = format!("gAAAAAB{}", "0g7gOInVU3UTzqL".repeat(10));
        assert!(is_encrypted_envelope(&sealed));
        assert!(is_encrypted_envelope(&format!("  {sealed}  ")));
        // A prompt that merely mentions or embeds base64 still renders: the
        // prefix must lead, the blob must be long, and it must be one token.
        assert!(!is_encrypted_envelope(&format!("decode this: {sealed}")));
        assert!(!is_encrypted_envelope(&format!("{sealed} and report back")));
        assert!(!is_encrypted_envelope("gAAAAAB"));
        assert!(!is_encrypted_envelope("run pnpm build"));
        assert!(!is_encrypted_envelope(""));
    }

    #[test]
    fn forked_child_transcript_yields_no_stats() {
        // A codex 0.147 sub-agent thread opens with its own `session_meta`
        // (carrying `parent_thread_id`) and then replays the PARENT's history,
        // tool calls included. Counting those would credit the child with the
        // parent's work, so the whole file is refused.
        let child = "01a0098a-7e8a-72d3-b7c0-2df130c84063";
        let dir = temp_session_dir("forked-child");
        // The lookup matches a rollout whose stem ends with the thread id,
        // which is exactly how codex names a sub-agent's file.
        fs::write(
            dir.join(format!("rollout-2026-08-16T07-47-46-{child}.jsonl")),
            [
                rollout_line(
                    "2026-08-16T07:47:46Z",
                    "session_meta",
                    serde_json::json!({"id":child,"parent_thread_id":"parent","cwd":"/tmp/demo"}),
                ),
                rollout_line(
                    "2026-08-16T07:47:46Z",
                    "session_meta",
                    serde_json::json!({"id":"parent","cwd":"/tmp/demo"}),
                ),
                rollout_line(
                    "2026-08-16T07:47:47Z",
                    "response_item",
                    serde_json::json!({
                        "type":"function_call","call_id":"parents_own","name":"exec_command",
                        "arguments": serde_json::json!({"cmd":"git status"}).to_string(),
                    }),
                ),
            ]
            .join("\n"),
        )
        .expect("write forked child");
        assert!(
            parse_codex_subagent_stats(&dir, child).is_none(),
            "a forked child transcript must contribute no stats"
        );

        // The legacy shape has no replayed prefix and still resolves.
        fs::write(
            dir.join("agent-solo.jsonl"),
            [
                rollout_line(
                    "2026-08-16T07:47:46Z",
                    "session_meta",
                    serde_json::json!({"id":"solo","cwd":"/tmp/demo"}),
                ),
                rollout_line(
                    "2026-08-16T07:47:47Z",
                    "response_item",
                    serde_json::json!({
                        "type":"function_call","call_id":"c1","name":"exec_command",
                        "arguments": serde_json::json!({"cmd":"pnpm build"}).to_string(),
                    }),
                ),
            ]
            .join("\n"),
        )
        .expect("write own thread");
        let stats = parse_codex_subagent_stats(&dir, "solo").expect("stats for an own thread");
        assert_eq!(stats.tool_calls.len(), 1);

        let _ = fs::remove_dir_all(dir);
    }

    // ── codex code mode ──────────────────────────────────────────────────
    //
    // Newer codex wraps EVERY tool call in a JS script persisted as one
    // `custom_tool_call` named `exec`. Without unwrapping, the whole history
    // renders as `const r = await tools.…` shell cards (see
    // `parsers/codex_code_mode.rs`).

    fn tool_uses(
        detail: &crate::models::ConversationDetail,
    ) -> Vec<(String, String, Option<String>)> {
        detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_use_id,
                    tool_name,
                    input_preview,
                    ..
                } => Some((
                    tool_use_id.clone().unwrap_or_default(),
                    tool_name.clone(),
                    input_preview.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    fn tool_results(
        detail: &crate::models::ConversationDetail,
    ) -> Vec<(String, Option<String>, bool)> {
        detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    output_preview,
                    is_error,
                    ..
                } => Some((
                    tool_use_id.clone().unwrap_or_default(),
                    output_preview.clone(),
                    *is_error,
                )),
                _ => None,
            })
            .collect()
    }

    fn code_mode_rollout(script: &str, output: serde_json::Value) -> Vec<String> {
        vec![
            rollout_line(
                "2026-07-20T08:40:00Z",
                "event_msg",
                serde_json::json!({"type": "user_message", "message": "go"}),
            ),
            rollout_line(
                "2026-07-20T08:40:01Z",
                "response_item",
                serde_json::json!({
                    "type": "custom_tool_call",
                    "name": "exec",
                    "call_id": "call_1",
                    "input": script,
                }),
            ),
            rollout_line(
                "2026-07-20T08:40:02Z",
                "response_item",
                serde_json::json!({
                    "type": "custom_tool_call_output",
                    "call_id": "call_1",
                    "output": output,
                }),
            ),
        ]
    }

    #[test]
    fn code_mode_single_call_renders_as_the_inner_tool() {
        let lines = code_mode_rollout(
            "const r = await tools.exec_command({\n  cmd: \"git status --short\",\n  workdir: \"/repo\"\n});\ntext(r.output);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.5 seconds\nOutput:\n"},
                {"type": "input_text", "text": " M src/main.rs"},
            ]),
        );
        let path = write_temp_rollout("code-mode-single", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-single")
            .expect("parse ok");

        assert_eq!(
            tool_uses(&detail),
            vec![(
                "call_1".to_string(),
                "exec_command".to_string(),
                Some("git status --short".to_string())
            )]
        );
        assert_eq!(
            tool_results(&detail),
            vec![(
                "call_1".to_string(),
                Some(" M src/main.rs".to_string()),
                false
            )]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn code_mode_parallel_calls_split_output_per_card() {
        let lines = code_mode_rollout(
            "const rs = await Promise.all([\n  tools.exec_command({cmd:\"one\"}),\n  tools.exec_command({cmd:\"two\"}),\n  tools.exec_command({cmd:\"three\"})\n]);\nrs.forEach(r => text(r.output));\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type": "input_text", "text": "out-one"},
                {"type": "input_text", "text": "out-two"},
                {"type": "input_text", "text": "out-three"},
            ]),
        );
        let path = write_temp_rollout("code-mode-split", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-split")
            .expect("parse ok");

        assert_eq!(
            tool_uses(&detail),
            vec![
                (
                    "call_1#0".to_string(),
                    "exec_command".to_string(),
                    Some("one".to_string())
                ),
                (
                    "call_1#1".to_string(),
                    "exec_command".to_string(),
                    Some("two".to_string())
                ),
                (
                    "call_1#2".to_string(),
                    "exec_command".to_string(),
                    Some("three".to_string())
                ),
            ]
        );
        assert_eq!(
            tool_results(&detail),
            vec![
                ("call_1#0".to_string(), Some("out-one".to_string()), false),
                ("call_1#1".to_string(), Some("out-two".to_string()), false),
                ("call_1#2".to_string(), Some("out-three".to_string()), false),
            ]
        );

        let _ = fs::remove_file(path);
    }

    const THREE_CALLS: &str = "const r = await Promise.all([\n  tools.exec_command({cmd:\"one\"}),\n  tools.exec_command({cmd:\"two\"}),\n  tools.exec_command({cmd:\"three\"})\n]);\nfor (const x of r) { text(x.output); text(`exit_code=${x.exit_code}`); }\n";

    /// `text(x.output); text(\`exit_code=…\`)` — two chunks per call, so the 1:1
    /// count fails and the whole thing used to stay one script card.
    #[test]
    fn a_trailing_chunk_per_call_still_splits_per_call() {
        let lines = code_mode_rollout(
            THREE_CALLS,
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type": "input_text", "text": "out-one"},
                {"type": "input_text", "text": "exit_code=0"},
                {"type": "input_text", "text": "out-two"},
                {"type": "input_text", "text": "exit_code=1"},
                {"type": "input_text", "text": "out-three"},
                {"type": "input_text", "text": "exit_code=0"},
            ]),
        );
        let path = write_temp_rollout("code-mode-stride-tail", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-stride-tail")
            .expect("parse ok");

        assert_eq!(
            tool_uses(&detail),
            vec![
                (
                    "call_1#0".to_string(),
                    "exec_command".to_string(),
                    Some("one".to_string())
                ),
                (
                    "call_1#1".to_string(),
                    "exec_command".to_string(),
                    Some("two".to_string())
                ),
                (
                    "call_1#2".to_string(),
                    "exec_command".to_string(),
                    Some("three".to_string())
                ),
            ]
        );
        assert_eq!(
            tool_results(&detail),
            vec![
                (
                    "call_1#0".to_string(),
                    Some("out-one\nexit_code=0".to_string()),
                    false
                ),
                (
                    "call_1#1".to_string(),
                    Some("out-two\nexit_code=1".to_string()),
                    false
                ),
                (
                    "call_1#2".to_string(),
                    Some("out-three\nexit_code=0".to_string()),
                    false
                ),
            ]
        );

        let _ = fs::remove_file(path);
    }

    /// The other half of the shape: a `---RESULT n---` header ahead of each
    /// output. The counter differs per call, so the repeat is only visible with
    /// digits blinded — and each chunk of the group is unwrapped on its own, so
    /// an envelope behind a header still reads as a terminal.
    #[test]
    fn a_numbered_header_per_call_still_splits_per_call() {
        let envelope = |out: &str| {
            serde_json::json!({
                "chunk_id": "abc123",
                "wall_time_seconds": 0.5,
                "exit_code": 0,
                "original_token_count": 9,
                "output": out,
            })
            .to_string()
        };
        let lines = code_mode_rollout(
            "const r = await Promise.all([\n  tools.exec_command({cmd:\"one\"}),\n  tools.exec_command({cmd:\"two\"})\n]);\nr.forEach((x, i) => { text(`---RESULT ${i + 1}---`); text(JSON.stringify(x)); });\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type": "input_text", "text": "---RESULT 1---"},
                {"type": "input_text", "text": envelope("out-one")},
                {"type": "input_text", "text": "---RESULT 2---"},
                {"type": "input_text", "text": envelope("out-two")},
            ]),
        );
        let path = write_temp_rollout("code-mode-stride-head", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-stride-head")
            .expect("parse ok");

        assert_eq!(
            tool_results(&detail),
            vec![
                (
                    "call_1#0".to_string(),
                    Some("---RESULT 1---\nout-one".to_string()),
                    false
                ),
                (
                    "call_1#1".to_string(),
                    Some("---RESULT 2---\nout-two".to_string()),
                    false
                ),
            ]
        );

        let _ = fs::remove_file(path);
    }

    /// The adversarial version of the shape above: the second command's own
    /// stdout IS `exit_code=0`, so slot 1 of every assumed pair repeats and the
    /// chunk content alone stops telling the two loops apart. Only the script
    /// says which it is — two loops, so no grouping.
    #[test]
    fn a_command_whose_output_mimics_the_marker_does_not_force_a_grouping() {
        let lines = code_mode_rollout(
            "const r = await Promise.all([\n  tools.exec_command({cmd:\"printf one\"}),\n  tools.exec_command({cmd:\"printf 'exit_code=0'\"}),\n  tools.exec_command({cmd:\"printf three\"})\n]);\nfor (const x of r) text(x.output);\nfor (const x of r) text(`exit_code=${x.exit_code}`);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type": "input_text", "text": "one"},
                {"type": "input_text", "text": "exit_code=0"},
                {"type": "input_text", "text": "three"},
                {"type": "input_text", "text": "exit_code=0"},
                {"type": "input_text", "text": "exit_code=0"},
                {"type": "input_text", "text": "exit_code=0"},
            ]),
        );
        let path = write_temp_rollout("code-mode-stride-mimic", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-stride-mimic")
            .expect("parse ok");

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, CODEX_SCRIPT_TOOL_NAME);
        let results = tool_results(&detail);
        assert_eq!(results.len(), 1);
        assert!(results[0]
            .1
            .as_deref()
            .expect("script output")
            .starts_with("one\nexit_code=0\nthree"));

        let _ = fs::remove_file(path);
    }

    /// Dividing evenly is NOT the proof. This script prints every output and
    /// then every exit code — the same 6 chunks for 3 calls as the interleaved
    /// shape above, in an order where grouping them in pairs would pin call 2's
    /// output to call 1's card. Only the script says which shape it is.
    #[test]
    fn chunks_that_merely_divide_evenly_are_not_grouped() {
        let lines = code_mode_rollout(
            "const r = await Promise.all([\n  tools.exec_command({cmd:\"one\"}),\n  tools.exec_command({cmd:\"two\"}),\n  tools.exec_command({cmd:\"three\"})\n]);\nfor (const x of r) text(x.output);\nfor (const x of r) text(`exit_code=${x.exit_code}`);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type": "input_text", "text": "out-one"},
                {"type": "input_text", "text": "out-two"},
                {"type": "input_text", "text": "out-three"},
                {"type": "input_text", "text": "exit_code=0"},
                {"type": "input_text", "text": "exit_code=0"},
                {"type": "input_text", "text": "exit_code=0"},
            ]),
        );
        let path = write_temp_rollout("code-mode-stride-interleaved", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-stride-interleaved")
            .expect("parse ok");

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, CODEX_SCRIPT_TOOL_NAME);

        let _ = fs::remove_file(path);
    }

    /// What codex sends instead of the chunks when a script's `text()` stream is
    /// too long: the whole stream re-rendered as one blob, one line per `text()`,
    /// behind a banner declaring how many lines it started with.
    fn collapsed_blob(declared: usize, outputs: &[&str]) -> String {
        let lines: Vec<String> = outputs
            .iter()
            .enumerate()
            .map(|(index, out)| {
                serde_json::json!({
                    "chunk_id": format!("chunk{index}"),
                    "wall_time_seconds": 0.5,
                    "exit_code": 0,
                    "original_token_count": 9,
                    "output": out,
                })
                .to_string()
            })
            .collect();
        format!(
            "Warning: truncated output (original token count: 10644)\nTotal output lines: {declared}\n\n{}",
            lines.join("\n")
        )
    }

    #[test]
    fn a_collapsed_output_blob_splits_back_into_one_card_per_call() {
        let lines = code_mode_rollout(
            "const r = await Promise.all([\n  tools.exec_command({cmd:\"one\"}),\n  tools.exec_command({cmd:\"two\"}),\n  tools.exec_command({cmd:\"three\"})\n]);\nfor (const x of r) text(x);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type": "input_text", "text": collapsed_blob(3, &["out-one", "out-two", "out-three"])},
            ]),
        );
        let path = write_temp_rollout("code-mode-collapsed", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-collapsed")
            .expect("parse ok");

        assert_eq!(
            tool_uses(&detail),
            vec![
                (
                    "call_1#0".to_string(),
                    "exec_command".to_string(),
                    Some("one".to_string())
                ),
                (
                    "call_1#1".to_string(),
                    "exec_command".to_string(),
                    Some("two".to_string())
                ),
                (
                    "call_1#2".to_string(),
                    "exec_command".to_string(),
                    Some("three".to_string())
                ),
            ]
        );
        assert_eq!(
            tool_results(&detail),
            vec![
                ("call_1#0".to_string(), Some("out-one".to_string()), false),
                ("call_1#1".to_string(), Some("out-two".to_string()), false),
                ("call_1#2".to_string(), Some("out-three".to_string()), false),
            ]
        );

        let _ = fs::remove_file(path);
    }

    /// One call, one line, and the line is the raw result object — the shape
    /// that used to leave a card whose entire body was `{"chunk_id":…}`.
    #[test]
    fn a_collapsed_single_call_blob_still_shows_its_output() {
        let lines = code_mode_rollout(
            "const r = await tools.exec_command({cmd:\"cargo test\"});\ntext(r);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 9.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": collapsed_blob(1, &["test result: ok. 42 passed"])},
            ]),
        );
        let path = write_temp_rollout("code-mode-collapsed-single", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-collapsed-single")
            .expect("parse ok");

        assert_eq!(
            tool_results(&detail),
            vec![(
                "call_1".to_string(),
                Some("test result: ok. 42 passed".to_string()),
                false
            )]
        );

        let _ = fs::remove_file(path);
    }

    /// Past the cap codex drops whole lines — and it drops them from the middle,
    /// so a survivor's position no longer names its call. Measured: with a line
    /// missing, 13 of 57 checkable lines belong to a different command than
    /// their position claims. Splitting anyway would put a command's output
    /// under another command's title, so the script card stays.
    #[test]
    fn a_collapsed_blob_missing_a_line_keeps_the_script_card() {
        let lines = code_mode_rollout(
            "const r = await Promise.all([\n  tools.exec_command({cmd:\"one\"}),\n  tools.exec_command({cmd:\"two\"}),\n  tools.exec_command({cmd:\"three\"})\n]);\nfor (const x of r) text(x);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type": "input_text", "text": collapsed_blob(3, &["out-one", "out-three"])},
            ]),
        );
        let path = write_temp_rollout("code-mode-collapsed-lossy", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-collapsed-lossy")
            .expect("parse ok");

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, CODEX_SCRIPT_TOOL_NAME);
        let results = tool_results(&detail);
        assert_eq!(results.len(), 1);
        assert!(results[0]
            .1
            .as_deref()
            .expect("script output")
            .starts_with("Warning: truncated output"));

        let _ = fs::remove_file(path);
    }

    /// The same banner fronts the far more common case: ONE command whose own
    /// stdout was too long. Its lines are stdout, not `text()` results, and
    /// splitting them would shred one command's output across every card.
    #[test]
    fn a_truncated_command_output_is_not_read_as_one_line_per_call() {
        let lines = code_mode_rollout(
            "const r = await Promise.all([\n  tools.exec_command({cmd:\"one\"}),\n  tools.exec_command({cmd:\"two\"})\n]);\ntext(r[0].output);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type": "input_text", "text": "Warning: truncated output (original token count: 900)\nTotal output lines: 2\n\nfirst line of stdout\nsecond line of stdout"},
            ]),
        );
        let path = write_temp_rollout("code-mode-collapsed-stdout", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-collapsed-stdout")
            .expect("parse ok");

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, CODEX_SCRIPT_TOOL_NAME);
        let results = tool_results(&detail);
        assert_eq!(results.len(), 1);
        assert!(results[0]
            .1
            .as_deref()
            .expect("script output")
            .contains("first line of stdout\nsecond line of stdout"));

        let _ = fs::remove_file(path);
    }

    /// Fewer `text()` chunks than calls means chunk *i* is NOT provably call
    /// *i*'s output — keep one script card rather than misattribute.
    #[test]
    fn code_mode_mismatched_chunks_keep_the_script_card() {
        let lines = code_mode_rollout(
            "const rs = await Promise.all([\n  tools.exec_command({cmd:\"one\"}),\n  tools.exec_command({cmd:\"two\"})\n]);\ntext(JSON.stringify(rs));\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
                {"type": "input_text", "text": "[{\"output\":\"a\"},{\"output\":\"b\"}]"},
            ]),
        );
        let path = write_temp_rollout("code-mode-mismatch", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-mismatch")
            .expect("parse ok");

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, CODEX_SCRIPT_TOOL_NAME);
        let card: serde_json::Value =
            serde_json::from_str(uses[0].2.as_deref().expect("script card input"))
                .expect("script card input is JSON");
        assert_eq!(card["title"], "one");
        assert_eq!(card["call_count"], 2);
        assert!(card["source"]
            .as_str()
            .expect("source")
            .contains("Promise.all"));

        let results = tool_results(&detail);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].1.as_deref(),
            Some("[{\"output\":\"a\"},{\"output\":\"b\"}]")
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn code_mode_apply_patch_renders_as_a_patch_card() {
        let lines = code_mode_rollout(
            "const patch = \"*** Begin Patch\\n*** Update File: a.rs\\n@@\\n-old\\n+new\\n*** End Patch\";\nconst result = await tools.apply_patch(patch);\ntext(result);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.2 seconds\nOutput:\n"},
                {"type": "input_text", "text": "Success. Updated the following files:\nM a.rs"},
            ]),
        );
        let path = write_temp_rollout("code-mode-patch", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-patch")
            .expect("parse ok");

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, "apply_patch");
        assert!(uses[0]
            .2
            .as_deref()
            .expect("patch text")
            .starts_with("*** Begin Patch"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn code_mode_script_failure_marks_the_result_as_an_error() {
        let lines = code_mode_rollout(
            "const r = await tools.exec_command({cmd: \"rm -rf /tmp/x\"});\ntext(r.output);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script failed\nWall time 0.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": "Script error:\nexec_command failed for `rm -rf /tmp/x`"},
            ]),
        );
        let path = write_temp_rollout("code-mode-failed", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-failed")
            .expect("parse ok");

        let results = tool_results(&detail);
        assert_eq!(results.len(), 1);
        assert!(results[0].2, "a failed script must render as an error");

        let _ = fs::remove_file(path);
    }

    /// An interrupted turn never writes the output record; the script card must
    /// stay where it was rather than vanish or jump to the end.
    #[test]
    fn code_mode_call_without_output_keeps_its_script_card() {
        let lines = vec![
            rollout_line(
                "2026-07-20T08:40:00Z",
                "event_msg",
                serde_json::json!({"type": "user_message", "message": "go"}),
            ),
            rollout_line(
                "2026-07-20T08:40:01Z",
                "response_item",
                serde_json::json!({
                    "type": "custom_tool_call",
                    "name": "exec",
                    "call_id": "call_1",
                    "input": "const r = await tools.exec_command({cmd: \"sleep 60\"});",
                }),
            ),
        ];
        let path = write_temp_rollout("code-mode-dangling", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-dangling")
            .expect("parse ok");

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, CODEX_SCRIPT_TOOL_NAME);
        assert!(tool_results(&detail).is_empty());

        let _ = fs::remove_file(path);
    }

    /// The unified-exec `wait` tool shares code mode's array output shape; it
    /// used to render as a raw `[{"text":"Script completed…` JSON blob.
    #[test]
    fn wait_tool_output_is_joined_text_not_raw_json() {
        let lines = vec![
            rollout_line(
                "2026-07-20T08:40:00Z",
                "event_msg",
                serde_json::json!({"type": "user_message", "message": "go"}),
            ),
            rollout_line(
                "2026-07-20T08:40:01Z",
                "response_item",
                serde_json::json!({
                    "type": "function_call",
                    "name": "wait",
                    "call_id": "call_w",
                    "arguments": "{\"cell_id\":\"55\",\"yield_time_ms\":30000}",
                }),
            ),
            rollout_line(
                "2026-07-20T08:40:09Z",
                "response_item",
                serde_json::json!({
                    "type": "function_call_output",
                    "call_id": "call_w",
                    "output": [
                        {"type": "input_text", "text": "Script completed\nWall time 5.6 seconds\nOutput:\n"},
                        {"type": "input_text", "text": "test result: ok. 12 passed"},
                    ],
                }),
            ),
        ];
        let path = write_temp_rollout("code-mode-wait", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "code-mode-wait")
            .expect("parse ok");

        assert_eq!(
            tool_results(&detail),
            vec![(
                "call_w".to_string(),
                Some("test result: ok. 12 passed".to_string()),
                false
            )]
        );

        let _ = fs::remove_file(path);
    }

    /// `wait` / `write_stdin` carry a session id, never a command — which is
    /// why their cards used to be titled by the bare tool name. The command
    /// that started the session is recovered from the announcement in that
    /// command's own output.
    fn session_tool_rollout(
        exec_output: serde_json::Value,
        session_call: serde_json::Value,
    ) -> Vec<String> {
        let mut lines = background_session_head(exec_output);
        lines.push(rollout_line(
            "2026-07-20T08:40:03Z",
            "response_item",
            session_call,
        ));
        lines
    }

    /// `pnpm dev`, answered with whatever it had printed when unified-exec's
    /// yield ran out.
    fn background_session_head(exec_output: serde_json::Value) -> Vec<String> {
        vec![
            rollout_line(
                "2026-07-20T08:40:00Z",
                "event_msg",
                serde_json::json!({"type": "user_message", "message": "go"}),
            ),
            rollout_line(
                "2026-07-20T08:40:01Z",
                "response_item",
                serde_json::json!({
                    "type": "function_call",
                    "name": "exec_command",
                    "call_id": "call_e",
                    "arguments": "{\"cmd\":\"pnpm dev\",\"yield_time_ms\":1000}",
                }),
            ),
            rollout_line(
                "2026-07-20T08:40:02Z",
                "response_item",
                serde_json::json!({
                    "type": "function_call_output",
                    "call_id": "call_e",
                    "output": exec_output,
                }),
            ),
        ]
    }

    /// One `write_stdin` collecting more of a background session's output, and
    /// the output it collected.
    fn poll_lines(call_id: &str, args: &str, output: serde_json::Value) -> Vec<String> {
        vec![
            rollout_line(
                "2026-07-20T08:40:03Z",
                "response_item",
                serde_json::json!({
                    "type": "function_call",
                    "name": "write_stdin",
                    "call_id": call_id,
                    "arguments": args,
                }),
            ),
            rollout_line(
                "2026-07-20T08:40:04Z",
                "response_item",
                serde_json::json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }),
            ),
        ]
    }

    fn parse_lines(lines: &[String], tag: &str) -> crate::models::ConversationDetail {
        let path = write_temp_rollout(tag, lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, tag)
            .expect("parse ok");
        let _ = fs::remove_file(path);
        detail
    }

    fn session_tool_input(lines: &[String], tag: &str) -> Option<String> {
        tool_uses(&parse_lines(lines, tag))
            .into_iter()
            .find(|(id, _, _)| id == "call_s")
            .and_then(|(_, _, input)| input)
    }

    /// A call that gets a card of its own — here a `wait` that also kills the
    /// session — is titled by the command it is acting on, not by the bare tool
    /// name. (A call that only collects output gets no card at all; see
    /// `polling_a_background_session_appends_to_the_card_of_its_command`.)
    #[test]
    fn wait_carries_the_command_whose_session_it_polls() {
        let lines = session_tool_rollout(
            serde_json::json!(
                "Chunk ID: 523e44\nWall time: 1.0 seconds\nProcess running with session ID 22068\nOutput:\nstarting…"
            ),
            serde_json::json!({
                "type": "function_call",
                "name": "wait",
                "call_id": "call_s",
                "arguments": "{\"cell_id\":\"22068\",\"terminate\":true,\"yield_time_ms\":30000}",
            }),
        );

        let input: serde_json::Value =
            serde_json::from_str(&session_tool_input(&lines, "session-wait").expect("input"))
                .expect("json args");
        assert_eq!(input["session_command"], "pnpm dev");
        // The original arguments survive untouched next to it.
        assert_eq!(input["cell_id"], "22068");
        assert_eq!(input["yield_time_ms"], 30000);
    }

    #[test]
    fn write_stdin_resolves_a_numeric_session_id_from_a_code_mode_result() {
        let lines = session_tool_rollout(
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 1.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": "{\"chunk_id\":\"a74f94\",\"session_id\":7106,\"output\":\"\"}"},
            ]),
            serde_json::json!({
                "type": "function_call",
                "name": "write_stdin",
                "call_id": "call_s",
                "arguments": "{\"session_id\":7106,\"chars\":\"q\\n\"}",
            }),
        );

        let input: serde_json::Value =
            serde_json::from_str(&session_tool_input(&lines, "session-stdin").expect("input"))
                .expect("json args");
        assert_eq!(input["session_command"], "pnpm dev");
    }

    /// A `wait` / `write_stdin` that only collects output is the earlier command
    /// still running — not a second command. Before this, a `cargo test` cut
    /// short by unified-exec's yield showed as two adjacent cards with the same
    /// title, the first holding a few lines of output and the second holding
    /// the rest.
    #[test]
    fn polling_a_background_session_appends_to_the_card_of_its_command() {
        let mut lines = background_session_head(serde_json::json!(
            "Chunk ID: 523e44\nWall time: 30.0 seconds\nProcess running with session ID 22068\nOutput:\n   Compiling codeg"
        ));
        // Nothing new yet, and the session says so by naming itself again.
        lines.extend(poll_lines(
            "call_p1",
            "{\"session_id\":22068,\"chars\":\"\"}",
            serde_json::json!(
                "Chunk ID: 9a1\nWall time: 30.0 seconds\nProcess running with session ID 22068\nOutput:\n"
            ),
        ));
        lines.extend(poll_lines(
            "call_p2",
            "{\"session_id\":22068,\"chars\":\"\"}",
            serde_json::json!(
                "Chunk ID: 9a2\nWall time: 0.1 seconds\nProcess exited with code 0\nOutput:\ntest result: ok. 1 passed"
            ),
        ));

        let detail = parse_lines(&lines, "session-fold");
        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1, "polls must not add cards: {uses:?}");
        assert_eq!(uses[0].1, "exec_command");

        let results = tool_results(&detail);
        assert_eq!(results.len(), 1, "{results:?}");
        let output = results[0].1.clone().expect("output");
        assert_eq!(output, "   Compiling codeg\ntest result: ok. 1 passed");
    }

    /// The chunk envelope in its object form — what a script that prints its
    /// `exec_command` result verbatim leaves behind. Unwrapped to the output it
    /// carries, exactly like the string form's header is stripped, so a folded
    /// session does not read as JSON followed by terminal output.
    #[test]
    fn a_code_mode_exec_result_shows_its_output_not_its_envelope() {
        let mut lines = code_mode_rollout(
            "const r = await tools.exec_command({cmd:\"pnpm dev\"});\ntext(JSON.stringify(r));\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 1.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": "{\"chunk_id\":\"6a10a9\",\"wall_time_seconds\":1.0,\"session_id\":75100,\"original_token_count\":0,\"output\":\"\"}"},
            ]),
        );
        lines.extend(poll_lines(
            "call_p",
            "{\"session_id\":75100,\"chars\":\"\"}",
            serde_json::json!(
                "Chunk ID: 9a2\nWall time: 9.0 seconds\nProcess exited with code 0\nOutput:\n[INFO] Scanning for projects..."
            ),
        ));

        let detail = parse_lines(&lines, "session-envelope");
        let results = tool_results(&detail);
        assert_eq!(results.len(), 1, "{results:?}");
        // The envelope is gone; what the command printed took its place.
        assert_eq!(
            results[0].1.as_deref(),
            Some("[INFO] Scanning for projects...")
        );
    }

    /// Keystrokes are an action, not a poll: they earn a card, and everything
    /// the session prints afterwards belongs below that card rather than folded
    /// back above it.
    #[test]
    fn keystrokes_get_a_card_and_end_the_folding() {
        let mut lines = background_session_head(serde_json::json!(
            "Chunk ID: 523e44\nWall time: 1.0 seconds\nProcess running with session ID 22068\nOutput:\noverwrite? [y/N]"
        ));
        lines.extend(poll_lines(
            "call_k",
            "{\"session_id\":22068,\"chars\":\"y\\n\"}",
            serde_json::json!(
                "Chunk ID: 9a1\nWall time: 1.0 seconds\nProcess running with session ID 22068\nOutput:\nwriting"
            ),
        ));
        lines.extend(poll_lines(
            "call_p",
            "{\"session_id\":22068,\"chars\":\"\"}",
            serde_json::json!(
                "Chunk ID: 9a2\nWall time: 0.1 seconds\nProcess exited with code 0\nOutput:\ndone"
            ),
        ));

        let detail = parse_lines(&lines, "session-keys");
        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 3, "{uses:?}");
        assert_eq!(uses[1].0, "call_k");
        assert_eq!(uses[2].0, "call_p");

        let results = tool_results(&detail);
        let origin = results
            .iter()
            .find(|(id, _, _)| id == "call_e")
            .expect("origin result");
        assert_eq!(origin.1.as_deref(), Some("overwrite? [y/N]"));
    }

    /// A command's own JSON stdout may well carry `chunk_id` and `output`.
    /// Truncating it to the `output` field would silently drop the rest, so the
    /// whole envelope shape has to be there before anything is unwrapped.
    #[test]
    fn json_stdout_that_only_resembles_an_envelope_is_left_whole() {
        let payload =
            r#"{"chunk_id":"artifact-7","output":"ok","checksum":"abc","files":["a.rs"]}"#;
        let lines = code_mode_rollout(
            "const r = await tools.exec_command({cmd: \"node build.js\"});\ntext(r.output);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 1.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": payload},
            ]),
        );

        let detail = parse_lines(&lines, "envelope-lookalike");
        assert_eq!(
            tool_results(&detail)
                .first()
                .and_then(|(_, out, _)| out.clone())
                .as_deref(),
            Some(payload)
        );
    }

    /// One `tools.exec_command` call site driven by a loop is many commands. The
    /// first command literal in the source is then just a guess, and a session
    /// some later command started would be titled — and folded — under it.
    #[test]
    fn a_looped_call_site_attributes_no_session() {
        let mut lines = code_mode_rollout(
            "const cmds = [\"pnpm build\", \"pnpm dev\"];\nfor (const cmd of cmds) {\n  const r = await tools.exec_command({cmd});\n  text(JSON.stringify(r));\n}\n",
            // Two commands ran; the second announced session 4242.
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 1.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": "{\"chunk_id\":\"a1\",\"wall_time_seconds\":1.0,\"original_token_count\":0,\"exit_code\":0,\"output\":\"built\"}"},
                {"type": "input_text", "text": "{\"chunk_id\":\"b2\",\"wall_time_seconds\":1.0,\"original_token_count\":0,\"session_id\":4242,\"output\":\"\"}"},
            ]),
        );
        lines.extend(poll_lines(
            "call_p",
            "{\"session_id\":4242,\"chars\":\"\"}",
            serde_json::json!(
                "Chunk ID: 9a2\nWall time: 1.0 seconds\nProcess exited with code 0\nOutput:\nready"
            ),
        ));

        let detail = parse_lines(&lines, "looped-site");
        let poll = tool_uses(&detail)
            .into_iter()
            .find(|(id, _, _)| id == "call_p")
            .expect("the poll keeps its card");
        let input = poll.2.expect("input");
        assert!(
            !input.contains("session_command"),
            "session 4242 was started by one of two commands — attributing it to \
             either is a guess: {input}"
        );
    }

    /// The numeric session id and the hex chunk id name the same cell. Once a
    /// keystroke earns a card, a poll arriving through the *other* spelling must
    /// not still fold its output in above that card.
    #[test]
    fn an_action_ends_the_folding_for_every_alias_of_its_session() {
        let mut lines = background_session_head(serde_json::json!(
            "Chunk ID: 523e44\nWall time: 1.0 seconds\nProcess running with session ID 22068\nOutput:\noverwrite? [y/N]"
        ));
        lines.extend(poll_lines(
            "call_k",
            "{\"session_id\":22068,\"chars\":\"y\\n\"}",
            serde_json::json!(
                "Chunk ID: 9a1\nWall time: 1.0 seconds\nProcess running with session ID 22068\nOutput:\nwriting"
            ),
        ));
        // Same cell, addressed by the chunk id of the announcing envelope.
        lines.extend(poll_lines(
            "call_p",
            "{\"session_id\":\"523e44\",\"chars\":\"\"}",
            serde_json::json!(
                "Chunk ID: 9a2\nWall time: 0.1 seconds\nProcess exited with code 0\nOutput:\ndone"
            ),
        ));

        let detail = parse_lines(&lines, "session-alias-boundary");
        let results = tool_results(&detail);
        let origin = results
            .iter()
            .find(|(id, _, _)| id == "call_e")
            .expect("origin result");
        assert_eq!(origin.1.as_deref(), Some("overwrite? [y/N]"));
        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 3, "the alias poll keeps its own card: {uses:?}");
    }

    /// codex names sessions after pids, and pids come back around. Whichever
    /// command announced an id last owns it: its polls must land on its own
    /// card, never on the card of the command that held the id before.
    #[test]
    fn a_reused_session_id_binds_to_whoever_announced_it_last() {
        let mut lines = background_session_head(serde_json::json!(
            "Chunk ID: 523e44\nWall time: 1.0 seconds\nProcess running with session ID 22068\nOutput:\nstarting"
        ));
        lines.push(rollout_line(
            "2026-07-20T08:41:00Z",
            "response_item",
            serde_json::json!({
                "type": "function_call",
                "name": "exec_command",
                "call_id": "call_e2",
                "arguments": "{\"cmd\":\"cargo watch\",\"yield_time_ms\":1000}",
            }),
        ));
        lines.push(rollout_line(
            "2026-07-20T08:41:01Z",
            "response_item",
            serde_json::json!({
                "type": "function_call_output",
                "call_id": "call_e2",
                "output": "Chunk ID: 7c11\nWall time: 1.0 seconds\nProcess running with session ID 22068\nOutput:\nwatching",
            }),
        ));
        lines.extend(poll_lines(
            "call_p",
            "{\"session_id\":22068,\"chars\":\"\"}",
            serde_json::json!(
                "Chunk ID: 9a2\nWall time: 0.1 seconds\nProcess exited with code 0\nOutput:\nrebuilt"
            ),
        ));

        let detail = parse_lines(&lines, "session-reused");
        let results = tool_results(&detail);
        let first = results
            .iter()
            .find(|(id, _, _)| id == "call_e")
            .expect("first result");
        let second = results
            .iter()
            .find(|(id, _, _)| id == "call_e2")
            .expect("second result");
        assert_eq!(first.1.as_deref(), Some("starting"));
        assert_eq!(second.1.as_deref(), Some("watching\nrebuilt"));
    }

    #[test]
    fn an_unannounced_session_leaves_the_arguments_alone() {
        // The command ran to completion — it never announced a session — so
        // there is nothing to attribute this wait to. Inventing a command here
        // would be worse than the bare id the card falls back to.
        let lines = session_tool_rollout(
            serde_json::json!("Chunk ID: 523e44\nWall time: 1.0 seconds\nOutput:\ndone"),
            serde_json::json!({
                "type": "function_call",
                "name": "wait",
                "call_id": "call_s",
                "arguments": "{\"cell_id\":\"999\",\"yield_time_ms\":30000}",
            }),
        );

        let input = session_tool_input(&lines, "session-unknown").expect("input");
        assert!(
            !input.contains("session_command"),
            "unexpected attribution: {input}"
        );
    }

    /// A code-mode script that answers `Script running with cell ID N` has not
    /// finished — the `wait` that later collects cell N carries that script's
    /// own return value. It belongs on the script's card, not on a card of its
    /// own: before this, the delegation-status card showed only "Script running
    /// with cell ID 35" while its actual `{"tasks":[…]}` sat in a separate
    /// terminal card further down the transcript.
    fn deferred_script_rollout(wait_output: serde_json::Value) -> Vec<String> {
        let mut lines = code_mode_rollout(
            "const r = await tools.mcp__codeg_mcp__get_delegation_status({task_ids:[\"t1\"],wait_ms:60000});\ntext(JSON.stringify(r));\n",
            serde_json::json!("Script running with cell ID 34\nWall time 11.0 seconds\nOutput:\n"),
        );
        lines.push(rollout_line(
            "2026-07-20T08:40:03Z",
            "response_item",
            serde_json::json!({
                "type": "function_call",
                "name": "wait",
                "call_id": "call_w",
                "arguments": "{\"cell_id\":\"34\",\"yield_time_ms\":60000}",
            }),
        ));
        lines.push(rollout_line(
            "2026-07-20T08:40:48Z",
            "response_item",
            serde_json::json!({
                "type": "function_call_output",
                "call_id": "call_w",
                "output": wait_output,
            }),
        ));
        lines
    }

    #[test]
    fn a_deferred_scripts_result_lands_on_its_own_card() {
        let lines = deferred_script_rollout(serde_json::json!([
            {"type": "input_text", "text": "Script completed\nWall time 44.5 seconds\nOutput:\n"},
            {"type": "input_text", "text": "{\"tasks\":[{\"task_id\":\"t1\",\"child_conversation_id\":2890}]}"},
        ]));
        let path = write_temp_rollout("deferred-script", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "deferred-script")
            .expect("parse ok");

        // One card for the call, carrying the result the `wait` collected.
        assert_eq!(
            tool_uses(&detail)
                .into_iter()
                .map(|(id, name, _)| (id, name))
                .collect::<Vec<_>>(),
            vec![(
                "call_1".to_string(),
                "mcp__codeg_mcp__get_delegation_status".to_string()
            )]
        );
        assert_eq!(
            tool_results(&detail),
            vec![(
                "call_1".to_string(),
                Some(
                    "{\"tasks\":[{\"task_id\":\"t1\",\"child_conversation_id\":2890}]}".to_string()
                ),
                false
            )]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_script_still_running_after_a_wait_stays_collectable() {
        // The wait came back with the script STILL parked: the card keeps the
        // running note, and the next wait must still find it.
        let lines = deferred_script_rollout(serde_json::json!(
            "Script running with cell ID 34\nWall time 60.0 seconds\nOutput:\n"
        ));
        let path = write_temp_rollout("deferred-still-running", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "deferred-still-running")
            .expect("parse ok");

        assert_eq!(tool_uses(&detail).len(), 1);
        assert_eq!(
            tool_results(&detail),
            vec![(
                "call_1".to_string(),
                Some("Script running with cell ID 34".to_string()),
                false
            )]
        );

        let _ = fs::remove_file(path);
    }

    /// A `wait` answers with what the script printed SINCE it parked, not with
    /// the whole run. Rewriting the cards from that answer alone would drop
    /// whatever the script had already printed — and decompose against a chunk
    /// count missing its front, so a two-call script could end up undecomposable
    /// and leave its `call_1#i` cards without results.
    #[test]
    fn a_deferred_script_keeps_what_it_printed_before_the_wait() {
        let mut lines = code_mode_rollout(
            "const a = await tools.exec_command({cmd: \"pnpm build\"});\ntext(a.output);\nconst b = await tools.exec_command({cmd: \"pnpm test\"});\ntext(b.output);\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script running with cell ID 34\nWall time 11.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": "build finished"},
            ]),
        );
        lines.push(rollout_line(
            "2026-07-20T08:40:03Z",
            "response_item",
            serde_json::json!({
                "type": "function_call",
                "name": "wait",
                "call_id": "call_w",
                "arguments": "{\"cell_id\":\"34\",\"yield_time_ms\":60000}",
            }),
        ));
        lines.push(rollout_line(
            "2026-07-20T08:40:48Z",
            "response_item",
            serde_json::json!({
                "type": "function_call_output",
                "call_id": "call_w",
                "output": [
                    {"type": "input_text", "text": "Script completed\nWall time 45.0 seconds\nOutput:\n"},
                    {"type": "input_text", "text": "1 passed"},
                ],
            }),
        ));

        let detail = parse_lines(&lines, "deferred-accumulates");
        // Two chunks across the two answers, two calls: decomposed per call.
        assert_eq!(
            tool_uses(&detail)
                .iter()
                .map(|(id, name, _)| (id.as_str(), name.as_str()))
                .collect::<Vec<_>>(),
            vec![("call_1#0", "exec_command"), ("call_1#1", "exec_command")]
        );
        assert_eq!(
            tool_results(&detail)
                .iter()
                .map(|(id, out, _)| (id.as_str(), out.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("call_1#0", Some("build finished")),
                ("call_1#1", Some("1 passed")),
            ]
        );
    }

    #[test]
    fn a_script_cell_is_never_mistaken_for_a_shell_session() {
        // `Script running with cell ID N` names the SCRIPT, not a shell inside
        // it — so the wait that collects it folds into the script's card and
        // contributes no card of its own. Nothing may leak into the shell
        // session map either: that cell is not a shell, and the long-running
        // call here is not even a command.
        let mut lines = code_mode_rollout(
            "const r = await tools.mcp__codeg_mcp__ask_user_question({questions: []});\ntext(JSON.stringify(r));\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script running with cell ID 15\nWall time 11.0 seconds\nOutput:\n"},
            ]),
        );
        lines.push(rollout_line(
            "2026-07-20T08:40:03Z",
            "response_item",
            serde_json::json!({
                "type": "function_call",
                "name": "wait",
                "call_id": "call_s",
                "arguments": "{\"cell_id\":\"15\"}",
            }),
        ));

        let path = write_temp_rollout("session-script-cell", &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, "session-script-cell")
            .expect("parse ok");
        let uses = tool_uses(&detail);
        assert_eq!(
            uses.iter()
                .map(|(id, name, _)| (id.as_str(), name.as_str()))
                .collect::<Vec<_>>(),
            vec![("call_1", "mcp__codeg_mcp__ask_user_question")]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn code_mode_session_announcements_are_attributed_per_chunk() {
        // Two commands in one script, one `text()` chunk each: the second
        // command's session id must resolve to the SECOND command.
        let mut lines = code_mode_rollout(
            "const a = await tools.exec_command({cmd: \"pnpm dev\"});\ntext(JSON.stringify(a));\nconst b = await tools.exec_command({cmd: \"cargo watch\"});\ntext(JSON.stringify(b));\n",
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 1.0 seconds\nOutput:\n"},
                {"type": "input_text", "text": "{\"chunk_id\":\"a1\",\"session_id\":11,\"output\":\"\"}"},
                {"type": "input_text", "text": "{\"chunk_id\":\"b2\",\"session_id\":22,\"output\":\"\"}"},
            ]),
        );
        lines.push(rollout_line(
            "2026-07-20T08:40:03Z",
            "response_item",
            serde_json::json!({
                "type": "function_call",
                "name": "wait",
                // `terminate` keeps it a card of its own — a bare poll would be
                // folded into the command's card and have no title to check.
                "call_id": "call_s",
                "arguments": "{\"cell_id\":\"22\",\"terminate\":true}",
            }),
        ));

        let input: serde_json::Value =
            serde_json::from_str(&session_tool_input(&lines, "session-chunked").expect("input"))
                .expect("json args");
        assert_eq!(input["session_command"], "cargo watch");
    }

    // ── separator split ──────────────────────────────────────────────────

    /// The script from the reported session: a literal table of labelled
    /// commands fanned out through one `tools.exec_command({cmd, …})`.
    fn labelled_fanout(labels: &[&str]) -> String {
        let rows: Vec<String> = labels
            .iter()
            .enumerate()
            .map(|(i, label)| format!("  [\"{label}\", \"echo {i}\"],\n"))
            .collect();
        format!(
            "const cmds = [\n{}];\nconst out = await Promise.all(cmds.map(async ([k,cmd]) => [k, await tools.exec_command({{cmd, workdir:\"/repo\", yield_time_ms:10000}})]));\nfor (const [k,r] of out) text(`===== ${{k}} =====\\n${{r.output}}`);\n",
            rows.concat()
        )
    }

    /// What codex renders when a labelled fan-out's `text()` stream is too
    /// long: one blob, the separators still in it, behind a banner declaring
    /// how many lines it started with.
    fn labelled_blob(declared: usize, sections: &[(&str, &str)]) -> String {
        let body: Vec<String> = sections
            .iter()
            .map(|(label, out)| format!("===== {label} =====\n{out}"))
            .collect();
        format!(
            "Warning: truncated output (original token count: 23243)\nTotal output lines: {declared}\n\n{}",
            body.join("\n")
        )
    }

    fn code_mode_detail(script: &str, blob: String, id: &str) -> crate::models::ConversationDetail {
        let lines = code_mode_rollout(
            script,
            serde_json::json!([
                {"type": "input_text", "text": "Script completed\nWall time 0.3 seconds\nOutput:\n"},
                {"type": "input_text", "text": blob},
            ]),
        );
        let path = write_temp_rollout(id, &lines);
        let detail = CodexParser::new()
            .parse_conversation_detail(&path, id)
            .expect("parse ok");
        let _ = fs::remove_file(path);
        detail
    }

    fn tool_metas(detail: &crate::models::ConversationDetail) -> Vec<serde_json::Value> {
        detail
            .turns
            .iter()
            .flat_map(|t| t.blocks.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolUse { meta, .. } => Some(
                    meta.clone()
                        .and_then(|m| m.get("codeg.codexScript").cloned())
                        .unwrap_or(serde_json::Value::Null),
                ),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_labelled_fanout_splits_a_collapsed_blob_per_command() {
        let detail = code_mode_detail(
            &labelled_fanout(&["query-entry", "formula-service", "factor-full"]),
            labelled_blob(6, &[
                ("query-entry", "one"),
                ("formula-service", "two"),
                ("factor-full", "three"),
            ]),
            "code-mode-labelled",
        );

        assert_eq!(
            tool_uses(&detail),
            vec![
                ("call_1#0".into(), "exec_command".into(), Some("echo 0".into())),
                ("call_1#1".into(), "exec_command".into(), Some("echo 1".into())),
                ("call_1#2".into(), "exec_command".into(), Some("echo 2".into())),
            ]
        );
        assert_eq!(
            tool_results(&detail),
            vec![
                ("call_1#0".into(), Some("one".into()), false),
                ("call_1#1".into(), Some("two".into()), false),
                ("call_1#2".into(), Some("three".into()), false),
            ]
        );
        let metas = tool_metas(&detail);
        assert_eq!(metas[0]["label"], "query-entry");
        assert_eq!(metas[2]["label"], "factor-full");
        assert!(metas.iter().all(|m| m.get("outputMissing").is_none()));
    }

    /// The reported card: truncation ate two separators, so the span after
    /// `formula-service` holds three commands' output with no boundary. The
    /// commands it provably brackets still get their own output; the ones whose
    /// separator is gone say so instead of being handed someone else's.
    #[test]
    fn a_truncated_separator_leaves_its_command_without_output() {
        let detail = code_mode_detail(
            &labelled_fanout(&["query-entry", "formula-service", "vo", "formula-splice", "factor-full"]),
            labelled_blob(20, &[
                ("query-entry", "first"),
                ("formula-service", "second\nvo-output\nsplice-output"),
                ("factor-full", "last"),
            ]),
            "code-mode-labelled-partial",
        );

        assert_eq!(
            tool_results(&detail),
            vec![
                ("call_1#0".into(), Some("first".into()), false),
                ("call_1#1".into(), Some("second\nvo-output\nsplice-output".into()), false),
                ("call_1#2".into(), None, false),
                ("call_1#3".into(), None, false),
                ("call_1#4".into(), Some("last".into()), false),
            ]
        );

        let metas = tool_metas(&detail);
        assert_eq!(metas[1]["sharedWith"], serde_json::json!(["vo", "formula-splice"]));
        assert_eq!(metas[2]["outputMissing"], true);
        assert_eq!(metas[3]["outputMissing"], true);
        assert!(metas[0].get("sharedWith").is_none());
        assert!(metas[4].get("sharedWith").is_none());
        assert!(metas.iter().all(|m| m["truncated"] == true));
    }

    /// A command that printed the separator itself makes the line ambiguous;
    /// the whole candidate is refused rather than cutting on the wrong one.
    #[test]
    fn a_repeated_separator_line_keeps_the_script_card() {
        let detail = code_mode_detail(
            &labelled_fanout(&["alpha", "beta"]),
            labelled_blob(8, &[
                ("alpha", "one\n===== beta ====="),
                ("beta", "two"),
            ]),
            "code-mode-labelled-dup",
        );

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, CODEX_SCRIPT_TOOL_NAME);
    }

    /// Only a line some separator actually predicts can make a candidate
    /// ambiguous. Two commands printing the same ordinary line is the normal
    /// case — `index_body_lines` marks it repeated, but nothing looks it up.
    #[test]
    fn a_repeated_output_line_still_splits() {
        let detail = code_mode_detail(
            &labelled_fanout(&["alpha", "beta", "gamma"]),
            labelled_blob(8, &[
                ("alpha", "shared line\none"),
                ("beta", "shared line\ntwo"),
                ("gamma", "three"),
            ]),
            "code-mode-labelled-repeat",
        );

        assert_eq!(
            tool_results(&detail),
            vec![
                ("call_1#0".into(), Some("shared line\none".into()), false),
                ("call_1#1".into(), Some("shared line\ntwo".into()), false),
                ("call_1#2".into(), Some("three".into()), false),
            ]
        );
        // Every line the banner declared is present, so nothing went missing.
        let metas = tool_metas(&detail);
        assert!(metas.iter().all(|m| m.get("truncated").is_none()));
        assert!(metas.iter().all(|m| m.get("outputMissing").is_none()));
    }

    /// One separator divides nothing: it would hand the whole blob to the first
    /// command and leave the rest blank.
    #[test]
    fn a_single_surviving_separator_keeps_the_script_card() {
        let detail = code_mode_detail(
            &labelled_fanout(&["alpha", "beta", "gamma"]),
            labelled_blob(9, &[("beta", "two")]),
            "code-mode-labelled-one",
        );

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, CODEX_SCRIPT_TOOL_NAME);
    }

    /// Separators in the wrong order are not this script's separator run.
    #[test]
    fn separators_out_of_order_keep_the_script_card() {
        let detail = code_mode_detail(
            &labelled_fanout(&["alpha", "beta", "gamma"]),
            labelled_blob(9, &[("gamma", "three"), ("beta", "two"), ("alpha", "one")]),
            "code-mode-labelled-unordered",
        );

        let uses = tool_uses(&detail);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].1, CODEX_SCRIPT_TOOL_NAME);
    }

    /// Scripts that number their results instead of naming them split the same
    /// way — the hole is the loop index.
    #[test]
    fn numbered_separators_split_a_collapsed_blob() {
        let script = concat!(
            "const cmds = [[\"echo one\", 20000], [\"echo two\", 20000]];\n",
            "const rs = await Promise.all(cmds.map(([cmd,max]) => tools.exec_command({cmd, max_output_tokens:max})));\n",
            "rs.forEach((r,i)=>text(`---RESULT ${i+1}---\\n${r.output}`));\n",
        );
        let blob = "Warning: truncated output (original token count: 900)\nTotal output lines: 7\n\n---RESULT 1---\none\n---RESULT 2---\ntwo".to_string();
        let detail = code_mode_detail(script, blob, "code-mode-numbered");

        assert_eq!(
            tool_results(&detail),
            vec![
                ("call_1#0".into(), Some("one".into()), false),
                ("call_1#1".into(), Some("two".into()), false),
            ]
        );
        // No table label to show, so the chip has nothing to say.
        assert!(tool_metas(&detail).iter().all(|m| m.get("label").is_none()));
    }
}
