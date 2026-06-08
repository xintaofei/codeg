use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use walkdir::WalkDir;

use crate::models::*;
use crate::parsers::{folder_name_from_path, truncate_str, AgentParser, ParseError};

pub struct GeminiParser {
    base_dir: PathBuf,
}

impl Default for GeminiParser {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiParser {
    pub fn new() -> Self {
        let base_dir = resolve_gemini_base_dir();
        Self { base_dir }
    }

    /// Test-only constructor that lets callers point the parser at a fixture
    /// directory instead of `~/.gemini`.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    fn tmp_dir(&self) -> PathBuf {
        self.base_dir.join("tmp")
    }

    fn history_dir(&self) -> PathBuf {
        self.base_dir.join("history")
    }

    fn projects_json_path(&self) -> PathBuf {
        self.base_dir.join("projects.json")
    }

    fn is_chat_file(path: &Path) -> bool {
        let Some(extension) = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
        else {
            return false;
        };
        if !matches!(extension.as_str(), "json" | "jsonl") {
            return false;
        }
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !file_name.starts_with("session-") {
            return false;
        }
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            == Some("chats")
    }

    fn parse_chat_value(path: &Path, raw: &str) -> Option<Value> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());
        if extension.as_deref() == Some("jsonl") {
            Self::parse_jsonl_chat_value(raw)
        } else {
            serde_json::from_str(raw).ok()
        }
    }

    fn set_jsonl_root_field(
        root: &mut Map<String, Value>,
        key: &str,
        value: Option<&Value>,
        overwrite: bool,
    ) {
        let Some(value) = value else {
            return;
        };
        if overwrite || !root.contains_key(key) {
            root.insert(key.to_string(), value.clone());
        }
    }

    fn merge_message_value(existing: &mut Value, update: Value) {
        match (existing.as_object_mut(), update) {
            (Some(existing_map), Value::Object(update_map)) => {
                for (key, value) in update_map {
                    existing_map.insert(key, value);
                }
            }
            (_, update) => {
                *existing = update;
            }
        }
    }

    fn parse_jsonl_chat_value(raw: &str) -> Option<Value> {
        let mut root = Map::new();
        let mut messages = Vec::new();
        let mut message_index_by_id: HashMap<String, usize> = HashMap::new();
        let mut saw_json_line = false;

        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let value: Value = serde_json::from_str(trimmed).ok()?;
            let Some(object) = value.as_object() else {
                continue;
            };
            saw_json_line = true;

            Self::set_jsonl_root_field(&mut root, "kind", object.get("kind"), false);
            Self::set_jsonl_root_field(&mut root, "sessionId", object.get("sessionId"), false);
            Self::set_jsonl_root_field(&mut root, "projectHash", object.get("projectHash"), false);
            Self::set_jsonl_root_field(&mut root, "startTime", object.get("startTime"), false);
            Self::set_jsonl_root_field(&mut root, "lastUpdated", object.get("lastUpdated"), true);

            if let Some(set) = object.get("$set").and_then(|v| v.as_object()) {
                Self::set_jsonl_root_field(&mut root, "lastUpdated", set.get("lastUpdated"), true);
            }

            if object.get("type").and_then(|v| v.as_str()).is_none() {
                continue;
            }

            if let Some(id) = object.get("id").and_then(|v| v.as_str()) {
                if let Some(index) = message_index_by_id.get(id).copied() {
                    Self::merge_message_value(&mut messages[index], value);
                    continue;
                }

                message_index_by_id.insert(id.to_string(), messages.len());
            }

            messages.push(value);
        }

        if !saw_json_line {
            return None;
        }

        root.insert("messages".to_string(), Value::Array(messages));
        Some(Value::Object(root))
    }

    fn list_chat_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = Vec::new();

        // Scan both tmp/ (active sessions) and history/ (archived sessions)
        for dir in [self.tmp_dir(), self.history_dir()] {
            if !dir.exists() {
                continue;
            }
            let found = WalkDir::new(&dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .map(|e| e.path().to_path_buf())
                .filter(|p| p.is_file() && Self::is_chat_file(p));
            files.extend(found);
        }

        files.sort();
        files.dedup();
        files
    }

    fn project_alias_from_chat_path(path: &Path) -> Option<String> {
        path.parent()?
            .parent()?
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
    }

    fn read_project_root_file(path: PathBuf) -> Option<String> {
        let raw = fs::read_to_string(path).ok()?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn resolve_project_root(&self, alias: &str) -> Option<String> {
        let tmp_root = self.tmp_dir().join(alias).join(".project_root");
        if let Some(path) = Self::read_project_root_file(tmp_root) {
            return Some(path);
        }

        let history_root = self.history_dir().join(alias).join(".project_root");
        if let Some(path) = Self::read_project_root_file(history_root) {
            return Some(path);
        }

        self.resolve_project_root_from_projects_json(alias)
    }

    fn resolve_project_root_from_projects_json(&self, alias: &str) -> Option<String> {
        let raw = fs::read_to_string(self.projects_json_path()).ok()?;
        let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let projects = value.get("projects")?.as_object()?;
        projects
            .iter()
            .find_map(|(path, mapped_alias)| (mapped_alias.as_str() == Some(alias)).then_some(path))
            .map(|s| s.to_string())
    }

    fn parse_timestamp(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
        value.and_then(|v| v.as_str()?.parse::<DateTime<Utc>>().ok())
    }

    fn extract_text(value: &Value) -> Option<String> {
        match value {
            Value::String(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            Value::Array(items) => {
                let mut parts = Vec::new();
                for item in items {
                    if let Some(text) = item.get("text").and_then(Self::extract_text) {
                        parts.push(text);
                    } else if let Some(text) = Self::extract_text(item) {
                        parts.push(text);
                    }
                }
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join("\n"))
                }
            }
            Value::Object(map) => {
                if let Some(text) = map.get("text").and_then(Self::extract_text) {
                    return Some(text);
                }
                if let Some(text) = map.get("message").and_then(Self::extract_text) {
                    return Some(text);
                }
                None
            }
            _ => None,
        }
    }

    fn extract_message_text(message: &Value) -> Option<String> {
        message
            .get("content")
            .and_then(Self::extract_text)
            .or_else(|| message.get("message").and_then(Self::extract_text))
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

    fn parse_user_image_part(part: &Value) -> Option<ContentBlock> {
        let inline = part
            .get("inlineData")
            .or_else(|| part.get("inline_data"))
            .unwrap_or(part);
        let data = inline
            .get("data")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())?;

        if let Some((mime_type, data)) = Self::parse_data_uri_image(data) {
            return Some(ContentBlock::Image {
                data,
                mime_type,
                uri: None,
            });
        }

        let mime_type = inline
            .get("mimeType")
            .or_else(|| inline.get("mime_type"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|m| !m.is_empty() && m.starts_with("image/"))?;
        let uri = inline
            .get("fileUri")
            .or_else(|| inline.get("uri"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string());

        Some(ContentBlock::Image {
            data: data.to_string(),
            mime_type: mime_type.to_string(),
            uri,
        })
    }

    fn parse_user_blocks(message: &Value) -> Vec<ContentBlock> {
        let mut blocks = Vec::new();
        let content = match message.get("content") {
            Some(c) => c,
            None => {
                if let Some(text) = message.get("message").and_then(Self::extract_text) {
                    blocks.push(ContentBlock::Text { text });
                }
                return blocks;
            }
        };

        if let Some(text) = content
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
        {
            blocks.push(ContentBlock::Text { text });
            return blocks;
        }

        if let Some(parts) = content.as_array() {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Self::extract_text) {
                    blocks.push(ContentBlock::Text { text });
                } else if let Some(text) = Self::extract_text(part) {
                    blocks.push(ContentBlock::Text { text });
                }

                if let Some(image) = Self::parse_user_image_part(part) {
                    blocks.push(image);
                }
            }
            return blocks;
        }

        if let Some(image) = Self::parse_user_image_part(content) {
            blocks.push(image);
            return blocks;
        }

        if let Some(text) = Self::extract_text(content) {
            blocks.push(ContentBlock::Text { text });
        }

        blocks
    }

    fn parse_summary_from_value(&self, path: &Path, value: &Value) -> Option<ConversationSummary> {
        let id = value.get("sessionId").and_then(|v| v.as_str())?.to_string();
        let messages = value
            .get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();

        let first_message_ts = messages
            .first()
            .and_then(|m| Self::parse_timestamp(m.get("timestamp")));
        let last_message_ts = messages
            .iter()
            .rev()
            .find_map(|m| Self::parse_timestamp(m.get("timestamp")));

        let started_at = Self::parse_timestamp(value.get("startTime"))
            .or(first_message_ts)
            .unwrap_or_else(Utc::now);
        let ended_at = Self::parse_timestamp(value.get("lastUpdated")).or(last_message_ts);

        let title = messages
            .iter()
            .filter(|m| m.get("type").and_then(|t| t.as_str()) == Some("user"))
            .find_map(Self::extract_message_text)
            .map(|t| truncate_str(&t, 100));

        let model = messages.iter().rev().find_map(|m| {
            m.get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

        let folder_alias = Self::project_alias_from_chat_path(path);
        let folder_path = folder_alias
            .as_deref()
            .and_then(|alias| self.resolve_project_root(alias));
        let folder_name = folder_path
            .as_ref()
            .map(|p| folder_name_from_path(p))
            .or(folder_alias);

        Some(ConversationSummary {
            id,
            agent_type: AgentType::Gemini,
            folder_path,
            folder_name,
            title,
            started_at,
            ended_at,
            message_count: messages.len() as u32,
            model,
            git_branch: None,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        })
    }

    fn result_preview(result: Option<&Value>) -> Option<String> {
        let v = result?;
        if let Some(s) = v.as_str() {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            return Some(trimmed.to_string());
        }
        serde_json::to_string(v).ok()
    }

    fn result_display_preview(result_display: Option<&Value>) -> Option<String> {
        let value = result_display?;
        if let Some(summary) = value
            .get("summary")
            .and_then(Self::extract_text)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return Some(summary);
        }

        Self::result_preview(Some(value))
    }

    fn tool_call_is_error(call: &Value, output_preview: Option<&str>) -> bool {
        if call
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| {
                matches!(
                    s.to_ascii_lowercase().as_str(),
                    "error" | "failed" | "failure" | "cancelled" | "canceled"
                )
            })
            .unwrap_or(false)
        {
            return true;
        }

        if call
            .get("result")
            .and_then(|r| r.as_array())
            .map(|items| {
                items.iter().any(|item| {
                    item.get("functionResponse")
                        .and_then(|fr| fr.get("response"))
                        .and_then(|resp| resp.get("error"))
                        .is_some()
                })
            })
            .unwrap_or(false)
        {
            return true;
        }

        output_preview
            .map(|s| s.trim_start().to_ascii_lowercase().starts_with("error"))
            .unwrap_or(false)
    }

    fn parse_assistant_blocks(message: &Value) -> Vec<ContentBlock> {
        let mut blocks: Vec<ContentBlock> = Vec::new();

        if let Some(thoughts) = message.get("thoughts").and_then(|v| v.as_array()) {
            for thought in thoughts {
                let subject = thought
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let description = thought
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let text = match (subject, description) {
                    (Some(sub), Some(desc)) => format!("{sub}: {desc}"),
                    (Some(sub), None) => sub.to_string(),
                    (None, Some(desc)) => desc.to_string(),
                    (None, None) => continue,
                };
                blocks.push(ContentBlock::Thinking { text });
            }
        }

        if let Some(tool_calls) = message.get("toolCalls").and_then(|v| v.as_array()) {
            for call in tool_calls {
                let tool_use_id = call
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let tool_name = call
                    .get("displayName")
                    .or_else(|| call.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let input_preview = call
                    .get("args")
                    .and_then(|v| serde_json::to_string(v).ok())
                    .or_else(|| {
                        call.get("input")
                            .and_then(|v| Self::result_preview(Some(v)))
                    });

                blocks.push(ContentBlock::ToolUse {
                    tool_use_id: tool_use_id.clone(),
                    tool_name,
                    input_preview,
                    meta: None,
                });

                let output_preview = Self::result_display_preview(call.get("resultDisplay"))
                    .or_else(|| Self::result_preview(call.get("result")));
                let is_error = Self::tool_call_is_error(call, output_preview.as_deref());

                blocks.push(ContentBlock::ToolResult {
                    tool_use_id,
                    output_preview,
                    is_error,
                    agent_stats: None,
                });
            }
        }

        if let Some(text) = Self::extract_message_text(message) {
            blocks.push(ContentBlock::Text { text });
        }

        blocks
    }

    fn parse_usage(message: &Value) -> Option<TurnUsage> {
        let tokens = message.get("tokens")?;
        let input_tokens = tokens.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
        let output_tokens = tokens.get("output").and_then(|v| v.as_u64()).unwrap_or(0);
        let cached_tokens = tokens.get("cached").and_then(|v| v.as_u64()).unwrap_or(0);
        Some(TurnUsage {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: cached_tokens,
        })
    }

    fn parse_conversation_detail(
        &self,
        path: &Path,
        value: &Value,
        conversation_id: &str,
    ) -> Result<ConversationDetail, ParseError> {
        let mut summary = self
            .parse_summary_from_value(path, value)
            .ok_or_else(|| ParseError::ConversationNotFound(conversation_id.to_string()))?;
        let messages_raw = value
            .get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();

        let mut messages: Vec<UnifiedMessage> = Vec::new();
        for raw in messages_raw {
            let msg_id = raw
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("msg-{}", messages.len()));
            let timestamp =
                Self::parse_timestamp(raw.get("timestamp")).unwrap_or(summary.started_at);
            let msg_type = raw
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            match msg_type.as_str() {
                "user" => {
                    let blocks = Self::parse_user_blocks(&raw);
                    if blocks.is_empty() {
                        continue;
                    }
                    messages.push(UnifiedMessage {
                        id: msg_id,
                        role: MessageRole::User,
                        content: blocks,
                        timestamp,
                        usage: None,
                        duration_ms: None,
                        model: None,
                        completed_at: Some(timestamp),
                    });
                }
                "gemini" | "assistant" | "model" => {
                    let blocks = Self::parse_assistant_blocks(&raw);
                    if blocks.is_empty() {
                        continue;
                    }
                    messages.push(UnifiedMessage {
                        id: msg_id,
                        role: MessageRole::Assistant,
                        content: blocks,
                        timestamp,
                        usage: Self::parse_usage(&raw),
                        duration_ms: None,
                        model: raw
                            .get("model")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        completed_at: Some(timestamp),
                    });
                }
                "system" => {
                    let Some(text) = Self::extract_message_text(&raw) else {
                        continue;
                    };
                    messages.push(UnifiedMessage {
                        id: msg_id,
                        role: MessageRole::System,
                        content: vec![ContentBlock::Text { text }],
                        timestamp,
                        usage: None,
                        duration_ms: None,
                        model: None,
                        completed_at: Some(timestamp),
                    });
                }
                _ => {}
            }
        }

        // Approximate duration for assistant messages from adjacent timestamps
        for i in 0..messages.len() {
            if matches!(messages[i].role, MessageRole::Assistant)
                && messages[i].duration_ms.is_none()
            {
                if let Some(next) = messages.get(i + 1) {
                    let dur = (next.timestamp - messages[i].timestamp).num_milliseconds();
                    if dur > 0 && dur < 300_000 {
                        messages[i].duration_ms = Some(dur as u64);
                    }
                }
            }
        }

        let mut turns = group_into_turns(messages);
        super::relocate_orphaned_tool_results(&mut turns);
        super::structurize_read_tool_output(&mut turns);
        super::resolve_patch_line_numbers(&mut turns, summary.folder_path.as_deref());
        summary.message_count = turns.len() as u32;
        summary.id = conversation_id.to_string();
        let context_window_used_tokens = super::latest_turn_total_usage_tokens(&turns);
        let context_window_max_tokens =
            super::infer_context_window_max_tokens(summary.model.as_deref());
        let session_stats = super::merge_context_window_stats(
            super::compute_session_stats(&turns),
            context_window_used_tokens,
            context_window_max_tokens,
        );

        Ok(ConversationDetail {
            summary,
            turns,
            session_stats,
        })
    }
}

