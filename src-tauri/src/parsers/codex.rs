use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use walkdir::WalkDir;

use crate::models::*;
use crate::parsers::{folder_name_from_path, truncate_str, AgentParser, ParseError};

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
                            if role == "user" && title.is_none() {
                                title = extract_codex_text_content(payload)
                                    .and_then(|t| extract_codex_title_candidate(&t, false));
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

            match self.parse_jsonl_summary(&path) {
                Ok(Some(summary)) => conversations.push(summary),
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
                return self.parse_conversation_detail(&path, conversation_id);
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

fn clean_codex_exec_output(text: &str) -> String {
    let mut cmd_line: Option<&str> = None;
    let mut in_output = false;
    let mut output_lines = Vec::new();

    for line in text.lines() {
        if cmd_line.is_none() && line.starts_with("$ ") {
            cmd_line = Some(line);
            continue;
        }
        if line == "Output:" || line == "Output: " {
            in_output = true;
            continue;
        }
        if in_output {
            output_lines.push(line);
        }
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

    if result.is_empty() {
        text.to_string()
    } else {
        result
    }
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
    let mut pending_calls: HashMap<String, AgentToolCall> = HashMap::new();
    let mut first_ts: Option<DateTime<Utc>> = None;
    let mut last_ts: Option<DateTime<Utc>> = None;

    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

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

                let input_preview = if tool_name == "exec_command" {
                    parse_codex_json_arg(payload)
                        .and_then(|a| a.get("cmd").and_then(|v| v.as_str()).map(|s| s.to_string()))
                        .or_else(|| {
                            value_to_preview(
                                payload.get("arguments").or_else(|| payload.get("input")),
                            )
                        })
                } else {
                    value_to_preview(payload.get("arguments").or_else(|| payload.get("input")))
                };

                let tc = AgentToolCall {
                    tool_name,
                    input_preview: input_preview.map(|s| truncate_str(&s, 500)),
                    output_preview: None,
                    is_error: false,
                };
                if let Some(id) = call_id {
                    pending_calls.insert(id, tc);
                } else {
                    tool_calls.push(tc);
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = payload
                    .get("call_id")
                    .or_else(|| payload.get("tool_call_id"))
                    .or_else(|| payload.get("id"))
                    .and_then(|id| id.as_str());

                if let Some(id) = call_id {
                    if let Some(mut tc) = pending_calls.remove(id) {
                        let output_value = payload.get("output");
                        let raw_output = value_to_preview(output_value);
                        if tc.tool_name == "exec_command" {
                            tc.output_preview =
                                raw_output.map(|s| truncate_str(&clean_codex_exec_output(&s), 500));
                        } else {
                            tc.output_preview = raw_output.map(|s| truncate_str(&s, 500));
                        }
                        tc.is_error = infer_tool_call_output_is_error(
                            payload,
                            output_value,
                            tc.output_preview.as_deref(),
                        );
                        tool_calls.push(tc);
                    }
                }
            }
            _ => {}
        }
    }

    tool_calls.extend(pending_calls.into_values());

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
        let mut last_turn_context_ts: Option<DateTime<Utc>> = None;
        let mut context_window_used_tokens: Option<u64> = None;
        let mut context_window_max_tokens: Option<u64> = None;
        let mut latest_total_usage: Option<TurnUsage> = None;
        let mut latest_total_tokens: Option<u64> = None;

        let mut first_timestamp: Option<DateTime<Utc>> = None;
        let mut last_timestamp: Option<DateTime<Utc>> = None;

        // Agent subagent tracking (spawn_agent / wait_agent / close_agent)
        let mut spawn_agent_call_ids: HashSet<String> = HashSet::new();
        let mut agent_id_to_spawn_call_id: HashMap<String, String> = HashMap::new();
        let mut agent_final_results: HashMap<String, String> = HashMap::new();
        let mut wait_agent_call_ids: HashSet<String> = HashSet::new();
        let mut close_agent_call_ids: HashSet<String> = HashSet::new();
        let mut close_agent_targets: HashMap<String, String> = HashMap::new();
        let mut active_agent_count: u32 = 0;
        let mut call_id_tool_names: HashMap<String, String> = HashMap::new();
        // Codex 0.129+ writes a generated image both as `event_msg.image_generation_end`
        // and as `response_item.image_generation_call`, sharing the same call_id/id.
        // Emit at most one ContentBlock::Image per id to avoid duplicate display.
        let mut emitted_image_ids: HashSet<String> = HashSet::new();

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
                    last_turn_context_ts = parse_codex_timestamp(&value);
                }
                "event_msg" => {
                    if let Some(payload) = value.get("payload") {
                        let payload_type =
                            payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

                        let timestamp = parse_codex_timestamp(&value).unwrap_or_else(Utc::now);

                        match payload_type {
                            "task_started" if context_window_max_tokens.is_none() => {
                                context_window_max_tokens =
                                    payload.get("model_context_window").and_then(|v| v.as_u64());
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
                            "agent_message" if active_agent_count == 0 => {
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
                            "agent_reasoning" if active_agent_count == 0 => {
                                let text = payload
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                if !text.is_empty() {
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
                                        if let Some(usage) = info
                                            .get("last_token_usage")
                                            .and_then(extract_turn_usage_from_codex_usage)
                                        {
                                            // Attach to the last assistant message
                                            if let Some(last_msg) = messages
                                                .iter_mut()
                                                .rev()
                                                .find(|m| matches!(m.role, MessageRole::Assistant))
                                            {
                                                if last_msg.usage.is_none() {
                                                    last_msg.usage = Some(usage);
                                                }
                                            }
                                        }
                                    }
                                }
                                // Compute duration from turn_context to token_count
                                if let (Some(start_ts), Some(end_ts)) =
                                    (last_turn_context_ts, parse_codex_timestamp(&value))
                                {
                                    let duration = (end_ts - start_ts).num_milliseconds();
                                    if duration > 0 {
                                        if let Some(last_msg) = messages
                                            .iter_mut()
                                            .rev()
                                            .find(|m| matches!(m.role, MessageRole::Assistant))
                                        {
                                            if last_msg.duration_ms.is_none() {
                                                last_msg.duration_ms = Some(duration as u64);
                                            }
                                        }
                                    }
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
                        let timestamp = parse_codex_timestamp(&value).unwrap_or_else(Utc::now);

                        match payload_type {
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

                                match raw_tool_name {
                                    "spawn_agent" => {
                                        let args = parse_codex_json_arg(payload);
                                        let agent_type = args
                                            .as_ref()
                                            .and_then(|a| a.get("agent_type"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("agent");
                                        let message = args
                                            .as_ref()
                                            .and_then(|a| a.get("message"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let description =
                                            truncate_str(message.lines().next().unwrap_or(""), 60);

                                        if let Some(ref id) = tool_use_id {
                                            spawn_agent_call_ids.insert(id.clone());
                                        }
                                        active_agent_count += 1;

                                        let agent_input = serde_json::json!({
                                            "subagent_type": agent_type,
                                            "prompt": message,
                                            "description": description,
                                        });

                                        messages.push(UnifiedMessage {
                                            id: format!("tool-{}", messages.len()),
                                            role: MessageRole::Assistant,
                                            content: vec![ContentBlock::ToolUse {
                                                tool_use_id,
                                                tool_name: "Agent".to_string(),
                                                input_preview: Some(agent_input.to_string()),
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
                                    _ => {
                                        if let Some(ref id) = tool_use_id {
                                            call_id_tool_names
                                                .insert(id.clone(), raw_tool_name.to_string());
                                        }
                                        let input_preview = if raw_tool_name == "exec_command" {
                                            parse_codex_json_arg(payload)
                                                .and_then(|a| {
                                                    a.get("cmd")
                                                        .and_then(|v| v.as_str())
                                                        .map(|s| s.to_string())
                                                })
                                                .or_else(|| {
                                                    value_to_preview(
                                                        payload
                                                            .get("arguments")
                                                            .or_else(|| payload.get("input")),
                                                    )
                                                })
                                        } else {
                                            value_to_preview(
                                                payload
                                                    .get("arguments")
                                                    .or_else(|| payload.get("input")),
                                            )
                                        };
                                        messages.push(UnifiedMessage {
                                            id: format!("tool-{}", messages.len()),
                                            role: MessageRole::Assistant,
                                            content: vec![ContentBlock::ToolUse {
                                                tool_use_id,
                                                tool_name: raw_tool_name.to_string(),
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

                                if is_spawn {
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
                                        }],
                                        timestamp,
                                        usage: None,
                                        duration_ms: None,
                                        model: None,
                                        completed_at: Some(timestamp),
                                    });
                                } else if is_wait {
                                    if let Some(output_obj) = parse_codex_json_output(payload) {
                                        if let Some(status) =
                                            output_obj.get("status").and_then(|s| s.as_object())
                                        {
                                            for (agent_id, result) in status {
                                                if let Some(text) =
                                                    result.get("completed").and_then(|v| v.as_str())
                                                {
                                                    agent_final_results
                                                        .entry(agent_id.clone())
                                                        .or_insert_with(|| text.to_string());
                                                }
                                            }
                                        }
                                    }
                                } else if is_close {
                                    active_agent_count = active_agent_count.saturating_sub(1);
                                    if let Some(output_obj) = parse_codex_json_output(payload) {
                                        if let Some(agent_id) = tool_use_id
                                            .as_ref()
                                            .and_then(|id| close_agent_targets.get(id))
                                        {
                                            if let Some(text) = output_obj
                                                .get("previous_status")
                                                .and_then(|s| s.get("completed"))
                                                .and_then(|v| v.as_str())
                                            {
                                                agent_final_results
                                                    .entry(agent_id.clone())
                                                    .or_insert_with(|| text.to_string());
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
                                    let raw_output = value_to_preview(output_value);
                                    let output = if is_exec {
                                        raw_output.map(|s| clean_codex_exec_output(&s))
                                    } else {
                                        raw_output
                                    };
                                    let is_error = infer_tool_call_output_is_error(
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

        // Fill in agent final results and subagent tool call stats
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
                    if let ContentBlock::ToolResult {
                        tool_use_id: Some(ref id),
                        ref mut output_preview,
                        ref mut agent_stats,
                        ..
                    } = block
                    {
                        if let Some(&agent_id) = spawn_call_to_agent.get(id.as_str()) {
                            if let Some(result) = agent_final_results.get(agent_id) {
                                *output_preview = Some(result.clone());
                            }
                            if let Some(dir) = session_dir {
                                let stats = agent_stats_cache
                                    .entry(agent_id.to_string())
                                    .or_insert_with(|| parse_codex_subagent_stats(dir, agent_id));
                                if stats.is_some() {
                                    *agent_stats = stats.clone();
                                }
                            }
                        }
                    }
                }
            }
        }

        let folder_path = cwd.clone();
        let folder_name = folder_path.as_ref().map(|p| folder_name_from_path(p));

        let mut turns = group_into_turns(messages);
        super::relocate_orphaned_tool_results(&mut turns);
        super::structurize_read_tool_output(&mut turns);
        super::resolve_patch_line_numbers(&mut turns, cwd.as_deref());
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

fn extract_codex_title_candidate(input: &str, fallback_attached: bool) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty()
        || is_agents_instruction_message(trimmed)
        || is_environment_context_message(trimmed)
    {
        return None;
    }

    let without_agents = strip_agents_instructions_block(trimmed);
    if without_agents.is_empty()
        || is_agents_instruction_message(&without_agents)
        || is_environment_context_message(&without_agents)
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
        Some(truncate_str(&cleaned, 100))
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
    use super::extract_codex_title_candidate;
    use super::extract_context_window_used_tokens_from_token_count_info;
    use super::extract_response_item_user_image_blocks;
    use super::extract_turn_usage_from_codex_usage;
    use super::merge_codex_context_window_stats;
    use super::merge_codex_total_usage_stats;
    use super::resolve_codex_home_dir_from;
    use super::should_skip_duplicate_user_message;
    use super::strip_blocked_resource_mentions;
    use super::CodexParser;
    use crate::models::{
        ContentBlock, MessageRole, SessionStats, TurnRole, TurnUsage, UnifiedMessage,
    };
    use chrono::{DateTime, Duration, Utc};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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

        let content = concat!(
            "{\"timestamp\":\"2026-03-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"test-1\",\"cwd\":\"/tmp/demo\"}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"# AGENTS.md instructions for /tmp/demo\\n\\n<INSTRUCTIONS>\\nhello\\n</INSTRUCTIONS>\"}]}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"<environment_context>\\n  <cwd>/tmp/demo</cwd>\\n</environment_context>\"}]}}\n",
            "{\"timestamp\":\"2026-03-01T10:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"真实用户标题\"}]}}\n"
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

    #[test]
    fn parse_detail_completion_time_uses_agent_message_timestamp_not_added_turn_span() {
        // Regression: in Codex `duration_ms` is computed from the
        // turn_context → token_count span, while `timestamp` on the
        // assistant `UnifiedMessage` is the agent_message event time
        // (already near turn end). Adding them double-counts the entire
        // turn span. completed_at must reflect the agent_message arrival.
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

    /// Subagents in codex run inside the parent's JSONL — their
    /// agent_message / agent_reasoning events are filtered while
    /// `active_agent_count > 0`. image_generation events must follow the
    /// same rule, otherwise a subagent's generated image leaks into the
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
}
