use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{
    AgentExecutionStats, AgentToolCall, AgentType, ContentBlock, ConversationDetail,
    ConversationSummary, MessageRole, MessageTurn, TurnRole, TurnUsage, UnifiedMessage,
};
use crate::parsers::{
    compute_session_stats, folder_name_from_path, infer_context_window_max_tokens,
    is_safe_subagent_id, merge_context_window_stats, relocate_orphaned_tool_results,
    resolve_patch_line_numbers, structurize_read_tool_output, title_from_user_text, truncate_str,
    AgentParser, ParseError,
};

/// Resolve Kimi Code's data home, honoring `KIMI_CODE_HOME`, else `~/.kimi-code`
/// (mirrors `resolve_codebuddy_config_dir`). The transcript store lives under
/// the `sessions/` subdirectory of this path.
pub(crate) fn resolve_kimi_code_home_dir() -> PathBuf {
    resolve_kimi_code_home_from(std::env::var_os("KIMI_CODE_HOME"), dirs::home_dir())
}

fn resolve_kimi_code_home_from(
    kimi_code_home_env: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    kimi_code_home_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.unwrap_or_default().join(".kimi-code"))
}

/// Kimi Code (Moonshot AI) stores its session transcripts under a
/// **directory-per-session** layout — a third archetype distinct from CodeBuddy
/// (one JSONL file per session) and Hermes (a single SQLite DB):
///
/// ```text
/// $KIMI_CODE_HOME/                 (default ~/.kimi-code)
/// ├── config.toml
/// ├── session_index.jsonl          # {sessionId, sessionDir, workDir} per line
/// └── sessions/
///     └── <workDirKey>/            # bucketed by working directory (wd_<name>_<hash>)
///         └── <sessionId>/
///             ├── state.json        # {title, createdAt, updatedAt, agents, ...}
///             ├── logs/kimi-code.log
///             └── agents/
///                 ├── main/wire.jsonl       # the primary agent event stream
///                 └── agent-<n>/wire.jsonl  # sub-agent streams
/// ```
///
/// `base_dir` points at the `sessions/` directory.
///
/// `wire.jsonl` is an **event-sourcing log** (newline-delimited JSON), NOT an ACP
/// `session/update` stream. Each line has a top-level `type` and a millisecond
/// `time`. The records that carry conversation content are:
///
/// - `turn.prompt` — a user prompt (`input[]` of `{type:"text", text}` parts).
/// - `context.append_loop_event` — the assistant's work, where `event.type` is:
///   - `content.part` with `part.type` `"text"` (assistant message) or `"think"`
///     (reasoning, text under `part.think`),
///   - `tool.call` (`toolCallId` / `name` / `args`),
///   - `tool.result` (`toolCallId` / `result.output` / optional `result.isError`),
///   - `step.begin` / `step.end` (ignored; `step.end.usage` duplicates the
///     adjacent `usage.record`).
/// - `usage.record` — **per-step** token usage (`inputOther` / `output` /
///   `inputCacheRead` / `inputCacheCreation`); a turn's total is the sum of its
///   steps' records.
///
/// `context.append_message` records merely echo the prompt into the model context
/// (and carry `origin.kind == "injection"` system reminders), so they are skipped
/// to avoid duplicate / noise messages. The working directory is recovered from
/// `session_index.jsonl` (state.json has none); the model name from the session's
/// own `logs/kimi-code.log` (the wire only stores the codeg-managed model alias).
pub struct KimiCodeParser {
    base_dir: PathBuf,
}

impl KimiCodeParser {
    pub fn new() -> Self {
        Self {
            base_dir: resolve_kimi_code_home_dir().join("sessions"),
        }
    }

    /// Construct a parser pointed at an explicit `sessions` directory (test
    /// fixtures).
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Load `session_index.jsonl` (sibling of `sessions/`) into a
    /// `sessionId → workDir` map. The index is the only source of a session's
    /// working directory (state.json does not record one). A missing or
    /// malformed index degrades to an empty map (cwd unknown).
    fn load_work_dir_index(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        let Some(home) = self.base_dir.parent() else {
            return map;
        };
        let Ok(file) = fs::File::open(home.join("session_index.jsonl")) else {
            return map;
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let session_id = value.get("sessionId").and_then(Value::as_str);
            let work_dir = value
                .get("workDir")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let (Some(id), Some(dir)) = (session_id, work_dir) {
                map.insert(id.to_string(), dir.to_string());
            }
        }
        map
    }

    fn build_summary(
        &self,
        session_dir: &Path,
        session_id: &str,
        cwd: Option<String>,
    ) -> Option<ConversationSummary> {
        // The list view never renders sub-agent stats, so pass `None` to skip the
        // per-session sub-agent transcript I/O — only `build_detail` loads them.
        let parsed = parse_wire(&main_wire_path(session_dir), None);
        // A session that never produced a user/assistant/tool event (only the
        // metadata + system-prompt config records) is treated as empty, matching
        // the "metadata-only is not listed" rule of the other parsers.
        if parsed.content_events == 0 {
            return None;
        }
        let started_at = parsed.first_ts?;

        let model = read_session_log_model(session_dir).or_else(|| parsed.model_alias.clone());
        let folder_name = cwd
            .as_deref()
            .map(folder_name_from_path)
            .or_else(|| decode_work_dir_name(session_dir));

        Some(ConversationSummary {
            id: session_id.to_string(),
            agent_type: AgentType::KimiCode,
            folder_path: cwd,
            folder_name,
            title: resolve_title(read_state_title(session_dir), parsed.first_user_text),
            started_at,
            ended_at: parsed.last_ts,
            message_count: parsed.message_count,
            model,
            git_branch: None,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        })
    }

