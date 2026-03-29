use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::models::{
    AgentType, ContentBlock, ConversationDetail, ConversationSummary, MessageTurn, TurnRole,
    TurnUsage,
};

use super::{compute_session_stats, folder_name_from_path, truncate_str, AgentParser, ParseError};

// ---------------------------------------------------------------------------
// On-disk JSON structures
// ---------------------------------------------------------------------------

/// One entry in `~/.cline/data/state/taskHistory.json`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskHistoryEntry {
    id: String,
    ts: i64,
    task: Option<String>,
    #[allow(dead_code)]
    tokens_in: Option<u64>,
    #[allow(dead_code)]
    tokens_out: Option<u64>,
    #[allow(dead_code)]
    total_cost: Option<f64>,
    cwd_on_task_initialization: Option<String>,
    #[serde(default)]
    model_id: Option<String>,
}

/// `task_metadata.json` – we only need `model_usage`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskMetadata {
    #[serde(default)]
    model_usage: Vec<ModelUsageEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelUsageEntry {
    model_id: Option<String>,
    #[allow(dead_code)]
    model_provider_id: Option<String>,
}

/// One message in `api_conversation_history.json`.
#[derive(Debug, Deserialize)]
struct ApiMessage {
    role: String,
    #[serde(default)]
    content: serde_json::Value,
    ts: Option<i64>,
    #[serde(default, rename = "modelInfo")]
    model_info: Option<ApiModelInfo>,
    #[serde(default)]
    metrics: Option<ApiMetrics>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiModelInfo {
    model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiMetrics {
    tokens: Option<ApiTokenMetrics>,
}

#[derive(Debug, Deserialize)]
struct ApiTokenMetrics {
    #[serde(default)]
    prompt: Option<u64>,
    #[serde(default)]
    completion: Option<u64>,
    #[serde(default)]
    cached: Option<u64>,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

fn cline_data_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("CLINE_DIR") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cline")
        .join("data")
}

fn ts_to_datetime(ts: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ts).single().unwrap_or_default()
}

pub struct ClineParser;

impl ClineParser {
    pub fn new() -> Self {
        Self
    }
}

impl AgentParser for ClineParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let history_path = cline_data_dir().join("state").join("taskHistory.json");
        if !history_path.exists() {
            return Ok(vec![]);
        }

        let raw = fs::read_to_string(&history_path)?;
        let entries: Vec<TaskHistoryEntry> = serde_json::from_str(&raw)?;

        let mut summaries = Vec::new();
        for entry in entries {
            let tasks_dir = cline_data_dir().join("tasks").join(&entry.id);
            if !tasks_dir.exists() {
                continue;
            }

            // Read model from task_metadata.json or taskHistory entry
            let model = entry.model_id.clone().or_else(|| {
                let meta_path = tasks_dir.join("task_metadata.json");
                fs::read_to_string(meta_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<TaskMetadata>(&raw).ok())
                    .and_then(|meta| {
                        meta.model_usage
                            .first()
                            .and_then(|u| u.model_id.clone())
                    })
            });

            let folder_path = entry.cwd_on_task_initialization.clone();
            let folder_name = folder_path.as_deref().map(folder_name_from_path);

            let title = entry
                .task
                .as_deref()
                .map(|t| truncate_str(t.trim(), 100));

            // Count messages from api_conversation_history.json
            let api_path = tasks_dir.join("api_conversation_history.json");
            let message_count = fs::read_to_string(&api_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<Vec<serde_json::Value>>(&raw).ok())
                .map(|msgs| msgs.len() as u32)
                .unwrap_or(0);

            // started_at from task id (which is a timestamp), ended_at from ts
            let started_at = ts_to_datetime(entry.id.parse::<i64>().unwrap_or(entry.ts));
            let ended_at = if entry.ts > 0 {
                Some(ts_to_datetime(entry.ts))
            } else {
                None
            };

            summaries.push(ConversationSummary {
                id: entry.id,
                agent_type: AgentType::Cline,
                folder_path,
                folder_name,
                title,
                started_at,
                ended_at,
                message_count,
                model,
                git_branch: None,
            });
        }

