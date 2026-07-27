use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use walkdir::WalkDir;

use crate::models::*;
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

/// Synthetic rawInput key the live input shaper uses to carry the collab op
/// through to the card (see frontend `collab-tool.ts` `COLLAB_OP_KEY`). Kept in
/// sync here so history `wait_agent` capsules render with an op-aware title.
const COLLAB_OP_KEY: &str = "__codegCollabOp";

/// Whether a collab status string is an error (mirrors the frontend
/// `isErrorCollabStatusKind`: only `errored` / `failed` / `notFound`).
fn is_error_collab_status(status: &str) -> bool {
    matches!(status, "errored" | "failed" | "notFound")
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
        let mut last_turn_context_ts: Option<DateTime<Utc>> = None;
        let mut context_window_used_tokens: Option<u64> = None;
        let mut context_window_max_tokens: Option<u64> = None;
        let mut latest_total_usage: Option<TurnUsage> = None;
        let mut latest_total_tokens: Option<u64> = None;

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
                    last_turn_context_ts = parse_codex_timestamp(&value);
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
                                    // built from THIS wait's own returned agents
                                    // (`output.status`). Routes through the same
                                    // CollabAgentCard as the live wait capsule.
                                    if let Some(output_obj) = parse_codex_json_output(payload) {
                                        if let Some(status) =
                                            output_obj.get("status").and_then(|s| s.as_object())
                                        {
                                            // Mark returned agents so the spawn
                                            // capsule won't also show their result,
                                            // and record per-agent error state so
                                            // the execution capsule can render
                                            // failed (live parity).
                                            for (agent_id, value) in status {
                                                agent_waited.insert(agent_id.clone());
                                                let (st, _) = extract_wait_agent_status(value);
                                                if is_error_collab_status(&st) {
                                                    agent_errored.insert(agent_id.clone());
                                                }
                                            }
                                            if !status.is_empty() {
                                                let (collab_input, is_error) =
                                                    build_collab_wait_input(status);
                                                messages.push(UnifiedMessage {
                                                    id: format!("tool-{}", messages.len()),
                                                    role: MessageRole::Assistant,
                                                    content: vec![ContentBlock::ToolUse {
                                                        tool_use_id: tool_use_id.clone(),
                                                        tool_name: "collab_agent".to_string(),
                                                        input_preview: Some(collab_input),
                                                        meta: None,
                                                    }],
                                                    timestamp,
                                                    usage: None,
                                                    duration_ms: None,
                                                    model: None,
                                                    completed_at: Some(timestamp),
                                                });
                                                messages.push(UnifiedMessage {
                                                    id: format!(
                                                        "tool-result-{}",
                                                        messages.len()
                                                    ),
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
                                        }
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
    use super::merge_codex_context_window_stats;
    use super::merge_codex_total_usage_stats;
    use super::resolve_codex_home_dir_from;
    use super::should_skip_duplicate_user_message;
    use super::strip_blocked_resource_mentions;
    use super::CodexParser;
    use crate::models::{
        ContentBlock, MessageRole, MessageTurn, SessionStats, TurnRole, TurnUsage, UnifiedMessage,
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
}