    fn build_detail(
        &self,
        session_dir: &Path,
        conversation_id: &str,
        cwd: Option<String>,
    ) -> ConversationDetail {
        // `agents/` holds both the main wire and each sub-agent's wire, so an
        // `Agent` delegation result can load its sub-agent transcript from here.
        let parsed = parse_wire(
            &main_wire_path(session_dir),
            Some(&session_dir.join("agents")),
        );

        let mut turns = group_into_turns(parsed.messages);
        relocate_orphaned_tool_results(&mut turns);
        structurize_read_tool_output(&mut turns);
        resolve_patch_line_numbers(&mut turns, cwd.as_deref());

        let model = read_session_log_model(session_dir).or_else(|| parsed.model_alias.clone());
        // Context-window occupancy is the LATEST step's snapshot, never the
        // per-turn sum (which re-counts the cached prefix once per step).
        let used_tokens = parsed
            .last_step_usage
            .as_ref()
            .and_then(kimi_context_window_used_tokens_from_usage);
        let max_tokens = infer_context_window_max_tokens(model.as_deref());
        let session_stats =
            merge_context_window_stats(compute_session_stats(&turns), used_tokens, max_tokens);

        let folder_name = cwd
            .as_deref()
            .map(folder_name_from_path)
            .or_else(|| decode_work_dir_name(session_dir));

        let summary = ConversationSummary {
            id: conversation_id.to_string(),
            agent_type: AgentType::KimiCode,
            folder_path: cwd,
            folder_name,
            title: resolve_title(read_state_title(session_dir), parsed.first_user_text),
            started_at: parsed.first_ts.unwrap_or_else(Utc::now),
            ended_at: parsed.last_ts,
            message_count: parsed.message_count,
            model,
            git_branch: None,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        };

        ConversationDetail {
            summary,
            turns,
            session_stats,
            transcript_watermark: None,
        }
    }

    /// Locate the `<sessionId>` directory matching `conversation_id` across the
    /// `base_dir/<workDirKey>/` buckets (two shallow levels; never descends into
    /// `agents/`).
    fn find_session_dir(&self, conversation_id: &str) -> Option<PathBuf> {
        for bucket in read_subdirs(&self.base_dir) {
            let candidate = bucket.join(conversation_id);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        None
    }
}

impl Default for KimiCodeParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentParser for KimiCodeParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let mut conversations = Vec::new();
        if !self.base_dir.is_dir() {
            return Ok(conversations);
        }
        let index = self.load_work_dir_index();

        for bucket in read_subdirs(&self.base_dir) {
            for session_dir in read_subdirs(&bucket) {
                let Some(session_id) = session_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                else {
                    continue;
                };
                let cwd = index.get(&session_id).cloned();
                if let Some(summary) = self.build_summary(&session_dir, &session_id, cwd) {
                    conversations.push(summary);
                }
            }
        }

        conversations.sort_by_key(|c| std::cmp::Reverse(c.started_at));
        Ok(conversations)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        let Some(session_dir) = self.find_session_dir(conversation_id) else {
            return Err(ParseError::ConversationNotFound(
                conversation_id.to_string(),
            ));
        };
        let cwd = self.load_work_dir_index().get(conversation_id).cloned();
        Ok(self.build_detail(&session_dir, conversation_id, cwd))
    }
}

/// The accumulated result of scanning one agent's `wire.jsonl`.
#[derive(Default)]
struct WireParse {
    messages: Vec<UnifiedMessage>,
    first_ts: Option<DateTime<Utc>>,
    last_ts: Option<DateTime<Utc>>,
    /// The codeg-managed model alias from a `config.update` record (fallback only;
    /// the real model name is recovered from the session log).
    model_alias: Option<String>,
    /// First user prompt, already truncated for use as a fallback title.
    first_user_text: Option<String>,
    /// User + assistant-text messages (tool calls/results and thinking excluded),
    /// a coarse activity count for the list view.
    message_count: u32,
    /// Number of content-bearing records — used to decide whether the session is
    /// worth listing at all.
    content_events: u32,
    /// The most recent *single* `usage.record` snapshot — NOT the per-turn sum
    /// that lands on the messages' `usage`. Kimi emits one usage record per step,
    /// and every step's `inputCacheRead` re-reads the same growing context prefix,
    /// so summing a multi-step turn's records over-counts the cached context many
    /// times over. The context window's current occupancy is the input side of the
    /// latest step alone (see `kimi_context_window_used_tokens_from_usage`).
    last_step_usage: Option<TurnUsage>,
}

fn main_wire_path(session_dir: &Path) -> PathBuf {
    session_dir.join("agents").join("main").join("wire.jsonl")
}