        Ok(summaries)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        let tasks_dir = cline_data_dir().join("tasks").join(conversation_id);
        if !tasks_dir.exists() {
            return Err(ParseError::ConversationNotFound(
                conversation_id.to_string(),
            ));
        }

        let api_path = tasks_dir.join("api_conversation_history.json");
        if !api_path.exists() {
            return Err(ParseError::ConversationNotFound(
                conversation_id.to_string(),
            ));
        }

        let raw = fs::read_to_string(&api_path)?;
        let messages: Vec<ApiMessage> = serde_json::from_str(&raw)?;

        // Read metadata for model/cwd
        let meta_path = tasks_dir.join("task_metadata.json");
        let metadata = fs::read_to_string(&meta_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<TaskMetadata>(&raw).ok());

        let default_model = metadata
            .as_ref()
            .and_then(|m| m.model_usage.first())
            .and_then(|u| u.model_id.clone());

        // Read taskHistory for cwd and title
        let history_path = cline_data_dir().join("state").join("taskHistory.json");
        let history_entry = fs::read_to_string(&history_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<TaskHistoryEntry>>(&raw).ok())
            .and_then(|entries| entries.into_iter().find(|e| e.id == conversation_id));

        let folder_path = history_entry
            .as_ref()
            .and_then(|e| e.cwd_on_task_initialization.clone());
        let folder_name = folder_path.as_deref().map(folder_name_from_path);
        let title = history_entry
            .as_ref()
            .and_then(|e| e.task.as_deref())
            .map(|t| truncate_str(t.trim(), 100));

        let mut turns: Vec<MessageTurn> = Vec::new();
        let mut turn_counter = 0u32;

        for msg in &messages {
            let ts = msg.ts.unwrap_or(0);
            let timestamp = if ts > 0 {
                ts_to_datetime(ts)
            } else {
                Utc::now()
            };

            let model = msg
                .model_info
                .as_ref()
                .and_then(|info| info.model_id.clone())
                .or_else(|| default_model.clone());

            let usage = msg.metrics.as_ref().and_then(|m| {
                m.tokens.as_ref().map(|t| TurnUsage {
                    input_tokens: t.prompt.unwrap_or(0),
                    output_tokens: t.completion.unwrap_or(0),
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: t.cached.unwrap_or(0),
                })
            });

            match msg.role.as_str() {
                "assistant" => {
                    let blocks = parse_content_blocks(&msg.content);
                    if blocks.is_empty() {
                        continue;
                    }
                    turn_counter += 1;
                    turns.push(MessageTurn {
                        id: format!("{}-{}", conversation_id, turn_counter),
                        role: TurnRole::Assistant,
                        blocks,
                        timestamp,
                        usage,
                        duration_ms: None,
                        model,
                    });
                }
                "user" => {
                    // Cline packs tool results, user feedback, and automated
                    // messages into role:"user".  Split them into proper turns.
                    let parsed = parse_user_message_parts(&msg.content);

                    // Emit tool-result blocks as a system turn so they attach
                    // to the preceding assistant tool_use.
                    if !parsed.tool_results.is_empty() {
                        turn_counter += 1;
                        turns.push(MessageTurn {
                            id: format!("{}-{}", conversation_id, turn_counter),
                            role: TurnRole::System,
                            blocks: parsed.tool_results,
                            timestamp,
                            usage: None,
                            duration_ms: None,
                            model: None,
                        });
                    }

                    // Emit real user text (feedback / initial task) as a user turn.
                    if !parsed.user_blocks.is_empty() {
                        turn_counter += 1;
                        turns.push(MessageTurn {
                            id: format!("{}-{}", conversation_id, turn_counter),
                            role: TurnRole::User,
                            blocks: parsed.user_blocks,
                            timestamp,
                            usage: None,
                            duration_ms: None,
                            model: None,
                        });
                    }
                }
                _ => continue,
            }
        }

        let started_at = turns
            .first()
            .map(|t| t.timestamp)
            .unwrap_or_else(Utc::now);
        let ended_at = turns.last().map(|t| t.timestamp);

        let session_stats = compute_session_stats(&turns);

