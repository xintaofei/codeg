use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Set,
};

use crate::db::entities::conversation;
use crate::db::error::DbError;
use crate::models::{AgentType, ImportResult};
use crate::parsers::claude::ClaudeParser;
use crate::parsers::cline::ClineParser;
use crate::parsers::codex::CodexParser;
use crate::parsers::gemini::GeminiParser;
use crate::parsers::grok::GrokParser;
use crate::parsers::openclaw::OpenClawParser;
use crate::parsers::opencode::OpenCodeParser;
use crate::parsers::{path_eq_for_matching, AgentParser};

pub async fn import_local_conversations(
    conn: &DatabaseConnection,
    folder_id: i32,
    folder_path: &str,
) -> Result<ImportResult, DbError> {
    let path = folder_path.to_string();

    // Run parsers in blocking task since they do filesystem I/O
    let summaries = tokio::task::spawn_blocking(move || {
        let parsers: Vec<(AgentType, Box<dyn AgentParser>)> = vec![
            (AgentType::ClaudeCode, Box::new(ClaudeParser::new())),
            (AgentType::Codex, Box::new(CodexParser::new())),
            (AgentType::OpenCode, Box::new(OpenCodeParser::new())),
            (AgentType::Gemini, Box::new(GeminiParser::new())),
            (AgentType::OpenClaw, Box::new(OpenClawParser::new())),
            (AgentType::Cline, Box::new(ClineParser::new())),
            (AgentType::Grok, Box::new(GrokParser::new())),
        ];

        let mut matched = Vec::new();
        for (at, parser) in &parsers {
            match parser.list_conversations() {
                Ok(convs) => {
                    for c in convs {
                        if c.folder_path
                            .as_deref()
                            .map(|p| path_eq_for_matching(p, path.as_str()))
                            .unwrap_or(false)
                        {
                            matched.push((*at, c));
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error listing {} conversations: {}", at, e);
                }
            }
        }
        matched
    })
    .await
    .map_err(|e| DbError::Migration(e.to_string()))?;

    let mut imported = 0u32;
    let mut skipped = 0u32;

    for (agent_type, summary) in &summaries {
        let at_str = serde_json::to_value(agent_type)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();

        // Check if already imported
        let exists = conversation::Entity::find()
            .filter(conversation::Column::ExternalId.eq(&summary.id))
            .filter(conversation::Column::AgentType.eq(&at_str))
            .one(conn)
            .await?;

        if exists.is_some() {
            skipped += 1;
            continue;
        }

        let created_at = summary.started_at;
        let updated_at = summary.ended_at.unwrap_or(created_at);
        let conv = conversation::ActiveModel {
            id: NotSet,
            folder_id: Set(folder_id),
            title: Set(summary.title.clone()),
            agent_type: Set(at_str.clone()),
            status: Set(conversation::ConversationStatus::Completed),
            model: Set(summary.model.clone()),
            git_branch: Set(summary.git_branch.clone()),
            external_id: Set(Some(summary.id.clone())),
            parent_id: Set(None),
            message_count: Set(summary.message_count as i32),
            created_at: Set(created_at),
            updated_at: Set(updated_at),
            deleted_at: Set(None),
        };
        conv.insert(conn).await?;

        imported += 1;
    }

    Ok(ImportResult { imported, skipped })
}
