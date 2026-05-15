use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use walkdir::WalkDir;

use crate::models::{
    AgentType, ContentBlock, ConversationDetail, ConversationSummary, MessageTurn, TurnRole,
};
use crate::parsers::{compute_session_stats, folder_name_from_path, AgentParser, ParseError};

pub struct GrokParser {
    base_dir: PathBuf,
}

impl GrokParser {
    pub fn new() -> Self {
        Self {
            base_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".grok")
                .join("sessions"),
        }
    }

    #[cfg(test)]
    fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    fn session_dir_by_id(&self, conversation_id: &str) -> Option<PathBuf> {
        let direct = self.base_dir.join(conversation_id);
        if direct.join("chat_history.jsonl").exists() {
            return Some(direct);
        }

        WalkDir::new(&self.base_dir)
            .min_depth(1)
            .max_depth(3)
            .into_iter()
            .filter_map(Result::ok)
            .find_map(|entry| {
                if !entry.file_type().is_dir() {
                    return None;
                }
                if entry.file_name().to_string_lossy() != conversation_id {
                    return None;
                }
                let path = entry.into_path();
                path.join("chat_history.jsonl").exists().then_some(path)
            })
    }

    fn parse_summary(
        &self,
        session_dir: &Path,
        conversation_id: &str,
    ) -> Result<ConversationSummary, ParseError> {
        let path = session_dir.join("summary.json");
        let raw = fs::read_to_string(&path)?;
        let summary: GrokSummaryFile = serde_json::from_str(&raw)?;
        let cwd = summary.info.cwd.filter(|s| !s.trim().is_empty());
        let title = summary
            .generated_title
            .or(summary.session_summary)
            .filter(|s| !s.trim().is_empty());
        let started_at = summary.created_at.unwrap_or_else(Utc::now);
        let ended_at = summary.updated_at.or(summary.last_active_at);

        Ok(ConversationSummary {
            id: summary
                .info
                .id
                .unwrap_or_else(|| conversation_id.to_string()),
            agent_type: AgentType::Grok,
            folder_name: cwd.as_ref().map(|p| folder_name_from_path(p)),
            folder_path: cwd,
            title,
            started_at,
            ended_at,
            message_count: summary
                .num_chat_messages
                .or(summary.num_messages)
                .unwrap_or(0),
            model: summary.current_model_id,
            git_branch: summary.head_branch,
        })
    }

    fn parse_conversation_detail(
        &self,
        session_dir: &Path,
        conversation_id: &str,
    ) -> Result<ConversationDetail, ParseError> {
        let summary = self.parse_summary(session_dir, conversation_id)?;
        let turns =
            parse_chat_history(&session_dir.join("chat_history.jsonl"), summary.started_at)?;
        let session_stats = compute_session_stats(&turns);
        Ok(ConversationDetail {
            summary,
            turns,
            session_stats,
        })
    }
}

impl AgentParser for GrokParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        if !self.base_dir.exists() {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        for entry in WalkDir::new(&self.base_dir)
            .min_depth(2)
            .max_depth(3)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() || entry.file_name() != "summary.json" {
                continue;
            }
            let Some(session_dir) = entry.path().parent() else {
                continue;
            };
            if !session_dir.join("chat_history.jsonl").exists() {
                continue;
            }
            let id = session_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Ok(summary) = self.parse_summary(session_dir, &id) {
                out.push(summary);
            }
        }
        out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(out)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        let session_dir = self
            .session_dir_by_id(conversation_id)
            .ok_or_else(|| ParseError::ConversationNotFound(conversation_id.to_string()))?;
        self.parse_conversation_detail(&session_dir, conversation_id)
    }
}