/// Parse a `wire.jsonl` event stream into a flat, chronologically-ordered list of
/// `UnifiedMessage`s plus session metadata. Unknown / malformed lines are skipped
/// (`continue`) so a forward-compatible or partially-written log never panics.
///
/// When `agents_dir` is `Some` (the conversation-detail path), an `Agent`
/// delegation's tool result loads the sub-agent's own `wire.jsonl` from
/// `<agents_dir>/<agent_id>/` and attaches its nested tool calls as `agent_stats`
/// so the sub-agent renders as an expandable Agent pill. `None` (the list path)
/// skips that per-session I/O entirely.
fn parse_wire(path: &Path, agents_dir: Option<&Path>) -> WireParse {
    let mut wp = WireParse::default();
    let Ok(file) = fs::File::open(path) else {
        return wp;
    };

    // Per-step `usage.record`s accumulate into the turn's total, then flush onto
    // the turn's last assistant message at the next `turn.prompt` (or EOF).
    let mut pending_usage: Option<TurnUsage> = None;
    let mut last_assistant_idx: Option<usize> = None;
    // `toolCallId`s of `tool.call`s classified as `Agent` delegations. Only their
    // paired results may load a sub-agent transcript, so an ordinary tool result
    // can never gain `agent_stats` (mirrors CodeBuddy's `agent_call_ids` gate).
    let mut agent_call_ids: HashSet<String> = HashSet::new();

    for (idx, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let record_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        let ts_raw = event_millis(&value);

        match record_type {
            "config.update" => {
                if let Some(alias) = value
                    .get("modelAlias")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    wp.model_alias.get_or_insert_with(|| alias.to_string());
                }
            }
            "turn.prompt" => {
                flush_usage(&mut wp.messages, &mut pending_usage, &mut last_assistant_idx);
                let text = collect_prompt_text(&value);
                if text.trim().is_empty() {
                    continue;
                }
                let ts = note_content_ts(&mut wp, ts_raw);
                if wp.first_user_text.is_none() {
                    wp.first_user_text = Some(title_from_user_text(text.trim()));
                }
                wp.content_events += 1;
                wp.message_count += 1;
                wp.messages.push(text_message(
                    format!("kc-user-{idx}"),
                    MessageRole::User,
                    text,
                    ts,
                ));
            }
            "context.append_loop_event" => {
                let Some(event) = value.get("event") else {
                    continue;
                };
                let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
                match event_type {
                    "content.part" => {
                        let part = event.get("part").cloned().unwrap_or(Value::Null);
                        match part.get("type").and_then(Value::as_str).unwrap_or("") {
                            "text" => {
                                let text = part_text(&part, "text");
                                if text.trim().is_empty() {
                                    continue;
                                }
                                let ts = note_content_ts(&mut wp, ts_raw);
                                wp.content_events += 1;
                                wp.message_count += 1;
                                wp.messages.push(text_message(
                                    format!("kc-text-{idx}"),
                                    MessageRole::Assistant,
                                    text,
                                    ts,
                                ));
                                last_assistant_idx = Some(wp.messages.len() - 1);
                            }
                            "think" => {
                                let text = part_text(&part, "think");
                                if text.trim().is_empty() {
                                    continue;
                                }
                                let ts = note_content_ts(&mut wp, ts_raw);
                                wp.content_events += 1;
                                wp.messages.push(block_message(
                                    format!("kc-think-{idx}"),
                                    MessageRole::Assistant,
                                    ContentBlock::Thinking { text },
                                    ts,
                                ));
                                last_assistant_idx = Some(wp.messages.len() - 1);
                            }
                            _ => {}
                        }
                    }
                    "tool.call" => {
                        let ts = note_content_ts(&mut wp, ts_raw);
                        wp.content_events += 1;
                        let tool_call_id = event
                            .get("toolCallId")
                            .and_then(Value::as_str)
                            .map(String::from);
                        // Record `Agent` delegations so only their paired results
                        // load a sub-agent transcript (the gate is applied below).
                        if is_agent_tool_call(event) {
                            if let Some(id) = &tool_call_id {
                                agent_call_ids.insert(id.clone());
                            }
                        }
                        wp.messages.push(block_message(
                            format!("kc-toolcall-{idx}"),
                            MessageRole::Assistant,
                            ContentBlock::ToolUse {
                                tool_use_id: tool_call_id,
                                tool_name: event
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown")
                                    .to_string(),
                                input_preview: tool_args_preview(event),
                                meta: None,
                            },
                            ts,
                        ));
                        last_assistant_idx = Some(wp.messages.len() - 1);
                    }
                    "tool.result" => {
                        let ts = note_content_ts(&mut wp, ts_raw);
                        wp.content_events += 1;
                        let result = event.get("result");
                        let tool_call_id = event
                            .get("toolCallId")
                            .and_then(Value::as_str)
                            .map(String::from);
                        let output_preview = result.and_then(tool_result_preview);
                        // Load the sub-agent transcript only for a result paired
                        // (by `toolCallId`) to a `tool.call` classified as an
                        // `Agent` delegation. Every ordinary result stays `None`,
                        // even one whose output coincidentally opens with an
                        // `agent_id:` line — the gate is the call classification,
                        // not the marker's presence.
                        let agent_stats = agents_dir
                            .filter(|_| {
                                tool_call_id
                                    .as_deref()
                                    .is_some_and(|id| agent_call_ids.contains(id))
                            })
                            .and_then(|dir| {
                                agent_stats_from_subagent(output_preview.as_deref(), dir)
                            });
                        wp.messages.push(block_message(
                            format!("kc-toolresult-{idx}"),
                            MessageRole::Tool,
                            ContentBlock::ToolResult {
                                tool_use_id: tool_call_id,
                                output_preview,
                                is_error: result
                                    .and_then(|r| r.get("isError"))
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                                agent_stats,
                                // Kimi tool results are text/JSON today; image
                                // capture (cf. main's tool-result image support)
                                // is a follow-up that needs a real image sample.
                                images: Vec::new(),
                            },
                            ts,
                        ));
                    }
                    _ => {} // step.begin / step.end carry no renderable content
                }
            }
            "usage.record" => {
                if let Some(usage) = usage_from_record(value.get("usage")) {
                    // Snapshot the latest step for the context-window occupancy
                    // (see `WireParse::last_step_usage`), then fold it into the
                    // per-turn sum that feeds the cumulative usage meter.
                    wp.last_step_usage = Some(usage.clone());
                    pending_usage = Some(match pending_usage.take() {
                        Some(prev) => add_usage(prev, usage),
                        None => usage,
                    });
                }
            }
            _ => {}
        }
    }

    flush_usage(&mut wp.messages, &mut pending_usage, &mut last_assistant_idx);
    wp
}

/// Record a content event's timestamp into the session span and return a concrete
/// timestamp for the message (falling back to the last seen one, then now).
fn note_content_ts(wp: &mut WireParse, ts_raw: Option<DateTime<Utc>>) -> DateTime<Utc> {
    if let Some(ts) = ts_raw {
        wp.first_ts.get_or_insert(ts);
        wp.last_ts = Some(ts);
    }
    ts_raw.or(wp.last_ts).unwrap_or_else(Utc::now)
}

/// Attach the accumulated per-turn usage to the turn's last assistant message and
/// reset the accumulator for the next turn.
fn flush_usage(
    messages: &mut [UnifiedMessage],
    pending: &mut Option<TurnUsage>,
    last_assistant_idx: &mut Option<usize>,
) {
    if let (Some(usage), Some(i)) = (pending.take(), *last_assistant_idx) {
        if let Some(message) = messages.get_mut(i) {
            message.usage = Some(match message.usage.take() {
                Some(existing) => add_usage(existing, usage),
                None => usage,
            });
        }
    }
    *last_assistant_idx = None;
}

/// Top-level millisecond `time` → `DateTime<Utc>` (Kimi uses numeric epoch ms).
fn event_millis(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(value.get("time")?.as_i64()?)
}

/// Concatenate the `text` of every `{type:"text"}` part in a `turn.prompt.input`.
fn collect_prompt_text(value: &Value) -> String {
    let mut out = String::new();
    if let Some(items) = value.get("input").and_then(Value::as_array) {
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                }
            }
        }
    }
    out
}

