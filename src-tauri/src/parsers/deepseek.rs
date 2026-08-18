use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{
    AgentType, ContentBlock, ConversationDetail, ConversationSummary, MessageTurn, TurnRole,
    TurnUsage,
};
use crate::parsers::{
    backfill_turn_durations, compute_session_stats, folder_name_from_path,
    infer_context_window_max_tokens, merge_context_window_stats, relocate_orphaned_tool_results,
    resolve_patch_line_numbers, structurize_read_tool_output, title_from_user_text, truncate_str,
    AgentParser, ParseError,
};

/// Expand a leading `~` the way `dsh-home-paths`' `expandHomePath` does: the
/// bare `~` and a `~/` or `~\` prefix only — a `~user` form is left verbatim,
/// exactly as upstream leaves it.
fn expand_home_prefix(value: &str, home_dir: Option<&PathBuf>) -> PathBuf {
    let Some(home) = home_dir else {
        return PathBuf::from(value);
    };
    if value == "~" {
        return home.clone();
    }
    if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        return home.join(rest);
    }
    PathBuf::from(value)
}

/// Resolve the DeepSeek Harness home the way `@deepseek-ai/dsh-home-paths`'
/// `resolveDshHome` does: `DSH_HOME` wins, else `~/.dsh`.
///
/// Two details are upstream's, not conveniences: a WHITESPACE-only `DSH_HOME`
/// counts as unset (`fromEnv.trim().length > 0`), so a blank override never
/// resolves the home to a whitespace path; and a leading `~` is expanded
/// (`expandHomePath`) before use. The value itself is otherwise passed through
/// untrimmed, again like upstream. Upstream additionally absolutizes a
/// relative value against the harness process's cwd — deliberately NOT
/// mirrored, because that cwd is the session workspace, not codeg's, so
/// resolving it here would name a directory the agent never uses.
fn resolve_dsh_home_from(dsh_home_env: Option<OsString>, home_dir: Option<PathBuf>) -> PathBuf {
    dsh_home_env
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(|value| expand_home_prefix(&value.to_string_lossy(), home_dir.as_ref()))
        .unwrap_or_else(|| home_dir.unwrap_or_default().join(".dsh"))
}

/// The DeepSeek Harness home (`DSH_HOME`, default `~/.dsh`) — the root of the
/// skills store, the credentials document, and (unless relocated) the session
/// logs.
pub(crate) fn resolve_dsh_home_dir() -> PathBuf {
    resolve_dsh_home_from(std::env::var_os("DSH_HOME"), dirs::home_dir())
}

/// Resolve the shared cross-agent home the way `dsh-skill-filesystem` does:
/// `DSH_AGENTS_HOME` wins, else `~/.agents`. Its `skills/` subdirectory is the
/// same store Codex, pi, Cursor and OpenCode read, so a skill linked there is
/// visible to DeepSeek too.
pub(crate) fn resolve_dsh_agents_home_dir() -> PathBuf {
    resolve_dsh_agents_home_from(std::env::var_os("DSH_AGENTS_HOME"), dirs::home_dir())
}

fn resolve_dsh_agents_home_from(
    agents_home_env: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    agents_home_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.unwrap_or_default().join(".agents"))
}

/// Resolve the session-log root the way deepseek-acp's `sessionsRoot` does:
/// `DEEPSEEK_ACP_SESSIONS_ROOT` wins, else `$DSH_HOME/sessions`, else
/// `~/.dsh/sessions`.
pub(crate) fn resolve_deepseek_sessions_root() -> PathBuf {
    resolve_deepseek_sessions_root_from(
        std::env::var_os("DEEPSEEK_ACP_SESSIONS_ROOT"),
        std::env::var_os("DSH_HOME"),
        dirs::home_dir(),
    )
}

fn resolve_deepseek_sessions_root_from(
    sessions_env: Option<OsString>,
    dsh_home_env: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    sessions_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| resolve_dsh_home_from(dsh_home_env, home_dir).join("sessions"))
}

/// DeepSeek Harness (driven through the `deepseek-acp` editor bridge) persists
/// each session as an **append-only event log**:
///
/// ```text
/// $DSH_HOME/sessions/               (default ~/.dsh/sessions; whole root
/// └── <munged cwd>/                  relocatable via DEEPSEEK_ACP_SESSIONS_ROOT)
///     └── <session uuid>/
///         └── session.jsonl.zstd    # or session.jsonl when compression=none
/// ```
///
/// The `.zstd` file is a sequence of complete Zstandard frames (one appended
/// per write batch); a standard streaming decode walks them all. Each line is
/// `{type, seq, time, data}` (`time` in epoch ms). The first line is the
/// session header (`type: "session"`, carrying `id` / `createdAt` / `cwd` /
/// `delegationDepth`). Content comes from:
///
/// - `user/message` — `data.source.kind` separates a real human prompt
///   (`"user"`) from plugin-injected runtime context (`"plugin"`, skipped).
/// - `assistant/message` — the ASSEMBLED assistant message for one step
///   (`content[]` of `text` / `reasoning` / `tool-call {id, name, arguments}`
///   blocks) plus that step's `usage` (`inputTokens` / `outputTokens` /
///   `cacheReadTokens` / `reasoningTokens`). The raw stream duplicates it as
///   `assistant/chunk` / `*-chunks` rows, which are skipped.
/// - `tool/result` — the paired result (`message.content[0]` is a
///   `tool-result` with `toolCallId` / `content[]` / `isError`).
/// - `turn/start` / `turn/end` — authoritative turn boundaries; `turn/end`
///   carries the end reason (`completed` / `aborted` / `error` / …) and its
///   `time` is the turn's completion clock. A turn missing its `turn/end`
///   (killed mid-flight) keeps honest `None` clocks.
/// - `request/header` — per-request `config.model`; `request/context` — the
///   advertised `contextWindow`; `session/title` — the running title
///   (last one wins).
///
/// Usage semantics: per-step records are SUMMED into the turn (mirroring the
/// Kimi parser); the context-window occupancy is the LATEST step's input side
/// (`inputTokens + cacheReadTokens`) — never the per-turn sum, which re-counts
/// the cached prefix once per step.
pub struct DeepSeekParser {
    base_dir: PathBuf,
}

