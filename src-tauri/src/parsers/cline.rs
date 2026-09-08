use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::models::{
    AgentType, ContentBlock, ConversationDetail, ConversationSummary, MessageTurn, TurnRole,
    TurnUsage,
};

use super::{
    backfill_turn_durations, compute_session_stats, folder_name_from_path, title_from_user_text,
    truncate_str, AgentParser, ParseError,
};

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

pub(crate) fn cline_data_dir() -> PathBuf {
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

pub struct ClineParser {
    base_dir: PathBuf,
}

impl Default for ClineParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ClineParser {
    pub fn new() -> Self {
        Self {
            base_dir: cline_data_dir(),
        }
    }

    /// Point the parser at an explicit Cline data dir (`CLINE_DIR`, containing
    /// `tasks/` + `state/`) instead of the env-resolved `~/.cline/data` — the
    /// provider workspace or a test fixture.
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    #[cfg(test)]
    /// Read-only access to the Cline data dir this parser reads from.
    pub(crate) fn base_dir(&self) -> &std::path::Path {
        &self.base_dir
    }
}

impl AgentParser for ClineParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let history_path = self.base_dir.join("state").join("taskHistory.json");
        if !history_path.exists() {
            return Ok(vec![]);
        }

        let raw = fs::read_to_string(&history_path)?;
        let entries: Vec<TaskHistoryEntry> = serde_json::from_str(&raw)?;

        let mut summaries = Vec::new();
        for entry in entries {
            let tasks_dir = self.base_dir.join("tasks").join(&entry.id);
            if !tasks_dir.exists() {
                continue;
            }

            // Read model from task_metadata.json or taskHistory entry
            let model = entry.model_id.clone().or_else(|| {
                let meta_path = tasks_dir.join("task_metadata.json");
                fs::read_to_string(meta_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<TaskMetadata>(&raw).ok())
                    .and_then(|meta| meta.model_usage.first().and_then(|u| u.model_id.clone()))
            });

            let folder_path = entry.cwd_on_task_initialization.clone();
            let folder_name = folder_path.as_deref().map(folder_name_from_path);

            let title = entry
                .task
                .as_deref()
                .map(|t| title_from_user_text(t.trim()));

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
                parent_id: None,
                parent_tool_use_id: None,
                delegation_call_id: None,
            });
        }

        Ok(summaries)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        let tasks_dir = self.base_dir.join("tasks").join(conversation_id);
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
        let history_path = self.base_dir.join("state").join("taskHistory.json");
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
            .map(|t| title_from_user_text(t.trim()));

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
                        completed_at: Some(timestamp),
                        agent_message_id: None,
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
                            completed_at: Some(timestamp),
                            agent_message_id: None,
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
                            completed_at: Some(timestamp),
                            agent_message_id: None,
                        });
                    }
                }
                _ => continue,
            }
        }

        let started_at = turns.first().map(|t| t.timestamp).unwrap_or_else(Utc::now);
        let ended_at = turns.last().map(|t| t.timestamp);

        backfill_turn_durations(&mut turns, &[]);
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
                agent_stats: None,
                images: Vec::new(),
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

/// Markers Cline wraps around the parts of a user message.
const ENV_OPEN: &str = "<environment_details>";
const ENV_CLOSE: &str = "</environment_details>";
const TASK_OPEN: &str = "<task>";
const TASK_CLOSE: &str = "</task>";
const FEEDBACK_OPEN: &str = "<feedback>";
const FEEDBACK_CLOSE: &str = "</feedback>";
const TASK_PROGRESS_MARKER: &str = "# task_progress RECOMMENDED";

/// Check if text looks like a Cline tool result: `[tool_name ...] Result:`
fn is_tool_result_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('[') && trimmed.contains("] Result:")
}