/// Pull a string field (`text` or `think`) out of a `content.part`.
fn part_text(part: &Value, key: &str) -> String {
    part.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `tool.call.args` is an object (e.g. `{command, cwd, timeout}`); serialize it
/// for the input preview, defensively accepting a pre-stringified value.
fn tool_args_preview(event: &Value) -> Option<String> {
    let args = event.get("args")?;
    if let Some(text) = args.as_str() {
        (!text.is_empty()).then(|| text.to_string())
    } else if args.is_null() {
        None
    } else {
        serde_json::to_string(args).ok()
    }
}

/// `tool.result.result.output` is usually a string; rich outputs (e.g. images)
/// arrive as an array/object, which is serialized as a fallback.
fn tool_result_preview(result: &Value) -> Option<String> {
    let output = result.get("output")?;
    if let Some(text) = output.as_str() {
        (!text.is_empty()).then(|| text.to_string())
    } else if output.is_null() {
        None
    } else {
        serde_json::to_string(output).ok()
    }
}

/// True when a `tool.call` event is an `Agent` sub-agent delegation — its `name`
/// is `"Agent"`, or (defensively) its `args` carry a non-empty `subagent_type`.
/// Only such calls' paired results may load a sub-agent transcript.
fn is_agent_tool_call(event: &Value) -> bool {
    if event.get("name").and_then(Value::as_str) == Some("Agent") {
        return true;
    }
    event
        .get("args")
        .and_then(|args| args.get("subagent_type"))
        .and_then(Value::as_str)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// The sub-agent transcript id Kimi writes at the head of an `Agent` tool
/// result's output: the first line is `agent_id: agent-0` (followed by
/// `actual_subagent_type:` / `status:` / a blank line / the summary). Only the
/// first line is inspected, so an `agent_id:` substring appearing inside the
/// summary body can never be mistaken for the marker. Returns `None` for an
/// ordinary result whose output carries no such header.
fn subagent_id_from_output(output: &str) -> Option<&str> {
    output
        .lines()
        .next()?
        .trim()
        .strip_prefix("agent_id:")
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

/// Walk a sub-agent's `wire.jsonl` — the same event-sourcing format as the main
/// wire — and extract its tool calls as `AgentToolCall`s, pairing each
/// `tool.call` with its `tool.result` by `toolCallId`. The outer
/// `tool_args_preview` / `tool_result_preview` helpers are reused so nested calls
/// render identically to top-level ones. Mirrors `codebuddy.rs`'s
/// `parse_codebuddy_subagent_tool_calls`.
///
/// Intentionally non-recursive: a nested `Agent` call inside the sub-agent shows
/// as a flat leaf tool here (no further descent), which bounds the work and
/// matches the frontend stripping `agent_stats` from nested renders.
fn parse_kimi_subagent_tool_calls(path: &Path) -> Vec<AgentToolCall> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };

    // (toolCallId, name, input) in encounter order, paired against results by id.
    let mut calls: Vec<(Option<String>, String, Option<String>)> = Vec::new();
    let mut results: HashMap<String, (Option<String>, bool)> = HashMap::new();

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("context.append_loop_event") {
            continue;
        }
        let Some(event) = value.get("event") else {
            continue;
        };
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "tool.call" => {
                calls.push((
                    event
                        .get("toolCallId")
                        .and_then(Value::as_str)
                        .map(String::from),
                    event
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    tool_args_preview(event).map(|s| truncate_str(&s, 500)),
                ));
            }
            "tool.result" => {
                if let Some(id) = event.get("toolCallId").and_then(Value::as_str) {
                    let result = event.get("result");
                    let output =
                        result.and_then(tool_result_preview).map(|s| truncate_str(&s, 500));
                    let is_error = result
                        .and_then(|r| r.get("isError"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    results.insert(id.to_string(), (output, is_error));
                }
            }
            _ => {}
        }
    }

    calls
        .into_iter()
        .map(|(id, tool_name, input_preview)| {
            let (output_preview, is_error) =
                id.and_then(|i| results.remove(&i)).unwrap_or((None, false));
            AgentToolCall {
                tool_name,
                input_preview,
                output_preview,
                is_error,
            }
        })
        .collect()
}

/// Build `agent_stats` for an `Agent` tool result by reading the sub-agent id
/// from its output header (`subagent_id_from_output`) and loading the sub-agent's
/// own `wire.jsonl` from `<agents_dir>/<agent_id>/`. The historical mirror of the
/// live path, which synthesizes the same `agent_stats` from the streamed child
/// tool calls.
///
/// Returns `None` for an ordinary result (no `agent_id:` header), an unsafe id, a
/// missing transcript, or a sub-agent that ran no tools — so the common case
/// stays a plain tool result.
fn agent_stats_from_subagent(
    output: Option<&str>,
    agents_dir: &Path,
) -> Option<AgentExecutionStats> {
    let id = subagent_id_from_output(output?)?;
    // `id` becomes a path component under `agents_dir`; reject anything that
    // could escape the directory before a file is opened.
    if !is_safe_subagent_id(id) {
        return None;
    }
    let transcript = agents_dir.join(id).join("wire.jsonl");
    if !transcript.exists() {
        return None;
    }
    let tool_calls = parse_kimi_subagent_tool_calls(&transcript);
    if tool_calls.is_empty() {
        return None;
    }
    let tool_count = tool_calls.len() as u32;
    Some(AgentExecutionStats {
        agent_type: None,
        status: None,
        total_duration_ms: None,
        total_tokens: None,
        total_tool_use_count: Some(tool_count),
        read_count: None,
        search_count: None,
        bash_count: None,
        edit_file_count: None,
        lines_added: None,
        lines_removed: None,
        other_tool_count: None,
        tool_calls,
    })
}

/// Map a `usage.record.usage` object onto `TurnUsage`; `None` when all counters
/// are absent or zero so empty records do not create spurious usage.
fn usage_from_record(usage: Option<&Value>) -> Option<TurnUsage> {
    let usage = usage?;
    let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let input = field("inputOther");
    let output = field("output");
    let cache_read = field("inputCacheRead");
    let cache_creation = field("inputCacheCreation");
    if input == 0 && output == 0 && cache_read == 0 && cache_creation == 0 {
        return None;
    }
    Some(TurnUsage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
    })
}

fn add_usage(a: TurnUsage, b: TurnUsage) -> TurnUsage {
    TurnUsage {
        input_tokens: a.input_tokens.saturating_add(b.input_tokens),
        output_tokens: a.output_tokens.saturating_add(b.output_tokens),
        cache_creation_input_tokens: a
            .cache_creation_input_tokens
            .saturating_add(b.cache_creation_input_tokens),
        cache_read_input_tokens: a
            .cache_read_input_tokens
            .saturating_add(b.cache_read_input_tokens),
    }
}

