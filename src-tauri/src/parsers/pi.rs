use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;
use walkdir::WalkDir;

use crate::models::{
    AgentType, ContentBlock, ConversationDetail, ConversationSummary, MessageRole, MessageTurn,
    TurnRole, TurnUsage, UnifiedMessage,
};
use crate::parsers::{
    backfill_turn_durations, compute_session_stats, folder_name_from_path,
    infer_context_window_max_tokens, latest_turn_total_usage_tokens, merge_context_window_stats,
    relocate_orphaned_tool_results, resolve_patch_line_numbers, structurize_read_tool_output,
    title_from_user_text, truncate_str, AgentParser, ParseError,
};

/// Resolve the `pi` coding agent's sessions directory, honoring (highest
/// precedence first):
///   1. `PI_CODING_AGENT_SESSION_DIR` — the sessions dir directly;
///   2. `PI_CODING_AGENT_DIR` — the agent home, with `sessions` appended;
///   3. `~/.pi/agent/sessions/` — the default.
///
/// Mirrors the `resolve_*`/`resolve_*_from` split of `parsers::kimi_code` so the
/// environment lookup is a pure function over its inputs (testable without
/// touching the process environment). The parser's `base_dir` IS this sessions
/// directory.
pub(crate) fn resolve_pi_sessions_dir() -> PathBuf {
    resolve_pi_sessions_dir_from(
        std::env::var_os("PI_CODING_AGENT_SESSION_DIR"),
        std::env::var_os("PI_CODING_AGENT_DIR"),
        dirs::home_dir(),
    )
}

fn resolve_pi_sessions_dir_from(
    session_dir_env: Option<OsString>,
    agent_dir_env: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    if let Some(session_dir) = session_dir_env.filter(|value| !value.is_empty()) {
        return PathBuf::from(session_dir);
    }
    if let Some(agent_dir) = agent_dir_env.filter(|value| !value.is_empty()) {
        return PathBuf::from(agent_dir).join("sessions");
    }
    home_dir
        .unwrap_or_default()
        .join(".pi")
        .join("agent")
        .join("sessions")
}

/// `pi` (pi.dev) stores its transcripts as **one JSONL file per session** under
/// a working-directory bucket — Claude Code's / CodeBuddy's archetype:
///
/// ```text
/// <base_dir>/                         (default ~/.pi/agent/sessions)
/// └── --<cwd-with-'/'-replaced-by-'-'>--/
///     └── <timestamp>_<uuid>.jsonl     # one session, JSONL
/// ```
///
/// The dashed directory name is a one-way encoding of the working directory, so
/// the real `cwd` is read from the session's HEADER line — never reverse-decoded
/// from the directory name.
///
/// Line 1 is the header
/// (`{"type":"session","version":3,"id":…,"timestamp":…,"cwd":…}`); its `id` is
/// the stable external conversation id. Every other line shares `type` / `id` /
/// `parentId` / `timestamp` and is one of:
///
/// - `message` — a nested `message` object keyed by `role`:
///   - `user`: `content` is a STRING or an ARRAY of blocks (text parts joined),
///   - `assistant`: `content` is an ARRAY of `{type:"text"|"thinking"|"toolCall"}`
///     blocks, plus `provider` / `model` / `usage` / `stopReason`,
///   - `toolResult`: `toolCallId` / `toolName` / `content` / `isError`, where
///     `content` is an ARRAY of MCP-shaped blocks (`[{"type":"text","text":…}]`)
///     for every tool — bash, read, write alike (see
///     `tool_result_content_text`).
/// - `bashExecution` — a `command` + `output` + `exitCode` pair, surfaced as a
///   synthetic `bash` tool use + result.
/// - `usage` — `{input,output,cacheRead,cacheWrite,totalTokens,cost}` per step.
/// - `model_change` — `{provider,modelId}`; tracks the latest model.
/// - `session_info` — `{name}`; the session's display name (preferred title).
/// - `compaction` / `thinking_level_change` / `label` / `custom` / … — metadata
///   that is parsed best-effort or skipped; an unknown line NEVER errors.
///
/// Unknown / malformed lines are skipped (`continue`) so a forward-compatible or
/// partially-written log is read robustly rather than panicking.
pub struct PiParser {
    base_dir: PathBuf,
}

