use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use regex::Regex;

use crate::models::*;
use crate::parsers::{folder_name_from_path, truncate_str, AgentParser, ParseError};

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

/// Regex that matches an optional model capacity suffix like `[1M]` / `[500k]`.
fn model_capacity_suffix_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\[\s*([0-9]+(?:\.[0-9]+)?)\s*([km])\s*\]\s*$")
            .expect("valid model capacity regex")
    })
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
fn slash_command_display(text: &str) -> Option<String> {
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

fn is_meta_message(value: &serde_json::Value) -> bool {
    value
        .get("isMeta")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Check if an assistant message is a synthetic placeholder (e.g. generated by
/// Claude Code for local commands like `/context` or `/model`).
/// These carry `model: "<synthetic>"` and all-zero usage, so they should be
/// excluded from conversation turns and stats.
const CONTEXT_CONTINUATION_PREFIX: &str =
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

fn parse_model_capacity_suffix(model: &str) -> Option<u64> {
    let captures = model_capacity_suffix_regex().captures(model.trim())?;
    let value = captures.get(1)?.as_str().parse::<f64>().ok()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }

    let unit = captures
        .get(2)
        .map(|m| m.as_str().to_ascii_lowercase())
        .unwrap_or_default();
    let multiplier = match unit.as_str() {
        "m" => 1_000_000.0,
        "k" => 1_000.0,
        _ => return None,
    };

    Some((value * multiplier) as u64)
}

fn claude_context_window_max_tokens_for_model(model: Option<&str>) -> Option<u64> {
    let model = model?.trim();
    if model.is_empty() {
        return None;
    }

    // If user/model config contains an explicit capacity suffix, prefer it.
    if let Some(suffixed_limit) = parse_model_capacity_suffix(model) {
        return Some(suffixed_limit);
    }

    // Claude models default to 1M when no explicit capacity is provided.
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
            if is_meta_message(&value) {
                continue;
            }

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
                    title = extract_user_text(&value).map(|t| truncate_str(&t, 100));
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

                match self.parse_jsonl_summary(&file_path) {
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

impl ClaudeParser {
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
        let mut first_timestamp: Option<DateTime<Utc>> = None;
        let mut last_timestamp: Option<DateTime<Utc>> = None;
        // A prompt-expanding slash command is buffered (with its promptId) until
        // the next entry confirms it expanded into a real prompt — see
        // `is_slash_command_expansion`. Client commands (`/model`) never confirm
        // and are dropped, so they stay hidden as before.
        let mut pending_command: Option<(UnifiedMessage, Option<String>)> = None;

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

            if msg_type == "file-history-snapshot" || msg_type == "progress" {
                continue;
            }

            // Resolve a buffered slash command against this entry: emit it only
            // if this entry is its expanded prompt, otherwise drop it (a client
            // command like `/model` that produced no model turn).
            if let Some((command_msg, prompt_id)) = pending_command.take() {
                if is_slash_command_expansion(&value, prompt_id.as_deref()) {
                    messages.push(command_msg);
                }
            }

            // Skip system meta messages
            if is_meta_message(&value) {
                continue;
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
                    pending_command = Some((
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
                    continue;
                }
            }

            match msg_type {
                "assistant" if is_synthetic_assistant(&value) => {
                    // Skip synthetic assistant placeholders for local commands
                    continue;
                }
                "user" => {
                    let mut content = extract_user_content(&value);

                    // Skip user messages that are empty after system tag stripping
                    if content.is_empty() {
                        continue;
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
                                title = Some(truncate_str(&first_text, 100));
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

                        // Extract agent execution stats from toolUseResult
                        if tur.get("agentType").is_some() {
                            let mut stats = extract_agent_execution_stats(tur);
                            // Load tool calls from subagent's own JSONL transcript
                            if let Some(agent_id) = tur.get("agentId").and_then(|v| v.as_str()) {
                                // Reject path traversal: agentId must be alphanumeric
                                if !agent_id.is_empty()
                                    && !agent_id.contains('/')
                                    && !agent_id.contains('\\')
                                    && !agent_id.contains("..")
                                {
                                    let subagent_dir = path.with_extension("").join("subagents");
                                    let subagent_path =
                                        subagent_dir.join(format!("agent-{}.jsonl", agent_id));
                                    if subagent_path.exists() {
                                        stats.tool_calls =
                                            parse_subagent_tool_calls(&subagent_path);
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
                        model = msg_model.clone();
                    }

                    let content = extract_assistant_content(&value);
                    let usage = extract_usage(&value);

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

        let folder_path = cwd.clone();
        let folder_name = folder_path.as_ref().map(|p| folder_name_from_path(p));

        let mut turns = group_into_turns(messages);
        super::relocate_orphaned_tool_results(&mut turns);
        super::structurize_read_tool_output(&mut turns);
        super::resolve_patch_line_numbers(&mut turns, cwd.as_deref());
        let context_window_used_tokens = latest_claude_context_window_used_tokens(&turns);
        let context_window_max_tokens =
            claude_context_window_max_tokens_for_model(model.as_deref());
        let session_stats = merge_claude_context_window_stats(
            super::compute_session_stats(&turns),
            context_window_used_tokens,
            context_window_max_tokens,
        );

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
        })
    }
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
                    let is_error = item
                        .get("is_error")
                        .and_then(|e| e.as_bool())
                        .unwrap_or(false);
                    blocks.push(ContentBlock::ToolResult {
                        tool_use_id,
                        output_preview: output,
                        is_error,
                        agent_stats: None,
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
    }
}

/// Parse a subagent's JSONL transcript and extract its tool calls.
///
/// The subagent JSONL has the same format as the main session:
/// assistant messages with tool_use blocks, followed by user messages
/// with tool_result blocks. We pair them by tool_use_id and produce
/// a compact list of `AgentToolCall` records.
fn parse_subagent_tool_calls(path: &PathBuf) -> Vec<AgentToolCall> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);

    // Collect tool_use entries and build a result map
    let mut calls: Vec<(String, String, Option<String>)> = Vec::new(); // (id, name, input)
    let mut results: std::collections::HashMap<String, (Option<String>, bool)> =
        std::collections::HashMap::new();

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

        if msg_type == "assistant" {
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

    calls
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
        .collect()
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
fn group_into_turns(messages: Vec<UnifiedMessage>) -> Vec<MessageTurn> {
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

    #[test]
    fn parses_model_capacity_suffix() {
        assert_eq!(
            parse_model_capacity_suffix("claude-sonnet-4-6[1.5M]"),
            Some(1_500_000)
        );
        assert_eq!(
            parse_model_capacity_suffix("claude-opus-4-6 [500k]"),
            Some(500_000)
        );
        assert_eq!(parse_model_capacity_suffix("claude-sonnet-4-6"), None);
    }

    #[test]
    fn defaults_context_limit_for_claude_models() {
        assert_eq!(
            claude_context_window_max_tokens_for_model(Some("claude-sonnet-4-6")),
            Some(1_000_000)
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
}