/// The context-window *occupancy* implied by a single `usage.record`: the input
/// side of that one request (`inputOther + inputCacheRead + inputCacheCreation`).
/// Output tokens are excluded — they are the model's reply, not context the
/// request occupied — mirroring the Claude parser's occupancy formula.
///
/// This must be fed the LATEST step's record, never the per-turn sum: Kimi emits
/// one record per step and each step's `inputCacheRead` re-reads the same growing
/// prefix, so summing a multi-step turn's records over-counts the cached context
/// many times over (a 26-step turn reported ~840K "used" against a 262K window — a
/// false 100%). The final step's input side is the true current occupancy.
fn kimi_context_window_used_tokens_from_usage(usage: &TurnUsage) -> Option<u64> {
    let used = usage
        .input_tokens
        .saturating_add(usage.cache_creation_input_tokens)
        .saturating_add(usage.cache_read_input_tokens);
    (used > 0).then_some(used)
}

fn text_message(
    id: String,
    role: MessageRole,
    text: String,
    ts: DateTime<Utc>,
) -> UnifiedMessage {
    block_message(id, role, ContentBlock::Text { text }, ts)
}

fn block_message(
    id: String,
    role: MessageRole,
    block: ContentBlock,
    ts: DateTime<Utc>,
) -> UnifiedMessage {
    UnifiedMessage {
        id,
        role,
        content: vec![block],
        timestamp: ts,
        usage: None,
        duration_ms: None,
        model: None,
        completed_at: Some(ts),
    }
}

/// Read `state.json`'s `title`, ignoring the placeholder "New Session" so the
/// caller can fall back to the first user prompt.
fn read_state_title(session_dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(session_dir.join("state.json")).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    value
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "New Session")
        .map(String::from)
}

fn resolve_title(state_title: Option<String>, first_user_text: Option<String>) -> Option<String> {
    state_title.or(first_user_text)
}

/// Best-effort real model name from the session's own log
/// (`… llm config … model=kimi-k2.7-code modelAlias=codeg-managed …`). The wire
/// only stores the (codeg-managed) alias, so the log is the sole place the actual
/// model id appears. `modelAlias=` does not collide: only the exact `model=`
/// token is matched.
fn read_session_log_model(session_dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(session_dir.join("logs").join("kimi-code.log")).ok()?;
    for line in raw.lines() {
        if !line.contains("llm config") {
            continue;
        }
        for token in line.split_whitespace() {
            if let Some(model) = token.strip_prefix("model=") {
                let model = model.trim();
                if !model.is_empty() {
                    return Some(model.to_string());
                }
            }
        }
    }
    None
}

/// Recover a folder *label* from the `wd_<name>_<hash>` bucket directory when the
/// real working directory is unknown (no `session_index.jsonl` entry). The hash
/// is one-way, so only the human-readable name is recovered — never a fabricated
/// path. Returns `None` if the bucket does not follow the `wd_…` convention.
fn decode_work_dir_name(session_dir: &Path) -> Option<String> {
    let bucket = session_dir.parent()?.file_name()?.to_str()?;
    let rest = bucket.strip_prefix("wd_")?;
    // Drop the trailing `_<hash>` segment; tolerate names containing underscores.
    rest.rsplit_once('_')
        .map(|(name, _hash)| name)
        .filter(|name| !name.is_empty())
        .map(String::from)
}

