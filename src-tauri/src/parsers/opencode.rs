use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, QueryResult,
    Statement,
};

use crate::models::*;
use crate::parsers::{folder_name_from_path, truncate_str, AgentParser, ParseError};

pub struct OpenCodeParser {
    base_dir: PathBuf,
}

impl Default for OpenCodeParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenCodeParser {
    pub fn new() -> Self {
        let base_dir = resolve_opencode_base_dir();
        Self { base_dir }
    }

    /// Test-only constructor that lets callers point the parser at a fixture
    /// directory containing an `opencode.db` SQLite file.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    fn sqlite_db_path(&self) -> PathBuf {
        self.base_dir.join("opencode.db")
    }

    fn block_on<F, T>(&self, fut: F) -> Result<T, ParseError>
    where
        F: Future<Output = Result<T, ParseError>>,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| ParseError::InvalidData(format!("failed to build runtime: {e}")))?;
        runtime.block_on(fut)
    }

    async fn open_sqlite_connection(&self) -> Result<DatabaseConnection, ParseError> {
        let db_path = self.sqlite_db_path();
        let db_url = format!(
            "sqlite:{}?mode=ro",
            urlencoding::encode(&db_path.to_string_lossy())
        );

        let mut opts = ConnectOptions::new(db_url);
        opts.max_connections(1)
            .min_connections(1)
            .connect_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(30))
            .sqlx_logging(false);

        let conn = Database::connect(opts).await?;
        conn.execute(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA busy_timeout=3000;".to_owned(),
        ))
        .await?;

        Ok(conn)
    }

    fn parse_sqlite_summary_row(row: &QueryResult) -> Result<ConversationSummary, ParseError> {
        let id: String = row.try_get("", "id")?;
        let directory: Option<String> = row.try_get("", "directory")?;
        let title: Option<String> = row.try_get("", "title")?;
        let created_ms: i64 = row.try_get("", "created_ms")?;
        let updated_ms: i64 = row.try_get("", "updated_ms")?;
        let message_count_i64: i64 = row.try_get("", "message_count")?;
        let model: Option<String> = row.try_get("", "model")?;

        let folder_path = normalize_optional_string(directory);
        let folder_name = folder_path.as_ref().map(|p| folder_name_from_path(p));

        let message_count = if message_count_i64 <= 0 {
            0
        } else {
            u32::try_from(message_count_i64).unwrap_or(u32::MAX)
        };

        Ok(ConversationSummary {
            id,
            agent_type: AgentType::OpenCode,
            folder_path,
            folder_name,
            title: normalize_optional_string(title),
            started_at: millis_to_datetime(created_ms),
            ended_at: (updated_ms > 0).then(|| millis_to_datetime(updated_ms)),
            message_count,
            model: normalize_optional_string(model),
            git_branch: None,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        })
    }

    async fn list_conversations_from_sqlite(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let conn = self.open_sqlite_connection().await?;

        let rows = conn
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                r#"
                SELECT
                    s.id AS id,
                    s.directory AS directory,
                    s.title AS title,
                    s.time_created AS created_ms,
                    s.time_updated AS updated_ms,
                    COALESCE((
                        SELECT COUNT(*)
                        FROM message m
                        WHERE m.session_id = s.id
                    ), 0) AS message_count,
                    (
                        SELECT json_extract(m2.data, '$.modelID')
                        FROM message m2
                        WHERE m2.session_id = s.id
                          AND json_extract(m2.data, '$.role') = 'assistant'
                        ORDER BY m2.time_created DESC
                        LIMIT 1
                    ) AS model
                FROM session s
                ORDER BY s.time_created DESC
                "#
                .to_string(),
            ))
            .await?;

        let mut conversations = Vec::with_capacity(rows.len());
        for row in rows {
            let summary = Self::parse_sqlite_summary_row(&row)?;
            if summary.message_count == 0 {
                continue;
            }
            conversations.push(summary);
        }

        Ok(conversations)
    }

    async fn sqlite_summary_by_id(
        &self,
        conn: &DatabaseConnection,
        conversation_id: &str,
    ) -> Result<Option<ConversationSummary>, ParseError> {
        let row = conn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                r#"
                SELECT
                    s.id AS id,
                    s.directory AS directory,
                    s.title AS title,
                    s.time_created AS created_ms,
                    s.time_updated AS updated_ms,
                    COALESCE((
                        SELECT COUNT(*)
                        FROM message m
                        WHERE m.session_id = s.id
                    ), 0) AS message_count,
                    (
                        SELECT json_extract(m2.data, '$.modelID')
                        FROM message m2
                        WHERE m2.session_id = s.id
                          AND json_extract(m2.data, '$.role') = 'assistant'
                        ORDER BY m2.time_created DESC
                        LIMIT 1
                    ) AS model
                FROM session s
                WHERE s.id = ?
                LIMIT 1
                "#,
                [conversation_id.into()],
            ))
            .await?;

        row.map(|r| Self::parse_sqlite_summary_row(&r)).transpose()
    }

    async fn get_conversation_from_sqlite(
        &self,
        conversation_id: &str,
    ) -> Result<ConversationDetail, ParseError> {
        let conn = self.open_sqlite_connection().await?;
        let summary = self
            .sqlite_summary_by_id(&conn, conversation_id)
            .await?
            .ok_or_else(|| ParseError::ConversationNotFound(conversation_id.to_string()))?;

        let messages = self.load_sqlite_messages(&conn, conversation_id).await?;
        let mut turns = group_into_turns(messages);
        super::relocate_orphaned_tool_results(&mut turns);
        super::structurize_read_tool_output(&mut turns);
        super::resolve_patch_line_numbers(&mut turns, summary.folder_path.as_deref());
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

    async fn load_sqlite_messages(
        &self,
        conn: &DatabaseConnection,
        conversation_id: &str,
    ) -> Result<Vec<UnifiedMessage>, ParseError> {
        let rows = conn
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                r#"
                SELECT id, time_created, data
                FROM message
                WHERE session_id = ?
                ORDER BY time_created ASC, id ASC
                "#,
                [conversation_id.into()],
            ))
            .await?;

        // Pre-scan: collect all subagent session IDs from task tool parts so we
        // can batch-load their tool calls in a single query instead of N queries.
        let subagent_session_ids = self.scan_subagent_session_ids(conn, conversation_id).await;
        let subagent_tools = batch_load_subagent_tool_calls(conn, &subagent_session_ids).await;

        let mut messages = Vec::with_capacity(rows.len());

        for row in rows {
            let msg_id: String = row.try_get("", "id")?;
            let row_time_created: i64 = row.try_get("", "time_created")?;
            let data_raw: String = row.try_get("", "data")?;

            let value: serde_json::Value = match serde_json::from_str(&data_raw) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let role = match value.get("role").and_then(|r| r.as_str()) {
                Some("user") => MessageRole::User,
                Some("assistant") => MessageRole::Assistant,
                Some("system") => MessageRole::System,
                Some("tool") => MessageRole::Tool,
                _ => continue,
            };

            let created_ms = value
                .get("time")
                .and_then(|t| t.get("created"))
                .and_then(|c| c.as_i64())
                .unwrap_or(row_time_created);
            let timestamp = millis_to_datetime(created_ms);

            let is_assistant = matches!(role, MessageRole::Assistant);
            let msg_model = if is_assistant {
                value
                    .get("modelID")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            };

            let (content_blocks, usage_from_step_finish) = self
                .load_sqlite_parts(conn, &msg_id, &subagent_tools)
                .await?;

            let usage = if is_assistant {
                extract_opencode_usage(&value).or(usage_from_step_finish)
            } else {
                None
            };

            let completed_ms = if is_assistant {
                value
                    .get("time")
                    .and_then(|t| t.get("completed"))
                    .and_then(|c| c.as_i64())
            } else {
                None
            };
            let duration_ms = match completed_ms {
                Some(done) if done > created_ms => Some((done - created_ms) as u64),
                _ => None,
            };
            // OpenCode is the only parser whose `timestamp` is the message
            // creation time; for assistants the real completion is the
            // explicit `time.completed` millisecond. Reject values that
            // aren't strictly after `created_ms` (zero, partial writes,
            // clock skew) — those would render as 1970 or before the start.
            // Fall back to the creation timestamp in that case.
            let completed_at = match completed_ms {
                Some(done) if done > created_ms => Some(millis_to_datetime(done)),
                _ => Some(timestamp),
            };

            messages.push(UnifiedMessage {
                id: msg_id,
                role,
                content: content_blocks,
                timestamp,
                usage,
                duration_ms,
                model: msg_model,
                completed_at,
            });
        }

        Ok(messages)
    }

    /// Scan all tool parts in this conversation to extract subagent session IDs.
    async fn scan_subagent_session_ids(
        &self,
        conn: &DatabaseConnection,
        conversation_id: &str,
    ) -> Vec<String> {
        let rows = match conn
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                r#"
                SELECT DISTINCT json_extract(p.data, '$.state.metadata.sessionId') AS sid
                FROM part p
                INNER JOIN message m ON m.id = p.message_id
                WHERE m.session_id = ?
                  AND json_extract(p.data, '$.type') = 'tool'
                  AND json_extract(p.data, '$.tool') = 'task'
                  AND json_extract(p.data, '$.state.input.subagent_type') IS NOT NULL
                  AND sid IS NOT NULL
                "#,
                [conversation_id.into()],
            ))
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        rows.iter()
            .filter_map(|row| row.try_get::<String>("", "sid").ok())
            .collect()
    }

    async fn load_sqlite_parts(
        &self,
        conn: &DatabaseConnection,
        message_id: &str,
        subagent_tools: &HashMap<String, Vec<AgentToolCall>>,
    ) -> Result<(Vec<ContentBlock>, Option<TurnUsage>), ParseError> {
        let rows = conn
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                r#"
                SELECT data
                FROM part
                WHERE message_id = ?
                ORDER BY time_created ASC, id ASC
                "#,
                [message_id.into()],
            ))
            .await?;

        let mut blocks = Vec::new();
        let mut usage_from_step_finish: Option<TurnUsage> = None;

        for row in rows {
            let data_raw: String = row.try_get("", "data")?;
            let value: serde_json::Value = match serde_json::from_str(&data_raw) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let part_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match part_type {
                "text" => {
                    if let Some(text) = value
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        blocks.push(ContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }
                "reasoning" => {
                    if let Some(text) = value
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        blocks.push(ContentBlock::Thinking {
                            text: text.to_string(),
                        });
                    }
                }
                "tool" => {
                    let raw_tool_name = value
                        .get("tool")
                        .and_then(|t| t.as_str())
                        .unwrap_or("unknown");

                    let call_id = value
                        .get("callID")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string());

                    let state = value.get("state");
                    let status = state
                        .and_then(|s| s.get("status"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("");

                    let state_input = state.and_then(|s| s.get("input"));
                    let is_agent_task = raw_tool_name == "task"
                        && state_input
                            .and_then(|i| i.get("subagent_type"))
                            .and_then(|v| v.as_str())
                            .is_some();

                    if is_agent_task {
                        // Transform task tool into Agent card
                        let subagent_type = state_input
                            .and_then(|i| i.get("subagent_type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("agent");
                        let prompt = state_input
                            .and_then(|i| i.get("prompt"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let description = state
                            .and_then(|s| s.get("title"))
                            .and_then(|v| v.as_str())
                            .or_else(|| {
                                state_input
                                    .and_then(|i| i.get("description"))
                                    .and_then(|v| v.as_str())
                            })
                            .unwrap_or("");

                        let metadata = state.and_then(|s| s.get("metadata"));
                        let model_id = metadata
                            .and_then(|m| m.get("model"))
                            .and_then(|m| m.get("modelID"))
                            .and_then(|v| v.as_str());
                        let session_id = metadata
                            .and_then(|m| m.get("sessionId"))
                            .and_then(|v| v.as_str());

                        let mut agent_input = serde_json::json!({
                            "subagent_type": subagent_type,
                            "description": description,
                            "prompt": prompt,
                        });
                        if let Some(model) = model_id {
                            agent_input["model"] = serde_json::Value::String(model.to_string());
                        }

                        blocks.push(ContentBlock::ToolUse {
                            tool_use_id: call_id.clone(),
                            tool_name: "Agent".to_string(),
                            input_preview: Some(agent_input.to_string()),
                            meta: None,
                        });

                        let output_preview = state
                            .and_then(|s| s.get("output"))
                            .and_then(|v| value_to_preview(Some(v)))
                            .map(|s| extract_task_result_content(&s));

                        // Compute duration from time fields
                        let time = state.and_then(|s| s.get("time"));
                        let start_ms = time.and_then(|t| t.get("start")).and_then(|v| v.as_i64());
                        let end_ms = time.and_then(|t| t.get("end")).and_then(|v| v.as_i64());
                        let duration_ms = match (start_ms, end_ms) {
                            (Some(s), Some(e)) if e > s => Some((e - s) as u64),
                            _ => None,
                        };

                        // Look up pre-fetched sub-agent tool calls
                        let tool_calls = session_id
                            .and_then(|sid| subagent_tools.get(sid))
                            .cloned()
                            .unwrap_or_default();

                        let tool_count = tool_calls.len() as u32;
                        let agent_stats = Some(AgentExecutionStats {
                            agent_type: Some(subagent_type.to_string()),
                            status: Some(status.to_string()),
                            total_duration_ms: duration_ms,
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
                        });

                        let has_error_field = state.and_then(|s| s.get("error")).is_some();
                        blocks.push(ContentBlock::ToolResult {
                            tool_use_id: call_id,
                            output_preview,
                            is_error: is_error_status(status) || has_error_field,
                            agent_stats,
                        });
                    } else {
                        let input_preview = state_input.and_then(|v| value_to_preview(Some(v)));

                        blocks.push(ContentBlock::ToolUse {
                            tool_use_id: call_id.clone(),
                            tool_name: raw_tool_name.to_string(),
                            input_preview,
                            meta: None,
                        });

                        let output_preview = state
                            .and_then(|s| s.get("output"))
                            .and_then(|v| value_to_preview(Some(v)));

                        let has_error_field = state.and_then(|s| s.get("error")).is_some();

                        blocks.push(ContentBlock::ToolResult {
                            tool_use_id: call_id,
                            output_preview,
                            is_error: is_error_status(status) || has_error_field,
                            agent_stats: None,
                        });
                    }
                }
                "file" => {
                    if let Some(image_block) = extract_opencode_file_image(&value) {
                        blocks.push(image_block);
                    } else if let Some(file_ref) = extract_file_reference(&value) {
                        blocks.push(ContentBlock::Text {
                            text: format!("@{}", file_ref),
                        });
                    }
                }
                "patch" => {
                    let files = value
                        .get("files")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|item| item.as_str())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    if !files.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: format!("Applied patch: {}", files.join(", ")),
                        });
                    }
                }
                "step-finish" if usage_from_step_finish.is_none() => {
                    usage_from_step_finish = value
                        .get("tokens")
                        .and_then(extract_opencode_usage_from_tokens);
                }
                _ => {}
            }
        }

        Ok((blocks, usage_from_step_finish))
    }
}