        let summary = ConversationSummary {
            id: conversation_id.to_string(),
            agent_type: AgentType::Cline,
            folder_path,
            folder_name,
            title,
            started_at,
            ended_at,
            message_count: turns.len() as u32,
            model: default_model,
            git_branch: None,
        };

        Ok(ConversationDetail {
            summary,
            turns,
            session_stats,
        })
    }
}

// ---------------------------------------------------------------------------
// Content block parsing
// ---------------------------------------------------------------------------

/// Result of splitting a Cline `role:"user"` message.
struct UserMessageParts {
    /// Tool result blocks (e.g. `[read_file for ...] Result:`)
    tool_results: Vec<ContentBlock>,
    /// Real user content (initial task text or `<feedback>` text)
    user_blocks: Vec<ContentBlock>,
}

/// Cline puts tool results, feedback, and automated prompts all into
/// `role:"user"` messages.  This function splits them apart.
fn parse_user_message_parts(content: &serde_json::Value) -> UserMessageParts {
    let texts = collect_text_parts(content);
    let mut tool_results = Vec::new();
    let mut user_blocks = Vec::new();

    for text in texts {
        let cleaned = strip_environment_details(&text);
        if cleaned.is_empty() {
            continue;
        }

        // Tool result pattern: `[tool_name ...] Result:`
        if is_tool_result_text(&cleaned) {
            let (tool_name, output, is_error) = parse_tool_result_text(&cleaned);
            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: None,
                output_preview: Some(truncate_str(&output, 2000)),
                is_error,
            });

            // If the tool result also contains <feedback>, extract it
            if let Some(feedback) = extract_feedback(&text) {
                let fb = feedback.trim();
                if !fb.is_empty() {
                    user_blocks.push(ContentBlock::Text {
                        text: fb.to_string(),
                    });
                }
            }
            // After extracting tool result, also check for non-feedback user
            // text following the result (e.g. "The user has provided feedback...")
            // — we intentionally skip these automated bridging messages.
            let _ = tool_name;
            continue;
        }

        // Pure feedback without tool result prefix
        if let Some(feedback) = extract_feedback(&cleaned) {
            let fb = feedback.trim();
            if !fb.is_empty() {
                user_blocks.push(ContentBlock::Text {
                    text: fb.to_string(),
                });
            }
            continue;
        }

        // Regular user text (initial task, etc.)
        user_blocks.push(ContentBlock::Text { text: cleaned });
    }

    UserMessageParts {
        tool_results,
        user_blocks,
    }
}

/// Collect all text strings from a content value (string or array of text blocks).
fn collect_text_parts(content: &serde_json::Value) -> Vec<String> {
    match content {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|item| {
                let t = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if t == "text" || t.is_empty() {
                    item.get("text")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .or_else(|| {
                            if t.is_empty() {
                                item.as_str().map(String::from)
                            } else {
                                None
                            }
                        })
                } else {
                    None
                }
            })
            .collect(),
        _ => vec![],
    }
}

/// Check if text looks like a Cline tool result: `[tool_name ...] Result:`
fn is_tool_result_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('[')
        && trimmed.contains("] Result:")
}

/// Parse `[tool_name for 'arg'] Result:\ncontent` into (tool_name, output, is_error).
fn parse_tool_result_text(text: &str) -> (String, String, bool) {
    let trimmed = text.trim_start();
    // Extract tool name from [tool_name ...] or [tool_name] prefix
    let tool_name = trimmed
        .strip_prefix('[')
        .and_then(|s| {
            s.find([']', ' '])
                .map(|i| s[..i].to_string())
        })
        .unwrap_or_default();

    let is_error = trimmed.contains("[ERROR]") || trimmed.contains("Error:");

    // Extract the content after "Result:\n"
    let output = trimmed
        .find("] Result:")
        .map(|i| {
            let after = &trimmed[i + "] Result:".len()..];
            after.trim().to_string()
        })
        .unwrap_or_default();

    // Strip automated bridging text that follows some results
    let output = strip_automated_bridging(&output);

    (tool_name, output, is_error)
}

