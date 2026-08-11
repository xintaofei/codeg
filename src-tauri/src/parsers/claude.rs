use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use chrono::{DateTime, Utc};
use regex::Regex;

use crate::models::*;
use crate::parsers::{
    folder_name_from_path, is_safe_subagent_id, title_from_user_text, truncate_str, AgentParser,
    ParseError,
};

/// Regex that matches Claude Code system-injected XML tags and their content.
/// These tags are internal metadata and should not be displayed to users.
/// Note: Rust regex doesn't support backreferences, so each tag is listed explicitly.
fn system_tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"(?s)",
            r"<system-reminder>.*?</system-reminder>",
            r"|<local-command-caveat>.*?</local-command-caveat>",
            r"|<command-name>.*?</command-name>",
            r"|<command-message>.*?</command-message>",
            r"|<command-args>.*?</command-args>",
            r"|<local-command-stdout>.*?</local-command-stdout>",
            r"|<user-prompt-submit-hook>.*?</user-prompt-submit-hook>",
            r"|<task-notification>.*?</task-notification>",
            r"|<fast_mode_info>.*?</fast_mode_info>",
        ))
        .unwrap()
    })
}

/// Sentinel prefixing the structured lifecycle payload the parser writes into
/// an async-launch ack's `output_preview` (see
/// `ClaudeRecordAccumulator::finalize_background_lifecycle`). The frontend
/// (`lib/background-agent.ts`) splits on it and renders a lifecycle card;
/// anything else renders the preview verbatim, so the prefix must never occur
/// in organic tool output.
pub(crate) const BACKGROUND_TASK_MARKER: &str = "[[codeg-background-task]]";

/// Cap for the folded `<result>` markdown carried on the lifecycle marker —
/// generous for a sub-agent summary, bounded against a pathological one. Also
/// applied by `background_watch.rs` to the `<result>` it carries on a live
/// `settled` event, so the live-flipped card matches the cold-parse cap and an
/// oversized report can't blow the event-stream size budget.
pub(crate) const BACKGROUND_RESULT_MAX_CHARS: usize = 16_000;

/// Latest `<task-notification>` observed for a background task id.
struct BackgroundNotification {
    status: String,
    summary: Option<String>,
    result: Option<String>,
}

pub(crate) fn task_notification_task_id_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<task-id>(.*?)</task-id>").unwrap())
}

pub(crate) fn task_notification_status_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<status>(.*?)</status>").unwrap())
}

pub(crate) fn task_notification_summary_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<summary>(.*?)</summary>").unwrap())
}

/// The `<tool-use-id>` of the launching tool call, carried by every async
/// sub-agent `<task-notification>`. Lets the background watcher tie a settlement
/// back to the exact launch card without a separate ack→id map (both ids are
/// siblings in the notification record). Background-shell notifications don't
/// carry this tag.
pub(crate) fn task_notification_tool_use_id_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<tool-use-id>(.*?)</tool-use-id>").unwrap())
}

pub(crate) fn task_notification_result_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<result>(.*?)</result>").unwrap())
}

/// First capture group of `re` in `text`, trimmed; `None` when absent/empty.
pub(crate) fn capture_tag(re: &Regex, text: &str) -> Option<String> {
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Strip system-injected XML tags from text content.
/// Returns None if the text becomes empty after stripping.
fn strip_system_tags(text: &str) -> Option<String> {
    let cleaned = system_tag_regex().replace_all(text, "");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Regex capturing the inner text of a `<command-name>...</command-name>` tag.
fn command_name_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<command-name>(.*?)</command-name>").unwrap())
}

/// Regex capturing the inner text of a `<command-args>...</command-args>` tag.
fn command_args_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<command-args>(.*?)</command-args>").unwrap())
}

/// Render a user-typed slash command for display.
///
/// Claude Code persists a slash command (e.g. `/init`, `/brainstorming`) as a
/// user message whose string content holds `<command-name>`, `<command-message>`
/// and `<command-args>` tags. Reconstruct the original input as `/name args`
/// (e.g. `/init 初始化`). Returns `None` when no `<command-name>` tag is present.
pub(crate) fn slash_command_display(text: &str) -> Option<String> {
    let name = command_name_regex()
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim())
        .filter(|n| n.starts_with('/'))?;

    let args = command_args_regex()
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim())
        .unwrap_or("");

    if args.is_empty() {
        Some(name.to_string())
    } else {
        Some(format!("{name} {args}"))
    }
}

/// A user JSONL entry's slash command, if its string content carries command
/// tags. Returns `(display, promptId)`.
fn slash_command_value_display(value: &serde_json::Value) -> Option<(String, Option<String>)> {
    let text = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())?;
    let display = slash_command_display(text)?;
    let prompt_id = value
        .get("promptId")
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());
    Some((display, prompt_id))
}

/// Whether `value` is the expanded-prompt entry Claude Code writes immediately
/// after a prompt-expanding slash command (`/init`, custom commands, …): an
/// `isMeta` user message sharing the command's `promptId`. Client-side commands
/// (`/model`, `/compact`) are instead followed by `<local-command-stdout>` and
/// never match, so they stay hidden.
fn is_slash_command_expansion(value: &serde_json::Value, prompt_id: Option<&str>) -> bool {
    if value.get("type").and_then(|t| t.as_str()) != Some("user") {
        return false;
    }
    if !is_meta_message(value) {
        return false;
    }
    // The expanded prompt is always array content; a string-content isMeta entry
    // (e.g. a `<local-command-caveat>`) is never an expansion. This also keeps
    // the promptId-less adjacency fallback below from confirming such entries.
    if !value
        .get("message")
        .and_then(|m| m.get("content"))
        .map(|c| c.is_array())
        .unwrap_or(false)
    {
        return false;
    }
    match (prompt_id, value.get("promptId").and_then(|p| p.as_str())) {
        // Both present: require a match. Otherwise fall back to adjacency
        // (the expansion always immediately follows its command).
        (Some(cmd), Some(next)) => cmd == next,
        _ => true,
    }
}

/// Check if a JSONL entry is a system meta message (isMeta: true).
/// Rebuild a standard unified diff from `toolUseResult.structuredPatch`.
///
/// Each hunk in `structuredPatch` has `oldStart`, `oldLines`, `newStart`,
/// `newLines`, and `lines` (prefixed with ` `, `+`, or `-`).
fn rebuild_diff_from_structured_patch(
    file_path: &str,
    structured_patch: &serde_json::Value,
) -> Option<String> {
    let hunks = structured_patch.as_array()?;
    if hunks.is_empty() {
        return None;
    }

    let mut output = String::new();
    output.push_str(&format!("--- a/{}\n+++ b/{}\n", file_path, file_path));

    for hunk in hunks {
        let old_start = hunk.get("oldStart").and_then(|v| v.as_u64()).unwrap_or(1);
        let old_lines = hunk.get("oldLines").and_then(|v| v.as_u64()).unwrap_or(0);
        let new_start = hunk.get("newStart").and_then(|v| v.as_u64()).unwrap_or(1);
        let new_lines = hunk.get("newLines").and_then(|v| v.as_u64()).unwrap_or(0);

        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start, old_lines, new_start, new_lines
        ));

        if let Some(lines) = hunk.get("lines").and_then(|v| v.as_array()) {
            for line in lines {
                if let Some(text) = line.as_str() {
                    output.push_str(text);
                    output.push('\n');
                }
            }
        }
    }

    Some(output)
}