impl PiParser {
    pub fn new() -> Self {
        Self {
            base_dir: resolve_pi_sessions_dir(),
        }
    }

    /// Construct a parser pointed at an explicit `sessions` directory (test
    /// fixtures).
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    fn parse_summary(&self, path: &Path) -> Option<ConversationSummary> {
        let parsed = parse_session(path);
        // A file with no header AND no content events is treated as empty.
        let started_at = parsed.first_ts?;
        if parsed.content_events == 0 {
            return None;
        }

        let id = parsed.session_id.unwrap_or_else(|| fallback_id(path));
        let folder_name = parsed.cwd.as_deref().map(folder_name_from_path);

        Some(ConversationSummary {
            id,
            agent_type: AgentType::Pi,
            folder_path: parsed.cwd,
            folder_name,
            title: resolve_title(parsed.session_name, parsed.first_user_text),
            started_at,
            ended_at: parsed.last_ts,
            message_count: parsed.message_count,
            model: parsed.model,
            git_branch: None,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        })
    }

    fn parse_detail(
        &self,
        path: &Path,
        conversation_id: &str,
    ) -> Result<ConversationDetail, ParseError> {
        let parsed = parse_session(path);

        let mut turns = group_into_turns(parsed.messages);
        relocate_orphaned_tool_results(&mut turns);
        structurize_read_tool_output(&mut turns);
        resolve_patch_line_numbers(&mut turns, parsed.cwd.as_deref());
        backfill_turn_durations(&mut turns, &[]);

        let used_tokens = latest_turn_total_usage_tokens(&turns);
        let max_tokens = infer_context_window_max_tokens(parsed.model.as_deref());
        let session_stats =
            merge_context_window_stats(compute_session_stats(&turns), used_tokens, max_tokens);

        let folder_name = parsed.cwd.as_deref().map(folder_name_from_path);
        let summary = ConversationSummary {
            id: conversation_id.to_string(),
            agent_type: AgentType::Pi,
            folder_path: parsed.cwd,
            folder_name,
            title: resolve_title(parsed.session_name, parsed.first_user_text),
            started_at: parsed.first_ts.unwrap_or_else(Utc::now),
            ended_at: parsed.last_ts,
            message_count: parsed.message_count,
            model: parsed.model,
            git_branch: None,
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

impl Default for PiParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentParser for PiParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let mut conversations = Vec::new();
        if !self.base_dir.exists() {
            return Ok(conversations);
        }

        for entry in WalkDir::new(&self.base_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(Some(summary)) =
                super::summary_cache::get_or_parse(AgentType::Pi, path, || {
                    Ok(self.parse_summary(path))
                })
            {
                conversations.push(summary);
            }
        }

        conversations.sort_by_key(|c| std::cmp::Reverse(c.started_at));
        Ok(conversations)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        if self.base_dir.exists() {
            for entry in WalkDir::new(&self.base_dir)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                // Match by the header `id` (the stable external id); fall back to
                // the filename uuid when a malformed file has no header.
                if session_id_of(path).as_deref() == Some(conversation_id) {
                    return self.parse_detail(path, conversation_id);
                }
            }
        }

        Err(ParseError::ConversationNotFound(
            conversation_id.to_string(),
        ))
    }
}

/// The accumulated result of scanning one session `.jsonl`.
#[derive(Default)]
struct SessionParse {
    messages: Vec<UnifiedMessage>,
    first_ts: Option<DateTime<Utc>>,
    last_ts: Option<DateTime<Utc>>,
    /// Header `id` (the stable external id; `None` when the header is missing).
    session_id: Option<String>,
    /// Header `cwd`.
    cwd: Option<String>,
    /// `session_info.name` — the preferred display title.
    session_name: Option<String>,
    /// First user prompt, already truncated for use as a fallback title.
    first_user_text: Option<String>,
    /// Latest model from an assistant message's `model` or a `model_change`.
    model: Option<String>,
    /// User + assistant turns (tool calls/results and thinking excluded), the
    /// list-view activity count.
    message_count: u32,
    /// Number of content-bearing records — decides whether the session is listed.
    content_events: u32,
}