/// Parse `[tool_name for 'arg'] Result:\ncontent` into (tool_name, output, is_error).
fn parse_tool_result_text(text: &str) -> (String, String, bool) {
    let trimmed = text.trim_start();
    // Extract tool name from [tool_name ...] or [tool_name] prefix
    let tool_name = trimmed
        .strip_prefix('[')
        .and_then(|s| s.find([']', ' ']).map(|i| s[..i].to_string()))
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
///
/// The closing tag is searched from after the opening one, for the reason
/// spelled out on [`strip_environment_details`]: a message that quotes
/// `</feedback>` ahead of the real block would otherwise drop the feedback.
fn extract_feedback(text: &str) -> Option<String> {
    let start = text.find(FEEDBACK_OPEN)?;
    let inner_start = start + FEEDBACK_OPEN.len();
    let end = inner_start + text[inner_start..].find(FEEDBACK_CLOSE)?;
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
                        let tool_use_id = item.get("id").and_then(|v| v.as_str()).map(String::from);
                        let input_preview = item.get("input").map(|v| {
                            let s = v.to_string();
                            truncate_str(&s, 2000)
                        });
                        blocks.push(ContentBlock::ToolUse {
                            tool_use_id,
                            tool_name,
                            input_preview,
                            status: None,
                            meta: None,
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
                            agent_stats: None,
                            images: Vec::new(),
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
///
/// Every closing tag is searched from after the opening tag it belongs to, not
/// from the start of the message. Cline wraps what the user typed, so the
/// user's own words land in the same string as these markers, and a message
/// that quotes `</environment_details>` or `</task>` (asking about the wrapper,
/// or pasting a transcript) puts a closing tag ahead of the block it appears to
/// close. Rebuilding the message around that earlier tag splices the opening
/// tag back in and makes the string longer every pass, and reads a reversed
/// byte range.
fn strip_environment_details(text: &str) -> String {
    let mut result = text.to_string();

    // Remove <environment_details>...</environment_details>
    while let Some(start) = result.find(ENV_OPEN) {
        let before = result.len();
        let inner_start = start + ENV_OPEN.len();
        if let Some(close) = result[inner_start..].find(ENV_CLOSE) {
            let end = inner_start + close + ENV_CLOSE.len();
            result = format!("{}{}", &result[..start], &result[end..]);
        } else {
            // Unclosed tag — remove from start to end
            result = result[..start].to_string();
        }
        // Searching the closing tag from `inner_start` is what keeps every
        // pass strictly shorter, and a pass that does not shrink is a pass
        // this loop repeats forever. Asserted rather than left implied
        // because the regression it guards grew the string exponentially: a
        // test that trips it again would hang the run and exhaust memory
        // instead of failing, and this fails it on the first pass.
        debug_assert!(
            result.len() < before,
            "environment strip must shrink the message on every pass"
        );
    }

    // Remove <task>...</task> wrappers, keeping inner content
    while let Some(start) = result.find(TASK_OPEN) {
        let inner_start = start + TASK_OPEN.len();
        let Some(close) = result[inner_start..]
            .find(TASK_CLOSE)
            .map(|i| inner_start + i)
        else {
            break;
        };
        let inner = result[inner_start..close].to_string();
        let after = &result[close + TASK_CLOSE.len()..];
        result = format!("{}{}{}", &result[..start], inner, after);
    }

    // Remove task_progress RECOMMENDED blocks
    while let Some(start) = result.find(TASK_PROGRESS_MARKER) {
        // Find the end: whichever section boundary comes first, or end of
        // string. A `\n#` heading between the block and the next `\n<` tag
        // starts its own section, so it ends this one.
        let rest = &result[start..];
        let end = match (rest.find("\n<"), rest.find("\n#")) {
            (Some(tag), Some(heading)) => Some(tag.min(heading)),
            (tag, heading) => tag.or(heading),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole message Cline writes for one user turn: the wrapper, then the
    /// environment block it appends. Everything here reaches
    /// `strip_environment_details` as a single string.
    fn cline_user_message(task: &str) -> String {
        format!(
            "<task>\n{task}\n</task>\n\n<environment_details>\n# VSCode Visible Files\nsrc/main.rs\n\n# Current Time\n2026-03-01T08:00:00Z\n</environment_details>"
        )
    }

    #[test]
    fn strips_the_wrapper_and_the_environment_block() {
        assert_eq!(
            strip_environment_details(&cline_user_message("Fix the parser")),
            "Fix the parser"
        );
    }

    /// A user asking about the wrapper puts `</environment_details>` in their
    /// own text, ahead of the block Cline appends. Closing the block at that
    /// earlier tag re-splices the opening tag into the result and makes the
    /// string longer on every pass, so the loop never ends: opening the
    /// conversation used to hang the parse and grow memory without bound.
    #[test]
    fn a_quoted_closing_tag_does_not_hang_the_environment_strip() {
        let quoted = "Why do I see </environment_details> in my logs?";
        let cleaned = strip_environment_details(&cline_user_message(quoted));
        assert_eq!(cleaned, quoted);
    }

    /// Same slip in the `<task>` loop reads a reversed byte range instead of
    /// looping, because the close tag ends up before the open tag's inner
    /// start. `&result[23..0]` panics.
    #[test]
    fn a_leading_closing_task_tag_does_not_panic() {
        assert_eq!(
            strip_environment_details("</task> leftover\n<task>real work</task>"),
            "</task> leftover\nreal work"
        );
    }

    /// Nested wrappers must still collapse to their innermost content, which is
    /// the behaviour the first closing tag after the opening one already gave.
    #[test]
    fn nested_task_wrappers_collapse() {
        assert_eq!(
            strip_environment_details("<task>outer <task>inner</task> tail</task>"),
            "outer inner tail"
        );
    }

    /// An unclosed opening tag still truncates at it, and an unclosed `<task>`
    /// still leaves the message alone rather than dropping the rest of it.
    #[test]
    fn unclosed_openers_keep_their_old_behaviour() {
        assert_eq!(
            strip_environment_details("kept\n<environment_details>\nnoise"),
            "kept"
        );
        assert_eq!(
            strip_environment_details("<task>no close"),
            "<task>no close"
        );
    }

    /// `# task_progress RECOMMENDED` runs to the next section. A markdown
    /// heading is a section, so a heading between the block and the next tag
    /// ends it. Preferring `\n<` regardless of position swallowed everything
    /// in between, here the whole `# Notes` section. The tag has to be one the
    /// earlier loops leave alone, which is every Cline tool tag.
    #[test]
    fn the_task_progress_block_ends_at_the_first_boundary() {
        let text = "intro\n# task_progress RECOMMENDED\n- [ ] step one\n# Notes\nkeep this\n<read_file>\n<path>src/main.rs</path>\n</read_file>";
        assert_eq!(
            strip_environment_details(text),
            "intro\n\n# Notes\nkeep this\n<read_file>\n<path>src/main.rs</path>\n</read_file>"
        );
    }

    /// The block Cline really sends, so the boundary above is pinned against
    /// the shape it exists for and not only against the synthetic one. Cline
    /// pushes the focus-chain instructions as their own content part
    /// (`FocusChainManager.generateFocusChainInstructions` →
    /// `userContent.push`), and no line inside them opens with `#` or `<`, so
    /// both searches come back empty and the whole block runs off the end of
    /// the string. Ending at the first boundary therefore changes nothing for
    /// a real transcript. Text mirrors cline's
    /// `src/core/task/focus-chain/prompts.ts`.
    #[test]
    fn the_real_task_progress_block_is_removed_whole() {
        let recommended = "\n\
             # task_progress RECOMMENDED\n\
             \n\
             When starting a new task, it is recommended to include a todo list \
             using the task_progress parameter.\n\
             \n\
             \n\
             1. Include a todo list using the task_progress parameter in your next tool call\n\
             2. Create a comprehensive checklist of all steps needed\n\
             3. Use markdown format: - [ ] for incomplete, - [x] for complete\n\
             \n\
             **Benefits of creating a todo/task_progress list now:**\n\
             \t- Clear roadmap for implementation\n\
             \t- Progress tracking throughout the task\n\
             \t- Nothing gets forgotten or missed\n\
             \t- Users can see, monitor, and edit the plan\n\
             \n\
             **Example structure:**```\n\
             - [ ] Analyze requirements\n\
             - [ ] Set up necessary files\n\
             - [ ] Implement main functionality\n\
             - [ ] Handle edge cases\n\
             - [ ] Test the implementation\n\
             - [ ] Verify results```\n\
             \n\
             Keeping the task_progress list updated helps track progress and \
             ensures nothing is missed.\n";
        assert!(recommended.starts_with("\n# task_progress RECOMMENDED\n"));
        assert_eq!(strip_environment_details(recommended), "");
    }

    /// Every index this file splices on comes from `str::find` and is a byte
    /// offset, so a message in a non-ASCII script has to come through intact:
    /// an offset that lands inside a multi-byte character panics with `byte
    /// index is not a char boundary` on the very next slice.
    #[test]
    fn multibyte_text_around_the_markers_survives() {
        let quoted = "环境说明 </environment_details> 是什么？🎉";
        assert_eq!(
            strip_environment_details(&cline_user_message(quoted)),
            quoted
        );
    }

    /// Feedback quoted ahead of the real block used to come back as `None`,
    /// because the closing tag found first sat before the opening one.
    #[test]
    fn feedback_is_read_from_its_own_closing_tag() {
        assert_eq!(
            extract_feedback("the </feedback> tag: <feedback>looks good</feedback>"),
            Some("looks good".to_string())
        );
        assert_eq!(
            extract_feedback("<feedback>plain</feedback>"),
            Some("plain".to_string())
        );
        assert_eq!(extract_feedback("no tags here"), None);
        assert_eq!(extract_feedback("<feedback>unclosed"), None);
    }
}