pub(crate) fn is_meta_message(value: &serde_json::Value) -> bool {
    value
        .get("isMeta")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// The bookkeeping records Claude Code appends when a turn is interrupted:
/// a `user` record whose entire content is `[Request interrupted by user]`
/// (or `… for tool use` when the interrupt caught a running tool call).
///
/// They are addressed to the MODEL — they explain why a tool call has no
/// result — so they are dropped rather than rendered: as chat bubbles they put
/// words in the user's mouth, and a user record is also a turn boundary, so
/// they open an empty trailing turn. (A dedicated in-transcript marker was
/// tried and removed: wherever it landed — inside the interrupted turn, or as
/// its own row — it read as noise, and the turn's own cancelled status already
/// carries the fact.)
///
/// Byte-exact, never trimmed and never a substring test: a real message that
/// quotes the phrase (a bug report, a transcript pasted for review) must still
/// render verbatim. This drops user-authored content, so it errs toward
/// under-matching — a future CLI that pads the marker would show the raw text
/// again, which is visible and fixable, where over-matching silently deletes
/// what someone actually said.
pub(crate) fn is_interrupt_marker(value: &serde_json::Value) -> bool {
    const MARKERS: [&str; 2] = [
        "[Request interrupted by user]",
        "[Request interrupted by user for tool use]",
    ];
    if value.get("type").and_then(|t| t.as_str()) != Some("user") {
        return false;
    }
    let Some(content) = value.pointer("/message/content") else {
        return false;
    };
    let text = match content {
        serde_json::Value::String(s) => s.as_str(),
        serde_json::Value::Array(blocks) => {
            let [block] = blocks.as_slice() else {
                return false;
            };
            if block.get("type").and_then(|t| t.as_str()) != Some("text") {
                return false;
            }
            match block.get("text").and_then(|t| t.as_str()) {
                Some(t) => t,
                None => return false,
            }
        }
        _ => return false,
    };
    MARKERS.contains(&text)
}

/// Capture Claude Code's two dedicated title records into their slots.
///
/// * `{"type":"custom-title","customTitle":…}` — the name the USER set, via
///   `/rename`, `claude -n <name>`, `Ctrl+R` in the `/resume` picker, or a
///   `/branch`/fork (which stamps `"<name> (fork)"`).
/// * `{"type":"ai-title","aiTitle":…}` — the summary Claude Code generates in
///   the background when the session has no user-set name.
///
/// Both records are appended (never rewritten) and can repeat over a session's
/// life, so the newest non-empty value of each wins. Claude Code resolves the
/// pair the same way — its session picker folds these records into per-session
/// maps (last write wins) and renders `customTitle ?? aiTitle` — so the caller
/// must prefer `custom_title` over `ai_title`. Empty/whitespace values are
/// ignored: Claude Code refuses to set a blank name, and it emits an empty
/// `aiTitle` for trivial sessions.
fn capture_title_record(
    value: &serde_json::Value,
    msg_type: &str,
    custom_title: &mut Option<String>,
    ai_title: &mut Option<String>,
) {
    let (field, slot) = match msg_type {
        "custom-title" => ("customTitle", custom_title),
        "ai-title" => ("aiTitle", ai_title),
        _ => return,
    };
    if let Some(t) = value.get(field).and_then(|v| v.as_str()) {
        let t = t.trim();
        if !t.is_empty() {
            *slot = Some(truncate_str(t, 100));
        }
    }
}

/// Check if an assistant message is a synthetic placeholder (e.g. generated by
/// Claude Code for local commands like `/context` or `/model`).
/// These carry `model: "<synthetic>"` and all-zero usage, so they should be
/// excluded from conversation turns and stats.
pub(crate) const CONTEXT_CONTINUATION_PREFIX: &str =
    "This session is being continued from a previous conversation";

/// Detect Claude Code context continuation summary messages.
/// These are injected as "user" type but are actually system context.
fn is_context_continuation(content: &[ContentBlock]) -> bool {
    content.iter().any(|block| {
        if let ContentBlock::Text { text } = block {
            text.starts_with(CONTEXT_CONTINUATION_PREFIX)
        } else {
            false
        }
    })
}

fn is_synthetic_assistant(value: &serde_json::Value) -> bool {
    value
        .get("message")
        .and_then(|m| m.get("model"))
        .and_then(|m| m.as_str())
        .map(|s| s == "<synthetic>")
        .unwrap_or(false)
}

/// Context window to display for a *file-parsed* Claude Code session, which
/// carries no authoritative window of its own (live sessions get one from the
/// ACP `usage_update.size` and never reach here).
///
/// Deliberately defaults higher than [`super::infer_context_window_max_tokens`]
/// does for the same `claude-*` model, because the two read different inputs:
/// Claude Code's transcripts record the *resolved* model with the 1M marker
/// stripped — a session launched as `claude-opus-5[1m]` is written to the
/// JSONL as plain `claude-opus-5` — so neither the bracket spelling nor the
/// `-1m` id spelling survives to be detected, and any per-session guess is
/// unavoidable. Assuming the 1M lane keeps the meter from over-reporting
/// pressure ~5x on the extended-context models most sessions run; the cost is
/// under-reporting for a session that really was on the 200K lane. Other
/// agents' transcripts keep the id verbatim, so that path can detect the lane
/// and defaults to 200K instead. Change one of these without the other and
/// they silently disagree again.
fn claude_context_window_max_tokens_for_model(model: Option<&str>) -> Option<u64> {
    let model = model?.trim();
    if model.is_empty() {
        return None;
    }

    // A capacity suffix that did survive (hand-written config, non-CLI writer)
    // is authoritative.
    if let Some(suffixed_limit) = super::parse_model_capacity_suffix(model) {
        return Some(suffixed_limit);
    }

    if model.to_ascii_lowercase().starts_with("claude") {
        return Some(1_000_000);
    }

    None
}

fn claude_context_window_used_tokens_from_usage(usage: &TurnUsage) -> Option<u64> {
    let used_tokens = usage
        .input_tokens
        .saturating_add(usage.cache_creation_input_tokens)
        .saturating_add(usage.cache_read_input_tokens);
    if used_tokens > 0 {
        Some(used_tokens)
    } else {
        None
    }
}

fn latest_claude_context_window_used_tokens(turns: &[MessageTurn]) -> Option<u64> {
    turns.iter().rev().find_map(|turn| {
        turn.usage
            .as_ref()
            .and_then(claude_context_window_used_tokens_from_usage)
    })
}

fn merge_claude_context_window_stats(
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

pub struct ClaudeParser {
    base_dir: PathBuf,
}

impl Default for ClaudeParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeParser {
    pub fn new() -> Self {
        let base_dir = resolve_claude_config_dir().join("projects");
        Self { base_dir }
    }

    /// Test-only constructor that lets callers point the parser at a fixture
    /// directory instead of `~/.claude/projects`.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    fn decode_folder_path(encoded: &str) -> String {
        encoded.replace('-', "/")
    }
}

/// Locate `<session_id>.jsonl` under any project directory of the Claude
/// config dir. Used by the background watcher (`crate::acp::background_watch`)
/// to arm its transcript tail; mirrors `get_conversation`'s discovery scan
/// (Claude offers no forward cwd→project-dir encoding, so discovery is by
/// session-id filename).
pub(crate) fn find_session_file(session_id: &str) -> Option<PathBuf> {
    find_session_file_in(&resolve_claude_config_dir().join("projects"), session_id)
}

/// `find_session_file` against an explicit base dir (test seam). `session_id`
/// is embedded in a filename, so path-traversal shapes are rejected outright
/// (`is_safe_subagent_id`: separators, `..`, drive colon, NUL).
pub(crate) fn find_session_file_in(base_dir: &Path, session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() || !is_safe_subagent_id(session_id) {
        return None;
    }
    let entries = fs::read_dir(base_dir).ok()?;
    for entry in entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let candidate = project_dir.join(format!("{session_id}.jsonl"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

impl ClaudeParser {

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
        let mut ai_title: Option<String> = None;
        let mut custom_title: Option<String> = None;
        let mut first_timestamp: Option<DateTime<Utc>> = None;
        let mut last_timestamp: Option<DateTime<Utc>> = None;
        let mut message_count: u32 = 0;

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

            // Skip non-conversation entries
            if msg_type == "file-history-snapshot" || msg_type == "progress" {
                continue;
            }

            // Skip system meta messages (e.g. local-command-caveat injections)
            // and the interrupt bookkeeping records, which are addressed to the
            // model rather than spoken by the user.
            if is_meta_message(&value) || is_interrupt_marker(&value) {
                continue;
            }

            // Claude Code records the user-set name (`/rename`) and its own
            // generated title as dedicated entries — prefer both over the first
            // user message. See `capture_title_record`.
            capture_title_record(&value, msg_type, &mut custom_title, &mut ai_title);

            if conversation_id.is_none() {
                conversation_id = value
                    .get("sessionId")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }

            if cwd.is_none() {
                cwd = value
                    .get("cwd")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }

            if git_branch.is_none() {
                git_branch = value
                    .get("gitBranch")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }

            if let Some(ts_str) = value.get("timestamp").and_then(|t| t.as_str()) {
                if let Ok(ts) = ts_str.parse::<DateTime<Utc>>() {
                    if first_timestamp.is_none() {
                        first_timestamp = Some(ts);
                    }
                    last_timestamp = Some(ts);
                }
            }

            if msg_type == "user" || msg_type == "assistant" {
                // Skip synthetic assistant placeholders for local commands
                if msg_type == "assistant" && is_synthetic_assistant(&value) {
                    continue;
                }

                message_count += 1;

                // Extract model from assistant messages
                if msg_type == "assistant" && model.is_none() {
                    model = value
                        .get("message")
                        .and_then(|m| m.get("model"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string());
                }

                // Extract title from first user message
                if msg_type == "user" && title.is_none() {
                    title = extract_user_text(&value).map(|t| title_from_user_text(&t));
                }
            }
        }

        let started_at = match first_timestamp {
            Some(ts) => ts,
            None => return Ok(None),
        };

        // Use filename (without .jsonl) as ID fallback
        let id = conversation_id.unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });

        let folder_path = cwd.clone();
        let folder_name = folder_path.as_ref().map(|p| folder_name_from_path(p));

        // The user's own `/rename` first, then Claude Code's generated title,
        // then the first user message.
        let title = custom_title.or(ai_title).or(title);

        Ok(Some(ConversationSummary {
            id,
            agent_type: AgentType::ClaudeCode,
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

pub(crate) fn resolve_claude_config_dir() -> PathBuf {
    resolve_claude_config_dir_from(std::env::var_os("CLAUDE_CONFIG_DIR"), dirs::home_dir())
}

fn resolve_claude_config_dir_from(
    claude_config_dir_env: Option<std::ffi::OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    claude_config_dir_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.unwrap_or_default().join(".claude"))
}

impl AgentParser for ClaudeParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let mut conversations = Vec::new();

        if !self.base_dir.exists() {
            return Ok(conversations);
        }

        let entries = fs::read_dir(&self.base_dir)?;
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let project_dir = entry.path();
            if !project_dir.is_dir() {
                continue;
            }

            let jsonl_files = fs::read_dir(&project_dir)?;
            for file_entry in jsonl_files {
                let file_entry = match file_entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let file_path = file_entry.path();
                if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }

                match super::summary_cache::get_or_parse(AgentType::ClaudeCode, &file_path, || {
                    self.parse_jsonl_summary(&file_path)
                }) {
                    Ok(Some(mut summary)) => {
                        // If folder_path is still None, derive from directory name
                        if summary.folder_path.is_none() {
                            let dir_name = project_dir
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let decoded = Self::decode_folder_path(&dir_name);
                            summary.folder_path = Some(decoded.clone());
                            summary.folder_name = Some(folder_name_from_path(&decoded));
                        }
                        conversations.push(summary);
                    }
                    Ok(None) => continue,
                    Err(_) => continue,
                }
            }
        }

        conversations.sort_by_key(|b| std::cmp::Reverse(b.started_at));
        Ok(conversations)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        // Find the conversation file by searching all directories
        if !self.base_dir.exists() {
            return Err(ParseError::ConversationNotFound(
                conversation_id.to_string(),
            ));
        }

        for entry in fs::read_dir(&self.base_dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let project_dir = entry.path();
            if !project_dir.is_dir() {
                continue;
            }

            let file_path = project_dir.join(format!("{}.jsonl", conversation_id));
            if file_path.exists() {
                return self.parse_conversation_detail(&file_path, conversation_id);
            }
        }

        Err(ParseError::ConversationNotFound(
            conversation_id.to_string(),
        ))
    }
}

/// Streaming Stage-A accumulator: interprets raw session-JSONL records into
/// flat `UnifiedMessage`s plus session metadata (cwd/title/model/timestamps).
/// Extracted from the `parse_conversation_detail` line loop so the background
/// watcher (`crate::acp::background_watch`) can run the SAME record
/// interpretation over an incremental transcript tail; full-file behavior is
/// unchanged (guarded by the parser snapshot tests and the whole-vs-chunked
/// differential test in this file's test module).
pub(crate) struct ClaudeRecordAccumulator {
    /// Session transcript path — the subagent-stats lookup resolves
    /// `<session>/subagents/agent-<id>.jsonl` relative to it.
    session_path: PathBuf,
    pub(crate) messages: Vec<UnifiedMessage>,
    pub(crate) cwd: Option<String>,
    pub(crate) git_branch: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) ai_title: Option<String>,
    pub(crate) custom_title: Option<String>,
    pub(crate) first_timestamp: Option<DateTime<Utc>>,
    pub(crate) last_timestamp: Option<DateTime<Utc>>,
    /// A prompt-expanding slash command is buffered (with its promptId) until
    /// the next entry confirms it expanded into a real prompt — see
    /// `is_slash_command_expansion`. Client commands (`/model`) never confirm
    /// and are dropped, so they stay hidden as before.
    pending_command: Option<(UnifiedMessage, Option<String>)>,
    /// Async background launches seen in this feed: the ack tool_result's
    /// `tool_use_id` → the launched task id (`toolUseResult.agentId`). Joined
    /// with `background_notifications` by `finalize_background_lifecycle`.
    background_acks: std::collections::HashMap<String, String>,
    /// task id → LATEST `<task-notification>` payload (the same id can notify
    /// more than once — a resumed sub-agent re-notifies; last wins).
    background_notifications: std::collections::HashMap<String, BackgroundNotification>,
    /// `message.id` → index in `messages` of the line currently carrying that
    /// API call's usage. See [`Self::claim_assistant_usage`] for why only one
    /// line of a group may carry it.
    usage_owner_by_message_id: std::collections::HashMap<String, usize>,
}

impl ClaudeRecordAccumulator {
    pub(crate) fn new(session_path: PathBuf) -> Self {
        Self {
            session_path,
            messages: Vec::new(),
            cwd: None,
            git_branch: None,
            model: None,
            title: None,
            ai_title: None,
            custom_title: None,
            first_timestamp: None,
            last_timestamp: None,
            pending_command: None,
            background_acks: std::collections::HashMap::new(),
            background_notifications: std::collections::HashMap::new(),
            usage_owner_by_message_id: std::collections::HashMap::new(),
        }
    }

    /// Decide whether the assistant line about to be pushed keeps its `usage`.
    ///
    /// Claude Code writes **one JSONL line per content block**, not one per API
    /// call: a response that thinks, then answers, then calls two tools becomes
    /// four `assistant` lines sharing a single `message.id` — and every one of
    /// them repeats that call's *complete* usage object. Each line becomes its
    /// own [`UnifiedMessage`], its own turn, and (for the dashboard) its own
    /// fact row, so summing them multiplies one API call's tokens by its block
    /// count. Measured over a real transcript tree that is a 2.4× over-count
    /// (17.1 B counted vs 7.0 B actually spent), and 74 % of all calls are
    /// affected — a tool-heavy session inflates the most.
    ///
    /// So the usage is attributed to exactly one line per `message.id`. Which
    /// one barely matters (the payloads are byte-identical in all but a handful
    /// of cases), but the tie-break is not arbitrary: a few groups carry one
    /// real payload plus all-zero siblings, so the line with the **largest
    /// billable total** wins and an earlier winner is demoted retroactively.
    /// Ties keep the earliest line, which makes the choice stable under
    /// incremental feeding — the live watcher must not move the number from one
    /// bubble to another as the rest of a response streams in.
    ///
    /// A line with no `message.id` cannot be grouped, so it keeps whatever it
    /// reported.
    fn claim_assistant_usage(
        messages: &mut [UnifiedMessage],
        usage_owner_by_message_id: &mut std::collections::HashMap<String, usize>,
        message_id: Option<&str>,
        usage: Option<TurnUsage>,
    ) -> Option<TurnUsage> {
        let usage = usage?;
        let Some(message_id) = message_id.filter(|id| !id.is_empty()) else {
            return Some(usage);
        };

        let billable = |u: &TurnUsage| -> u64 {
            u.input_tokens
                .saturating_add(u.output_tokens)
                .saturating_add(u.cache_creation_input_tokens)
                .saturating_add(u.cache_read_input_tokens)
        };

        match usage_owner_by_message_id.get(message_id).copied() {
            Some(owner) => {
                let held = messages
                    .get(owner)
                    .and_then(|m| m.usage.as_ref())
                    .map_or(0, billable);
                if billable(&usage) > held {
                    if let Some(previous) = messages.get_mut(owner) {
                        previous.usage = None;
                    }
                    usage_owner_by_message_id.insert(message_id.to_string(), messages.len());
                    Some(usage)
                } else {
                    None
                }
            }
            None => {
                usage_owner_by_message_id.insert(message_id.to_string(), messages.len());
                Some(usage)
            }
        }
    }