pub(crate) fn resolve_gemini_base_dir() -> PathBuf {
    resolve_gemini_base_dir_from(std::env::var_os("GEMINI_CLI_HOME"), dirs::home_dir())
}

fn resolve_gemini_base_dir_from(
    gemini_cli_home_env: Option<std::ffi::OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    gemini_cli_home_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.unwrap_or_default())
        .join(".gemini")
}

impl AgentParser for GeminiParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let mut conversations = Vec::new();

        for chat_file in self.list_chat_files() {
            let raw = match fs::read_to_string(&chat_file) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            let Some(value) = Self::parse_chat_value(&chat_file, &raw) else {
                continue;
            };
            if let Some(summary) = self.parse_summary_from_value(&chat_file, &value) {
                conversations.push(summary);
            }
        }

        conversations.sort_by_key(|b| std::cmp::Reverse(b.started_at));
        Ok(conversations)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        for chat_file in self.list_chat_files() {
            let raw = match fs::read_to_string(&chat_file) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            if !raw.contains(conversation_id) {
                continue;
            }

            let Some(value) = Self::parse_chat_value(&chat_file, &raw) else {
                continue;
            };
            let session_id = value.get("sessionId").and_then(|v| v.as_str());
            if session_id != Some(conversation_id) {
                continue;
            }

            return self.parse_conversation_detail(&chat_file, &value, conversation_id);
        }

        Err(ParseError::ConversationNotFound(
            conversation_id.to_string(),
        ))
    }
}

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
            continue;
        }

        if matches!(msg.role, MessageRole::System) {
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
            continue;
        }

        let mut blocks = msg.content.clone();
        let mut usage = msg.usage.clone();
        let mut duration_ms = msg.duration_ms;
        let mut models: Vec<String> = msg.model.iter().cloned().collect();
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
            if let Some(model) = &messages[i].model {
                models.push(model.clone());
            }
            if messages[i].completed_at.is_some() {
                completed_at = messages[i].completed_at;
            }
            i += 1;
        }

        let model = models.pop();

        turns.push(MessageTurn {
            id: format!("turn-{}", turns.len()),
            role: TurnRole::Assistant,
            blocks,
            timestamp,
            usage,
            duration_ms,
            model,
            completed_at,
        });
    }

    turns
}