/// Parse a `pi` session `.jsonl` into a flat, chronologically-ordered list of
/// `UnifiedMessage`s plus session metadata. Unknown / malformed lines are
/// skipped so a forward-compatible or partially-written log never panics.
fn parse_session(path: &Path) -> SessionParse {
    let mut sp = SessionParse::default();
    let Ok(file) = fs::File::open(path) else {
        return sp;
    };

    for (idx, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let record_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        let ts_raw = record_iso_ts(&value);
        // Header / first line always seeds the span; content lines extend it.
        let ts = note_ts(&mut sp, ts_raw);

        match record_type {
            "session" => {
                if sp.session_id.is_none() {
                    sp.session_id = value
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from);
                }
                if sp.cwd.is_none() {
                    sp.cwd = value
                        .get("cwd")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from);
                }
            }
            "session_info" => {
                if sp.session_name.is_none() {
                    sp.session_name = value
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from);
                }
            }
            "model_change" => {
                if let Some(model) = value
                    .get("modelId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    sp.model = Some(model.to_string());
                }
            }
            "message" => parse_message_record(&mut sp, &value, ts, idx),
            "bashExecution" => parse_bash_execution(&mut sp, &value, ts, idx),
            // `usage`, `compaction`, `thinking_level_change`, `label`, `custom`,
            // `custom_message`, `branch_summary`, `compactionSummary`,
            // `branchSummary`, and any unknown line: best-effort / skip.
            _ => {}
        }
    }

    sp
}

/// Parse a `message` record (nested `message` object keyed by `role`).
fn parse_message_record(sp: &mut SessionParse, value: &Value, ts: DateTime<Utc>, idx: usize) {
    let Some(message) = value.get("message") else {
        return;
    };
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");

    match role {
        "user" => {
            let text = user_content_text(message.get("content"));
            if text.trim().is_empty() {
                return;
            }
            sp.content_events += 1;
            sp.message_count += 1;
            if sp.first_user_text.is_none() {
                sp.first_user_text = Some(title_from_user_text(text.trim()));
            }
            sp.messages.push(text_message(
                format!("pi-user-{idx}"),
                MessageRole::User,
                vec![ContentBlock::Text { text }],
                ts,
                None,
                None,
            ));
        }
        "assistant" => {
            let model = message
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from);
            if let Some(ref m) = model {
                sp.model = Some(m.clone());
            }

            let blocks = assistant_content_blocks(message.get("content"));
            if blocks.is_empty() {
                return;
            }
            sp.content_events += 1;
            sp.message_count += 1;
            sp.messages.push(text_message(
                format!("pi-assistant-{idx}"),
                MessageRole::Assistant,
                blocks,
                ts,
                usage_from_object(message.get("usage")),
                model,
            ));
        }
        "toolResult" => {
            let tool_call_id = message
                .get("toolCallId")
                .and_then(Value::as_str)
                .map(String::from);
            let output_preview =
                content_to_text(message.get("content")).map(|s| truncate_str(&s, 4000));
            let is_error = message
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            sp.content_events += 1;
            sp.messages.push(text_message(
                format!("pi-toolresult-{idx}"),
                MessageRole::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: tool_call_id,
                    output_preview,
                    is_error,
                    agent_stats: None,
                    images: Vec::new(),
                }],
                ts,
                None,
                None,
            ));
        }
        _ => {}
    }
}

/// Parse a `bashExecution` record into a synthetic `bash` tool use + result pair
/// (so it threads and renders like an ordinary tool call). A non-zero `exitCode`
/// (or a missing one) marks the result an error.
fn parse_bash_execution(sp: &mut SessionParse, value: &Value, ts: DateTime<Utc>, idx: usize) {
    let command = value
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let output = value
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let exit_code = value.get("exitCode").and_then(Value::as_i64).unwrap_or(0);
    let tool_use_id = value
        .get("id")
        .and_then(Value::as_str)
        .map(|id| format!("bash-{id}"));

    sp.content_events += 1;
    sp.messages.push(text_message(
        format!("pi-bashcall-{idx}"),
        MessageRole::Assistant,
        vec![ContentBlock::ToolUse {
            tool_use_id: tool_use_id.clone(),
            tool_name: "bash".to_string(),
            input_preview: (!command.is_empty()).then_some(command),
            status: None,
            meta: None,
        }],
        ts,
        None,
        None,
    ));
    sp.messages.push(text_message(
        format!("pi-bashresult-{idx}"),
        MessageRole::Tool,
        vec![ContentBlock::ToolResult {
            tool_use_id,
            output_preview: (!output.is_empty()).then(|| truncate_str(&output, 4000)),
            is_error: exit_code != 0,
            agent_stats: None,
            images: Vec::new(),
        }],
        ts,
        None,
        None,
    ));
}