    /// Feed one raw JSONL line. Blank and non-JSON lines are skipped, mirroring
    /// the historical `BufReader::lines()` loop (whose per-line errors were
    /// skipped via `Err(_) => continue`).
    pub(crate) fn feed_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return,
        };
        self.feed_value(value);
    }

    pub(crate) fn feed_value(&mut self, value: serde_json::Value) {
        let Self {
            session_path: path,
            messages,
            cwd,
            git_branch,
            model,
            title,
            ai_title,
            custom_title,
            first_timestamp,
            last_timestamp,
            pending_command,
            background_acks,
            background_notifications,
            usage_owner_by_message_id,
        } = self;

        let msg_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if msg_type == "file-history-snapshot" || msg_type == "progress" {
            return;
        }

        // Resolve a buffered slash command against this entry: emit it only
        // if this entry is its expanded prompt, otherwise drop it (a client
        // command like `/model` that produced no model turn).
        if let Some((command_msg, prompt_id)) = pending_command.take() {
            if is_slash_command_expansion(&value, prompt_id.as_deref()) {
                messages.push(command_msg);
            }
        }

        // Skip system meta messages and interrupt bookkeeping (see the
        // matching filter on the batch path).
        if is_meta_message(&value) || is_interrupt_marker(&value) {
            return;
        }

        // Claude Code records the user-set name (`/rename`) and its own
        // generated title as dedicated entries — prefer both over the first
        // user message. See `capture_title_record`.
        capture_title_record(&value, msg_type, custom_title, ai_title);

        if cwd.is_none() {
            *cwd = value
                .get("cwd")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
        }
        if git_branch.is_none() {
            *git_branch = value
                .get("gitBranch")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
        }

        if let Some(ts_str) = value.get("timestamp").and_then(|t| t.as_str()) {
            if let Ok(ts) = ts_str.parse::<DateTime<Utc>>() {
                if first_timestamp.is_none() {
                    *first_timestamp = Some(ts);
                }
                *last_timestamp = Some(ts);
            }
        }

        // Buffer a user-typed slash command and decide on the next entry
        // whether it drove a real prompt (keep it) or was a client command
        // (drop it). The command's own string content strips to empty, so it
        // would otherwise vanish and make adjacent assistant turns look merged.
        if msg_type == "user" {
            if let Some((display, prompt_id)) = slash_command_value_display(&value) {
                let timestamp = parse_timestamp(&value).unwrap_or_else(Utc::now);
                let uuid = value
                    .get("uuid")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();
                *pending_command = Some((
                    UnifiedMessage {
                        id: uuid,
                        role: MessageRole::User,
                        content: vec![ContentBlock::Text { text: display }],
                        timestamp,
                        usage: None,
                        duration_ms: None,
                        model: None,
                        completed_at: Some(timestamp),
                    },
                    prompt_id,
                ));
                return;
            }
        }

        match msg_type {
            "assistant" if is_synthetic_assistant(&value) => {
                // Skip synthetic assistant placeholders for local commands
            }
            "user" => {
                // Capture `<task-notification>` payloads for the background
                // lifecycle fold BEFORE tag-stripping empties the record out
                // of the message stream (same id can notify more than once —
                // a resumed sub-agent re-notifies — so last wins).
                if let Some(raw) = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                {
                    if raw.trim_start().starts_with("<task-notification>") {
                        if let Some(task_id) =
                            capture_tag(task_notification_task_id_regex(), raw)
                        {
                            background_notifications.insert(
                                task_id,
                                BackgroundNotification {
                                    status: capture_tag(
                                        task_notification_status_regex(),
                                        raw,
                                    )
                                    .unwrap_or_else(|| "completed".to_string()),
                                    summary: capture_tag(
                                        task_notification_summary_regex(),
                                        raw,
                                    ),
                                    result: capture_tag(
                                        task_notification_result_regex(),
                                        raw,
                                    )
                                    .map(|r| truncate_str(&r, BACKGROUND_RESULT_MAX_CHARS)),
                                },
                            );
                        }
                    }
                }

                let mut content = extract_user_content(&value);

                // Skip user messages that are empty after system tag stripping
                if content.is_empty() {
                    return;
                }

                let timestamp = parse_timestamp(&value).unwrap_or_else(Utc::now);
                let uuid = value
                    .get("uuid")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();

                // Detect context continuation summary and treat as system message
                let role = if is_context_continuation(&content) {
                    MessageRole::System
                } else {
                    if title.is_none() {
                        if let Some(first_text) = content.iter().find_map(|c| match c {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        }) {
                            *title = Some(title_from_user_text(&first_text));
                        }
                    }
                    MessageRole::User
                };

                // Check toolUseResult for structured patch and agent execution stats
                if let Some(tur) = value.get("toolUseResult") {
                    if let Some(sp) = tur.get("structuredPatch") {
                        let fp = tur
                            .get("filePath")
                            .and_then(|v| v.as_str())
                            .unwrap_or("file");
                        if let Some(diff) = rebuild_diff_from_structured_patch(fp, sp) {
                            // Find the matching ToolResult in this user message's content
                            // and replace its output_preview with the real diff
                            for block in content.iter_mut() {
                                if let ContentBlock::ToolResult {
                                    ref mut output_preview,
                                    is_error: false,
                                    ..
                                } = block
                                {
                                    *output_preview = Some(diff.clone());
                                    break;
                                }
                            }
                        }
                    }

                    // Record an async background launch: the ack's structured
                    // sibling names the task (`agentId`), and the ack block's
                    // tool_use_id anchors the lifecycle fold to the LAUNCHING
                    // tool call (see `finalize_background_lifecycle`).
                    if tur.get("status").and_then(|s| s.as_str()) == Some("async_launched") {
                        if let Some(task_id) = tur
                            .get("agentId")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            if let Some(ack_tool_use_id) =
                                content.iter().find_map(|b| match b {
                                    ContentBlock::ToolResult {
                                        tool_use_id: Some(id),
                                        ..
                                    } => Some(id.clone()),
                                    _ => None,
                                })
                            {
                                background_acks
                                    .insert(ack_tool_use_id, task_id.to_string());
                            }
                        }
                    }

                    // Extract agent execution stats from toolUseResult
                    if tur.get("agentType").is_some() {
                        let mut stats = extract_agent_execution_stats(tur);
                        // Load tool calls from subagent's own JSONL transcript
                        if let Some(agent_id) = tur.get("agentId").and_then(|v| v.as_str()) {
                            // Reject path traversal: `agent_id` becomes a filename
                            // component under the session dir (see
                            // `is_safe_subagent_id` — rejects separators, `..`, a
                            // Windows drive colon, and NUL).
                            if is_safe_subagent_id(agent_id) {
                                let subagent_dir = path.with_extension("").join("subagents");
                                let subagent_path =
                                    subagent_dir.join(format!("agent-{}.jsonl", agent_id));
                                if subagent_path.exists() {
                                    stats.tool_calls =
                                        parse_subagent_tool_calls(&subagent_path).0;
                                }
                            }
                        }
                        for block in content.iter_mut() {
                            if let ContentBlock::ToolResult {
                                ref mut agent_stats,
                                ..
                            } = block
                            {
                                *agent_stats = Some(stats);
                                break;
                            }
                        }
                    }
                }

                messages.push(UnifiedMessage {
                    id: uuid,
                    role,
                    content,
                    timestamp,
                    usage: None,
                    duration_ms: None,
                    model: None,
                    completed_at: Some(timestamp),
                });
            }
            "assistant" => {
                let timestamp = parse_timestamp(&value).unwrap_or_else(Utc::now);
                let uuid = value
                    .get("uuid")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string();

                let msg_model = value
                    .get("message")
                    .and_then(|m| m.get("model"))
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string());

                if model.is_none() {
                    *model = msg_model.clone();
                }

                let content = extract_assistant_content(&value);
                // One API call is spread over several lines that each repeat
                // its full usage; only one of them may keep it.
                let usage = Self::claim_assistant_usage(
                    messages,
                    usage_owner_by_message_id,
                    value
                        .get("message")
                        .and_then(|m| m.get("id"))
                        .and_then(|id| id.as_str()),
                    extract_usage(&value),
                );

                messages.push(UnifiedMessage {
                    id: uuid,
                    role: MessageRole::Assistant,
                    content,
                    timestamp,
                    usage,
                    duration_ms: None,
                    model: msg_model,
                    completed_at: Some(timestamp),
                });
            }
            "system" => {
                let subtype = value.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
                if subtype == "turn_duration" {
                    if let Some(duration) = value.get("durationMs").and_then(|d| d.as_u64()) {
                        // Attach to the last assistant message
                        if let Some(last) = messages
                            .iter_mut()
                            .rev()
                            .find(|m| matches!(m.role, MessageRole::Assistant))
                        {
                            last.duration_ms = Some(duration);
                        }
                    }
                }
            }
            "tool_use" => {
                // Top-level tool_use record (Claude Code JSONL format)
                let timestamp = parse_timestamp(&value).unwrap_or_else(Utc::now);
                let tool_name = value
                    .get("tool_name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let input_preview = value.get("tool_input").map(|i| i.to_string());
                let synthetic_id = format!("tl-tool-{}", messages.len());

                // Attach to last assistant message, or create a synthetic one
                if let Some(last) = messages
                    .iter_mut()
                    .rev()
                    .find(|m| matches!(m.role, MessageRole::Assistant))
                {
                    last.content.push(ContentBlock::ToolUse {
                        tool_use_id: Some(synthetic_id),
                        tool_name,
                        input_preview,
                        meta: None,
                    });
                } else {
                    messages.push(UnifiedMessage {
                        id: format!("synth-assistant-{}", messages.len()),
                        role: MessageRole::Assistant,
                        content: vec![ContentBlock::ToolUse {
                            tool_use_id: Some(synthetic_id),
                            tool_name,
                            input_preview,
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
            "tool_result" => {
                // Top-level tool_result record (Claude Code JSONL format)
                let tool_output = value.get("tool_output");
                let tool_name = value
                    .get("tool_name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let is_error = tool_output
                    .and_then(|o| o.get("exit"))
                    .and_then(|e| e.as_i64())
                    .is_some_and(|code| code != 0);

                // Extract output text: prefer "preview" (read), then "output" (bash)
                let output_text = tool_output
                    .and_then(|o| {
                        o.get("preview")
                            .or_else(|| o.get("output"))
                            .and_then(|v| v.as_str())
                    })
                    .map(|s| s.to_string());

                // Don't structurize here — `structurize_read_tool_output`
                // will handle Read tool output uniformly after grouping.
                let output_preview = output_text;

                // Find the matching ToolUse by tool_name (reverse scan so the
                // most recent match wins), then fall back to the last ToolUse
                // without a paired ToolResult yet.
                let existing_result_ids: std::collections::HashSet<String> = messages
                    .iter()
                    .rev()
                    .find(|m| matches!(m.role, MessageRole::Assistant))
                    .map(|m| {
                        m.content
                            .iter()
                            .filter_map(|b| {
                                if let ContentBlock::ToolResult {
                                    tool_use_id: Some(ref id),
                                    ..
                                } = b
                                {
                                    Some(id.clone())
                                } else {
                                    None
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let matching_id = messages
                    .iter()
                    .rev()
                    .find(|m| matches!(m.role, MessageRole::Assistant))
                    .and_then(|m| {
                        // First: try to find an unpaired ToolUse with the same tool_name
                        let by_name = m.content.iter().rev().find_map(|b| {
                            if let ContentBlock::ToolUse {
                                tool_use_id: Some(ref id),
                                tool_name: ref tn,
                                ..
                            } = b
                            {
                                if tn == tool_name && !existing_result_ids.contains(id) {
                                    return Some(id.clone());
                                }
                            }
                            None
                        });
                        if by_name.is_some() {
                            return by_name;
                        }
                        // Fallback: last unpaired ToolUse regardless of name
                        m.content.iter().rev().find_map(|b| {
                            if let ContentBlock::ToolUse {
                                tool_use_id: Some(ref id),
                                ..
                            } = b
                            {
                                if !existing_result_ids.contains(id) {
                                    return Some(id.clone());
                                }
                            }
                            None
                        })
                    });

                // Append ToolResult to the same assistant message so they stay in the same turn
                if let Some(last) = messages
                    .iter_mut()
                    .rev()
                    .find(|m| matches!(m.role, MessageRole::Assistant))
                {
                    last.content.push(ContentBlock::ToolResult {
                        tool_use_id: matching_id,
                        output_preview,
                        is_error,
                        agent_stats: None,
                        images: Vec::new(),
                    });
                } else {
                    let timestamp = parse_timestamp(&value).unwrap_or_else(Utc::now);
                    messages.push(UnifiedMessage {
                        id: format!("synth-result-{}", messages.len()),
                        role: MessageRole::Assistant,
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: matching_id,
                            output_preview,
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
            _ => {}
        }
    }

    /// Rewrite each async-launch ack's `output_preview` — internal metadata
    /// text never meant for users ("Async agent launched successfully…
    /// never quote or paste any part of it") — into a structured
    /// [`BACKGROUND_TASK_MARKER`] payload, joined with the latest matching
    /// `<task-notification>`:
    ///
    /// ```text
    /// [[codeg-background-task]]{"task_id":…,"status":…,"summary":…,"result":…}
    /// ```
    ///
    /// `status` is null while no notification was observed — the frontend
    /// renders that as "launched, result pending" WITHOUT claiming it is
    /// still running (a killed CLI leaves it null forever; deriving
    /// "running" from transcript alone is the zombie trap CC desktop hit).
    /// Follows the parser's established output-rewrite pattern
    /// (`structuredPatch` diffs, `structurize_read_tool_output`).
    pub(crate) fn apply_background_lifecycle(&self, messages: &mut [UnifiedMessage]) {
        if self.background_acks.is_empty() {
            return;
        }
        for msg in messages {
            for block in &mut msg.content {
                let ContentBlock::ToolResult {
                    tool_use_id: Some(id),
                    output_preview,
                    is_error: false,
                    ..
                } = block
                else {
                    continue;
                };
                let Some(task_id) = self.background_acks.get(id) else {
                    continue;
                };
                let notification = self.background_notifications.get(task_id);
                let payload = serde_json::json!({
                    "task_id": task_id,
                    "status": notification.map(|n| n.status.clone()),
                    "summary": notification.and_then(|n| n.summary.clone()),
                    "result": notification.and_then(|n| n.result.clone()),
                });
                *output_preview = Some(format!("{BACKGROUND_TASK_MARKER}{payload}"));
            }
        }
    }

    /// In-place [`Self::apply_background_lifecycle`] over `self.messages` —
    /// the full-file detail parse calls this once after feeding every record.
    pub(crate) fn finalize_background_lifecycle(&mut self) {
        let mut messages = std::mem::take(&mut self.messages);
        self.apply_background_lifecycle(&mut messages);
        self.messages = messages;
    }
}

impl ClaudeParser {
    fn parse_conversation_detail(
        &self,
        path: &PathBuf,
        conversation_id: &str,
    ) -> Result<ConversationDetail, ParseError> {
        let RawSessionParse {
            transcript_watermark,
            cwd,
            git_branch,
            model,
            title,
            ai_title,
            custom_title,
            first_timestamp,
            last_timestamp,
            mut turns,
        } = raw_session_parse(path)?;

        let folder_path = cwd.clone();
        let folder_name = folder_path.as_ref().map(|p| folder_name_from_path(p));

        // Everything below re-runs on EVERY fetch (it is deliberately NOT in the
        // cache): these passes resolve against external files that change
        // independently of the transcript — patched source files
        // (`resolve_patch_line_numbers`) and sub-agent transcripts
        // (`attribute_subagent_usage`) — so their output stays fresh even when the
        // grouped turns come from the cache.
        super::relocate_orphaned_tool_results(&mut turns);
        super::structurize_read_tool_output(&mut turns);
        super::resolve_patch_line_numbers(&mut turns, cwd.as_deref());
        // Only very old Claude Code builds wrote `system` / `turn_duration`
        // records; current ones log no timings at all, so without this every
        // reply lost its elapsed-time chip the moment the live timer stopped.
        // Runs before the facts are derived, so the usage dashboard's elapsed
        // time is backfilled too.
        super::backfill_turn_durations(&mut turns, &[]);
        // Read the context window *before* folding in delegated spend: the
        // gauge measures how full this conversation's own prompt is, and a
        // sub-agent's context is its own, not this one's.
        let context_window_used_tokens = latest_claude_context_window_used_tokens(&turns);
        let context_window_max_tokens =
            claude_context_window_max_tokens_for_model(model.as_deref());
        attribute_subagent_usage(path, &mut turns);
        let session_stats = merge_claude_context_window_stats(
            super::compute_session_stats(&turns),
            context_window_used_tokens,
            context_window_max_tokens,
        );

        // Same precedence as `parse_jsonl_summary` — the two paths MUST agree,
        // or the auto-title backfill would oscillate between them.
        let title = custom_title.or(ai_title).or(title);

        let summary = ConversationSummary {
            id: conversation_id.to_string(),
            agent_type: AgentType::ClaudeCode,
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
            transcript_watermark: Some(transcript_watermark),
        })
    }
}

/// The transcript-only portion of a detail parse: read the session file, run it
/// through the record accumulator, and group into turns. This is the expensive
/// step for a long session (a full read plus a per-line JSON parse), and — unlike
/// the post-processing passes in `parse_conversation_detail` — it is a pure
/// function of the transcript bytes, so its result is cached keyed by the file's
/// `(mtime, size)` fingerprint. A detail fetch for an unchanged file (repeat
/// viewer poll, or each "load older" page of a settled conversation) then skips
/// the full re-read + re-parse and only re-runs the cheap/external post-processing.
#[derive(Clone)]
struct RawSessionParse {
    transcript_watermark: u64,
    cwd: Option<String>,
    git_branch: Option<String>,
    model: Option<String>,
    title: Option<String>,
    ai_title: Option<String>,
    custom_title: Option<String>,
    first_timestamp: Option<DateTime<Utc>>,
    last_timestamp: Option<DateTime<Utc>>,
    turns: Vec<MessageTurn>,
}

struct RawSessionCacheEntry {
    fingerprint: (Option<SystemTime>, u64),
    parse: RawSessionParse,
    last_used: Instant,
}

/// Total memory budget across all cached raw parses. A cached entry holds a
/// conversation's grouped turns (its full message content), so this bounds the
/// cache's footprint; `transcript_watermark` (the transcript's byte length) is a
/// faithful per-entry proxy. A single transcript larger than the budget is never
/// cached (its detail is parsed fresh each fetch, exactly as before).
const RAW_SESSION_CACHE_BYTE_BUDGET: u64 = 64 * 1024 * 1024;

fn raw_session_cache() -> &'static Mutex<HashMap<PathBuf, RawSessionCacheEntry>> {
    RAW_SESSION_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

static RAW_SESSION_CACHE: OnceLock<Mutex<HashMap<PathBuf, RawSessionCacheEntry>>> = OnceLock::new();

/// `(mtime, size)` fingerprint of the transcript; `None` (⇒ bypass the cache)
/// when the file can't be stat'd.
fn raw_session_fingerprint(path: &Path) -> Option<(Option<SystemTime>, u64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

/// Return the raw parse for `path`, serving a cached clone when the file's
/// fingerprint is unchanged. Mirrors `summary_cache::get_or_parse`: the parse
/// runs with the lock released, and a result is only cached when the fingerprint
/// is unchanged across the parse (a racing/streaming write is not memoized).
fn raw_session_parse(path: &PathBuf) -> Result<RawSessionParse, ParseError> {
    let Some(fp) = raw_session_fingerprint(path) else {
        return parse_raw_session(path);
    };

    if let Ok(mut map) = raw_session_cache().lock() {
        if let Some(entry) = map.get_mut(path) {
            if entry.fingerprint == fp {
                entry.last_used = Instant::now();
                return Ok(entry.parse.clone());
            }
        }
    }

    let parsed = parse_raw_session(path)?;

    // Cache only when the file didn't change mid-parse and it fits the budget.
    if parsed.transcript_watermark <= RAW_SESSION_CACHE_BYTE_BUDGET
        && raw_session_fingerprint(path) == Some(fp)
    {
        if let Ok(mut map) = raw_session_cache().lock() {
            // Bytes already attributed to the entry this insert would replace.
            let replaced = map
                .get(path)
                .map(|e| e.parse.transcript_watermark)
                .unwrap_or(0);
            let mut total: u64 = map
                .values()
                .map(|e| e.parse.transcript_watermark)
                .sum::<u64>()
                - replaced;
            // Evict least-recently-used entries (never the one being inserted)
            // until the new parse fits.
            while total + parsed.transcript_watermark > RAW_SESSION_CACHE_BYTE_BUDGET {
                let oldest = map
                    .iter()
                    .filter(|(p, _)| p.as_path() != path.as_path())
                    .min_by_key(|(_, e)| e.last_used)
                    .map(|(p, _)| p.clone());
                match oldest {
                    Some(key) => {
                        if let Some(e) = map.remove(&key) {
                            total -= e.parse.transcript_watermark;
                        }
                    }
                    None => break,
                }
            }
            map.insert(
                path.clone(),
                RawSessionCacheEntry {
                    fingerprint: fp,
                    parse: parsed.clone(),
                    last_used: Instant::now(),
                },
            );
        }
    }
    Ok(parsed)
}

/// Read + accumulate + group the transcript into turns. Pure function of the
/// transcript bytes; the watermark/patch/sub-agent-sensitive work lives in the
/// caller (`parse_conversation_detail`) so it stays fresh.
fn parse_raw_session(path: &PathBuf) -> Result<RawSessionParse, ParseError> {
    // Read the file fully up front: `transcript_watermark` must be EXACTLY the
    // byte length this parse consumed. Stat-ing around a streaming read could
    // over-claim (bytes appended mid-parse get counted but not read), and an
    // over-claiming watermark makes the frontend retire background-overlay turns
    // whose content this detail does NOT include — silent loss. An exact length
    // risks at most a transient duplicate.
    let bytes = fs::read(path)?;
    let transcript_watermark = bytes.len() as u64;

    let mut acc = ClaudeRecordAccumulator::new(path.clone());
    for chunk in bytes.split(|b| *b == b'\n') {
        // Mirror `BufReader::lines()`: a line that isn't valid UTF-8 is skipped
        // (the old loop's per-line `Err(_) => continue`).
        let Ok(line) = std::str::from_utf8(chunk) else {
            continue;
        };
        acc.feed_line(line);
    }
    acc.finalize_background_lifecycle();

    let ClaudeRecordAccumulator {
        messages,
        cwd,
        git_branch,
        model,
        title,
        ai_title,
        custom_title,
        first_timestamp,
        last_timestamp,
        ..
    } = acc;

    let turns = group_into_turns(messages);
    Ok(RawSessionParse {
        transcript_watermark,
        cwd,
        git_branch,
        model,
        title,
        ai_title,
        custom_title,
        first_timestamp,
        last_timestamp,
        turns,
    })
}

fn parse_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
}

fn extract_user_text(value: &serde_json::Value) -> Option<String> {
    let message = value.get("message")?;
    let content = message.get("content")?;

    if let Some(text) = content.as_str() {
        return strip_system_tags(text);
    }

    if let Some(arr) = content.as_array() {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    if let Some(cleaned) = strip_system_tags(text) {
                        return Some(cleaned);
                    }
                }
            }
        }
    }

    None
}

fn extract_user_content(value: &serde_json::Value) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    let message = match value.get("message") {
        Some(m) => m,
        None => return blocks,
    };
    let content = match message.get("content") {
        Some(c) => c,
        None => return blocks,
    };

    if let Some(text) = content.as_str() {
        if let Some(cleaned) = strip_system_tags(text) {
            blocks.push(ContentBlock::Text { text: cleaned });
        }
        return blocks;
    }

    if let Some(arr) = content.as_array() {
        for item in arr {
            let block_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        if let Some(cleaned) = strip_system_tags(text) {
                            blocks.push(ContentBlock::Text { text: cleaned });
                        }
                    }
                }
                "image" => {
                    if let Some(image_block) = extract_claude_user_image(item) {
                        blocks.push(image_block);
                    }
                }
                "tool_result" | "server_tool_result" => {
                    let tool_use_id = item
                        .get("tool_use_id")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string());
                    let output = extract_tool_result_text(item);
                    let images = extract_tool_result_images(item);
                    let is_error = item
                        .get("is_error")
                        .and_then(|e| e.as_bool())
                        .unwrap_or(false);
                    blocks.push(ContentBlock::ToolResult {
                        tool_use_id,
                        output_preview: output,
                        is_error,
                        agent_stats: None,
                        images,
                    });
                }
                _ => {}
            }
        }
    }

    blocks
}

fn extract_claude_user_image(item: &serde_json::Value) -> Option<ContentBlock> {
    let source = item.get("source");
    let source_data = source
        .and_then(|s| s.get("data"))
        .and_then(|d| d.as_str())
        .or_else(|| item.get("data").and_then(|d| d.as_str()))
        .map(str::trim)
        .filter(|v| !v.is_empty())?;

    if let Some((mime_type, data)) = parse_data_uri_image(source_data) {
        return Some(ContentBlock::Image {
            data,
            mime_type,
            uri: None,
        });
    }

    let mime_type = source
        .and_then(|s| s.get("media_type"))
        .and_then(|m| m.as_str())
        .or_else(|| {
            source
                .and_then(|s| s.get("mime_type"))
                .and_then(|m| m.as_str())
        })
        .or_else(|| item.get("media_type").and_then(|m| m.as_str()))
        .or_else(|| item.get("mime_type").and_then(|m| m.as_str()))
        .map(str::trim)
        .filter(|m| !m.is_empty() && m.starts_with("image/"))?;

    let uri = source
        .and_then(|s| s.get("url"))
        .and_then(|u| u.as_str())
        .or_else(|| item.get("url").and_then(|u| u.as_str()))
        .map(|u| u.to_string());

    Some(ContentBlock::Image {
        data: source_data.to_string(),
        mime_type: mime_type.to_string(),
        uri,
    })
}

fn parse_data_uri_image(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim();
    let without_prefix = trimmed.strip_prefix("data:")?;
    let marker = ";base64,";
    let marker_idx = without_prefix.find(marker)?;
    let mime_type = without_prefix.get(..marker_idx)?.trim();
    if !mime_type.starts_with("image/") {
        return None;
    }
    let data = without_prefix.get(marker_idx + marker.len()..)?.trim();
    if data.is_empty() {
        return None;
    }
    Some((mime_type.to_string(), data.to_string()))
}

fn extract_assistant_content(value: &serde_json::Value) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    let message = match value.get("message") {
        Some(m) => m,
        None => return blocks,
    };
    let content = match message.get("content") {
        Some(c) => c,
        None => return blocks,
    };

    if let Some(arr) = content.as_array() {
        for item in arr {
            let block_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        blocks.push(ContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }
                "thinking" => {
                    if let Some(text) = item.get("thinking").and_then(|t| t.as_str()) {
                        blocks.push(ContentBlock::Thinking {
                            text: text.to_string(),
                        });
                    }
                }
                "tool_use" | "server_tool_use" => {
                    let tool_use_id = item
                        .get("id")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string());
                    let tool_name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let input_preview = item.get("input").map(|i| i.to_string());
                    blocks.push(ContentBlock::ToolUse {
                        tool_use_id,
                        tool_name,
                        input_preview,
                        meta: None,
                    });
                }
                _ => {}
            }
        }
    }

    blocks
}

fn extract_usage(value: &serde_json::Value) -> Option<TurnUsage> {
    let usage = value.get("message")?.get("usage")?;
    Some(TurnUsage {
        input_tokens: usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_creation_input_tokens: usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read_input_tokens: usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    })
}

fn extract_agent_execution_stats(tur: &serde_json::Value) -> AgentExecutionStats {
    let tool_stats = tur.get("toolStats");
    AgentExecutionStats {
        agent_type: tur
            .get("agentType")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        status: tur
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        total_duration_ms: tur.get("totalDurationMs").and_then(|v| v.as_u64()),
        total_tokens: tur.get("totalTokens").and_then(|v| v.as_u64()),
        total_tool_use_count: tur
            .get("totalToolUseCount")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        read_count: tool_stats
            .and_then(|s| s.get("readCount"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        search_count: tool_stats
            .and_then(|s| s.get("searchCount"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        bash_count: tool_stats
            .and_then(|s| s.get("bashCount"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        edit_file_count: tool_stats
            .and_then(|s| s.get("editFileCount"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        lines_added: tool_stats
            .and_then(|s| s.get("linesAdded"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        lines_removed: tool_stats
            .and_then(|s| s.get("linesRemoved"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        other_tool_count: tool_stats
            .and_then(|s| s.get("otherToolCount"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        tool_calls: Vec::new(),
        // Claude's sub-agent lives inside the parent transcript (a sidechain),
        // not as a session of its own.
        child_session_id: None,
    }
}

/// Parse a subagent's JSONL transcript and extract its tool calls.
///
/// The subagent JSONL has the same format as the main session:
/// assistant messages with tool_use blocks, followed by user messages
/// with tool_result blocks. We pair them by tool_use_id and produce
/// a compact list of `AgentToolCall` records.
/// Fold what this session's `Task` sub-agents spent into its own messages.
///
/// A sub-agent runs its own conversation with the model and writes it to
/// `<session>/subagents/agent-<id>.jsonl`. Session discovery only globs the
/// session-level transcripts, so those files are never a conversation anyone
/// can open — and their tokens were counted nowhere at all: 4.9 % of all Claude
/// spend in a real transcript tree, concentrated in exactly the sessions that
/// delegate the most work.
///
/// The directory is the unit of truth here, not the `toolUseResult` entries
/// that reference it, and both halves of that matter. Following references
/// **over-counts**, because the same `agentId` is reported by more than one
/// result line (57 repeats in the same tree) and each would add the transcript
/// again. Following references also **under-counts**, badly: 217 of 295
/// transcripts have no completed result to reference them at all — a sub-agent
/// that was interrupted, or was still running when the session ended, spent its
/// tokens regardless. Reading each file exactly once, referenced or not, is the
/// only way to get both directions right.
///
/// Each transcript's spend lands on the assistant turn that was current when
/// the sub-agent started, so a session running across midnight attributes its
/// delegated work to the day it actually happened.
///
/// Runs on turns rather than messages, and deliberately after the context
/// window has been read: delegated tokens are this session's *spend*, but they
/// never occupied this session's prompt.
fn attribute_subagent_usage(session_path: &Path, turns: &mut [MessageTurn]) {
    let subagent_dir = session_path.with_extension("").join("subagents");
    let Ok(entries) = fs::read_dir(&subagent_dir) else {
        return;
    };

    let mut transcripts: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    // Directory order is filesystem-defined; sort so attribution is identical
    // on every parse of the same session.
    transcripts.sort();

    for transcript in transcripts {
        let (_, usage, started_at) = parse_subagent_tool_calls(&transcript);
        let Some(extra) = usage else { continue };

        // The turn that was speaking when the sub-agent began. Falling back to
        // the last assistant message keeps the tokens in the session even when
        // the transcript carries no usable timestamp.
        let target = started_at
            .and_then(|start| {
                turns
                    .iter()
                    .rposition(|t| matches!(t.role, TurnRole::Assistant) && t.timestamp <= start)
            })
            .or_else(|| {
                turns
                    .iter()
                    .rposition(|t| matches!(t.role, TurnRole::Assistant))
            });
        let Some(launcher) = target.and_then(|i| turns.get_mut(i)) else {
            continue;
        };
        launcher.usage = Some(match launcher.usage {
            Some(ref own) => TurnUsage {
                input_tokens: own.input_tokens.saturating_add(extra.input_tokens),
                output_tokens: own.output_tokens.saturating_add(extra.output_tokens),
                cache_creation_input_tokens: own
                    .cache_creation_input_tokens
                    .saturating_add(extra.cache_creation_input_tokens),
                cache_read_input_tokens: own
                    .cache_read_input_tokens
                    .saturating_add(extra.cache_read_input_tokens),
            },
            None => extra,
        });
    }
}

/// Read one sub-agent's transcript: the tool calls it made, what it spent, and
/// when it started.
///
/// The usage is deduped by `message.id` on the same rule the parent transcript
/// uses (see [`ClaudeRecordAccumulator::claim_assistant_usage`]) — a sub-agent
/// transcript has the identical one-line-per-content-block shape. It is
/// consumed by [`attribute_subagent_usage`]; the tool-call caller ignores it.
fn parse_subagent_tool_calls(
    path: &PathBuf,
) -> (Vec<AgentToolCall>, Option<TurnUsage>, Option<DateTime<Utc>>) {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (Vec::new(), None, None),
    };
    let reader = BufReader::new(file);

    // Collect tool_use entries and build a result map
    let mut calls: Vec<(String, String, Option<String>)> = Vec::new(); // (id, name, input)
    let mut results: std::collections::HashMap<String, (Option<String>, bool)> =
        std::collections::HashMap::new();
    // `message.id` → that API call's usage; one entry per call, largest wins.
    let mut usage_by_message_id: std::collections::HashMap<String, TurnUsage> =
        std::collections::HashMap::new();
    let mut started_at: Option<DateTime<Utc>> = None;

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

        if started_at.is_none() {
            started_at = parse_timestamp(&value);
        }

        if msg_type == "assistant" {
            if let Some(usage) = extract_usage(&value) {
                let billable = |u: &TurnUsage| {
                    u.input_tokens
                        .saturating_add(u.output_tokens)
                        .saturating_add(u.cache_creation_input_tokens)
                        .saturating_add(u.cache_read_input_tokens)
                };
                // A line with no `message.id` cannot be grouped with anything,
                // so it gets a key of its own rather than being dropped —
                // matching how the parent transcript treats the same shape.
                let key = value
                    .get("message")
                    .and_then(|m| m.get("id"))
                    .and_then(|id| id.as_str())
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("__ungrouped-{}__", usage_by_message_id.len()));
                let slot = usage_by_message_id.entry(key).or_default();
                if billable(&usage) > billable(slot) {
                    *slot = usage;
                }
            }
            if let Some(content) = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for item in content {
                    let block_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if block_type == "tool_use" || block_type == "server_tool_use" {
                        let id = item
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let input = item.get("input").map(|v| truncate_str(&v.to_string(), 500));
                        if !id.is_empty() {
                            calls.push((id, name, input));
                        }
                    }
                }
            }
        } else if msg_type == "user" {
            if let Some(content) = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for item in content {
                    let block_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if block_type == "tool_result" || block_type == "server_tool_result" {
                        let id = item
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let is_error = item
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let output = extract_tool_result_text(item).map(|s| truncate_str(&s, 500));
                        if !id.is_empty() {
                            results.insert(id, (output, is_error));
                        }
                    }
                }
            }
        }
    }

    let usage = usage_by_message_id
        .into_values()
        .reduce(|acc, u| TurnUsage {
            input_tokens: acc.input_tokens.saturating_add(u.input_tokens),
            output_tokens: acc.output_tokens.saturating_add(u.output_tokens),
            cache_creation_input_tokens: acc
                .cache_creation_input_tokens
                .saturating_add(u.cache_creation_input_tokens),
            cache_read_input_tokens: acc
                .cache_read_input_tokens
                .saturating_add(u.cache_read_input_tokens),
        })
        .filter(|u| u != &TurnUsage::default());

    let calls = calls
        .into_iter()
        .map(|(id, name, input)| {
            let (output, is_error) = results.remove(&id).unwrap_or((None, false));
            AgentToolCall {
                tool_name: name,
                input_preview: input,
                output_preview: output,
                is_error,
            }
        })
        .collect();
    (calls, usage, started_at)
}

fn extract_tool_result_text(item: &serde_json::Value) -> Option<String> {
    let content = item.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    if let Some(arr) = content.as_array() {
        let texts: Vec<String> = arr
            .iter()
            .filter_map(|c| {
                if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                    c.get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }
    None
}

/// Extract base64 `image` content blocks from a tool_result.
///
/// Claude Code's `Read` of an image (or a PDF, page-by-page) returns the bytes
/// as `{"type":"image","source":{"type":"base64","media_type":"image/png",
/// "data":"…"}}` blocks inside the tool_result `content` array — never as text,
/// so `extract_tool_result_text` returns `None` for them. We surface the images
/// separately so the renderer can show them in-position, matching the live ACP
/// path (which captures the same bytes via `extract_tool_call_images`).
///
/// Reuses `extract_claude_user_image` (the same `source.data`/`media_type` and
/// data-URI shapes apply) and unwraps its `ContentBlock::Image` into `ImageData`.
fn extract_tool_result_images(item: &serde_json::Value) -> Vec<ImageData> {
    let Some(arr) = item.get("content").and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("image"))
        .filter_map(|c| match extract_claude_user_image(c) {
            Some(ContentBlock::Image {
                data,
                mime_type,
                uri,
            }) => Some(ImageData {
                data,
                mime_type,
                uri,
            }),
            _ => None,
        })
        .collect()
}

/// Check if a user message contains ONLY tool_result blocks (no text).
/// In Claude Code, tool results come back as "user" messages.
fn is_tool_result_only(msg: &UnifiedMessage) -> bool {
    matches!(msg.role, MessageRole::User)
        && !msg.content.is_empty()
        && msg
            .content
            .iter()
            .all(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

/// Group flat messages into conversation turns.
/// Claude Code rule: assistant msg + following tool-result-only user msgs
/// merge into one Assistant turn.
///
/// `pub(crate)`: the background watcher (`crate::acp::background_watch`) runs
/// this same Stage-B grouping over an incremental record suffix so overlay
/// turns assemble exactly like a full detail parse would.
pub(crate) fn group_into_turns(messages: Vec<UnifiedMessage>) -> Vec<MessageTurn> {
    let mut turns = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        let msg = &messages[i];

        if matches!(msg.role, MessageRole::Assistant) {
            let mut blocks: Vec<ContentBlock> = msg.content.clone();
            let timestamp = msg.timestamp;
            let id = format!("turn-{}", turns.len());
            let usage = msg.usage.clone();
            let duration_ms = msg.duration_ms;
            let turn_model = msg.model.clone();
            // Track the latest event time across the assistant message and
            // any tool-result-only user messages absorbed below; that's the
            // turn's true completion moment, not `timestamp + duration_ms`
            // (turn_duration encodes the entire turn span and adding it to
            // the assistant event time double-counts).
            let mut completed_at = msg.completed_at;
            i += 1;

            // Only absorb immediately following tool-result-only user msgs
            // (stop at the next assistant message to keep turns small for virtualization)
            while i < messages.len() && is_tool_result_only(&messages[i]) {
                blocks.extend(messages[i].content.clone());
                if messages[i].completed_at.is_some() {
                    completed_at = messages[i].completed_at;
                }
                i += 1;
            }

            turns.push(MessageTurn {
                id,
                role: TurnRole::Assistant,
                blocks,
                timestamp,
                usage,
                duration_ms,
                model: turn_model,
                completed_at,
            });
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
        }
    }

    turns
}

#[cfg(test)]
mod tests {

    use std::io::Write;

    use super::*;
    use serde_json::json;

    /// Cancelling a turn makes Claude Code append a `user` record reading
    /// `[Request interrupted by user]`. It is addressed to the MODEL — it
    /// explains why a tool call has no result — so it is dropped: as a chat
    /// bubble it puts words in the user's mouth, and a user record is a turn
    /// boundary, so it also opens an empty trailing turn. Both parse paths
    /// (batch detail + the watcher's accumulator) must drop it, and neither
    /// may drop a real message that merely quotes the phrase.
    #[test]
    fn interrupt_bookkeeping_records_never_render_as_user_messages() {
        let marker = json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}
        });
        assert!(is_interrupt_marker(&marker));

        let tool_variant = json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "text", "text": "[Request interrupted by user for tool use]"}
            ]}
        });
        assert!(is_interrupt_marker(&tool_variant));

        // String-shaped content matches too.
        assert!(is_interrupt_marker(&json!({
            "type": "user",
            "message": {"role": "user", "content": "[Request interrupted by user]"}
        })));

        // A real message that QUOTES the marker still renders: the phrase is
        // embedded, not the record's whole content.
        assert!(!is_interrupt_marker(&json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "text", "text": "why does [Request interrupted by user] show up as a bubble?"}
            ]}
        })));

        // Padding makes it someone's own message again, in either content
        // shape — this deletes user content, so it under-matches.
        assert!(!is_interrupt_marker(&json!({
            "type": "user",
            "message": {"role": "user", "content": " [Request interrupted by user] "}
        })));
        assert!(!is_interrupt_marker(&json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "text", "text": "[Request interrupted by user]\n\nwhy?"}
            ]}
        })));

        // Assistant text and multi-block user records are never markers.
        assert!(!is_interrupt_marker(&json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [
                {"type": "text", "text": "[Request interrupted by user]"}
            ]}
        })));
        assert!(!is_interrupt_marker(&json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "text", "text": "[Request interrupted by user]"},
                {"type": "text", "text": "carry on"}
            ]}
        })));

        // End to end: a cancelled turn leaves the reply as the last thing on
        // screen — no trailing bubble, and no extra turn.
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("-Users-test-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let path = proj.join("sess-interrupt.jsonl");
        let lines = [
            r#"{"type":"user","timestamp":"2026-07-07T03:40:00.000Z","uuid":"u1","cwd":"/Users/test/proj","message":{"role":"user","content":[{"type":"text","text":"run the build"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-07T03:40:05.000Z","uuid":"a1","message":{"role":"assistant","model":"claude-sonnet-5","content":[{"type":"text","text":"Working on it."}]}}"#,
            r#"{"type":"user","timestamp":"2026-07-07T03:40:09.000Z","uuid":"u2","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#,
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let parser = ClaudeParser::with_base_dir(dir.path().to_path_buf());
        let detail = parser.get_conversation("sess-interrupt").unwrap();
        assert_eq!(
            detail.turns.len(),
            2,
            "the prompt and the assistant turn — the marker opens no third turn"
        );
        let rendered = serde_json::to_string(&detail.turns).unwrap();
        assert!(
            !rendered.contains("Request interrupted"),
            "the interrupt marker must not reach the rendered turns"
        );
        // Positive half: everything ELSE survives. Without this an over-broad
        // filter that dropped the whole conversation would pass too.
        assert!(rendered.contains("run the build"), "the prompt must remain");
        assert!(rendered.contains("Working on it."), "the reply must remain");

        let mut acc = ClaudeRecordAccumulator::new(path.clone());
        for line in lines {
            acc.feed_line(line);
        }
        assert!(
            !acc.messages
                .iter()
                .any(|m| serde_json::to_string(&m.content)
                    .unwrap_or_default()
                    .contains("Request interrupted")),
            "the watcher's accumulator must drop it too"
        );
    }

    #[test]
    fn accumulator_pipeline_matches_full_detail_parse() {
        // The background watcher runs ClaudeRecordAccumulator + group_into_turns
        // + the parsers post-processing over transcript suffixes. Feeding the
        // SAME records through that pipeline must produce exactly what
        // parse_conversation_detail produces — this is the refactor's
        // behavior-preservation guard (alongside the parser snapshots).
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("-Users-test-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let path = proj.join("sess-diff.jsonl");
        let lines = [
            r#"{"type":"user","timestamp":"2026-07-07T03:40:00.000Z","uuid":"u1","cwd":"/Users/test/proj","message":{"role":"user","content":[{"type":"text","text":"run the build"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-07T03:40:05.000Z","uuid":"a1","message":{"role":"assistant","model":"claude-sonnet-5","content":[{"type":"text","text":"Launching."},{"type":"tool_use","id":"toolu_01","name":"Agent","input":{"description":"Run pnpm build"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-07-07T03:40:06.000Z","uuid":"u2","message":{"role":"user","content":[{"tool_use_id":"toolu_01","type":"tool_result","content":[{"type":"text","text":"Async agent launched successfully. agentId: abc123"}]}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"abc123","description":"Run pnpm build"}}"#,
            r#"{"type":"user","timestamp":"2026-07-07T03:41:00.000Z","uuid":"u3","message":{"role":"user","content":"<task-notification>\n<task-id>abc123</task-id>\n<status>completed</status>\n<summary>Agent finished</summary>\n<result>Build OK</result>\n</task-notification>"}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-07T03:41:05.000Z","uuid":"a2","message":{"role":"assistant","model":"claude-sonnet-5","content":[{"type":"text","text":"Build finished cleanly."}]}}"#,
            r#"{"type":"user","timestamp":"2026-07-07T03:42:00.000Z","uuid":"u4","isMeta":true,"userType":"external","message":{"role":"user","content":"check the weather"}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-07T03:42:05.000Z","uuid":"a3","message":{"role":"assistant","model":"claude-sonnet-5","content":[{"type":"text","text":"Sunny, 25°C."}]}}"#,
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let parser = ClaudeParser::with_base_dir(dir.path().to_path_buf());
        let detail = parser.get_conversation("sess-diff").unwrap();
        assert_eq!(
            detail.transcript_watermark,
            Some(std::fs::metadata(&path).unwrap().len()),
            "watermark must be the exact byte length the parse consumed"
        );

        let mut acc = ClaudeRecordAccumulator::new(path.clone());
        for line in lines {
            acc.feed_line(line);
        }
        acc.finalize_background_lifecycle();
        let cwd = acc.cwd.clone();
        let mut turns = group_into_turns(acc.messages);
        crate::parsers::relocate_orphaned_tool_results(&mut turns);
        crate::parsers::structurize_read_tool_output(&mut turns);
        crate::parsers::resolve_patch_line_numbers(&mut turns, cwd.as_deref());
        crate::parsers::backfill_turn_durations(&mut turns, &[]);

        assert_eq!(
            serde_json::to_string(&turns).unwrap(),
            serde_json::to_string(&detail.turns).unwrap(),
            "accumulator pipeline must be byte-identical to the detail parse"
        );
    }

    #[test]
    fn background_lifecycle_folds_ack_and_notification_into_marker() {
        let ack = r#"{"type":"user","timestamp":"2026-07-07T03:40:06.000Z","uuid":"u2","message":{"role":"user","content":[{"tool_use_id":"toolu_01","type":"tool_result","content":[{"type":"text","text":"Async agent launched successfully. (internal metadata — never quote) agentId: abc123"}]}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"abc123","description":"Run pnpm build"}}"#;
        let unsettled_ack = r#"{"type":"user","timestamp":"2026-07-07T03:40:07.000Z","uuid":"u3","message":{"role":"user","content":[{"tool_use_id":"toolu_02","type":"tool_result","content":[{"type":"text","text":"Async agent launched successfully. agentId: nores99"}]}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"nores99","description":"long build"}}"#;
        let notification = r#"{"type":"user","timestamp":"2026-07-07T03:41:00.000Z","uuid":"u4","message":{"role":"user","content":"<task-notification>\n<task-id>abc123</task-id>\n<tool-use-id>toolu_01</tool-use-id>\n<status>completed</status>\n<summary>Agent \"Run pnpm build\" finished</summary>\n<result>Build OK — no warnings.</result>\n</task-notification>"}}"#;

        let mut acc = ClaudeRecordAccumulator::new(PathBuf::from("/nonexistent.jsonl"));
        for line in [ack, unsettled_ack, notification] {
            acc.feed_line(line);
        }
        acc.finalize_background_lifecycle();

        let previews: Vec<(String, String)> = acc
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id: Some(id),
                    output_preview: Some(p),
                    ..
                } => Some((id.clone(), p.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(previews.len(), 2);

        // Settled: marker carries status + summary + result; the internal ack
        // text ("never quote…") is gone.
        let settled = &previews.iter().find(|(id, _)| id == "toolu_01").unwrap().1;
        assert!(settled.starts_with(BACKGROUND_TASK_MARKER));
        assert!(!settled.contains("never quote"));
        let payload: serde_json::Value = serde_json::from_str(
            settled.strip_prefix(BACKGROUND_TASK_MARKER).unwrap(),
        )
        .unwrap();
        assert_eq!(payload["task_id"], "abc123");
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["summary"], "Agent \"Run pnpm build\" finished");
        assert_eq!(payload["result"], "Build OK — no warnings.");

        // Unsettled: marker present, status null (frontend must NOT claim
        // "running" from the transcript alone — CC's zombie trap).
        let unsettled = &previews.iter().find(|(id, _)| id == "toolu_02").unwrap().1;
        let payload: serde_json::Value = serde_json::from_str(
            unsettled.strip_prefix(BACKGROUND_TASK_MARKER).unwrap(),
        )
        .unwrap();
        assert_eq!(payload["task_id"], "nores99");
        assert!(payload["status"].is_null());
    }

    #[test]
    fn find_session_file_scans_project_dirs_and_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("-Users-x-proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("abc-123.jsonl"), b"{}\n").unwrap();

        assert_eq!(
            find_session_file_in(dir.path(), "abc-123"),
            Some(proj.join("abc-123.jsonl"))
        );
        assert!(find_session_file_in(dir.path(), "missing").is_none());
        assert!(find_session_file_in(dir.path(), "../abc-123").is_none());
        assert!(find_session_file_in(dir.path(), "").is_none());
    }

    #[test]
    fn honours_surviving_capacity_suffix() {
        assert_eq!(
            claude_context_window_max_tokens_for_model(Some("claude-opus-4-6 [500k]")),
            Some(500_000)
        );
    }

    #[test]
    fn defaults_context_limit_for_claude_models() {
        // Claude Code strips the 1M marker when it writes the transcript, so a
        // bare id has to be assumed to be the extended lane here — unlike the
        // shared inference other agents' parsers use. See the doc comment on
        // `claude_context_window_max_tokens_for_model`.
        assert_eq!(
            claude_context_window_max_tokens_for_model(Some("claude-sonnet-4-6")),
            Some(1_000_000)
        );
        assert_eq!(
            super::super::infer_context_window_max_tokens(Some("claude-sonnet-4-6")),
            Some(200_000)
        );
        assert_eq!(
            claude_context_window_max_tokens_for_model(Some("custom-model-x")),
            None
        );
    }

    #[test]
    fn uses_latest_assistant_usage_for_context_tokens() {
        let timestamp = Utc::now();
        let turns = vec![
            MessageTurn {
                id: "turn-0".to_string(),
                role: TurnRole::Assistant,
                blocks: vec![],
                timestamp,
                usage: Some(TurnUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    cache_creation_input_tokens: 30,
                    cache_read_input_tokens: 40,
                }),
                duration_ms: None,
                model: None,
                completed_at: None,
            },
            MessageTurn {
                id: "turn-1".to_string(),
                role: TurnRole::Assistant,
                blocks: vec![],
                timestamp,
                usage: Some(TurnUsage {
                    input_tokens: 250,
                    output_tokens: 60,
                    cache_creation_input_tokens: 70,
                    cache_read_input_tokens: 80,
                }),
                duration_ms: None,
                model: None,
                completed_at: None,
            },
        ];

        assert_eq!(
            latest_claude_context_window_used_tokens(&turns),
            Some(250 + 70 + 80)
        );
    }

    #[test]
    fn parse_detail_sets_claude_context_window_stats() {
        let path = std::env::temp_dir().join(format!(
            "codeg-claude-parser-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::File::create(&path).expect("create temp jsonl");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "user",
                "sessionId": "session-test",
                "timestamp": "2026-03-01T10:00:00Z",
                "uuid": "u1",
                "cwd": "/tmp/demo",
                "gitBranch": "main",
                "message": {
                    "content": [{"type": "text", "text": "hello"}]
                }
            })
        )
        .expect("write user line");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "assistant",
                "sessionId": "session-test",
                "timestamp": "2026-03-01T10:00:02Z",
                "uuid": "a1",
                "message": {
                    "model": "claude-sonnet-4-6",
                    "content": [{"type": "text", "text": "world"}],
                    "usage": {
                        "input_tokens": 1000,
                        "output_tokens": 200,
                        "cache_creation_input_tokens": 300,
                        "cache_read_input_tokens": 400
                    }
                }
            })
        )
        .expect("write assistant line");

        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };
        let detail = parser
            .parse_conversation_detail(&path, "session-test")
            .expect("parse conversation detail");
        fs::remove_file(&path).expect("cleanup temp jsonl");

        let stats = detail.session_stats.expect("session stats");
        assert_eq!(stats.context_window_used_tokens, Some(1700));
        assert_eq!(stats.context_window_max_tokens, Some(1_000_000));
        let percent = stats
            .context_window_usage_percent
            .expect("context window usage percent");
        assert!((percent - 0.17).abs() < 0.01);
    }

    #[test]
    fn parse_prefers_ai_title_over_first_user_message() {
        let path = std::env::temp_dir().join(format!(
            "codeg-claude-aititle-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::File::create(&path).expect("create temp jsonl");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "user",
                "sessionId": "ai-title-test",
                "timestamp": "2026-03-01T10:00:00Z",
                "uuid": "u1",
                "message": { "content": [{"type": "text", "text": "first user prompt"}] }
            })
        )
        .expect("write user line");
        // Claude Code records its own AI title after the messages and can repeat
        // it; the newest non-empty value must win for both detail and summary.
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "ai-title",
                "aiTitle": "Stale Title",
                "sessionId": "ai-title-test"
            })
        )
        .expect("write stale ai-title");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "ai-title",
                "aiTitle": "Concise AI Summary",
                "sessionId": "ai-title-test"
            })
        )
        .expect("write ai-title");

        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };
        let detail = parser
            .parse_conversation_detail(&path, "ai-title-test")
            .expect("parse conversation detail");
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary")
            .expect("summary present");
        fs::remove_file(&path).expect("cleanup temp jsonl");

        assert_eq!(detail.summary.title.as_deref(), Some("Concise AI Summary"));
        assert_eq!(summary.title.as_deref(), Some("Concise AI Summary"));
    }

    #[test]
    fn parse_falls_back_to_user_message_when_ai_title_empty() {
        let path = std::env::temp_dir().join(format!(
            "codeg-claude-aititle-empty-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::File::create(&path).expect("create temp jsonl");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "user",
                "sessionId": "ai-title-empty",
                "timestamp": "2026-03-01T10:00:00Z",
                "uuid": "u1",
                "message": { "content": [{"type": "text", "text": "fallback prompt"}] }
            })
        )
        .expect("write user line");
        // An empty aiTitle (Claude emits this for trivial sessions) must not
        // clobber the first-user-message fallback.
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "ai-title",
                "aiTitle": "   ",
                "sessionId": "ai-title-empty"
            })
        )
        .expect("write empty ai-title");

        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };
        let detail = parser
            .parse_conversation_detail(&path, "ai-title-empty")
            .expect("parse conversation detail");
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary")
            .expect("summary present");
        fs::remove_file(&path).expect("cleanup temp jsonl");

        assert_eq!(detail.summary.title.as_deref(), Some("fallback prompt"));
        assert_eq!(summary.title.as_deref(), Some("fallback prompt"));
    }

    /// Write a session whose entries are `lines`, parse it through BOTH title
    /// paths, and return `(detail_title, summary_title)`. The two must always
    /// agree — a divergence makes the auto-title backfill oscillate.
    fn parse_both_titles(
        tag: &str,
        lines: &[serde_json::Value],
    ) -> (Option<String>, Option<String>) {
        let path =
            std::env::temp_dir().join(format!("codeg-claude-{tag}-{}.jsonl", uuid::Uuid::new_v4()));
        let mut file = fs::File::create(&path).expect("create temp jsonl");
        for line in lines {
            writeln!(file, "{line}").expect("write line");
        }
        drop(file);

        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };
        let detail = parser
            .parse_conversation_detail(&path, tag)
            .expect("parse conversation detail");
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary")
            .expect("summary present");
        fs::remove_file(&path).expect("cleanup temp jsonl");

        (detail.summary.title, summary.title)
    }

    fn user_line(session: &str, text: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "user",
            "sessionId": session,
            "timestamp": "2026-03-01T10:00:00Z",
            "uuid": "u1",
            "message": { "content": [{"type": "text", "text": text}] }
        })
    }

    #[test]
    fn parse_prefers_rename_custom_title_over_ai_title() {
        // `/rename auth-refactor` appends a `custom-title` entry. Claude Code's
        // own picker renders `customTitle ?? aiTitle`, so the user's name must
        // beat the generated one no matter which came last in the file.
        let (detail, summary) = parse_both_titles(
            "customtitle",
            &[
                user_line("custom-title-test", "first user prompt"),
                serde_json::json!({
                    "type": "custom-title",
                    "customTitle": "auth-refactor",
                    "sessionId": "custom-title-test",
                    "timestamp": "2026-03-01T10:05:00Z"
                }),
                serde_json::json!({
                    "type": "ai-title",
                    "aiTitle": "Concise AI Summary",
                    "sessionId": "custom-title-test"
                }),
            ],
        );
        assert_eq!(detail.as_deref(), Some("auth-refactor"));
        assert_eq!(summary.as_deref(), Some("auth-refactor"));
    }

    #[test]
    fn parse_takes_the_last_non_empty_custom_title() {
        // Renaming twice appends twice — the newest name wins, and a blank
        // value (which Claude Code itself refuses to write) never clears one.
        let (detail, summary) = parse_both_titles(
            "customtitle-last",
            &[
                user_line("custom-title-last", "first user prompt"),
                serde_json::json!({
                    "type": "custom-title",
                    "customTitle": "old-name",
                    "sessionId": "custom-title-last"
                }),
                serde_json::json!({
                    "type": "custom-title",
                    "customTitle": "new-name",
                    "sessionId": "custom-title-last"
                }),
                serde_json::json!({
                    "type": "custom-title",
                    "customTitle": "  ",
                    "sessionId": "custom-title-last"
                }),
            ],
        );
        assert_eq!(detail.as_deref(), Some("new-name"));
        assert_eq!(summary.as_deref(), Some("new-name"));
    }

    #[test]
    fn parse_falls_back_to_ai_title_when_custom_title_blank() {
        let (detail, summary) = parse_both_titles(
            "customtitle-blank",
            &[
                user_line("custom-title-blank", "first user prompt"),
                serde_json::json!({
                    "type": "custom-title",
                    "customTitle": "",
                    "sessionId": "custom-title-blank"
                }),
                serde_json::json!({
                    "type": "ai-title",
                    "aiTitle": "Concise AI Summary",
                    "sessionId": "custom-title-blank"
                }),
            ],
        );
        assert_eq!(detail.as_deref(), Some("Concise AI Summary"));
        assert_eq!(summary.as_deref(), Some("Concise AI Summary"));
    }

    #[test]
    fn custom_title_entry_is_not_rendered_as_a_turn() {
        // The record carries a timestamp and a sessionId, so it must stay a
        // metadata line — never a visible turn or a counted message.
        let path = std::env::temp_dir().join(format!(
            "codeg-claude-customtitle-turn-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::File::create(&path).expect("create temp jsonl");
        writeln!(file, "{}", user_line("custom-title-turn", "hello")).expect("write user");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "custom-title",
                "customTitle": "named",
                "sessionId": "custom-title-turn",
                "timestamp": "2026-03-01T10:05:00Z"
            })
        )
        .expect("write custom-title");
        drop(file);

        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };
        let detail = parser
            .parse_conversation_detail(&path, "custom-title-turn")
            .expect("parse conversation detail");
        let summary = parser
            .parse_jsonl_summary(&path)
            .expect("parse summary")
            .expect("summary present");
        fs::remove_file(&path).expect("cleanup temp jsonl");

        assert_eq!(detail.turns.len(), 1, "only the user turn is rendered");
        assert_eq!(summary.message_count, 1);
    }

    #[test]
    fn parse_detail_completion_time_uses_event_log_timestamp_not_added_duration() {
        // Regression: turn_duration encodes the *entire* turn span, so
        // adding it to the assistant event timestamp lands far in the
        // future. completed_at must reflect when the message actually
        // finished, i.e. the assistant event timestamp itself (or the
        // turn_duration system event's timestamp ≈ same instant).
        let path = std::env::temp_dir().join(format!(
            "codeg-claude-completed-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::File::create(&path).expect("create temp jsonl");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "user",
                "sessionId": "session-completed",
                "timestamp": "2026-03-01T10:00:00Z",
                "uuid": "u1",
                "cwd": "/tmp/demo",
                "gitBranch": "main",
                "message": {"content": [{"type": "text", "text": "hi"}]}
            })
        )
        .expect("write user line");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "assistant",
                "sessionId": "session-completed",
                "timestamp": "2026-03-01T10:03:19.301Z",
                "uuid": "a1",
                "message": {
                    "model": "claude-sonnet-4-6",
                    "content": [{"type": "text", "text": "ok"}]
                }
            })
        )
        .expect("write assistant line");
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "system",
                "subtype": "turn_duration",
                "sessionId": "session-completed",
                "timestamp": "2026-03-01T10:03:19.353Z",
                "uuid": "s1",
                "durationMs": 199_033u64
            })
        )
        .expect("write turn_duration line");

        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };
        let detail = parser
            .parse_conversation_detail(&path, "session-completed")
            .expect("parse conversation detail");
        fs::remove_file(&path).expect("cleanup temp jsonl");

        let assistant = detail
            .turns
            .iter()
            .find(|t| matches!(t.role, TurnRole::Assistant))
            .expect("assistant turn");
        let completed_at = assistant.completed_at.expect("completed_at populated");
        // The assistant event's own timestamp.
        let expected = "2026-03-01T10:03:19.301Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(completed_at, expected);
        // Sanity: ensure we did NOT compute timestamp + duration_ms
        // (which would have landed at 10:06:38.334Z, ~3min 19s later).
        let wrong = "2026-03-01T10:06:38.334Z".parse::<DateTime<Utc>>().unwrap();
        assert_ne!(completed_at, wrong);
    }

    #[test]
    fn claude_config_dir_env_overrides_home() {
        let resolved = resolve_claude_config_dir_from(
            Some(std::ffi::OsString::from("/tmp/claude-config")),
            Some(PathBuf::from("/Users/default")),
        );
        assert_eq!(resolved, PathBuf::from("/tmp/claude-config"));
    }

    #[test]
    fn claude_config_dir_defaults_to_home_dot_claude() {
        let resolved = resolve_claude_config_dir_from(None, Some(PathBuf::from("/Users/default")));
        assert_eq!(resolved, PathBuf::from("/Users/default/.claude"));
    }

    #[test]
    fn synthetic_assistant_excluded_from_detail() {
        let path = std::env::temp_dir().join(format!(
            "codeg-claude-synthetic-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::File::create(&path).expect("create temp jsonl");
        // Normal user message
        writeln!(
            file,
            "{}",
            json!({
                "type": "user",
                "sessionId": "synth-test",
                "timestamp": "2026-03-01T10:00:00Z",
                "uuid": "u1",
                "cwd": "/tmp/demo",
                "message": {
                    "content": [{"type": "text", "text": "hello"}]
                }
            })
        )
        .unwrap();
        // Normal assistant message with real usage
        writeln!(
            file,
            "{}",
            json!({
                "type": "assistant",
                "sessionId": "synth-test",
                "timestamp": "2026-03-01T10:00:02Z",
                "uuid": "a1",
                "message": {
                    "model": "claude-sonnet-4-6",
                    "content": [{"type": "text", "text": "world"}],
                    "usage": {
                        "input_tokens": 1000,
                        "output_tokens": 200,
                        "cache_creation_input_tokens": 300,
                        "cache_read_input_tokens": 400
                    }
                }
            })
        )
        .unwrap();
        // Synthetic assistant from a local command like /context
        writeln!(
            file,
            "{}",
            json!({
                "type": "assistant",
                "sessionId": "synth-test",
                "timestamp": "2026-03-01T10:01:00Z",
                "uuid": "a2",
                "message": {
                    "model": "<synthetic>",
                    "content": [{"type": "text", "text": "No response requested."}],
                    "usage": {
                        "input_tokens": 0,
                        "output_tokens": 0,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0
                    }
                }
            })
        )
        .unwrap();

        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };
        let detail = parser
            .parse_conversation_detail(&path, "synth-test")
            .expect("parse detail");
        fs::remove_file(&path).unwrap();

        // Should have 2 turns (user + real assistant), synthetic is excluded
        assert_eq!(detail.turns.len(), 2);
        assert!(
            !detail
                .turns
                .iter()
                .any(|t| t.blocks.iter().any(|b| matches!(
                    b,
                    ContentBlock::Text { text } if text == "No response requested."
                ))),
            "synthetic assistant content should not appear in turns"
        );

        // Stats should reflect only the real assistant usage
        let stats = detail.session_stats.expect("session stats");
        assert_eq!(stats.context_window_used_tokens, Some(1700));
        assert_eq!(stats.context_window_max_tokens, Some(1_000_000));
        let total = stats.total_tokens.expect("total tokens");
        assert_eq!(total, 1900); // 1000 + 200 + 300 + 400
    }

    /// Build the `assistant` line Claude Code writes for one content block of a
    /// response — `id` identifies the API call, so several lines share it.
    fn assistant_block_line(
        uuid: &str,
        message_id: &str,
        at: &str,
        block: serde_json::Value,
        usage: serde_json::Value,
    ) -> String {
        json!({
            "type": "assistant",
            "sessionId": "dedup-test",
            "timestamp": at,
            "uuid": uuid,
            "message": {
                "id": message_id,
                "model": "claude-opus-5",
                "content": [block],
                "usage": usage,
            }
        })
        .to_string()
    }

    fn parse_lines_into_detail(lines: &[String]) -> crate::models::ConversationDetail {
        let path = std::env::temp_dir().join(format!(
            "codeg-claude-usage-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::File::create(&path).expect("create temp jsonl");
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        drop(file);
        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };
        let detail = parser
            .parse_conversation_detail(&path, "dedup-test")
            .expect("parse detail");
        fs::remove_file(&path).unwrap();
        detail
    }

    #[test]
    fn detail_parse_cache_hit_is_byte_identical_and_invalidates_on_growth() {
        let path = std::env::temp_dir().join(format!(
            "codeg-claude-detail-cache-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let write = |path: &PathBuf, lines: &[serde_json::Value]| {
            let mut file = fs::File::create(path).expect("create temp jsonl");
            for line in lines {
                writeln!(file, "{line}").unwrap();
            }
        };
        let user = serde_json::json!({
            "type": "user", "sessionId": "cache-session", "timestamp": "2026-03-01T10:00:00Z",
            "uuid": "u1", "cwd": "/tmp/demo", "gitBranch": "main",
            "message": {"content": [{"type": "text", "text": "hi"}]}
        });
        let assistant = serde_json::json!({
            "type": "assistant", "sessionId": "cache-session", "timestamp": "2026-03-01T10:00:05Z",
            "uuid": "a1",
            "message": {"model": "claude-sonnet-4-6", "content": [{"type": "text", "text": "ok"}]}
        });

        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };

        write(&path, &[user.clone(), assistant.clone()]);
        let first = parser
            .parse_conversation_detail(&path, "cache-session")
            .expect("first parse");
        // Second parse of the unchanged file is served from the raw-parse cache;
        // it must be byte-identical to the fresh parse.
        let second = parser
            .parse_conversation_detail(&path, "cache-session")
            .expect("second parse");
        assert_eq!(first.turns.len(), 2);
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&second).unwrap(),
            "cache hit must be byte-identical to a fresh parse"
        );

        // Appending a turn changes the fingerprint → cache miss → fresh parse.
        let assistant2 = serde_json::json!({
            "type": "assistant", "sessionId": "cache-session", "timestamp": "2026-03-01T10:00:09Z",
            "uuid": "a2",
            "message": {"model": "claude-sonnet-4-6", "content": [{"type": "text", "text": "more"}]}
        });
        write(&path, &[user, assistant, assistant2]);
        let third = parser
            .parse_conversation_detail(&path, "cache-session")
            .expect("third parse");
        assert_eq!(third.turns.len(), 3, "growth must invalidate the cache");

        fs::remove_file(&path).expect("cleanup temp jsonl");
    }

    fn total_usage_tokens(detail: &crate::models::ConversationDetail) -> u64 {
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

    #[test]
    fn one_api_call_is_counted_once_however_many_lines_it_was_written_as() {
        // Claude Code writes one line per content block and repeats the call's
        // complete usage on every one of them. Summing the lines multiplied a
        // single call's spend by its block count — a 2.4× inflation measured
        // over a real transcript tree, worst on the tool-heavy sessions.
        let usage = json!({
            "input_tokens": 2990,
            "output_tokens": 288,
            "cache_creation_input_tokens": 50908,
            "cache_read_input_tokens": 0
        });
        let lines = vec![
            assistant_block_line(
                "a1",
                "msg_one_call",
                "2026-03-01T10:00:00Z",
                json!({"type": "thinking", "thinking": "…"}),
                usage.clone(),
            ),
            assistant_block_line(
                "a2",
                "msg_one_call",
                "2026-03-01T10:00:01Z",
                json!({"type": "text", "text": "answer"}),
                usage.clone(),
            ),
            assistant_block_line(
                "a3",
                "msg_one_call",
                "2026-03-01T10:00:02Z",
                json!({"type": "tool_use", "id": "tu1", "name": "Read", "input": {}}),
                usage.clone(),
            ),
        ];

        let detail = parse_lines_into_detail(&lines);
        // Every block still renders — only the usage is attributed once.
        assert_eq!(detail.turns.len(), 3);
        assert_eq!(
            detail.turns.iter().filter(|t| t.usage.is_some()).count(),
            1,
            "exactly one turn of the group may carry the call's usage"
        );
        assert_eq!(total_usage_tokens(&detail), 54_186);
        assert_eq!(
            detail
                .session_stats
                .as_ref()
                .and_then(|s| s.total_tokens)
                .expect("total tokens"),
            54_186
        );
    }

    #[test]
    fn a_group_whose_siblings_report_zeros_keeps_the_real_payload() {
        // A handful of real groups carry one billed payload plus all-zero
        // siblings, in either order. Whichever line holds the real numbers wins,
        // so the tie-break can't quietly zero out a paid call.
        let real = json!({
            "input_tokens": 2,
            "output_tokens": 383,
            "cache_creation_input_tokens": 418,
            "cache_read_input_tokens": 177_892
        });
        let zeros = json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0
        });

        for (first, second) in [(zeros.clone(), real.clone()), (real.clone(), zeros.clone())] {
            let detail = parse_lines_into_detail(&[
                assistant_block_line(
                    "a1",
                    "msg_mixed",
                    "2026-03-01T10:00:00Z",
                    json!({"type": "text", "text": "one"}),
                    first,
                ),
                assistant_block_line(
                    "a2",
                    "msg_mixed",
                    "2026-03-01T10:00:01Z",
                    json!({"type": "text", "text": "two"}),
                    second,
                ),
            ]);
            assert_eq!(total_usage_tokens(&detail), 178_695);
        }
    }

    #[test]
    fn separate_api_calls_still_add_up() {
        // The dedup is per `message.id`, so two real calls that happen to report
        // identical usage must both count.
        let usage = json!({
            "input_tokens": 100,
            "output_tokens": 20,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0
        });
        let detail = parse_lines_into_detail(&[
            assistant_block_line(
                "a1",
                "msg_first",
                "2026-03-01T10:00:00Z",
                json!({"type": "text", "text": "one"}),
                usage.clone(),
            ),
            assistant_block_line(
                "a2",
                "msg_second",
                "2026-03-01T10:05:00Z",
                json!({"type": "text", "text": "two"}),
                usage.clone(),
            ),
        ]);
        assert_eq!(total_usage_tokens(&detail), 240);
    }

    #[test]
    fn an_assistant_line_with_no_message_id_keeps_its_own_usage() {
        // Nothing to group by, so nothing may be dropped.
        let usage = json!({
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0
        });
        let line = |uuid: &str, at: &str| {
            json!({
                "type": "assistant",
                "sessionId": "dedup-test",
                "timestamp": at,
                "uuid": uuid,
                "message": {
                    "model": "claude-opus-5",
                    "content": [{"type": "text", "text": "hi"}],
                    "usage": usage.clone(),
                }
            })
            .to_string()
        };
        let detail =
            parse_lines_into_detail(&[line("a1", "2026-03-01T10:00:00Z"), line("a2", "2026-03-01T10:00:01Z")]);
        assert_eq!(total_usage_tokens(&detail), 30);
    }

    /// Write a session transcript plus the sub-agent transcripts that live
    /// beside it, and parse the result.
    fn parse_with_subagents(
        lines: &[String],
        subagents: &[(&str, Vec<String>)],
    ) -> crate::models::ConversationDetail {
        let stem = std::env::temp_dir().join(format!("codeg-claude-sub-{}", uuid::Uuid::new_v4()));
        let path = stem.with_extension("jsonl");
        let mut file = fs::File::create(&path).expect("create session jsonl");
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        drop(file);

        let dir = stem.join("subagents");
        fs::create_dir_all(&dir).expect("create subagents dir");
        for (agent_id, agent_lines) in subagents {
            let mut f = fs::File::create(dir.join(format!("agent-{agent_id}.jsonl")))
                .expect("create subagent jsonl");
            for line in agent_lines {
                writeln!(f, "{line}").unwrap();
            }
        }

        let detail = ClaudeParser {
            base_dir: PathBuf::new(),
        }
        .parse_conversation_detail(&path, "dedup-test")
        .expect("parse detail");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&stem);
        detail
    }

    fn subagent_line(uuid: &str, message_id: &str, at: &str, tokens: u64) -> String {
        json!({
            "type": "assistant",
            "timestamp": at,
            "uuid": uuid,
            "message": {
                "id": message_id,
                "model": "claude-sonnet-5",
                "content": [{"type": "text", "text": "sub"}],
                "usage": {
                    "input_tokens": tokens,
                    "output_tokens": 0,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0
                }
            }
        })
        .to_string()
    }

    /// A session that launches a sub-agent, whose `toolUseResult` names it
    /// twice — the shape that makes reference-following double count.
    fn session_launching(agent_id: &str, references: usize) -> Vec<String> {
        let mut lines = vec![assistant_block_line(
            "a1",
            "msg_launch",
            "2026-03-01T10:00:00Z",
            json!({"type": "tool_use", "id": "tu1", "name": "Task", "input": {}}),
            json!({
                "input_tokens": 1000,
                "output_tokens": 0,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }),
        )];
        for i in 0..references {
            lines.push(
                json!({
                    "type": "user",
                    "timestamp": "2026-03-01T10:09:00Z",
                    "uuid": format!("r{i}"),
                    "message": {
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": "tu1",
                            "content": "done"
                        }]
                    },
                    "toolUseResult": {"agentType": "general-purpose", "agentId": agent_id}
                })
                .to_string(),
            );
        }
        lines
    }

    #[test]
    fn a_sub_agents_own_spend_is_counted_against_the_session_that_launched_it() {
        // `Task` sub-agents keep their own transcript under the session, which
        // is never opened as a conversation — so nothing counted those tokens.
        let detail = parse_with_subagents(
            &session_launching("abc", 1),
            &[(
                "abc",
                vec![subagent_line("s1", "msg_sub", "2026-03-01T10:02:00Z", 7_000)],
            )],
        );
        assert_eq!(total_usage_tokens(&detail), 8_000);
    }

    #[test]
    fn a_sub_agent_named_by_several_tool_results_is_still_counted_once() {
        // The same `agentId` is reported by more than one result line in real
        // transcripts, so attribution follows the directory, not the mentions.
        let detail = parse_with_subagents(
            &session_launching("abc", 3),
            &[(
                "abc",
                vec![subagent_line("s1", "msg_sub", "2026-03-01T10:02:00Z", 7_000)],
            )],
        );
        assert_eq!(total_usage_tokens(&detail), 8_000);
    }

    #[test]
    fn a_sub_agent_no_tool_result_ever_referenced_still_counts() {
        // Interrupted, or still running when the session ended: 217 of 295
        // transcripts in a real tree have no completed result naming them, and
        // they spent their tokens all the same.
        let detail = parse_with_subagents(
            &session_launching("abc", 0),
            &[(
                "orphan",
                vec![subagent_line("s1", "msg_sub", "2026-03-01T10:02:00Z", 7_000)],
            )],
        );
        assert_eq!(total_usage_tokens(&detail), 8_000);
    }

    #[test]
    fn delegated_spend_does_not_inflate_the_parents_context_window() {
        // The gauge answers "how full is *this* conversation's prompt". A
        // sub-agent runs in a context of its own, so its tokens are the
        // session's spend but never its occupancy.
        let launcher = json!({
            "input_tokens": 1000,
            "output_tokens": 200,
            "cache_creation_input_tokens": 300,
            "cache_read_input_tokens": 400
        });
        let detail = parse_with_subagents(
            &[assistant_block_line(
                "a1",
                "msg_launch",
                "2026-03-01T10:00:00Z",
                json!({"type": "tool_use", "id": "tu1", "name": "Task", "input": {}}),
                launcher,
            )],
            &[(
                "abc",
                vec![subagent_line("s1", "msg_sub", "2026-03-01T10:02:00Z", 900_000)],
            )],
        );
        let stats = detail.session_stats.expect("session stats");
        assert_eq!(stats.context_window_used_tokens, Some(1_700));
        // The spend, however, does include it.
        assert_eq!(stats.total_tokens, Some(901_900));
    }

    #[test]
    fn sub_agent_spend_lands_on_the_turn_that_was_running_when_it_started() {
        // A session working across midnight must not report a sub-agent's
        // tokens on whichever day the session happened to end.
        let mut lines = vec![
            assistant_block_line(
                "a1",
                "msg_before",
                "2026-03-01T23:00:00Z",
                json!({"type": "tool_use", "id": "tu1", "name": "Task", "input": {}}),
                json!({"input_tokens": 10, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}),
            ),
        ];
        lines.push(assistant_block_line(
            "a2",
            "msg_after",
            "2026-03-02T01:00:00Z",
            json!({"type": "text", "text": "later"}),
            json!({"input_tokens": 20, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}),
        ));

        let detail = parse_with_subagents(
            &lines,
            &[(
                "abc",
                vec![subagent_line("s1", "msg_sub", "2026-03-01T23:10:00Z", 5_000)],
            )],
        );
        let carrying: Vec<_> = detail
            .turns
            .iter()
            .filter(|t| t.usage.is_some())
            .map(|t| {
                (
                    t.timestamp.to_rfc3339(),
                    t.usage.as_ref().map(|u| u.input_tokens).unwrap_or(0),
                )
            })
            .collect();
        assert_eq!(total_usage_tokens(&detail), 5_030);
        assert!(
            carrying.iter().any(|(ts, n)| ts.starts_with("2026-03-01") && *n == 5_010),
            "sub-agent spend belongs to the turn that launched it, got {carrying:?}"
        );
    }

    #[test]
    fn slash_command_display_reconstructs_command() {
        // command-message-first ordering with args (as written for /init)
        assert_eq!(
            slash_command_display(
                "<command-message>init</command-message>\n<command-name>/init</command-name>\n<command-args>初始化</command-args>"
            ),
            Some("/init 初始化".to_string())
        );
        // command-name-first ordering, indented, empty args (as written for /compact)
        assert_eq!(
            slash_command_display(
                "<command-name>/compact</command-name>\n            <command-message>compact</command-message>\n            <command-args></command-args>"
            ),
            Some("/compact".to_string())
        );
        // plain user text is not a command
        assert_eq!(slash_command_display("just a normal message"), None);
        // a non-slash <command-name> is not treated as a command
        assert_eq!(
            slash_command_display(
                "<command-name>init</command-name><command-args>x</command-args>"
            ),
            None
        );
    }

    #[test]
    fn is_slash_command_expansion_only_matches_meta_array_prompts() {
        let array_meta = |pid: Option<&str>| {
            let mut v = json!({
                "type": "user",
                "isMeta": true,
                "message": { "content": [{"type": "text", "text": "expanded"}] }
            });
            if let Some(p) = pid {
                v["promptId"] = json!(p);
            }
            v
        };

        // isMeta + array + matching promptId -> the expansion
        assert!(is_slash_command_expansion(
            &array_meta(Some("p1")),
            Some("p1")
        ));
        // promptId absent on the entry -> adjacency fallback accepts it
        assert!(is_slash_command_expansion(&array_meta(None), Some("p1")));
        // mismatched promptId -> rejected
        assert!(!is_slash_command_expansion(
            &array_meta(Some("p2")),
            Some("p1")
        ));
        // isMeta but STRING content (e.g. a caveat) -> not an expansion even
        // when the command had a promptId and the caveat has none (fallback path)
        let string_meta = json!({
            "type": "user",
            "isMeta": true,
            "message": { "content": "<local-command-caveat>Caveat...</local-command-caveat>" }
        });
        assert!(!is_slash_command_expansion(&string_meta, Some("p1")));
        // non-meta entry (e.g. local-command-stdout) -> not an expansion
        let stdout = json!({
            "type": "user",
            "message": { "content": "<local-command-stdout>ok</local-command-stdout>" }
        });
        assert!(!is_slash_command_expansion(&stdout, Some("p1")));
        // assistant entry -> not an expansion
        let assistant = json!({
            "type": "assistant",
            "isMeta": true,
            "message": { "content": [{"type": "text", "text": "x"}] }
        });
        assert!(!is_slash_command_expansion(&assistant, Some("p1")));
    }

    #[test]
    fn slash_command_keeps_user_turn_between_assistant_turns() {
        let path =
            std::env::temp_dir().join(format!("codeg-claude-slash-{}.jsonl", uuid::Uuid::new_v4()));
        let mut file = fs::File::create(&path).expect("create temp jsonl");
        // Client command /model: followed by stdout, no model turn -> stays hidden
        writeln!(
            file,
            "{}",
            json!({
                "type": "user",
                "sessionId": "slash-test",
                "timestamp": "2026-06-01T11:59:59Z",
                "uuid": "m1",
                "cwd": "/tmp/demo",
                "promptId": "p-model",
                "message": { "content": "<command-name>/model</command-name>\n            <command-message>model</command-message>\n            <command-args>default</command-args>" }
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "type": "user",
                "sessionId": "slash-test",
                "timestamp": "2026-06-01T11:59:59Z",
                "uuid": "m2",
                "message": { "content": "<local-command-stdout>Set model to claude-opus-4-8</local-command-stdout>" }
            })
        )
        .unwrap();
        // Real first user message
        writeln!(
            file,
            "{}",
            json!({
                "type": "user",
                "sessionId": "slash-test",
                "timestamp": "2026-06-01T12:00:00Z",
                "uuid": "u1",
                "cwd": "/tmp/demo",
                "message": { "content": [{"type": "text", "text": "hi"}] }
            })
        )
        .unwrap();
        // Assistant reply to "hi"
        writeln!(
            file,
            "{}",
            json!({
                "type": "assistant",
                "sessionId": "slash-test",
                "timestamp": "2026-06-01T12:00:01Z",
                "uuid": "a1",
                "message": { "model": "claude-opus-4-8", "content": [{"type": "text", "text": "Hi! What can I help you with today?"}] }
            })
        )
        .unwrap();
        // Slash command the user typed (command tags, string content)
        writeln!(
            file,
            "{}",
            json!({
                "type": "user",
                "sessionId": "slash-test",
                "timestamp": "2026-06-01T12:00:08Z",
                "uuid": "u2",
                "promptId": "p-init",
                "message": { "content": "<command-message>init</command-message>\n<command-name>/init</command-name>\n<command-args>初始化</command-args>" }
            })
        )
        .unwrap();
        // Expanded prompt injected by the CLI (isMeta -> must stay hidden)
        writeln!(
            file,
            "{}",
            json!({
                "type": "user",
                "sessionId": "slash-test",
                "timestamp": "2026-06-01T12:00:08Z",
                "uuid": "u3",
                "isMeta": true,
                "promptId": "p-init",
                "message": { "content": [{"type": "text", "text": "EXPANDED_INIT_PROMPT_SENTINEL: long instructions injected by the CLI"}] }
            })
        )
        .unwrap();
        // Assistant reply to the slash command
        writeln!(
            file,
            "{}",
            json!({
                "type": "assistant",
                "sessionId": "slash-test",
                "timestamp": "2026-06-01T12:00:09Z",
                "uuid": "a2",
                "message": { "model": "claude-opus-4-8", "content": [{"type": "text", "text": "I'll analyze the codebase to create a CLAUDE.md file."}] }
            })
        )
        .unwrap();

        let parser = ClaudeParser {
            base_dir: PathBuf::new(),
        };
        let detail = parser
            .parse_conversation_detail(&path, "slash-test")
            .expect("parse detail");
        fs::remove_file(&path).unwrap();

        // user "hi" / assistant / user "/init 初始化" / assistant — the command
        // turn separates the two assistant turns instead of dropping out.
        let roles: Vec<_> = detail
            .turns
            .iter()
            .map(|t| match t.role {
                TurnRole::User => "user",
                TurnRole::Assistant => "assistant",
                TurnRole::System => "system",
            })
            .collect();
        assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
        assert!(matches!(
            &detail.turns[2].blocks[0],
            ContentBlock::Text { text } if text == "/init 初始化"
        ));
        // The huge isMeta expanded prompt must not leak into the transcript.
        assert!(!detail.turns.iter().any(|t| t
            .blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("EXPANDED_INIT_PROMPT_SENTINEL")))));
        // The client command /model (followed only by stdout) stays hidden, so
        // it neither renders as a turn nor becomes the title.
        assert!(!detail.turns.iter().any(|t| t
            .blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("/model")))));
        // Title comes from the first real prompt, not any slash command.
        assert_eq!(detail.summary.title.as_deref(), Some("hi"));
    }

    #[test]
    fn extract_user_content_parses_claude_base64_image_block() {
        let value = json!({
            "message": {
                "content": [
                    {"type": "text", "text": "这个图片里面是什么"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/jpeg",
                            "data": "QUJDREVGRw=="
                        }
                    }
                ]
            }
        });

        let blocks = extract_user_content(&value);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "这个图片里面是什么"));
        assert!(matches!(
            &blocks[1],
            ContentBlock::Image { data, mime_type, uri }
            if data == "QUJDREVGRw==" && mime_type == "image/jpeg" && uri.is_none()
        ));
    }

    #[test]
    fn extract_user_content_parses_claude_data_uri_image_block() {
        let value = json!({
            "message": {
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "data": "data:image/png;base64,QUJD"
                        }
                    }
                ]
            }
        });

        let blocks = extract_user_content(&value);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(
            &blocks[0],
            ContentBlock::Image { data, mime_type, uri }
            if data == "QUJD" && mime_type == "image/png" && uri.is_none()
        ));
    }

    #[test]
    fn tool_result_with_image_populates_images_not_text() {
        // Claude Code's `Read` of an image returns the bytes as an `image`
        // content block inside the tool_result — never as text. The history
        // parser must surface it on `ToolResult.images` (the live ACP path
        // captures the same bytes) instead of dropping it to an empty result.
        let value = json!({
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_01YU4QSMbQEMEizVEzHsCZKV",
                        "content": [
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "QUJDREVGRw=="
                                }
                            }
                        ]
                    }
                ]
            }
        });

        let blocks = extract_user_content(&value);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                output_preview,
                images,
                ..
            } => {
                assert_eq!(
                    tool_use_id.as_deref(),
                    Some("toolu_01YU4QSMbQEMEizVEzHsCZKV")
                );
                assert!(
                    output_preview.is_none(),
                    "image-only result carries no text preview"
                );
                assert_eq!(images.len(), 1);
                assert_eq!(images[0].data, "QUJDREVGRw==");
                assert_eq!(images[0].mime_type, "image/png");
                assert!(images[0].uri.is_none());
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn tool_result_with_text_carries_no_images() {
        // A normal text tool_result keeps its text and has an empty images vec
        // (so the field stays absent in serialized JSON).
        let value = json!({
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_text",
                        "content": [{"type": "text", "text": "hello"}]
                    }
                ]
            }
        });

        let blocks = extract_user_content(&value);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::ToolResult {
                output_preview,
                images,
                ..
            } => {
                assert_eq!(output_preview.as_deref(), Some("hello"));
                assert!(images.is_empty());
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }
}
