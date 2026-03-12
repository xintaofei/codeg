use std::collections::{HashMap, HashSet};

use crate::app_error::AppCommandError;
use crate::db::entities::conversation;
use crate::db::service::{conversation_service, folder_service, import_service};
use crate::db::AppDatabase;
use crate::models::*;
use crate::parsers::claude::ClaudeParser;
use crate::parsers::codex::CodexParser;
use crate::parsers::gemini::GeminiParser;
use crate::parsers::opencode::OpenCodeParser;
use crate::parsers::{path_eq_for_matching, AgentParser, ParseError};

#[tauri::command]
pub async fn list_folder_conversations(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    status: Option<String>,
) -> Result<Vec<DbConversationSummary>, AppCommandError> {
    conversation_service::list_by_folder(&db.conn, folder_id, agent_type, search, sort_by, status)
        .await
        .map_err(AppCommandError::from)
}

/// Synchronous implementation shared by list_conversations, list_folders, and get_stats.
fn list_conversations_sync(
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    folder_path: Option<String>,
) -> Vec<ConversationSummary> {
    let mut all_conversations = Vec::new();
    let mut seen_keys = HashSet::new();

    let parsers: Vec<(AgentType, Box<dyn AgentParser>)> = vec![
        (AgentType::ClaudeCode, Box::new(ClaudeParser::new())),
        (AgentType::Codex, Box::new(CodexParser::new())),
        (AgentType::OpenCode, Box::new(OpenCodeParser::new())),
        (AgentType::Gemini, Box::new(GeminiParser::new())),
    ];

    for (at, parser) in &parsers {
        if let Some(ref filter) = agent_type {
            if filter != at {
                continue;
            }
        }
        match parser.list_conversations() {
            Ok(conversations) => {
                // Deduplicate conversations based on (agent_type, id) combination
                for conversation in conversations {
                    let key = format!("{:?}-{}", conversation.agent_type, conversation.id);
                    if seen_keys.insert(key) {
                        all_conversations.push(conversation);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error listing {} conversations: {}", at, e);
            }
        }
    }

    // Apply search filter
    if let Some(ref query) = search {
        let query_lower = query.to_lowercase();
        all_conversations.retain(|s| {
            s.title
                .as_ref()
                .map(|t| t.to_lowercase().contains(&query_lower))
                .unwrap_or(false)
                || s.folder_name
                    .as_ref()
                    .map(|p| p.to_lowercase().contains(&query_lower))
                    .unwrap_or(false)
                || s.folder_path
                    .as_ref()
                    .map(|p| p.to_lowercase().contains(&query_lower))
                    .unwrap_or(false)
                || s.git_branch
                    .as_ref()
                    .map(|b| b.to_lowercase().contains(&query_lower))
                    .unwrap_or(false)
                || s.model
                    .as_ref()
                    .map(|m| m.to_lowercase().contains(&query_lower))
                    .unwrap_or(false)
        });
    }

    // Apply folder path filter
    if let Some(ref fp) = folder_path {
        all_conversations.retain(|s| {
            s.folder_path
                .as_deref()
                .map(|p| path_eq_for_matching(p, fp.as_str()))
                .unwrap_or(false)
        });
    }

    // Apply sorting
    match sort_by.as_deref() {
        Some("oldest") => all_conversations.sort_by(|a, b| a.started_at.cmp(&b.started_at)),
        Some("messages") => all_conversations.sort_by(|a, b| b.message_count.cmp(&a.message_count)),
        _ => all_conversations.sort_by(|a, b| b.started_at.cmp(&a.started_at)), // default: newest first
    }

    all_conversations
}

#[tauri::command]
pub async fn list_conversations(
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    folder_path: Option<String>,
) -> Result<Vec<ConversationSummary>, AppCommandError> {
    tokio::task::spawn_blocking(move || {
        list_conversations_sync(agent_type, search, sort_by, folder_path)
    })
    .await
    .map_err(|e| {
        AppCommandError::task_execution_failed("Failed to list conversations")
            .with_detail(e.to_string())
    })
}

#[tauri::command]
pub async fn get_conversation(
    agent_type: AgentType,
    conversation_id: String,
) -> Result<ConversationDetail, AppCommandError> {
    tokio::task::spawn_blocking(move || -> Result<ConversationDetail, AppCommandError> {
        let parser: Box<dyn AgentParser> = match agent_type {
            AgentType::ClaudeCode => Box::new(ClaudeParser::new()),
            AgentType::Codex => Box::new(CodexParser::new()),
            AgentType::OpenCode => Box::new(OpenCodeParser::new()),
            AgentType::Gemini => Box::new(GeminiParser::new()),
            _ => {
                return Err(AppCommandError::invalid_input(
                    "Conversation parsing is not supported for this agent",
                )
                .with_detail(format!("agent_type={agent_type}")))
            }
        };

        parser
            .get_conversation(&conversation_id)
            .map_err(parse_error_to_app_error)
    })
    .await
    .map_err(|e| {
        AppCommandError::task_execution_failed("Failed to load conversation")
            .with_detail(e.to_string())
    })?
}

#[tauri::command]
pub async fn list_folders() -> Result<Vec<FolderInfo>, AppCommandError> {
    tokio::task::spawn_blocking(move || -> Result<Vec<FolderInfo>, AppCommandError> {
        let all_conversations = list_conversations_sync(None, None, None, None);
        Ok(compute_folders(&all_conversations))
    })
    .await
    .map_err(|e| {
        AppCommandError::task_execution_failed("Failed to list folders").with_detail(e.to_string())
    })?
}

#[tauri::command]
pub async fn get_stats() -> Result<AgentStats, AppCommandError> {
    tokio::task::spawn_blocking(move || -> Result<AgentStats, AppCommandError> {
        let all_conversations = list_conversations_sync(None, None, None, None);
        Ok(compute_stats(&all_conversations))
    })
    .await
    .map_err(|e| {
        AppCommandError::task_execution_failed("Failed to compute conversation stats")
            .with_detail(e.to_string())
    })?
}

#[tauri::command]
pub async fn get_sidebar_data() -> Result<SidebarData, AppCommandError> {
    tokio::task::spawn_blocking(move || -> Result<SidebarData, AppCommandError> {
        let all_conversations = list_conversations_sync(None, None, None, None);
        let folders = compute_folders(&all_conversations);
        let stats = compute_stats(&all_conversations);
        Ok(SidebarData { folders, stats })
    })
    .await
    .map_err(|e| {
        AppCommandError::task_execution_failed("Failed to build sidebar data")
            .with_detail(e.to_string())
    })?
}

fn compute_folders(all_conversations: &[ConversationSummary]) -> Vec<FolderInfo> {
    let mut folder_map: HashMap<String, FolderInfo> = HashMap::new();

    for conversation in all_conversations {
        let path = conversation
            .folder_path
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let name = conversation
            .folder_name
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        let entry = folder_map
            .entry(path.clone())
            .or_insert_with(|| FolderInfo {
                path: path.clone(),
                name,
                agent_types: Vec::new(),
                conversation_count: 0,
            });

        entry.conversation_count += 1;
        if !entry.agent_types.contains(&conversation.agent_type) {
            entry.agent_types.push(conversation.agent_type);
        }
    }

    let mut folders: Vec<FolderInfo> = folder_map.into_values().collect();
    folders.sort_by(|a, b| b.conversation_count.cmp(&a.conversation_count));
    folders
}

#[tauri::command]
pub async fn import_local_conversations(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<ImportResult, AppCommandError> {
    let folder = folder_service::get_folder_by_id(&db.conn, folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| {
            AppCommandError::not_found("Folder not found")
                .with_detail(format!("folder_id={folder_id}"))
        })?;

    import_service::import_local_conversations(&db.conn, folder_id, &folder.path)
        .await
        .map_err(AppCommandError::from)
}

#[tauri::command]
pub async fn get_folder_conversation(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
) -> Result<DbConversationDetail, AppCommandError> {
    let summary = conversation_service::get_by_id(&db.conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?;

    let (turns, session_stats) = if let Some(ref ext_id) = summary.external_id {
        let at = summary.agent_type;
        let eid = ext_id.clone();
        tokio::task::spawn_blocking(move || -> Result<_, AppCommandError> {
            let parser: Box<dyn AgentParser> = match at {
                AgentType::ClaudeCode => Box::new(ClaudeParser::new()),
                AgentType::Codex => Box::new(CodexParser::new()),
                AgentType::OpenCode => Box::new(OpenCodeParser::new()),
                AgentType::Gemini => Box::new(GeminiParser::new()),
                _ => return Ok((vec![], None)),
            };
            // If the external session file doesn't exist yet (e.g., new ACP session
            // not yet synced to disk), return empty turns instead of failing.
            match parser.get_conversation(&eid) {
                Ok(d) => Ok((d.turns, d.session_stats)),
                Err(crate::parsers::ParseError::ConversationNotFound(_)) => Ok((vec![], None)),
                Err(e) => Err(parse_error_to_app_error(e)),
            }
        })
        .await
        .map_err(|e| {
            AppCommandError::task_execution_failed(
                "Failed to read conversation turns from session file",
            )
            .with_detail(e.to_string())
        })??
    } else {
        (vec![], None)
    };

    let mut summary = summary;
    summary.message_count = turns.len() as u32;

    Ok(DbConversationDetail {
        summary,
        turns,
        session_stats,
    })
}

#[tauri::command]
pub async fn create_conversation(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    agent_type: AgentType,
    title: Option<String>,
) -> Result<i32, AppCommandError> {
    // Detect current git branch from the folder path
    let git_branch = if let Some(folder) = folder_service::get_folder_by_id(&db.conn, folder_id)
        .await
        .map_err(AppCommandError::from)?
    {
        detect_git_branch(&folder.path).await
    } else {
        None
    };

    let model = conversation_service::create(&db.conn, folder_id, agent_type, title, git_branch)
        .await
        .map_err(AppCommandError::from)?;
    Ok(model.id)
}

async fn detect_git_branch(path: &str) -> Option<String> {
    let output = crate::process::tokio_command("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        return None;
    }
    Some(branch)
}

#[tauri::command]
pub async fn update_conversation_status(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
    status: String,
) -> Result<(), AppCommandError> {
    let status_enum: conversation::ConversationStatus =
        serde_json::from_value(serde_json::Value::String(status)).map_err(|e| {
            AppCommandError::invalid_input("Invalid conversation status").with_detail(e.to_string())
        })?;
    conversation_service::update_status(&db.conn, conversation_id, status_enum)
        .await
        .map_err(AppCommandError::from)
}

#[tauri::command]
pub async fn update_conversation_title(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
    title: String,
) -> Result<(), AppCommandError> {
    conversation_service::update_title(&db.conn, conversation_id, title)
        .await
        .map_err(AppCommandError::from)
}

#[tauri::command]
pub async fn update_conversation_external_id(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
    external_id: String,
) -> Result<(), AppCommandError> {
    conversation_service::update_external_id(&db.conn, conversation_id, external_id)
        .await
        .map_err(AppCommandError::from)
}

#[tauri::command]
pub async fn delete_conversation(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
) -> Result<(), AppCommandError> {
    conversation_service::soft_delete(&db.conn, conversation_id)
        .await
        .map_err(AppCommandError::from)
}

fn compute_stats(all_conversations: &[ConversationSummary]) -> AgentStats {
    let mut total_messages: u32 = 0;
    let mut counts: HashMap<AgentType, u32> = HashMap::new();

    for conversation in all_conversations {
        total_messages += conversation.message_count;
        *counts.entry(conversation.agent_type).or_insert(0) += 1;
    }

    let mut by_agent: Vec<AgentConversationCount> = counts
        .into_iter()
        .map(|(agent_type, conversation_count)| AgentConversationCount {
            agent_type,
            conversation_count,
        })
        .collect();
    by_agent.sort_by(|a, b| b.conversation_count.cmp(&a.conversation_count));

    AgentStats {
        total_conversations: all_conversations.len() as u32,
        total_messages,
        by_agent,
    }
}

fn parse_error_to_app_error(error: ParseError) -> AppCommandError {
    match error {
        ParseError::ConversationNotFound(id) => {
            AppCommandError::not_found("Conversation not found").with_detail(id)
        }
        ParseError::InvalidData(message) => {
            AppCommandError::invalid_input("Invalid conversation data").with_detail(message)
        }
        ParseError::Io(err) => AppCommandError::io(err),
        ParseError::Json(err) => {
            AppCommandError::invalid_input("Failed to parse conversation file")
                .with_detail(err.to_string())
        }
        ParseError::Db(err) => AppCommandError::database_error("Database operation failed")
            .with_detail(err.to_string()),
    }
}