#[derive(Debug, Deserialize)]
struct GrokSummaryFile {
    info: GrokSummaryInfo,
    #[serde(default)]
    session_summary: Option<String>,
    #[serde(default)]
    generated_title: Option<String>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    last_active_at: Option<DateTime<Utc>>,
    #[serde(default)]
    num_messages: Option<u32>,
    #[serde(default)]
    num_chat_messages: Option<u32>,
    #[serde(default)]
    current_model_id: Option<String>,
    #[serde(default)]
    head_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GrokSummaryInfo {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GrokChatRecord {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    synthetic_reason: Option<String>,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    model_id: Option<String>,
}

fn parse_chat_history(
    path: &Path,
    started_at: DateTime<Utc>,
) -> Result<Vec<MessageTurn>, ParseError> {
    let raw = fs::read_to_string(path)?;
    let mut turns = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: GrokChatRecord = serde_json::from_str(trimmed)?;
        let Some(text) = record_text(&record).map(strip_user_query_wrapper) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }

        let role = match record.kind.as_str() {
            "user" if record.synthetic_reason.is_none() && !is_grok_context_message(&text) => {
                TurnRole::User
            }
            "assistant" => TurnRole::Assistant,
            _ => continue,
        };
        let idx = i32::try_from(turns.len()).unwrap_or(i32::MAX);
        turns.push(MessageTurn {
            id: format!("turn-{}", turns.len()),
            role,
            blocks: vec![ContentBlock::Text { text }],
            timestamp: started_at + Duration::seconds(i64::from(idx)),
            usage: None,
            duration_ms: None,
            model: record.model_id,
            completed_at: None,
        });
    }

    Ok(turns)
}

fn record_text(record: &GrokChatRecord) -> Option<String> {
    match record.content.as_ref()? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(|item| item.get("text").and_then(|v| v.as_str()))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        _ => None,
    }
}

fn strip_user_query_wrapper(text: String) -> String {
    let trimmed = text.trim();
    if let Some(inner) = trimmed
        .strip_prefix("<user_query>")
        .and_then(|s| s.strip_suffix("</user_query>"))
    {
        return inner.trim().to_string();
    }
    trimmed.to_string()
}

fn is_grok_context_message(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("<user_info>")
        || trimmed.starts_with("<system-reminder>")
        || trimmed.starts_with("<environment_context>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContentBlock;

    #[test]
    fn parses_chat_history_and_filters_context_messages() {
        let temp = tempfile::tempdir().unwrap();
        let session_dir = temp.path().join("cwd").join("session-1");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("summary.json"),
            r#"{
              "info": {"id": "session-1", "cwd": "/tmp/project"},
              "generated_title": "Smoke Test",
              "created_at": "2026-05-15T19:44:35.810223Z",
              "updated_at": "2026-05-15T19:44:43.430756Z",
              "num_chat_messages": 7,
              "current_model_id": "grok-build",
              "head_branch": "main"
            }"#,
        )
        .unwrap();
        fs::write(
            session_dir.join("chat_history.jsonl"),
            [
                r#"{"type":"system","content":"ignore"}"#,
                r#"{"type":"user","content":[{"type":"text","text":"<user_info>\nignore\n</user_info>"}]}"#,
                r#"{"type":"user","synthetic_reason":"project_instructions","content":[{"type":"text","text":"ignore"}]}"#,
                r#"{"type":"user","content":[{"type":"text","text":"<user_query>\nHello Grok\n</user_query>"}]}"#,
                r#"{"type":"assistant","model_id":"grok-build","content":"Hello user"}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let detail = GrokParser::with_base_dir(temp.path().to_path_buf())
            .get_conversation("session-1")
            .unwrap();

        assert_eq!(detail.summary.id, "session-1");
        assert_eq!(detail.summary.folder_path.as_deref(), Some("/tmp/project"));
        assert_eq!(detail.turns.len(), 2);
        assert!(matches!(detail.turns[0].role, TurnRole::User));
        assert!(matches!(detail.turns[1].role, TurnRole::Assistant));
        match &detail.turns[0].blocks[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hello Grok"),
            other => panic!("expected text block, got {other:?}"),
        }
        match &detail.turns[1].blocks[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hello user"),
            other => panic!("expected text block, got {other:?}"),
        }
    }
}