impl AgentParser for OpenCodeParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        if !self.sqlite_db_path().exists() {
            return Ok(Vec::new());
        }

        self.block_on(self.list_conversations_from_sqlite())
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        if !self.sqlite_db_path().exists() {
            return Err(ParseError::ConversationNotFound(
                conversation_id.to_string(),
            ));
        }

        self.block_on(self.get_conversation_from_sqlite(conversation_id))
    }
}

pub(crate) fn resolve_opencode_base_dir() -> PathBuf {
    resolve_xdg_data_home(std::env::var_os("XDG_DATA_HOME"), dirs::home_dir())
        .map(|xdg_data_home| xdg_data_home.join("opencode"))
        .unwrap_or_else(|| PathBuf::from("opencode"))
}

fn resolve_xdg_data_home(
    xdg_data_home_env: Option<std::ffi::OsString>,
    home_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    xdg_data_home_env
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir.map(|home| home.join(".local").join("share")))
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn value_to_preview(value: Option<&serde_json::Value>) -> Option<String> {
    let v = value?;
    if v.is_null() {
        return None;
    }

    if let Some(s) = v.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else {
        serde_json::to_string(v).ok()
    }
}

fn extract_file_reference(value: &serde_json::Value) -> Option<String> {
    value
        .get("source")
        .and_then(|s| s.get("path"))
        .and_then(|v| v.as_str())
        .or_else(|| value.get("filename").and_then(|v| v.as_str()))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
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

fn extract_opencode_file_image(value: &serde_json::Value) -> Option<ContentBlock> {
    let mime = value
        .get("mime")
        .or_else(|| value.get("mimeType"))
        .or_else(|| value.get("mime_type"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|m| !m.is_empty() && m.starts_with("image/"))
        .map(|s| s.to_string());

    let url = value
        .get("url")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(raw_url) = url {
        if let Some((mime_type, data)) = parse_data_uri_image(raw_url) {
            let uri = value
                .get("filename")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            return Some(ContentBlock::Image {
                data,
                mime_type,
                uri,
            });
        }
    }

    let mime_type = mime?;
    let data = value
        .get("data")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())?;
    let uri = value
        .get("filename")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("source")
                .and_then(|s| s.get("path"))
                .and_then(|v| v.as_str())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    Some(ContentBlock::Image {
        data,
        mime_type,
        uri,
    })
}

fn is_error_status(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "error" | "failed" | "failure" | "cancelled" | "canceled"
    )
}