#[cfg(test)]
mod tests {
    use super::resolve_gemini_base_dir_from;
    use super::GeminiParser;
    use crate::models::{ContentBlock, TurnRole};
    use crate::parsers::AgentParser;
    use chrono::{DateTime, Utc};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_gemini_session_detail_from_chat_json() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let base: PathBuf = env::temp_dir().join(format!("codeg-gemini-test-{nanos}"));
        let chats_dir = base.join("tmp").join("codeg").join("chats");
        fs::create_dir_all(&chats_dir).expect("create chat dir");
        fs::write(
            base.join("tmp").join("codeg").join(".project_root"),
            "/Users/test/workspace/demo",
        )
        .expect("write project root");

        let file_path = chats_dir.join("session-2026-03-02T04-30-32c7d221.json");
        let content = r#"{
  "sessionId": "32c7d221-0553-46c8-ba50-e664719cae7f",
  "projectHash": "abc",
  "startTime": "2026-03-02T04:30:20.796Z",
  "lastUpdated": "2026-03-02T04:33:13.631Z",
  "messages": [
    {
      "id": "u1",
      "timestamp": "2026-03-02T04:30:20.796Z",
      "type": "user",
      "content": [{"text": "你会做什么"}]
    },
    {
      "id": "a1",
      "timestamp": "2026-03-02T04:33:13.631Z",
      "type": "gemini",
      "content": "我是一个助手",
      "toolCalls": [
        {
          "id": "cli_help-1",
          "name": "cli_help",
          "args": {"question": "你会做什么"},
          "resultDisplay": "ok",
          "status": "success"
        }
      ],
      "tokens": {"input": 12, "output": 34, "cached": 5},
      "model": "gemini-3.1-pro-preview"
    }
  ]
}"#;
        fs::write(&file_path, content).expect("write chat file");

        let parser = GeminiParser::with_base_dir(base.clone());
        let summaries = parser.list_conversations().expect("list conversations");
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].id,
            "32c7d221-0553-46c8-ba50-e664719cae7f".to_string()
        );

        let detail = parser
            .get_conversation("32c7d221-0553-46c8-ba50-e664719cae7f")
            .expect("get conversation");
        assert_eq!(detail.turns.len(), 2);
        assert_eq!(
            detail.summary.folder_path.as_deref(),
            Some("/Users/test/workspace/demo")
        );
        assert!(detail.session_stats.is_some());
        let stats = detail.session_stats.expect("session stats");
        assert_eq!(stats.context_window_used_tokens, Some(51));
        assert_eq!(stats.context_window_max_tokens, Some(1_000_000));
        let percent = stats
            .context_window_usage_percent
            .expect("context window percent");
        assert!((percent - 0.0051).abs() < 1e-9);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn parses_gemini_session_detail_from_jsonl_chat_log() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let base: PathBuf = env::temp_dir().join(format!("codeg-gemini-jsonl-test-{nanos}"));
        let chats_dir = base.join("tmp").join("codeg-jsonl").join("chats");
        fs::create_dir_all(&chats_dir).expect("create chat dir");
        fs::write(
            base.join("tmp").join("codeg-jsonl").join(".project_root"),
            "/Users/test/workspace/jsonl-demo",
        )
        .expect("write project root");

        let file_path = chats_dir.join("session-2026-05-11T13-22-jsonl.jsonl");
        let content = r#"{"kind":"main","sessionId":"jsonl-session-1","projectHash":"abc","startTime":"2026-05-11T13:22:43.000Z","lastUpdated":"2026-05-11T13:22:43.000Z"}
{"kind":"main","sessionId":"jsonl-session-1","projectHash":"abc","startTime":"2026-05-11T13:22:43.000Z","lastUpdated":"2026-05-11T13:22:44.000Z"}
{"id":"u1","timestamp":"2026-05-11T13:23:16.870Z","type":"user","content":[{"text":"hello from jsonl"}]}
{"$set":{"lastUpdated":"2026-05-11T13:23:16.870Z"}}
{"id":"a1","timestamp":"2026-05-11T13:23:23.568Z","type":"gemini","content":"partial answer","model":"gemini-2.5-pro"}
{"id":"a1","timestamp":"2026-05-11T13:23:23.568Z","type":"gemini","content":"final answer","toolCalls":[{"id":"read-1","name":"read_file","args":{"path":"README.md"},"resultDisplay":{"summary":"Read README.md"},"status":"success"}],"tokens":{"input":10,"output":20,"cached":3},"model":"gemini-2.5-pro"}
"#;
        fs::write(&file_path, content).expect("write jsonl chat file");

        let parser = GeminiParser::with_base_dir(base.clone());
        let summaries = parser.list_conversations().expect("list conversations");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "jsonl-session-1");
        assert_eq!(summaries[0].message_count, 2);
        assert_eq!(summaries[0].title.as_deref(), Some("hello from jsonl"));
        assert_eq!(
            summaries[0].folder_path.as_deref(),
            Some("/Users/test/workspace/jsonl-demo")
        );

        let detail = parser
            .get_conversation("jsonl-session-1")
            .expect("get conversation");
        assert_eq!(detail.turns.len(), 2);
        assert!(matches!(detail.turns[0].role, TurnRole::User));
        assert!(matches!(detail.turns[1].role, TurnRole::Assistant));

        let assistant = &detail.turns[1];
        assert_eq!(assistant.model.as_deref(), Some("gemini-2.5-pro"));
        let text_blocks: Vec<&str> = assistant
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text_blocks, vec!["final answer"]);
        assert!(assistant.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::ToolResult {
                output_preview: Some(output),
                is_error: false,
                ..
            } if output == "Read README.md"
        )));
        let stats = detail.session_stats.expect("session stats");
        assert_eq!(stats.total_tokens, Some(33));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn parse_detail_completion_time_uses_assistant_timestamp_not_next_message_gap() {
        // Regression: Gemini's `duration_ms` heuristic is `next_msg.ts -
        // assistant.ts`, which means a quick user follow-up makes the gap
        // meaningless as a duration. completed_at must NOT be derived from
        // that gap; it must reflect when the assistant message was logged.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time ok")
            .as_nanos();
        let base: PathBuf = env::temp_dir().join(format!("codeg-gemini-completed-{nanos}"));
        let chats_dir = base.join("tmp").join("codeg").join("chats");
        fs::create_dir_all(&chats_dir).expect("create chat dir");

        let file_path = chats_dir.join("session-completed.json");
        let content = r#"{
  "sessionId": "completed-1",
  "projectHash": "abc",
  "startTime": "2026-03-02T04:30:00.000Z",
  "lastUpdated": "2026-03-02T04:30:50.000Z",
  "messages": [
    {"id": "u1", "timestamp": "2026-03-02T04:30:00.000Z", "type": "user", "content": [{"text": "ping"}]},
    {"id": "a1", "timestamp": "2026-03-02T04:30:02.000Z", "type": "gemini", "content": "pong", "model": "gemini-3.1-pro-preview"},
    {"id": "u2", "timestamp": "2026-03-02T04:30:50.000Z", "type": "user", "content": [{"text": "follow-up after 48s"}]}
  ]
}"#;
        fs::write(&file_path, content).expect("write chat file");

        let parser = GeminiParser::with_base_dir(base.clone());
        let detail = parser
            .get_conversation("completed-1")
            .expect("get conversation");

        let assistant = detail
            .turns
            .iter()
            .find(|t| matches!(t.role, TurnRole::Assistant))
            .expect("assistant turn");
        let completed_at = assistant.completed_at.expect("completed_at populated");
        let expected = "2026-03-02T04:30:02.000Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(completed_at, expected);
        // The naive `timestamp + (next_user.ts - assistant.ts)` would land
        // on the second user message timestamp.
        let wrong = "2026-03-02T04:30:50.000Z".parse::<DateTime<Utc>>().unwrap();
        assert_ne!(completed_at, wrong);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn gemini_cli_home_env_overrides_user_home() {
        let resolved = resolve_gemini_base_dir_from(
            Some(std::ffi::OsString::from("/tmp/gemini-home")),
            Some(PathBuf::from("/Users/default")),
        );
        assert_eq!(resolved, PathBuf::from("/tmp/gemini-home/.gemini"));
    }

    #[test]
    fn gemini_defaults_to_home_dot_gemini() {
        let resolved = resolve_gemini_base_dir_from(None, Some(PathBuf::from("/Users/default")));
        assert_eq!(resolved, PathBuf::from("/Users/default/.gemini"));
    }

    #[test]
    fn parses_user_inline_image_block() {
        let message = serde_json::json!({
            "content": [
                {"text": "这是什么"},
                {"inlineData": {"mimeType": "image/jpeg", "data": "QUJD"}}
            ]
        });

        let blocks = GeminiParser::parse_user_blocks(&message);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "这是什么"));
        assert!(matches!(
            &blocks[1],
            ContentBlock::Image { data, mime_type, uri }
            if data == "QUJD" && mime_type == "image/jpeg" && uri.is_none()
        ));
    }

    #[test]
    fn parses_user_data_uri_image_block() {
        let message = serde_json::json!({
            "content": [
                {
                    "inlineData": {
                        "data": "data:image/png;base64,QUJD"
                    }
                }
            ]
        });

        let blocks = GeminiParser::parse_user_blocks(&message);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(
            &blocks[0],
            ContentBlock::Image { data, mime_type, uri }
            if data == "QUJD" && mime_type == "image/png" && uri.is_none()
        ));
    }
}