/// Remove automated bridging messages that Cline appends after tool results.
fn strip_automated_bridging(text: &str) -> String {
    let mut result = text.to_string();

    // Remove "The user has provided feedback..." bridging
    if let Some(pos) = result.find("The user has provided feedback") {
        result = result[..pos].to_string();
    }

    // Remove "(This is an automated message...)" blocks
    if let Some(pos) = result.find("(This is an automated message") {
        result = result[..pos].to_string();
    }

    // Remove "# Next Steps" blocks
    if let Some(pos) = result.find("# Next Steps") {
        result = result[..pos].to_string();
    }

    result.trim().to_string()
}

/// Extract text from `<feedback>...</feedback>` tags.
fn extract_feedback(text: &str) -> Option<String> {
    let start = text.find("<feedback>")?;
    let inner_start = start + "<feedback>".len();
    let end = text.find("</feedback>")?;
    if end > inner_start {
        Some(text[inner_start..end].to_string())
    } else {
        None
    }
}

fn parse_content_blocks(content: &serde_json::Value) -> Vec<ContentBlock> {
    match content {
        serde_json::Value::String(text) => {
            let cleaned = strip_environment_details(text);
            if cleaned.is_empty() {
                vec![]
            } else {
                vec![ContentBlock::Text { text: cleaned }]
            }
        }
        serde_json::Value::Array(arr) => {
            let mut blocks = Vec::new();
            for item in arr {
                let block_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match block_type {
                    "text" => {
                        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                            let cleaned = strip_environment_details(text);
                            if !cleaned.is_empty() {
                                blocks.push(ContentBlock::Text { text: cleaned });
                            }
                        }
                    }
                    "tool_use" => {
                        let tool_name = item
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let tool_use_id =
                            item.get("id").and_then(|v| v.as_str()).map(String::from);
                        let input_preview = item.get("input").map(|v| {
                            let s = v.to_string();
                            truncate_str(&s, 2000)
                        });
                        blocks.push(ContentBlock::ToolUse {
                            tool_use_id,
                            tool_name,
                            input_preview,
                        });
                    }
                    "tool_result" => {
                        let tool_use_id = item
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        let is_error = item
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let output_preview = item
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(|s| truncate_str(s, 500));
                        blocks.push(ContentBlock::ToolResult {
                            tool_use_id,
                            output_preview,
                            is_error,
                        });
                    }
                    "thinking" => {
                        if let Some(text) = item.get("thinking").and_then(|v| v.as_str()) {
                            if !text.is_empty() {
                                blocks.push(ContentBlock::Thinking {
                                    text: text.to_string(),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            blocks
        }
        _ => vec![],
    }
}

/// Strip Cline's `<environment_details>...</environment_details>` blocks and
/// `<task>...</task>` wrappers from user messages to keep content clean.
fn strip_environment_details(text: &str) -> String {
    let mut result = text.to_string();

    // Remove <environment_details>...</environment_details>
    while let Some(start) = result.find("<environment_details>") {
        if let Some(end) = result.find("</environment_details>") {
            let end = end + "</environment_details>".len();
            result = format!("{}{}", &result[..start], &result[end..]);
        } else {
            // Unclosed tag — remove from start to end
            result = result[..start].to_string();
        }
    }

    // Remove <task>...</task> wrappers, keeping inner content
    while let Some(start) = result.find("<task>") {
        let tag_end = start + "<task>".len();
        if let Some(close) = result.find("</task>") {
            let inner = result[tag_end..close].to_string();
            let after = &result[close + "</task>".len()..];
            result = format!("{}{}{}", &result[..start], inner, after);
        } else {
            break;
        }
    }

    // Remove task_progress RECOMMENDED blocks
    while let Some(start) = result.find("# task_progress RECOMMENDED") {
        // Find the end: next section or end of string
        let rest = &result[start..];
        let end = rest
            .find("\n<")
            .or_else(|| rest.find("\n#"))
            .map(|i| start + i)
            .unwrap_or(result.len());
        result = format!("{}{}", &result[..start], &result[end..]);
    }

    // Remove [ERROR] automated retry messages
    if result.contains("[ERROR] You did not use a tool") {
        return String::new();
    }

    result.trim().to_string()
}