/// A `user` message's `content` is either a plain string or an array of blocks;
/// join the text of every string / `{type:"text",text}` part.
fn user_content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.as_str() {
                    out.push_str(text);
                } else if item.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        out.push_str(text);
                    }
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// An `assistant` message's `content` is an array of blocks: `text` → `Text`,
/// `thinking` → `Thinking`, `toolCall` → `ToolUse`. Unknown block types are
/// skipped.
fn assistant_content_blocks(content: Option<&Value>) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    let Some(items) = content.and_then(Value::as_array) else {
        return blocks;
    };
    for item in items {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }
            }
            "thinking" => {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        blocks.push(ContentBlock::Thinking {
                            text: text.to_string(),
                        });
                    }
                }
            }
            "toolCall" => {
                let tool_use_id = item.get("id").and_then(Value::as_str).map(String::from);
                let tool_name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                blocks.push(ContentBlock::ToolUse {
                    tool_use_id,
                    tool_name,
                    input_preview: tool_arguments_preview(item.get("arguments")),
                    status: None,
                    meta: None,
                });
            }
            _ => {}
        }
    }
    blocks
}

/// Serialize a `toolCall.arguments` value (an object/array — or, defensively, a
/// pre-stringified string) into a compact JSON preview, truncated. `None` for a
/// missing / null value.
fn tool_arguments_preview(arguments: Option<&Value>) -> Option<String> {
    let arguments = arguments?;
    let serialized = if let Some(text) = arguments.as_str() {
        if text.is_empty() {
            return None;
        }
        text.to_string()
    } else if arguments.is_null() {
        return None;
    } else {
        serde_json::to_string(arguments).ok()?
    };
    Some(truncate_str(&serialized, 4000))
}

/// Flatten a pi tool-result `content` value into the text pi itself displays.
///
/// Every pi tool — `bash`, `read`, `write` — writes its result as an ARRAY of
/// MCP-shaped blocks (`[{"type":"text","text":…}]`); a bare string is accepted
/// defensively. Only `text` blocks carry readable output and pi joins them with
/// no separator, so this returns byte-for-byte what pi-acp puts on the live
/// `content[]` channel (its `toolResultToText`) — history and live must not
/// render two different strings for the same result.
///
/// `None` for a missing / null / empty value, and for any shape carrying no text
/// block at all; callers decide what to fall back to.
pub(crate) fn tool_result_content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return (!text.is_empty()).then(|| text.to_string());
    }
    let mut out = String::new();
    for item in content.as_array()? {
        if item.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A tool result's `content` — the MCP block array pi actually writes (or, for
/// robustness, a plain string). Anything else is serialized as a fallback so an
/// unrecognized future shape still surfaces something rather than nothing.
/// `None` for a missing / null / empty value.
fn content_to_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
    if let Some(text) = tool_result_content_text(content) {
        return Some(text);
    }
    if content.is_null() || content.is_string() {
        return None;
    }
    serde_json::to_string(content).ok()
}

/// Map a `usage` object (`{input,output,cacheRead,cacheWrite,…}`) onto
/// `TurnUsage`; `None` when every counter is absent or zero so an empty object
/// does not create spurious usage. Missing fields default to 0.
fn usage_from_object(usage: Option<&Value>) -> Option<TurnUsage> {
    let usage = usage?;
    let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let input = field("input");
    let output = field("output");
    let cache_read = field("cacheRead");
    let cache_write = field("cacheWrite");
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        return None;
    }
    Some(TurnUsage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: cache_write,
        cache_read_input_tokens: cache_read,
    })
}

/// ISO-8601 `timestamp` string → `DateTime<Utc>` (chrono's RFC3339 `FromStr`),
/// mirroring `parsers::openclaw::parse_iso_timestamp`.
fn record_iso_ts(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
}