impl DeepSeekParser {
    pub fn new() -> Self {
        Self {
            base_dir: resolve_deepseek_sessions_root(),
        }
    }

    /// Construct a parser pointed at an explicit sessions directory (test
    /// fixtures).
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    fn build_summary(&self, session_dir: &Path, session_id: &str) -> Option<ConversationSummary> {
        let parsed = parse_session_log(session_dir)?;
        // Sub-agent sessions (spawned by an upstream harness delegation) are
        // not editor conversations; only the top-level depth is listed.
        if parsed.delegation_depth > 0 {
            return None;
        }
        // A session that never produced a user/assistant/tool event (only the
        // header + config records) is treated as empty, matching the
        // "metadata-only is not listed" rule of the other parsers.
        if parsed.content_events == 0 {
            return None;
        }
        let started_at = parsed.created_at.or(parsed.first_ts)?;

        Some(ConversationSummary {
            id: session_id.to_string(),
            agent_type: AgentType::DeepSeek,
            folder_name: parsed.cwd.as_deref().map(folder_name_from_path),
            folder_path: parsed.cwd,
            title: parsed.title.or(parsed.first_user_text),
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

    fn build_detail(&self, session_dir: &Path, conversation_id: &str) -> ConversationDetail {
        let parsed = parse_session_log(session_dir).unwrap_or_default();

        let mut turns = parsed.turns;
        relocate_orphaned_tool_results(&mut turns);
        structurize_read_tool_output(&mut turns);
        resolve_patch_line_numbers(&mut turns, parsed.cwd.as_deref());
        backfill_turn_durations(&mut turns, &[]);

        // Context-window occupancy is the LATEST step's input side; capacity
        // comes from the log's own `request/context.contextWindow` (falling
        // back to model-name inference for logs that never recorded one).
        let max_tokens = parsed
            .context_window
            .or_else(|| infer_context_window_max_tokens(parsed.model.as_deref()));
        let session_stats = merge_context_window_stats(
            compute_session_stats(&turns),
            parsed.last_step_input_side,
            max_tokens,
        );

        let summary = ConversationSummary {
            id: conversation_id.to_string(),
            agent_type: AgentType::DeepSeek,
            folder_name: parsed.cwd.as_deref().map(folder_name_from_path),
            folder_path: parsed.cwd,
            title: parsed.title.or(parsed.first_user_text),
            started_at: parsed
                .created_at
                .or(parsed.first_ts)
                .unwrap_or_else(Utc::now),
            ended_at: parsed.last_ts,
            message_count: parsed.message_count,
            model: parsed.model,
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

    /// Locate the `<session uuid>` directory matching `conversation_id` across
    /// the `base_dir/<munged cwd>/` buckets (two shallow levels).
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

impl Default for DeepSeekParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentParser for DeepSeekParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let mut conversations = Vec::new();
        if !self.base_dir.is_dir() {
            return Ok(conversations);
        }

        for bucket in read_subdirs(&self.base_dir) {
            for session_dir in read_subdirs(&bucket) {
                let Some(session_id) = session_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                else {
                    continue;
                };
                if let Some(summary) = self.build_summary(&session_dir, &session_id) {
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
        Ok(self.build_detail(&session_dir, conversation_id))
    }
}

/// The accumulated result of scanning one session log.
#[derive(Default)]
struct SessionParse {
    turns: Vec<MessageTurn>,
    cwd: Option<String>,
    created_at: Option<DateTime<Utc>>,
    delegation_depth: u64,
    /// Latest `session/title` (the title service re-emits as it improves).
    title: Option<String>,
    /// First user prompt, already truncated for use as a fallback title.
    first_user_text: Option<String>,
    /// Latest `request/header` model (fallback: the assistant message source).
    model: Option<String>,
    /// Latest `request/context.contextWindow`.
    context_window: Option<u64>,
    /// The latest single step's input side (`inputTokens + cacheReadTokens`) —
    /// the context-window occupancy. NOT the per-turn sum (see the type doc).
    last_step_input_side: Option<u64>,
    first_ts: Option<DateTime<Utc>>,
    last_ts: Option<DateTime<Utc>>,
    /// User prompts + assistant text messages, the list view's activity count.
    message_count: u32,
    /// Content-bearing records — whether the session is worth listing at all.
    content_events: u32,
}

/// Read a session's log text: the Zstandard file when present, else the
/// plaintext `session.jsonl` written by a `compression: "none"` deployment.
fn read_session_log_text(session_dir: &Path) -> Option<String> {
    let zstd_path = session_dir.join("session.jsonl.zstd");
    match fs::read(&zstd_path) {
        Ok(bytes) => decode_zstd_frames_prefix(&bytes),
        Err(_) => fs::read_to_string(session_dir.join("session.jsonl")).ok(),
    }
}

/// Decode every complete Zstandard frame, KEEPING the prefix when the stream
/// errors partway. The writer appends one frame per batch, so a concurrent
/// read can catch the final frame half-written — failing the whole decode
/// there would make a LIVE session vanish from the list until the next flush.
/// The JSONL walk upstream skips whatever broken tail line the cut leaves.
fn decode_zstd_frames_prefix(bytes: &[u8]) -> Option<String> {
    use std::io::Read as _;
    let mut decoder = zstd::stream::read::Decoder::with_buffer(bytes).ok()?;
    let mut decoded = Vec::new();
    // On Err, `read_to_end` has already appended every byte that decoded
    // cleanly before the failure — exactly the prefix to keep.
    let _ = decoder.read_to_end(&mut decoded);
    // Lossy: one bad byte must not drop the whole session; the JSON lines
    // that decode cleanly still parse.
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

fn parse_session_log(session_dir: &Path) -> Option<SessionParse> {
    Some(parse_session_events(&read_session_log_text(session_dir)?))
}

/// Tool-call arguments arrive as the model's raw JSON string. Store it
/// verbatim when small; over the cap, re-serialize with string VALUES capped so
/// the stored preview stays VALID JSON (the delegation card `JSON.parse`s it to
/// recover `task` / `agent_type` — an opaque byte truncation would corrupt it).
/// Unparseable arguments (a model that emitted broken JSON) fall back to the
/// plain truncation, which cannot make them any less parseable.
const DEEPSEEK_TOOL_INPUT_CAP: usize = 8_000;

fn deepseek_tool_input_preview(arguments: &str) -> Option<String> {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= DEEPSEEK_TOOL_INPUT_CAP {
        return Some(trimmed.to_string());
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => crate::parsers::grok::cap_json_to_budget(&value, DEEPSEEK_TOOL_INPUT_CAP),
        Err(_) => Some(truncate_str(trimmed, DEEPSEEK_TOOL_INPUT_CAP)),
    }
}

fn event_millis(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(value.get("time")?.as_i64()?)
}

/// Concatenate the `text` of every `{type:"text"}` part in a message
/// `content[]` array.
fn collect_text_parts(content: Option<&Value>) -> String {
    let mut out = String::new();
    if let Some(items) = content.and_then(Value::as_array) {
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
        }
    }
    out
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

/// Map one step's `usage` object onto `TurnUsage`; `None` when all counters
/// are absent or zero. `reasoningTokens` is a subset of `outputTokens` (the
/// adapter reports both), so it is deliberately not added anywhere.
fn usage_from_step(usage: Option<&Value>) -> Option<TurnUsage> {
    let usage = usage?;
    let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let input = field("inputTokens");
    let output = field("outputTokens");
    let cache_read = field("cacheReadTokens");
    if input == 0 && output == 0 && cache_read == 0 {
        return None;
    }
    Some(TurnUsage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read,
    })
}

fn parse_session_events(text: &str) -> SessionParse {
    let mut sp = SessionParse::default();

    // The open assistant turn for the CURRENT log turn: index into `sp.turns`,
    // the accumulated per-step usage, and the `turn/start` clock.
    let mut open_assistant: Option<usize> = None;
    let mut pending_usage: Option<TurnUsage> = None;
    let mut turn_started_at: Option<DateTime<Utc>> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        // Raw stream chunks duplicate `assistant/message` content (and the
        // compacted `*-chunks` rows carry `seq0`/`time0` instead of `time`).
        if matches!(
            event_type,
            "assistant/chunk" | "tool-call-chunks" | "reasoning-chunks" | "text-chunks"
        ) {
            continue;
        }
        let ts = event_millis(&value);
        if let Some(ts) = ts {
            sp.first_ts.get_or_insert(ts);
            sp.last_ts = Some(ts);
        }
        let data = value.get("data");

        match event_type {
            "session" => {
                sp.cwd = value
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from);
                sp.created_at = value
                    .get("createdAt")
                    .and_then(Value::as_i64)
                    .and_then(DateTime::from_timestamp_millis);
                sp.delegation_depth = value
                    .get("delegationDepth")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
            }
            "turn/start" => {
                // Defensive: a dangling assistant turn from a log missing its
                // `turn/end` must not absorb the next turn's steps.
                finalize_assistant(
                    &mut sp.turns,
                    &mut open_assistant,
                    &mut pending_usage,
                    None,
                    turn_started_at,
                );
                turn_started_at = ts;
            }
            "turn/end" => {
                finalize_assistant(
                    &mut sp.turns,
                    &mut open_assistant,
                    &mut pending_usage,
                    ts,
                    turn_started_at,
                );
                turn_started_at = None;
            }
            "user/message" => {
                let Some(data) = data else { continue };
                // Plugin-injected runtime-context snapshots (sandbox/approval
                // policy, file-change notices, …) are model plumbing, not
                // conversation content.
                let kind = data
                    .pointer("/source/kind")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if kind != "user" {
                    continue;
                }
                let text = collect_text_parts(data.get("content"));
                if text.trim().is_empty() {
                    continue;
                }
                if sp.first_user_text.is_none() {
                    sp.first_user_text = Some(title_from_user_text(text.trim()));
                }
                sp.content_events += 1;
                sp.message_count += 1;
                let ts = ts.or(sp.last_ts).unwrap_or_else(Utc::now);
                sp.turns.push(MessageTurn {
                    id: format!("turn-{}", sp.turns.len()),
                    role: TurnRole::User,
                    blocks: vec![ContentBlock::Text { text }],
                    timestamp: ts,
                    usage: None,
                    duration_ms: None,
                    model: None,
                    completed_at: Some(ts),
                });
            }
            "request/header" => {
                if let Some(model) = data
                    .and_then(|d| d.pointer("/header/config/model"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    sp.model = Some(model.to_string());
                }
            }
            "request/context" => {
                if let Some(window) = data
                    .and_then(|d| d.get("contextWindow"))
                    .and_then(Value::as_u64)
                    .filter(|w| *w > 0)
                {
                    sp.context_window = Some(window);
                }
            }
            "session/title" => {
                if let Some(title) = data
                    .and_then(|d| d.get("title"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    sp.title = Some(title.to_string());
                }
            }
            "assistant/message" => {
                let Some(data) = data else { continue };
                let step_usage = usage_from_step(data.get("usage"));
                if let Some(usage) = &step_usage {
                    sp.last_step_input_side = Some(
                        usage
                            .input_tokens
                            .saturating_add(usage.cache_read_input_tokens),
                    );
                    pending_usage = Some(match pending_usage.take() {
                        Some(prev) => add_usage(prev, usage.clone()),
                        None => usage.clone(),
                    });
                }
                let message = data.get("message");
                let source_model = message
                    .and_then(|m| m.pointer("/source/model"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from);
                if sp.model.is_none() {
                    sp.model.clone_from(&source_model);
                }
                let Some(blocks) = message
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                // The list-view activity count grows once per assistant
                // MESSAGE with text, not once per text block inside it.
                let mut counted_text = false;
                for block in blocks {
                    let rendered = match block.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text" => {
                            let text = block
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if text.trim().is_empty() {
                                continue;
                            }
                            if !counted_text {
                                sp.message_count += 1;
                                counted_text = true;
                            }
                            ContentBlock::Text {
                                text: text.to_string(),
                            }
                        }
                        "reasoning" => {
                            let text = block
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if text.trim().is_empty() {
                                continue;
                            }
                            ContentBlock::Thinking {
                                text: text.to_string(),
                            }
                        }
                        "tool-call" => ContentBlock::ToolUse {
                            tool_use_id: block.get("id").and_then(Value::as_str).map(String::from),
                            tool_name: block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                            input_preview: block
                                .get("arguments")
                                .and_then(Value::as_str)
                                .and_then(deepseek_tool_input_preview),
                            status: None,
                            meta: None,
                        },
                        _ => continue,
                    };
                    sp.content_events += 1;
                    let ts = ts.or(sp.last_ts).unwrap_or_else(Utc::now);
                    let turn = ensure_assistant(
                        &mut sp.turns,
                        &mut open_assistant,
                        ts,
                        sp.model.clone().or(source_model.clone()),
                    );
                    turn.blocks.push(rendered);
                }
            }
            "tool/result" => {
                let Some(data) = data else { continue };
                let result = data
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .and_then(|items| {
                        items
                            .iter()
                            .find(|i| i.get("type").and_then(Value::as_str) == Some("tool-result"))
                    });
                let tool_use_id = result
                    .and_then(|r| r.get("toolCallId"))
                    .and_then(Value::as_str)
                    .or_else(|| {
                        data.pointer("/message/source/callId")
                            .and_then(Value::as_str)
                    })
                    .map(String::from);
                let output = result
                    .map(|r| collect_text_parts(r.get("content")))
                    .unwrap_or_default();
                // `data.error` is the tool's internal failure identity; the
                // model-facing `isError` flag is the primary signal.
                let is_error = result
                    .and_then(|r| r.get("isError"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || data.get("error").is_some_and(|e| !e.is_null());
                sp.content_events += 1;
                let ts = ts.or(sp.last_ts).unwrap_or_else(Utc::now);
                let model = sp.model.clone();
                let turn = ensure_assistant(&mut sp.turns, &mut open_assistant, ts, model);
                turn.blocks.push(ContentBlock::ToolResult {
                    tool_use_id,
                    output_preview: (!output.trim().is_empty()).then_some(output),
                    is_error,
                    agent_stats: None,
                    images: Vec::new(),
                });
            }
            // `agent/inbox/spliced` (queue bookkeeping), `step/start`/`step/end`
            // (sub-turn markers), `sandbox/mode`, `todo/write` (the todo_write
            // TOOL call already renders), and `session/end-seed` (resume seed
            // boundary) carry no conversation content.
            _ => {}
        }
    }

    finalize_assistant(
        &mut sp.turns,
        &mut open_assistant,
        &mut pending_usage,
        None,
        turn_started_at,
    );
    sp
}

/// The open assistant turn for the current log turn, created on first use.
fn ensure_assistant<'a>(
    turns: &'a mut Vec<MessageTurn>,
    open_assistant: &mut Option<usize>,
    ts: DateTime<Utc>,
    model: Option<String>,
) -> &'a mut MessageTurn {
    let idx = match open_assistant {
        Some(idx) => *idx,
        None => {
            turns.push(MessageTurn {
                id: format!("turn-{}", turns.len()),
                role: TurnRole::Assistant,
                blocks: Vec::new(),
                timestamp: ts,
                usage: None,
                duration_ms: None,
                model,
                completed_at: None,
            });
            let idx = turns.len() - 1;
            *open_assistant = Some(idx);
            idx
        }
    };
    &mut turns[idx]
}

/// Close the open assistant turn: attach the summed step usage, and — when the
/// log recorded a `turn/end` — the completion clock and the `turn/start`-based
/// duration. A turn missing its `turn/end` (killed mid-flight) keeps honest
/// `None` clocks; `backfill_turn_durations` tiles an estimate later.
fn finalize_assistant(
    turns: &mut [MessageTurn],
    open_assistant: &mut Option<usize>,
    pending_usage: &mut Option<TurnUsage>,
    ended_at: Option<DateTime<Utc>>,
    started_at: Option<DateTime<Utc>>,
) {
    let Some(idx) = open_assistant.take() else {
        *pending_usage = None;
        return;
    };
    let Some(turn) = turns.get_mut(idx) else {
        *pending_usage = None;
        return;
    };
    if let Some(usage) = pending_usage.take() {
        turn.usage = Some(match turn.usage.take() {
            Some(existing) => add_usage(existing, usage),
            None => usage,
        });
    }
    if let Some(end) = ended_at {
        turn.completed_at = Some(end);
        if let Some(start) = started_at {
            let ms = (end - start).num_milliseconds();
            if ms > 0 {
                turn.duration_ms = Some(ms as u64);
            }
        }
    }
}

/// List immediate sub-directories of `dir` (empty when `dir` is missing or not
/// a directory). Shallow by design — the layout is exactly two levels deep.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_home_prefers_env_override() {
        assert_eq!(
            resolve_dsh_home_from(
                Some(OsString::from("/tmp/custom-dsh")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/tmp/custom-dsh")
        );
        assert_eq!(
            resolve_dsh_home_from(None, Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.dsh")
        );
        // An empty env value must not produce an empty base path.
        assert_eq!(
            resolve_dsh_home_from(Some(OsString::new()), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.dsh")
        );
        // Upstream `resolveDshHome` treats a WHITESPACE-only override as unset
        // too ("a blank override never resolves the home to the cwd").
        assert_eq!(
            resolve_dsh_home_from(Some(OsString::from("   ")), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.dsh")
        );
        // ... and expands a leading `~` (`expandHomePath`) before use, so a
        // `DSH_HOME=~/custom` names the same directory for codeg as for the
        // agent instead of a literal `./~/custom`.
        assert_eq!(
            resolve_dsh_home_from(
                Some(OsString::from("~/custom-dsh")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/home/demo/custom-dsh")
        );
        assert_eq!(
            resolve_dsh_home_from(Some(OsString::from("~")), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo")
        );
        // `~user` is NOT a prefix upstream expands — kept verbatim.
        assert_eq!(
            resolve_dsh_home_from(Some(OsString::from("~root/x")), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("~root/x")
        );
    }

    #[test]
    fn resolve_agents_home_prefers_env_override() {
        assert_eq!(
            resolve_dsh_agents_home_from(
                Some(OsString::from("/tmp/shared-agents")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/tmp/shared-agents")
        );
        assert_eq!(
            resolve_dsh_agents_home_from(None, Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.agents")
        );
        assert_eq!(
            resolve_dsh_agents_home_from(Some(OsString::new()), Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.agents")
        );
    }

    #[test]
    fn resolve_sessions_root_mirrors_adapter_chain() {
        // Explicit sessions root wins over everything.
        assert_eq!(
            resolve_deepseek_sessions_root_from(
                Some(OsString::from("/data/ds-sessions")),
                Some(OsString::from("/tmp/dsh")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/data/ds-sessions")
        );
        // Else $DSH_HOME/sessions.
        assert_eq!(
            resolve_deepseek_sessions_root_from(
                None,
                Some(OsString::from("/tmp/dsh")),
                Some(PathBuf::from("/home/demo")),
            ),
            PathBuf::from("/tmp/dsh/sessions")
        );
        // Else ~/.dsh/sessions.
        assert_eq!(
            resolve_deepseek_sessions_root_from(None, None, Some(PathBuf::from("/home/demo"))),
            PathBuf::from("/home/demo/.dsh/sessions")
        );
    }

    fn header_line(cwd: &str) -> String {
        json!({
            "type": "session",
            "version": 0,
            "id": "0126397e-97b1-4420-a564-bffe4453915b",
            "createdAt": 1_786_708_736_990_i64,
            "cwd": cwd,
            "delegationDepth": 0
        })
        .to_string()
    }

    fn event(event_type: &str, seq: u64, time: i64, data: Value) -> String {
        json!({"type": event_type, "seq": seq, "time": time, "data": data}).to_string()
    }

    /// A faithful miniature of a real session: one turn with a user prompt, a
    /// plugin context snapshot, a reasoning + tool-call step, its result, and a
    /// closing text step.
    fn sample_log() -> String {
        let user = event(
            "user/message",
            4,
            1_786_708_737_018,
            json!({
                "content": [{"type": "text", "text": "读一下 /tmp/target.txt"}],
                "source": {"kind": "user"},
                "role": "user",
                "id": "u-1"
            }),
        );
        let plugin = event(
            "user/message",
            5,
            1_786_708_737_019,
            json!({
                "content": [{"type": "text", "text": "Current runtime context."}],
                "source": {"kind": "plugin", "plugin": "@deepseek-ai/dsh-system-prompt"},
                "role": "user",
                "id": "u-2"
            }),
        );
        [
            header_line("/Users/demo/project"),
            event("turn/start", 1, 1_786_708_737_006, json!({"turn": 1})),
            event("step/start", 3, 1_786_708_737_017, json!({"turn": 1, "step": 1})),
            user,
            plugin,
            event(
                "session/title",
                6,
                1_786_708_737_020,
                json!({"title": "读一下 /tmp/target.txt", "messageSeqs": [4], "source": {"kind": "fallback"}}),
            ),
            event(
                "request/header",
                7,
                1_786_708_737_022,
                json!({"header": {"config": {"provider": "deepseek-official", "model": "deepseek-v4-flash", "maxTokens": 256_000, "reasoningEffort": "high"}}, "reason": "initial"}),
            ),
            event(
                "request/context",
                8,
                1_786_708_737_023,
                json!({"provider": "deepseek-official", "model": "deepseek-v4-flash", "contextWindow": 1_000_000}),
            ),
            event(
                "assistant/message",
                29,
                1_786_708_738_169,
                json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "reasoning", "text": "先读文件。"},
                            {"type": "tool-call", "id": "call_1", "name": "read", "arguments": "{\"file_path\": \"/tmp/target.txt\"}"}
                        ],
                        "source": {"kind": "model", "provider": "deepseek-official", "model": "deepseek-v4-flash"},
                        "id": "a-1"
                    },
                    "usage": {"inputTokens": 1514, "outputTokens": 49, "cacheReadTokens": 1792, "reasoningTokens": 12}
                }),
            ),
            event(
                "tool/call",
                30,
                1_786_708_738_172,
                json!({"turn": 1, "step": 1, "callId": "call_1", "name": "read", "arguments": "{\"file_path\": \"/tmp/target.txt\"}"}),
            ),
            event(
                "tool/result",
                31,
                1_786_708_738_183,
                json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "source": {"kind": "tool", "callId": "call_1"},
                        "content": [{
                            "type": "tool-result",
                            "toolCallId": "call_1",
                            "content": [{"type": "text", "text": "<content>hello</content>"}],
                            "isError": false
                        }],
                        "role": "user",
                        "id": "t-1"
                    },
                    "meta": {"path": "/tmp/target.txt"}
                }),
            ),
            event(
                "assistant/message",
                40,
                1_786_708_739_000,
                json!({
                    "turn": 1, "step": 2,
                    "message": {
                        "role": "assistant",
                        "content": [{"type": "text", "text": "文件内容是 hello。"}],
                        "source": {"kind": "model", "provider": "deepseek-official", "model": "deepseek-v4-flash"},
                        "id": "a-2"
                    },
                    "usage": {"inputTokens": 111, "outputTokens": 100, "cacheReadTokens": 5248, "reasoningTokens": 0}
                }),
            ),
            event("turn/end", 41, 1_786_708_739_100, json!({"turn": 1, "reason": {"kind": "completed"}})),
            event("session/end-seed", 42, 1_786_708_739_101, json!({})),
        ]
        .join("\n")
    }

    #[test]
    fn parses_a_full_turn_with_usage_duration_and_title() {
        let sp = parse_session_events(&sample_log());

        assert_eq!(sp.cwd.as_deref(), Some("/Users/demo/project"));
        assert_eq!(sp.delegation_depth, 0);
        assert_eq!(sp.title.as_deref(), Some("读一下 /tmp/target.txt"));
        assert_eq!(sp.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(sp.context_window, Some(1_000_000));
        // Latest step's input side, never the per-turn sum.
        assert_eq!(sp.last_step_input_side, Some(111 + 5248));

        // One user turn + ONE assistant turn per log turn (the plugin context
        // snapshot is filtered).
        assert_eq!(sp.turns.len(), 2);
        assert!(matches!(sp.turns[0].role, TurnRole::User));
        assert_eq!(sp.turns[0].blocks.len(), 1);
        assert!(matches!(
            &sp.turns[0].blocks[0],
            ContentBlock::Text { text } if text == "读一下 /tmp/target.txt"
        ));

        let assistant = &sp.turns[1];
        assert!(matches!(assistant.role, TurnRole::Assistant));
        assert_eq!(assistant.model.as_deref(), Some("deepseek-v4-flash"));
        // reasoning → Thinking, tool-call → ToolUse, result → ToolResult,
        // closing text → Text, in stream order.
        assert_eq!(assistant.blocks.len(), 4);
        assert!(
            matches!(&assistant.blocks[0], ContentBlock::Thinking { text } if text == "先读文件。")
        );
        assert!(matches!(
            &assistant.blocks[1],
            ContentBlock::ToolUse { tool_use_id: Some(id), tool_name, input_preview: Some(input), .. }
                if id == "call_1" && tool_name == "read" && input.contains("/tmp/target.txt")
        ));
        assert!(matches!(
            &assistant.blocks[2],
            ContentBlock::ToolResult { tool_use_id: Some(id), output_preview: Some(out), is_error: false, .. }
                if id == "call_1" && out.contains("hello")
        ));
        assert!(
            matches!(&assistant.blocks[3], ContentBlock::Text { text } if text == "文件内容是 hello。")
        );

        // Steps SUMMED into the turn.
        let usage = assistant.usage.as_ref().expect("usage");
        assert_eq!(usage.input_tokens, 1514 + 111);
        assert_eq!(usage.output_tokens, 49 + 100);
        assert_eq!(usage.cache_read_input_tokens, 1792 + 5248);
        assert_eq!(usage.cache_creation_input_tokens, 0);

        // turn/start (…737_006) → turn/end (…739_100).
        assert_eq!(assistant.duration_ms, Some(2094));
        assert_eq!(
            assistant.completed_at,
            DateTime::from_timestamp_millis(1_786_708_739_100)
        );

        // user prompt + assistant text message.
        assert_eq!(sp.message_count, 2);
        assert_eq!(
            sp.created_at,
            DateTime::from_timestamp_millis(1_786_708_736_990)
        );
    }

    #[test]
    fn aborted_turn_without_end_keeps_honest_clocks() {
        let log = [
            header_line("/Users/demo/project"),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "user/message",
                2,
                1_010,
                json!({"content": [{"type": "text", "text": "hi"}], "source": {"kind": "user"}, "role": "user", "id": "u"}),
            ),
            event(
                "assistant/message",
                3,
                1_500,
                json!({
                    "turn": 1, "step": 1,
                    "message": {"role": "assistant", "content": [{"type": "text", "text": "part"}], "source": {"kind": "model", "model": "deepseek-v4"}, "id": "a"},
                    "usage": {"inputTokens": 10, "outputTokens": 5, "cacheReadTokens": 0}
                }),
            ),
            // Killed mid-flight: no turn/end, no session/end-seed.
        ]
        .join("\n");

        let sp = parse_session_events(&log);
        assert_eq!(sp.turns.len(), 2);
        let assistant = &sp.turns[1];
        // Usage still flushed at EOF; clocks stay honest.
        assert_eq!(assistant.usage.as_ref().unwrap().input_tokens, 10);
        assert_eq!(assistant.completed_at, None);
        assert_eq!(assistant.duration_ms, None);
        // The header model fallback comes from the assistant message source.
        assert_eq!(sp.model.as_deref(), Some("deepseek-v4"));
    }

    #[test]
    fn tool_error_results_are_flagged() {
        let log = [
            header_line("/w"),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "tool/result",
                2,
                1_100,
                json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "source": {"kind": "tool", "callId": "c1"},
                        "content": [{"type": "tool-result", "toolCallId": "c1", "content": [{"type": "text", "text": "Error: MCP error -32001: Request timed out"}], "isError": true}],
                        "role": "user", "id": "t"
                    }
                }),
            ),
        ]
        .join("\n");
        let sp = parse_session_events(&log);
        assert_eq!(sp.turns.len(), 1);
        assert!(matches!(
            &sp.turns[0].blocks[0],
            ContentBlock::ToolResult { is_error: true, output_preview: Some(out), .. }
                if out.contains("timed out")
        ));
    }

    #[test]
    fn oversized_tool_arguments_stay_valid_json() {
        let long_task = "长".repeat(DEEPSEEK_TOOL_INPUT_CAP);
        let arguments =
            json!({"agent_type": "codex", "task": long_task, "working_dir": "/w"}).to_string();
        assert!(arguments.len() > DEEPSEEK_TOOL_INPUT_CAP);

        let preview = deepseek_tool_input_preview(&arguments).expect("preview");
        assert!(preview.len() <= DEEPSEEK_TOOL_INPUT_CAP);
        let parsed: Value = serde_json::from_str(&preview).expect("valid JSON");
        assert_eq!(parsed["agent_type"], "codex");
        assert_eq!(parsed["working_dir"], "/w");

        // Small arguments pass through byte-identical.
        let small = "{\"file_path\": \"/tmp/a\"}";
        assert_eq!(deepseek_tool_input_preview(small).as_deref(), Some(small));
    }

    fn write_session(dir: &Path, bucket: &str, id: &str, log: &str, compressed: bool) {
        let session_dir = dir.join(bucket).join(id);
        fs::create_dir_all(&session_dir).expect("mkdir");
        if compressed {
            // Two separately-encoded frames concatenated — the exact shape the
            // append-only writer produces — must decode as one stream. The
            // split point is arbitrary (frames carry bytes, not lines).
            let raw = log.as_bytes();
            let split = raw.len() / 2;
            let mut bytes = zstd::stream::encode_all(&raw[..split], 0).expect("frame 1");
            bytes.extend(zstd::stream::encode_all(&raw[split..], 0).expect("frame 2"));
            fs::write(session_dir.join("session.jsonl.zstd"), bytes).expect("write zstd");
        } else {
            fs::write(session_dir.join("session.jsonl"), log).expect("write jsonl");
        }
    }

    #[test]
    fn lists_and_loads_a_zstd_multi_frame_session() {
        let dir = std::env::temp_dir().join(format!("deepseek-parser-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let id = "0126397e-97b1-4420-a564-bffe4453915b";
        write_session(&dir, "--Users-demo-project--", id, &sample_log(), true);

        let parser = DeepSeekParser::with_base_dir(dir.clone());
        let conversations = parser.list_conversations().expect("list");
        assert_eq!(conversations.len(), 1);
        let summary = &conversations[0];
        assert_eq!(summary.id, id);
        assert_eq!(summary.agent_type, AgentType::DeepSeek);
        assert_eq!(summary.folder_path.as_deref(), Some("/Users/demo/project"));
        assert_eq!(summary.folder_name.as_deref(), Some("project"));
        assert_eq!(summary.title.as_deref(), Some("读一下 /tmp/target.txt"));
        assert_eq!(summary.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(summary.message_count, 2);

        let detail = parser.get_conversation(id).expect("detail");
        assert_eq!(detail.turns.len(), 2);
        let stats = detail.session_stats.expect("stats");
        assert_eq!(stats.context_window_used_tokens, Some(111 + 5248));
        assert_eq!(stats.context_window_max_tokens, Some(1_000_000));
        let total = stats.total_usage.expect("usage");
        assert_eq!(total.input_tokens, 1514 + 111);
        assert_eq!(total.output_tokens, 49 + 100);

        let _ = fs::remove_dir_all(&dir);
    }

    // Mid-write race: the appender's final frame can be half-written when a
    // list refresh reads the file. The complete frames before it must still
    // parse — dropping them would make a LIVE session vanish from the list.
    #[test]
    fn torn_trailing_frame_keeps_the_decoded_prefix() {
        let mut bytes =
            zstd::stream::encode_all(sample_log().as_bytes(), 0).expect("complete frame");
        let torn = zstd::stream::encode_all(
            format!(
                "\n{}",
                event("turn/start", 50, 1_786_708_740_000, json!({"turn": 2}))
            )
            .as_bytes(),
            0,
        )
        .expect("tail frame");
        bytes.extend(&torn[..torn.len() / 2]);

        let text = decode_zstd_frames_prefix(&bytes).expect("prefix survives");
        let sp = parse_session_events(&text);
        assert_eq!(sp.turns.len(), 2);
        assert_eq!(sp.title.as_deref(), Some("读一下 /tmp/target.txt"));
    }

    #[test]
    fn assistant_message_with_many_text_blocks_counts_once() {
        let log = [
            header_line("/w"),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "assistant/message",
                2,
                1_500,
                json!({
                    "turn": 1, "step": 1,
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "text", "text": "part one"},
                            {"type": "text", "text": "part two"}
                        ],
                        "source": {"kind": "model", "model": "deepseek-v4"},
                        "id": "a"
                    }
                }),
            ),
        ]
        .join("\n");
        let sp = parse_session_events(&log);
        assert_eq!(sp.message_count, 1);
        assert_eq!(sp.turns[0].blocks.len(), 2);
    }

    #[test]
    fn skips_empty_plaintext_and_subagent_sessions() {
        let dir = std::env::temp_dir().join(format!("deepseek-parser-skip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        // Header-only session: metadata is not a conversation.
        write_session(&dir, "--w--", "empty", &header_line("/w"), false);
        // A delegated sub-agent session is not an editor conversation.
        let child = [
            json!({"type": "session", "version": 0, "id": "child", "createdAt": 1_000, "cwd": "/w", "delegationDepth": 1}).to_string(),
            event("turn/start", 1, 1_000, json!({"turn": 1})),
            event(
                "user/message",
                2,
                1_010,
                json!({"content": [{"type": "text", "text": "sub"}], "source": {"kind": "user"}, "role": "user", "id": "u"}),
            ),
        ]
        .join("\n");
        write_session(&dir, "--w--", "child", &child, false);
        // A plaintext (compression: none) session IS listed.
        write_session(&dir, "--w--", "plain", &sample_log(), false);

        let parser = DeepSeekParser::with_base_dir(dir.clone());
        let conversations = parser.list_conversations().expect("list");
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].id, "plain");

        let _ = fs::remove_dir_all(&dir);
    }
}