/// List immediate sub-directories of `dir` (empty when `dir` is missing or not a
/// directory). Shallow by design — the layout is exactly two levels deep and we
/// must not descend into `agents/`.
fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// Group the flat, chronologically-ordered `UnifiedMessage`s into `MessageTurn`s:
/// User/System messages each become their own turn; an Assistant message starts a
/// turn that absorbs the immediately-following Tool messages (its tool results),
/// stopping at the next Assistant message to keep turns small for virtualization.
/// (Private copy mirroring the other directory-layout parsers.)
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
            // Assistant or Tool — start a group and absorb following Tool messages.
            let mut blocks: Vec<ContentBlock> = msg.content.clone();
            let mut usage = msg.usage.clone();
            let mut duration_ms = msg.duration_ms;
            let mut turn_model = msg.model.clone();
            let timestamp = msg.timestamp;
            let mut completed_at = msg.completed_at;
            i += 1;

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
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn resolve_home_prefers_env_override() {
        let resolved = resolve_kimi_code_home_from(
            Some(OsString::from("/tmp/custom-kimi")),
            Some(PathBuf::from("/home/demo")),
        );
        assert_eq!(resolved, PathBuf::from("/tmp/custom-kimi"));
    }

    #[test]
    fn resolve_home_ignores_empty_env_and_uses_home() {
        let resolved = resolve_kimi_code_home_from(
            Some(OsString::from("")),
            Some(PathBuf::from("/home/demo")),
        );
        assert_eq!(resolved, PathBuf::from("/home/demo/.kimi-code"));
    }

    #[test]
    fn resolve_home_defaults_to_home_when_env_unset() {
        let resolved = resolve_kimi_code_home_from(None, Some(PathBuf::from("/home/demo")));
        assert_eq!(resolved, PathBuf::from("/home/demo/.kimi-code"));
    }

    #[test]
    fn skeleton_lists_nothing_for_missing_dir() {
        let parser = KimiCodeParser::with_base_dir(PathBuf::from("/nonexistent/kimi/sessions"));
        assert!(parser
            .list_conversations()
            .expect("list is infallible")
            .is_empty());
    }

    /// Write a session directory: `<sessions>/<bucket>/<sessionId>/` with a
    /// `state.json` and a `agents/main/wire.jsonl`. Returns the `sessions` root.
    fn write_session(
        sessions_root: &Path,
        bucket: &str,
        session_id: &str,
        state: &Value,
        wire: &[Value],
    ) {
        let dir = sessions_root.join(bucket).join(session_id);
        let main = dir.join("agents").join("main");
        std::fs::create_dir_all(&main).expect("create agent dir");
        std::fs::write(
            dir.join("state.json"),
            serde_json::to_string_pretty(state).expect("serialize state"),
        )
        .expect("write state.json");
        let mut file = std::fs::File::create(main.join("wire.jsonl")).expect("create wire.jsonl");
        for record in wire {
            writeln!(file, "{}", serde_json::to_string(record).expect("serialize"))
                .expect("write line");
        }
    }

    fn write_session_index(sessions_root: &Path, entries: &[(&str, &str, &str)]) {
        let home = sessions_root.parent().expect("home");
        std::fs::create_dir_all(home).ok();
        let mut file =
            std::fs::File::create(home.join("session_index.jsonl")).expect("create index");
        for (session_id, session_dir, work_dir) in entries {
            writeln!(
                file,
                "{}",
                json!({"sessionId": session_id, "sessionDir": session_dir, "workDir": work_dir})
            )
            .expect("write index line");
        }
    }

    fn unique_root(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("codeg-kimi-{tag}-{}", uuid::Uuid::new_v4()))
            .join(".kimi-code")
            .join("sessions")
    }

    /// A representative single-turn session: user prompt → thinking → tool call →
    /// tool result → final assistant text, with two per-step usage records.
    fn sample_wire() -> Vec<Value> {
        vec![
            json!({"type":"metadata","protocol_version":"1.4","created_at":1782276644193i64}),
            json!({"type":"config.update","profileName":"agent","systemPrompt":"…","time":1782276644193i64}),
            json!({"type":"config.update","modelAlias":"codeg-managed","thinkingLevel":"high","time":1782276644194i64}),
            json!({"type":"turn.prompt","input":[{"type":"text","text":"执行一下pnpm build"}],"origin":{"kind":"user"},"time":1782276649227i64}),
            // append_message echoes the prompt + a system-reminder injection — both must be ignored.
            json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"执行一下pnpm build"}],"origin":{"kind":"user"}},"time":1782276649228i64}),
            json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>noise</system-reminder>"}],"origin":{"kind":"injection","variant":"permission_mode"}},"time":1782276649233i64}),
            json!({"type":"context.append_loop_event","event":{"type":"step.begin","uuid":"s1","turnId":"0","step":1},"time":1782276649900i64}),
            json!({"type":"context.append_loop_event","event":{"type":"content.part","uuid":"p0","turnId":"0","step":1,"part":{"type":"think","think":"先看构建是否通过"}},"time":1782276650000i64}),
            json!({"type":"context.append_loop_event","event":{"type":"tool.call","uuid":"Bash_0","turnId":"0","step":1,"toolCallId":"Bash_0","name":"Bash","args":{"command":"pnpm build","cwd":"/Users/demo/my-app","timeout":300},"description":"Running: pnpm build"},"time":1782276651425i64}),
            json!({"type":"context.append_loop_event","event":{"type":"tool.result","parentUuid":"Bash_0","toolCallId":"Bash_0","result":{"output":"✓ Compiled successfully"}},"time":1782276660973i64}),
            json!({"type":"context.append_loop_event","event":{"type":"step.end","uuid":"s1","turnId":"0","step":1,"usage":{"inputOther":7962,"output":37,"inputCacheRead":9472,"inputCacheCreation":0},"finishReason":"tool_use"},"time":1782276660973i64}),
            json!({"type":"usage.record","model":"codeg-managed","usage":{"inputOther":7962,"output":37,"inputCacheRead":9472,"inputCacheCreation":0},"usageScope":"turn","time":1782276660974i64}),
            json!({"type":"context.append_loop_event","event":{"type":"content.part","uuid":"p1","turnId":"0","step":2,"part":{"type":"text","text":"构建完成，没有错误。"}},"time":1782276664343i64}),
            json!({"type":"usage.record","model":"codeg-managed","usage":{"inputOther":265,"output":36,"inputCacheRead":17408,"inputCacheCreation":0},"usageScope":"turn","time":1782276664500i64}),
        ]
    }

    #[test]
    fn parses_real_session_shape() {
        let root = unique_root("parse");
        let sid = "session_731f79bc";
        write_session(
            &root,
            "wd_my-app_d1a3666e54ae",
            sid,
            &json!({"title":"执行一下pnpm build","createdAt":"2026-06-24T04:50:44.154Z"}),
            &sample_wire(),
        );
        write_session_index(
            &root,
            &[(sid, "ignored", "/Users/demo/my-app")],
        );

        let parser = KimiCodeParser::with_base_dir(root.clone());

        let summaries = parser.list_conversations().expect("list");
        assert_eq!(summaries.len(), 1, "the single content session is listed");
        let summary = &summaries[0];
        assert_eq!(summary.agent_type, AgentType::KimiCode);
        assert_eq!(summary.id, sid);
        assert_eq!(summary.title.as_deref(), Some("执行一下pnpm build"));
        // cwd comes from session_index.jsonl (state.json has none).
        assert_eq!(summary.folder_path.as_deref(), Some("/Users/demo/my-app"));
        assert_eq!(summary.folder_name.as_deref(), Some("my-app"));

        let detail = parser.get_conversation(sid).expect("detail");

        let has_user = detail.turns.iter().any(|t| {
            matches!(t.role, TurnRole::User)
                && t.blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("pnpm build")))
        });
        assert!(has_user, "turn.prompt becomes a User turn");

        // The append_message echo + injection must NOT have produced extra user turns.
        let user_turns = detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::User))
            .count();
        assert_eq!(user_turns, 1, "append_message records are ignored");

        let has_thinking = detail.turns.iter().any(|t| {
            t.blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking { text } if text.contains("先看构建")))
        });
        assert!(has_thinking, "content.part type=think becomes Thinking");

        let has_tool_use = detail.turns.iter().any(|t| {
            t.blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { tool_name, input_preview, .. }
                if tool_name == "Bash" && input_preview.as_deref().unwrap_or_default().contains("pnpm build")))
        });
        assert!(has_tool_use, "tool.call becomes ToolUse with serialized args");

        let has_tool_result = detail.turns.iter().any(|t| {
            t.blocks.iter().any(|b| matches!(b, ContentBlock::ToolResult { output_preview, is_error, .. }
                if !*is_error && output_preview.as_deref().unwrap_or_default().contains("Compiled successfully")))
        });
        assert!(has_tool_result, "tool.result becomes a successful ToolResult");

        let has_assistant_text = detail.turns.iter().any(|t| {
            matches!(t.role, TurnRole::Assistant)
                && t.blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("构建完成")))
        });
        assert!(has_assistant_text, "content.part type=text becomes assistant Text");

        // Per-step usage records are summed across the turn (37+36 output, etc.).
        let usage = detail
            .session_stats
            .as_ref()
            .and_then(|s| s.total_usage.as_ref())
            .expect("usage");
        assert_eq!(usage.output_tokens, 37 + 36);
        assert_eq!(usage.input_tokens, 7962 + 265);
        assert_eq!(usage.cache_read_input_tokens, 9472 + 17408);

        // Context-window occupancy is the LATEST step's input side (265 + 17408),
        // NOT the per-turn sum (8227 + 26880) that the cumulative meter reports —
        // summing every step would re-count the cached prefix and inflate the bar.
        assert_eq!(
            detail
                .session_stats
                .as_ref()
                .and_then(|s| s.context_window_used_tokens),
            Some(265 + 17408),
        );
    }

    #[test]
    fn context_window_used_is_last_step_snapshot_not_turn_sum() {
        // Reproduces the reported bug: a single turn whose many tool-use steps
        // each re-read the growing context from cache. Summing every step's
        // `inputCacheRead` blows past the context window (a false 100%); the true
        // occupancy is the LAST step's input side.
        let root = unique_root("ctxwindow");
        let sid = "sess-ctx";
        write_session(
            &root,
            "wd_app_feed",
            sid,
            &json!({"title":"long single turn"}),
            &[
                json!({"type":"turn.prompt","input":[{"type":"text","text":"do a lot of work"}],"origin":{"kind":"user"},"time":1782276649000i64}),
                // step 1
                json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"t1","name":"Bash","args":{"command":"a"}},"time":1782276650000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"t1","result":{"output":"ok"}},"time":1782276650500i64}),
                json!({"type":"usage.record","usage":{"inputOther":1000,"output":50,"inputCacheRead":100000,"inputCacheCreation":0},"usageScope":"turn","time":1782276650600i64}),
                // step 2
                json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"t2","name":"Bash","args":{"command":"b"}},"time":1782276651000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"t2","result":{"output":"ok"}},"time":1782276651500i64}),
                json!({"type":"usage.record","usage":{"inputOther":500,"output":40,"inputCacheRead":150000,"inputCacheCreation":0},"usageScope":"turn","time":1782276651600i64}),
                // step 3 (final) — the snapshot that represents current occupancy
                json!({"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"done"}},"time":1782276652000i64}),
                json!({"type":"usage.record","usage":{"inputOther":200,"output":30,"inputCacheRead":180000,"inputCacheCreation":0},"usageScope":"turn","time":1782276652100i64}),
            ],
        );

        let parser = KimiCodeParser::with_base_dir(root.clone());
        let stats = parser
            .get_conversation(sid)
            .expect("detail")
            .session_stats
            .expect("session stats");

        // Context window = LAST step's input side (200 + 0 + 180000), NOT the sum
        // of every step's cache read (100000 + 150000 + 180000 = 430000), which
        // would exceed a Kimi context window and clamp to a false 100%.
        assert_eq!(
            stats.context_window_used_tokens,
            Some(200 + 180000),
            "context window uses the latest step snapshot, not the per-turn sum"
        );

        // The cumulative usage meter is UNCHANGED: it still sums every step.
        let total = stats.total_usage.expect("total usage");
        assert_eq!(total.cache_read_input_tokens, 100000 + 150000 + 180000);
        assert_eq!(total.input_tokens, 1000 + 500 + 200);
        assert_eq!(total.output_tokens, 50 + 40 + 30);

        std::fs::remove_dir_all(root.parent().unwrap_or(&root)).ok();
    }

    #[test]
    fn tool_result_error_flag_is_read() {
        let root = unique_root("toolerr");
        let sid = "sess-err";
        write_session(
            &root,
            "wd_app_deadbeef",
            sid,
            &json!({"title":"New Session"}),
            &[
                json!({"type":"turn.prompt","input":[{"type":"text","text":"run it"}],"origin":{"kind":"user"},"time":1782276649227i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"Bash_0","name":"Bash","args":{"command":"bad"}},"time":1782276650000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.result","parentUuid":"Bash_0","toolCallId":"Bash_0","result":{"output":"command failed","isError":true}},"time":1782276651000i64}),
            ],
        );

        let parser = KimiCodeParser::with_base_dir(root.clone());
        let detail = parser.get_conversation(sid).expect("detail");
        let errored = detail.turns.iter().flat_map(|t| &t.blocks).any(
            |b| matches!(b, ContentBlock::ToolResult { is_error, .. } if *is_error),
        );
        assert!(errored, "result.isError=true must set is_error");

        // "New Session" placeholder title falls back to the first user prompt.
        assert_eq!(detail.summary.title.as_deref(), Some("run it"));
    }

    #[test]
    fn content_free_session_is_not_listed() {
        let root = unique_root("empty");
        write_session(
            &root,
            "wd_app_cafe",
            "sess-empty",
            &json!({"title":"New Session"}),
            &[
                json!({"type":"metadata","protocol_version":"1.4","created_at":1782276644193i64}),
                json!({"type":"config.update","systemPrompt":"big system prompt","time":1782276644193i64}),
                json!({"type":"config.update","modelAlias":"codeg-managed","time":1782276644194i64}),
            ],
        );

        let parser = KimiCodeParser::with_base_dir(root.clone());
        assert!(
            parser.list_conversations().expect("list").is_empty(),
            "a session with only metadata/config records (no turn/content) is not listed"
        );
    }

    #[test]
    fn folder_name_decodes_from_bucket_without_index() {
        let root = unique_root("noindex");
        let sid = "sess-noidx";
        write_session(
            &root,
            "wd_my-app_d1a3666e54ae",
            sid,
            &json!({"title":"t"}),
            &[
                json!({"type":"turn.prompt","input":[{"type":"text","text":"hi"}],"origin":{"kind":"user"},"time":1782276649227i64}),
                json!({"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"hello"}},"time":1782276649300i64}),
            ],
        );
        // No session_index.jsonl written → cwd unknown, label decoded from bucket.

        let parser = KimiCodeParser::with_base_dir(root.clone());
        let summaries = parser.list_conversations().expect("list");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].folder_path, None, "no path is fabricated");
        assert_eq!(
            summaries[0].folder_name.as_deref(),
            Some("my-app"),
            "folder label decoded from wd_<name>_<hash>"
        );
    }

    #[test]
    fn unknown_conversation_is_not_found() {
        let root = unique_root("missing");
        std::fs::create_dir_all(&root).ok();
        let parser = KimiCodeParser::with_base_dir(root);
        assert!(matches!(
            parser.get_conversation("nope"),
            Err(ParseError::ConversationNotFound(_))
        ));
    }

    /// Write a sub-agent's wire at
    /// `<sessions>/<bucket>/<sessionId>/agents/<agentId>/wire.jsonl`, beside the
    /// `agents/main/` wire that `write_session` creates.
    fn write_subagent_wire(
        sessions_root: &Path,
        bucket: &str,
        session_id: &str,
        agent_id: &str,
        wire: &[Value],
    ) {
        let dir = sessions_root
            .join(bucket)
            .join(session_id)
            .join("agents")
            .join(agent_id);
        std::fs::create_dir_all(&dir).expect("create subagent dir");
        let mut file = std::fs::File::create(dir.join("wire.jsonl")).expect("create wire.jsonl");
        for record in wire {
            writeln!(file, "{}", serde_json::to_string(record).expect("serialize"))
                .expect("write line");
        }
    }

    #[test]
    fn agent_delegation_loads_subagent_tool_calls_into_agent_stats() {
        let root = unique_root("subagent");
        let bucket = "wd_my-app_d1a3666e54ae";
        let sid = "session_subagent";

        write_session(
            &root,
            bucket,
            sid,
            &json!({"title":"delegate it"}),
            &[
                json!({"type":"turn.prompt","input":[{"type":"text","text":"delegate the build"}],"origin":{"kind":"user"},"time":1782276649000i64}),
                // Agent delegation whose result header links to `agent-0`.
                json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"Agent_1","name":"Agent","args":{"description":"build it","prompt":"run pnpm build","subagent_type":"coder"}},"time":1782276650000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"Agent_1","result":{"output":"agent_id: agent-0\nactual_subagent_type: coder\nstatus: completed\n\n[summary]\n`pnpm build` succeeded."}},"time":1782276660000i64}),
                // A second Agent delegation whose transcript directory is absent.
                json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"Agent_2","name":"Agent","args":{"prompt":"x","subagent_type":"coder"}},"time":1782276661000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"Agent_2","result":{"output":"agent_id: agent-missing\nstatus: completed\n\n[summary]\ndone"}},"time":1782276662000i64}),
                // A plain tool with no sub-agent linkage.
                json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"Bash_0","name":"Bash","args":{"command":"ls"}},"time":1782276663000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"Bash_0","result":{"output":"a.ts"}},"time":1782276664000i64}),
                // Isolation guard: a non-Agent tool whose result output coincidentally
                // opens with a real `agent_id: agent-0` header must STILL get no
                // agent_stats — the gate is the call's classification, not the marker.
                json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"Bash_stray","name":"Bash","args":{"command":"echo agent_id"}},"time":1782276665000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"Bash_stray","result":{"output":"agent_id: agent-0\nstatus: completed"}},"time":1782276666000i64}),
            ],
        );

        // The sub-agent ran two tools: a successful Bash and a failed Read.
        write_subagent_wire(
            &root,
            bucket,
            sid,
            "agent-0",
            &[
                json!({"type":"metadata","protocol_version":"1.4","created_at":1782276651000i64}),
                json!({"type":"config.update","profileName":"coder","modelAlias":"codeg-managed","time":1782276651000i64}),
                json!({"type":"turn.prompt","input":[{"type":"text","text":"run pnpm build"}],"origin":{"kind":"user"},"time":1782276651500i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"s1","name":"Bash","args":{"command":"pnpm build","cwd":"/Users/demo/my-app"}},"time":1782276652000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"s1","result":{"output":"Exited with code 0"}},"time":1782276658000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"s2","name":"Read","args":{"file_path":"/missing"}},"time":1782276659000i64}),
                json!({"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"s2","result":{"output":"file not found","isError":true}},"time":1782276659500i64}),
            ],
        );

        let parser = KimiCodeParser::with_base_dir(root.clone());
        let detail = parser.get_conversation(sid).expect("detail");

        // Collect every (tool_use_id, agent_stats) across the rendered turns.
        let mut results: Vec<(Option<String>, Option<AgentExecutionStats>)> = Vec::new();
        for turn in &detail.turns {
            for block in &turn.blocks {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    agent_stats,
                    ..
                } = block
                {
                    results.push((tool_use_id.clone(), agent_stats.clone()));
                }
            }
        }

        // The first Agent result carries the sub-agent's nested tool calls.
        let stats = results
            .iter()
            .find(|(id, _)| id.as_deref() == Some("Agent_1"))
            .expect("Agent_1 result")
            .1
            .as_ref()
            .expect("agent_stats populated from the sub-agent wire");
        assert_eq!(stats.tool_calls.len(), 2, "two nested tool calls");
        assert_eq!(stats.total_tool_use_count, Some(2));

        let bash = &stats.tool_calls[0];
        assert_eq!(bash.tool_name, "Bash");
        assert!(bash
            .input_preview
            .as_deref()
            .unwrap_or_default()
            .contains("pnpm build"));
        assert!(bash
            .output_preview
            .as_deref()
            .unwrap_or_default()
            .contains("Exited with code 0"));
        assert!(!bash.is_error, "the successful Bash is not an error");

        let read = &stats.tool_calls[1];
        assert_eq!(read.tool_name, "Read");
        assert!(read.is_error, "result.isError=true marks the nested Read failed");

        // A delegation whose transcript directory is missing degrades to no stats.
        let missing = results
            .iter()
            .find(|(id, _)| id.as_deref() == Some("Agent_2"))
            .expect("Agent_2 result");
        assert!(
            missing.1.is_none(),
            "an absent sub-agent transcript leaves agent_stats None"
        );

        // A plain tool result is untouched.
        let plain = results
            .iter()
            .find(|(id, _)| id.as_deref() == Some("Bash_0"))
            .expect("plain tool result");
        assert!(plain.1.is_none(), "non-Agent results never carry agent_stats");

        // A non-Agent result with a real `agent_id:` header is still gated out by
        // the call-side classification.
        let stray = results
            .iter()
            .find(|(id, _)| id.as_deref() == Some("Bash_stray"))
            .expect("stray tool result");
        assert!(
            stray.1.is_none(),
            "a non-Agent result must not gain agent_stats even with a real agent_id header"
        );

        std::fs::remove_dir_all(root.parent().unwrap_or(&root)).ok();
    }

    #[test]
    fn subagent_id_is_parsed_only_from_the_first_line() {
        // The marker is the first line; an `agent_id:` later in the summary body
        // must not be mistaken for it.
        assert_eq!(
            subagent_id_from_output("agent_id: agent-0\nstatus: completed"),
            Some("agent-0")
        );
        assert_eq!(
            subagent_id_from_output("[summary]\nthen agent_id: agent-9 in prose"),
            None
        );
        assert_eq!(subagent_id_from_output("just a normal tool output"), None);
        assert_eq!(subagent_id_from_output("agent_id:   "), None);
    }
}