/// Record a line's timestamp into the session span and return a concrete
/// timestamp for the message (falling back to the last seen one, then now).
fn note_ts(sp: &mut SessionParse, ts_raw: Option<DateTime<Utc>>) -> DateTime<Utc> {
    if let Some(ts) = ts_raw {
        sp.first_ts.get_or_insert(ts);
        sp.last_ts = Some(ts);
    }
    ts_raw.or(sp.last_ts).unwrap_or_else(Utc::now)
}

/// Read just the header `id` of a session file (falling back to the filename
/// uuid), used to match `get_conversation` without parsing the whole file.
fn session_id_of(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("session") {
            if let Some(id) = value
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(id.to_string());
            }
        }
        // The header is line 1; stop after the first non-empty record so a
        // headerless file falls back to the filename uuid rather than scanning.
        break;
    }
    Some(fallback_id(path))
}

/// Recover a conversation id from a `<timestamp>_<uuid>.jsonl` filename when the
/// header is missing: the `<uuid>` after the first `_`, else the whole stem.
fn fallback_id(path: &Path) -> String {
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    stem.split_once('_')
        .map(|(_ts, uuid)| uuid.to_string())
        .filter(|uuid| !uuid.is_empty())
        .unwrap_or(stem)
}

fn resolve_title(session_name: Option<String>, first_user_text: Option<String>) -> Option<String> {
    session_name.or(first_user_text)
}

fn text_message(
    id: String,
    role: MessageRole,
    content: Vec<ContentBlock>,
    ts: DateTime<Utc>,
    usage: Option<TurnUsage>,
    model: Option<String>,
) -> UnifiedMessage {
    UnifiedMessage {
        id,
        role,
        content,
        timestamp: ts,
        usage,
        duration_ms: None,
        model,
        completed_at: Some(ts),
    }
}