fn extract_opencode_usage(value: &serde_json::Value) -> Option<TurnUsage> {
    value
        .get("tokens")
        .and_then(extract_opencode_usage_from_tokens)
}

fn extract_opencode_usage_from_tokens(tokens: &serde_json::Value) -> Option<TurnUsage> {
    let input = tokens.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
    let output = tokens.get("output").and_then(|v| v.as_u64()).unwrap_or(0);
    let cache = tokens.get("cache");
    let cache_write = cache
        .and_then(|c| c.get("write"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read = cache
        .and_then(|c| c.get("read"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if input == 0 && output == 0 && cache_write == 0 && cache_read == 0 {
        return None;
    }

    Some(TurnUsage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: cache_write,
        cache_read_input_tokens: cache_read,
    })
}

fn millis_to_datetime(ms: i64) -> DateTime<Utc> {
    let secs = ms / 1000;
    let nsecs = ((ms.rem_euclid(1000)) * 1_000_000) as u32;
    Utc.timestamp_opt(secs, nsecs)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Group flat messages into conversation turns (same strategy as Codex).
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

/// Extract the content inside `<task_result>…</task_result>` tags from OpenCode
/// task output, stripping the `task_id:` preamble and the wrapper tags.
/// Returns the original string unchanged if no tags are found.
fn extract_task_result_content(raw: &str) -> String {
    if let Some(start) = raw.find("<task_result>") {
        let content_start = start + "<task_result>".len();
        let content_end = raw[content_start..]
            .find("</task_result>")
            .map(|i| content_start + i)
            .unwrap_or(raw.len());
        let extracted = raw[content_start..content_end].trim();
        if !extracted.is_empty() {
            return extracted.to_string();
        }
    }
    raw.to_string()
}

/// Batch-load tool calls from multiple sub-agent sessions in a single query.
///
/// Returns a map from session_id to its list of `AgentToolCall` records.
/// This avoids N+1 queries when a conversation has many agent tasks.
async fn batch_load_subagent_tool_calls(
    conn: &DatabaseConnection,
    session_ids: &[String],
) -> HashMap<String, Vec<AgentToolCall>> {
    if session_ids.is_empty() {
        return HashMap::new();
    }

    // Build parameterized IN clause
    let placeholders: Vec<&str> = session_ids.iter().map(|_| "?").collect();
    let sql = format!(
        r#"
        SELECT m.session_id, p.data
        FROM part p
        INNER JOIN message m ON m.id = p.message_id
        WHERE m.session_id IN ({})
          AND json_extract(p.data, '$.type') = 'tool'
        ORDER BY m.session_id, p.time_created ASC, p.id ASC
        "#,
        placeholders.join(", ")
    );
    let values: Vec<sea_orm::Value> = session_ids.iter().map(|s| s.as_str().into()).collect();

    let rows = match conn
        .query_all(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            &sql,
            values,
        ))
        .await
    {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };

    let mut result: HashMap<String, Vec<AgentToolCall>> = HashMap::new();
    for row in rows {
        let sid: String = match row.try_get("", "session_id") {
            Ok(s) => s,
            Err(_) => continue,
        };
        let data_raw: String = match row.try_get("", "data") {
            Ok(d) => d,
            Err(_) => continue,
        };
        let value: serde_json::Value = match serde_json::from_str(&data_raw) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let tool_name = value
            .get("tool")
            .and_then(|t| t.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Skip nested task calls to avoid recursion
        let is_nested_task = tool_name == "task"
            && value
                .get("state")
                .and_then(|s| s.get("input"))
                .and_then(|i| i.get("subagent_type"))
                .is_some();
        if is_nested_task {
            continue;
        }

        let state = value.get("state");
        let input_preview = state
            .and_then(|s| s.get("input"))
            .and_then(|v| value_to_preview(Some(v)))
            .map(|s| truncate_str(&s, 500));
        let output_preview = state
            .and_then(|s| s.get("output"))
            .and_then(|v| value_to_preview(Some(v)))
            .map(|s| truncate_str(&s, 500));
        let status = state
            .and_then(|s| s.get("status"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let has_error_field = state.and_then(|s| s.get("error")).is_some();

        result.entry(sid).or_default().push(AgentToolCall {
            tool_name,
            input_preview,
            output_preview,
            is_error: is_error_status(status) || has_error_field,
        });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::{extract_opencode_file_image, resolve_xdg_data_home};
    use crate::models::ContentBlock;
    use std::path::PathBuf;

    #[test]
    fn xdg_data_home_env_overrides_home_fallback() {
        let resolved = resolve_xdg_data_home(
            Some(std::ffi::OsString::from("/tmp/xdg-data")),
            Some(PathBuf::from("/Users/default")),
        );
        assert_eq!(resolved, Some(PathBuf::from("/tmp/xdg-data")));
    }

    #[test]
    fn xdg_data_home_falls_back_to_home_local_share() {
        let resolved = resolve_xdg_data_home(None, Some(PathBuf::from("/Users/default")));
        assert_eq!(resolved, Some(PathBuf::from("/Users/default/.local/share")));
    }

    #[test]
    fn parses_opencode_user_image_file_part_from_data_uri() {
        let value = serde_json::json!({
            "type": "file",
            "mime": "image/jpeg",
            "filename": "avatar.jpg",
            "url": "data:image/jpeg;base64,QUJD"
        });

        let block = extract_opencode_file_image(&value);
        assert!(matches!(
            block,
            Some(ContentBlock::Image { data, mime_type, uri })
            if data == "QUJD" && mime_type == "image/jpeg" && uri.as_deref() == Some("avatar.jpg")
        ));
    }

    #[test]
    fn ignores_non_image_file_part_for_image_parsing() {
        let value = serde_json::json!({
            "type": "file",
            "mime": "text/plain",
            "filename": "notes.txt",
            "url": "file:///tmp/notes.txt"
        });

        assert!(extract_opencode_file_image(&value).is_none());
    }
}