/// Group the flat, chronologically-ordered `UnifiedMessage`s into `MessageTurn`s:
/// User/System messages each become their own turn; an Assistant message starts a
/// turn that absorbs the immediately-following Tool messages (its tool results),
/// stopping at the next Assistant message to keep turns small for virtualization.
/// (Private copy mirroring the other single-file-per-session parsers.)
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
    use tempfile::tempdir;

    #[test]
    fn resolve_sessions_dir_prefers_session_dir_env() {
        let resolved = resolve_pi_sessions_dir_from(
            Some(OsString::from("/custom/pi/sessions")),
            Some(OsString::from("/custom/pi-home")),
            Some(PathBuf::from("/home/demo")),
        );
        assert_eq!(resolved, PathBuf::from("/custom/pi/sessions"));
    }

    #[test]
    fn resolve_sessions_dir_appends_sessions_to_agent_dir() {
        let resolved = resolve_pi_sessions_dir_from(
            None,
            Some(OsString::from("/custom/pi-home")),
            Some(PathBuf::from("/home/demo")),
        );
        assert_eq!(resolved, PathBuf::from("/custom/pi-home/sessions"));
    }

    #[test]
    fn resolve_sessions_dir_defaults_to_home_dot_pi() {
        let resolved = resolve_pi_sessions_dir_from(None, None, Some(PathBuf::from("/home/demo")));
        assert_eq!(
            resolved,
            PathBuf::from("/home/demo/.pi/agent/sessions")
        );
    }

    #[test]
    fn resolve_sessions_dir_ignores_empty_env() {
        let resolved = resolve_pi_sessions_dir_from(
            Some(OsString::new()),
            Some(OsString::new()),
            Some(PathBuf::from("/home/demo")),
        );
        assert_eq!(
            resolved,
            PathBuf::from("/home/demo/.pi/agent/sessions")
        );
    }

    #[test]
    fn nonexistent_base_dir_lists_nothing() {
        let parser = PiParser::with_base_dir(PathBuf::from("/nonexistent/pi/sessions"));
        assert!(parser
            .list_conversations()
            .expect("list is infallible")
            .is_empty());
    }

    /// Write one session JSONL at
    /// `<base>/--<dashed-cwd>--/<timestamp>_<uuid>.jsonl`.
    fn write_session(base: &Path, dashed_cwd: &str, filename: &str, records: &[Value]) {
        let dir = base.join(dashed_cwd);
        std::fs::create_dir_all(&dir).expect("create session dir");
        let mut file = std::fs::File::create(dir.join(filename)).expect("create jsonl");
        for record in records {
            writeln!(file, "{}", serde_json::to_string(record).expect("serialize"))
                .expect("write line");
        }
    }

    fn sample_records(id: &str) -> Vec<Value> {
        vec![
            json!({"type":"session","version":3,"id":id,"timestamp":"2026-06-27T10:00:00.000Z","cwd":"/Users/demo/my-app"}),
            json!({"type":"session_info","id":"i1","parentId":null,"timestamp":"2026-06-27T10:00:00.100Z","name":"Build the app"}),
            json!({"type":"message","id":"m1","parentId":null,"timestamp":"2026-06-27T10:00:01.000Z",
                   "message":{"role":"user","content":"run pnpm build"}}),
            json!({"type":"model_change","id":"mc1","parentId":null,"timestamp":"2026-06-27T10:00:01.500Z",
                   "provider":"anthropic","modelId":"claude-sonnet-4-6"}),
            json!({"type":"message","id":"m2","parentId":"m1","timestamp":"2026-06-27T10:00:02.000Z",
                   "message":{"role":"assistant","provider":"anthropic","model":"claude-sonnet-4-6","stopReason":"tool_use",
                     "usage":{"input":1200,"output":80,"cacheRead":4000,"cacheWrite":0,"totalTokens":5280,"cost":0.01},
                     "content":[
                       {"type":"thinking","text":"check the build first"},
                       {"type":"text","text":"Running the build now."},
                       {"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"pnpm build"}}
                     ]}}),
            json!({"type":"message","id":"m3","parentId":"m2","timestamp":"2026-06-27T10:00:09.000Z",
                   "message":{"role":"toolResult","toolCallId":"call_1","toolName":"bash",
                     "content":[{"type":"text","text":"Compiled successfully"}],"isError":false}}),
            json!({"type":"message","id":"m4","parentId":"m3","timestamp":"2026-06-27T10:00:10.000Z",
                   "message":{"role":"assistant","provider":"anthropic","model":"claude-sonnet-4-6","stopReason":"end_turn",
                     "content":[{"type":"text","text":"Build succeeded."}]}}),
        ]
    }

    #[test]
    fn parses_pi_v3_session_shape() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path();
        let id = "0f3c1d2e-1111-2222-3333-444455556666";
        write_session(
            base,
            "--Users-demo-my-app--",
            "2026-06-27T10-00-00_0f3c1d2e.jsonl",
            &sample_records(id),
        );

        let parser = PiParser::with_base_dir(base.to_path_buf());

        // ---- list_conversations -------------------------------------------
        let summaries = parser.list_conversations().expect("list");
        assert_eq!(summaries.len(), 1, "one session listed");
        let summary = &summaries[0];
        assert_eq!(summary.agent_type, AgentType::Pi);
        assert_eq!(summary.id, id, "header id is the external id");
        assert_eq!(
            summary.title.as_deref(),
            Some("Build the app"),
            "session_info.name is the preferred title"
        );
        assert_eq!(
            summary.folder_path.as_deref(),
            Some("/Users/demo/my-app"),
            "cwd is read from the header, not the dashed dir name"
        );
        assert_eq!(summary.folder_name.as_deref(), Some("my-app"));
        assert_eq!(
            summary.model.as_deref(),
            Some("claude-sonnet-4-6"),
            "latest assistant/model_change model"
        );
        assert_eq!(
            summary.message_count, 3,
            "one user + two assistant turns (tool result excluded)"
        );

        // ---- get_conversation ---------------------------------------------
        let detail = parser.get_conversation(id).expect("detail");
        assert_eq!(detail.summary.agent_type, AgentType::Pi);
        assert_eq!(detail.summary.id, id);

        let has_user = detail.turns.iter().any(|t| {
            matches!(t.role, TurnRole::User)
                && t.blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("pnpm build")))
        });
        assert!(has_user, "user message becomes a User turn");

        let has_thinking = detail.turns.iter().any(|t| {
            t.blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking { text } if text.contains("check the build")))
        });
        assert!(has_thinking, "assistant thinking block becomes Thinking");

        let has_assistant_text = detail.turns.iter().any(|t| {
            matches!(t.role, TurnRole::Assistant)
                && t.blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("Running the build")))
        });
        assert!(has_assistant_text, "assistant text block renders");

        // A ToolUse and a matching ToolResult, threaded by call id.
        let tool_use_id = detail
            .turns
            .iter()
            .flat_map(|t| &t.blocks)
            .find_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_name,
                    tool_use_id,
                    input_preview,
                    ..
                } if tool_name == "bash" => {
                    assert!(input_preview
                        .as_deref()
                        .unwrap_or_default()
                        .contains("pnpm build"));
                    tool_use_id.clone()
                }
                _ => None,
            })
            .expect("a bash ToolUse");
        assert_eq!(tool_use_id, "call_1");

        let result = detail
            .turns
            .iter()
            .flat_map(|t| &t.blocks)
            .find_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id: Some(id),
                    output_preview,
                    is_error,
                    ..
                } if id == "call_1" => Some((output_preview.clone(), *is_error)),
                _ => None,
            })
            .expect("a matching ToolResult");
        assert_eq!(
            result.0.as_deref(),
            Some("Compiled successfully"),
            "the MCP block array is flattened to its text, not dumped as JSON"
        );
        assert!(!result.1, "a successful tool result is not an error");

        // The ToolUse and ToolResult must land in the same (assistant) turn.
        let same_turn = detail.turns.iter().any(|t| {
            let has_use = t.blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { tool_use_id: Some(id), .. } if id == "call_1"));
            let has_res = t.blocks.iter().any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id: Some(id), .. } if id == "call_1"));
            has_use && has_res
        });
        assert!(same_turn, "ToolUse and ToolResult thread into one turn");

        // Assistant usage is summed into the session stats.
        let usage = detail
            .session_stats
            .as_ref()
            .and_then(|s| s.total_usage.as_ref())
            .expect("usage");
        assert_eq!(usage.input_tokens, 1200);
        assert_eq!(usage.output_tokens, 80);
        assert_eq!(usage.cache_read_input_tokens, 4000);
    }

    /// Regression: a `bash` result is an MCP block array, and serializing it
    /// painted the terminal card with the JSON source string
    /// (`[{"text":"$ next build\n…","type":"text"}]`) instead of the output.
    #[test]
    fn tool_result_content_shapes() {
        assert_eq!(
            tool_result_content_text(&json!([{"type":"text","text":"$ next build\nok\n"}]))
                .as_deref(),
            Some("$ next build\nok\n"),
            "the real on-disk shape: one text block, unwrapped"
        );
        assert_eq!(
            tool_result_content_text(&json!([
                {"type":"text","text":"part one "},
                {"type":"image","data":"…"},
                {"type":"text","text":"part two"}
            ]))
            .as_deref(),
            Some("part one part two"),
            "text blocks join with no separator; non-text blocks are skipped"
        );
        assert_eq!(
            tool_result_content_text(&json!("plain string")).as_deref(),
            Some("plain string"),
            "a bare string is accepted defensively"
        );
        assert_eq!(tool_result_content_text(&json!([])), None, "empty array");
        assert_eq!(
            tool_result_content_text(&json!([{"type":"image","data":"…"}])),
            None,
            "no text block at all — caller decides the fallback"
        );

        // `content_to_text` keeps the JSON fallback for shapes with no text.
        assert_eq!(content_to_text(None), None);
        assert_eq!(content_to_text(Some(&Value::Null)), None);
        assert_eq!(content_to_text(Some(&json!(""))), None);
        assert_eq!(
            content_to_text(Some(&json!({"exitCode": 0}))).as_deref(),
            Some(r#"{"exitCode":0}"#),
            "an unrecognized object still surfaces something"
        );
    }

    #[test]
    fn user_content_array_text_parts_are_joined() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path();
        let id = "array-user-0001";
        write_session(
            base,
            "--Users-demo-app--",
            "ts_array-user-0001.jsonl",
            &[
                json!({"type":"session","version":3,"id":id,"timestamp":"2026-06-27T10:00:00.000Z","cwd":"/Users/demo/app"}),
                json!({"type":"message","id":"m1","timestamp":"2026-06-27T10:00:01.000Z",
                       "message":{"role":"user","content":[
                         {"type":"text","text":"part one "},
                         {"type":"text","text":"part two"}
                       ]}}),
                json!({"type":"message","id":"m2","timestamp":"2026-06-27T10:00:02.000Z",
                       "message":{"role":"assistant","model":"claude-sonnet-4-6",
                         "content":[{"type":"text","text":"ok"}]}}),
            ],
        );

        let parser = PiParser::with_base_dir(base.to_path_buf());
        let detail = parser.get_conversation(id).expect("detail");
        let joined = detail
            .turns
            .iter()
            .flat_map(|t| &t.blocks)
            .find_map(|b| match b {
                ContentBlock::Text { text } if text.contains("part one") => Some(text.clone()),
                _ => None,
            })
            .expect("user text");
        assert!(
            joined.contains("part one") && joined.contains("part two"),
            "array text parts are joined, got: {joined}"
        );
        // No session_info.name → falls back to the first user message text.
        assert_eq!(detail.summary.title.as_deref(), Some("part one part two"));
    }

    #[test]
    fn bash_execution_becomes_tool_pair_with_error_on_nonzero_exit() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path();
        let id = "bash-exec-0001";
        write_session(
            base,
            "--Users-demo-app--",
            "ts_bash-exec-0001.jsonl",
            &[
                json!({"type":"session","version":3,"id":id,"timestamp":"2026-06-27T10:00:00.000Z","cwd":"/Users/demo/app"}),
                json!({"type":"message","id":"m1","timestamp":"2026-06-27T10:00:01.000Z",
                       "message":{"role":"user","content":"run it"}}),
                json!({"type":"bashExecution","id":"b1","timestamp":"2026-06-27T10:00:02.000Z",
                       "command":"exit 1","output":"boom","exitCode":1,"cancelled":false,"truncated":false}),
            ],
        );

        let parser = PiParser::with_base_dir(base.to_path_buf());
        let detail = parser.get_conversation(id).expect("detail");

        let has_bash_use = detail.turns.iter().flat_map(|t| &t.blocks).any(|b| {
            matches!(b, ContentBlock::ToolUse { tool_name, input_preview, .. }
                if tool_name == "bash" && input_preview.as_deref() == Some("exit 1"))
        });
        assert!(has_bash_use, "bashExecution becomes a bash ToolUse");

        let errored = detail.turns.iter().flat_map(|t| &t.blocks).any(|b| {
            matches!(b, ContentBlock::ToolResult { is_error, output_preview, .. }
                if *is_error && output_preview.as_deref() == Some("boom"))
        });
        assert!(errored, "a non-zero exitCode marks the bash result an error");
    }

    #[test]
    fn malformed_lines_are_skipped_without_error() {
        let dir = tempdir().expect("tempdir");
        let base = dir.path();
        let id = "robust-0001";
        let session_dir = base.join("--Users-demo-app--");
        std::fs::create_dir_all(&session_dir).expect("dir");
        let mut file =
            std::fs::File::create(session_dir.join("ts_robust-0001.jsonl")).expect("file");
        writeln!(
            file,
            "{}",
            json!({"type":"session","version":3,"id":id,"timestamp":"2026-06-27T10:00:00.000Z","cwd":"/Users/demo/app"})
        )
        .unwrap();
        writeln!(file, "{{ this is not valid json").unwrap();
        writeln!(file).unwrap(); // blank line
        writeln!(
            file,
            "{}",
            json!({"type":"message","id":"m1","timestamp":"2026-06-27T10:00:01.000Z",
                   "message":{"role":"user","content":"hello"}})
        )
        .unwrap();
        // Unknown record type must not error.
        writeln!(
            file,
            "{}",
            json!({"type":"branch_summary","id":"x","timestamp":"2026-06-27T10:00:02.000Z","summary":"noise"})
        )
        .unwrap();

        let parser = PiParser::with_base_dir(base.to_path_buf());
        let summaries = parser.list_conversations().expect("list survives malformed lines");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, id);
        let detail = parser.get_conversation(id).expect("detail survives malformed lines");
        assert!(detail.turns.iter().any(|t| matches!(t.role, TurnRole::User)));
    }

    #[test]
    fn unknown_conversation_is_not_found() {
        let dir = tempdir().expect("tempdir");
        let parser = PiParser::with_base_dir(dir.path().to_path_buf());
        assert!(matches!(
            parser.get_conversation("nope"),
            Err(ParseError::ConversationNotFound(_))
        ));
    }
}
