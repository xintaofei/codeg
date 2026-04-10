use std::collections::{HashMap, HashSet};

use crate::app_error::AppCommandError;
#[cfg(feature = "tauri-runtime")]
use crate::db::entities::conversation;
use crate::db::service::{conversation_service, folder_service};
#[cfg(feature = "tauri-runtime")]
use crate::db::service::import_service;
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
use crate::models::*;
use crate::parsers::claude::ClaudeParser;
use crate::parsers::cline::ClineParser;
use crate::parsers::codex::CodexParser;
use crate::parsers::gemini::GeminiParser;
use crate::parsers::openclaw::OpenClawParser;
use crate::parsers::opencode::OpenCodeParser;
use crate::parsers::{path_eq_for_matching, AgentParser, ParseError};

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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
        (AgentType::OpenClaw, Box::new(OpenClawParser::new())),
        (AgentType::Cline, Box::new(ClineParser::new())),
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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
            AgentType::OpenClaw => Box::new(OpenClawParser::new()),
            AgentType::Cline => Box::new(ClineParser::new()),
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

/// Core logic for loading a folder conversation with full OpenClaw fallback.
/// Shared by both the Tauri command and the web handler.
pub async fn get_folder_conversation_core(
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
) -> Result<DbConversationDetail, AppCommandError> {
    let summary = conversation_service::get_by_id(conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?;

    let (turns, session_stats, resolved_ext_id) = if let Some(ref ext_id) = summary.external_id
    {
        let at = summary.agent_type;
        let eid = ext_id.clone();
        let db_created_at = summary.created_at;
        let folder_path_for_fallback = {
            let folder = folder_service::get_folder_by_id(conn, summary.folder_id)
                .await
                .ok()
                .flatten();
            folder.map(|f| f.path)
        };
        tokio::task::spawn_blocking(move || -> Result<_, AppCommandError> {
            let parser: Box<dyn AgentParser> = match at {
                AgentType::ClaudeCode => Box::new(ClaudeParser::new()),
                AgentType::Codex => Box::new(CodexParser::new()),
                AgentType::OpenCode => Box::new(OpenCodeParser::new()),
                AgentType::Gemini => Box::new(GeminiParser::new()),
                AgentType::OpenClaw => Box::new(OpenClawParser::new()),
                AgentType::Cline => Box::new(ClineParser::new()),
            };
            match parser.get_conversation(&eid) {
                Ok(d) => Ok((d.turns, d.session_stats, None)),
                Err(crate::parsers::ParseError::ConversationNotFound(_)) => {
                    // The external_id may no longer match any local file —
                    // e.g. an ACP session UUID (OpenClaw, Cline) or a stale
                    // ID after session/new fallback overwrote the original
                    // (Gemini CLI).  Fall back to matching by folder_path
                    // and started_at from the parsed conversation list.
                    if matches!(at, AgentType::OpenClaw | AgentType::Cline | AgentType::Gemini) {
                        if let Ok(all) = parser.list_conversations() {
                            // Filter by folder_path first, then find the closest
                            // started_at match within 300 seconds of db_created_at.
                            let matched = all
                                .into_iter()
                                .filter(|c| {
                                    c.folder_path
                                        .as_ref()
                                        .zip(folder_path_for_fallback.as_ref())
                                        .is_some_and(|(a, b)| path_eq_for_matching(a, b))
                                })
                                .min_by_key(|c| {
                                    (c.started_at - db_created_at).num_seconds().unsigned_abs()
                                })
                                .filter(|c| {
                                    let diff = (c.started_at - db_created_at).num_seconds().unsigned_abs();
                                    diff < 300
                                });
                            if let Some(conv) = matched {
                                let new_ext_id = conv.id.clone();
                                if let Ok(d) = parser.get_conversation(&new_ext_id) {
                                    return Ok((
                                        d.turns,
                                        d.session_stats,
                                        Some(new_ext_id),
                                    ));
                                }
                            }
                        }
                    }
                    Ok((vec![], None, None))
                }
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
        (vec![], None, None)
    };

    // If we resolved a different external_id (e.g. ACP UUID → parser branch ID),
    // update the database so future lookups are direct.
    if let Some(new_ext_id) = resolved_ext_id {
        let _ =
            conversation_service::update_external_id(conn, conversation_id, new_ext_id).await;
    }

    let mut summary = summary;
    summary.message_count = turns.len() as u32;

    Ok(DbConversationDetail {
        summary,
        turns,
        session_stats,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_folder_conversation(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
) -> Result<DbConversationDetail, AppCommandError> {
    get_folder_conversation_core(&db.conn, conversation_id).await
}

/// Core logic for creating a conversation with git branch detection.
/// Shared by both the Tauri command and the web handler.
pub async fn create_conversation_core(
    conn: &sea_orm::DatabaseConnection,
    folder_id: i32,
    agent_type: AgentType,
    title: Option<String>,
) -> Result<i32, AppCommandError> {
    let git_branch = if let Some(folder) = folder_service::get_folder_by_id(conn, folder_id)
        .await
        .map_err(AppCommandError::from)?
    {
        detect_git_branch(&folder.path).await
    } else {
        None
    };

    let model = conversation_service::create(conn, folder_id, agent_type, title, git_branch)
        .await
        .map_err(AppCommandError::from)?;
    Ok(model.id)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_conversation(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    agent_type: AgentType,
    title: Option<String>,
) -> Result<i32, AppCommandError> {
    create_conversation_core(&db.conn, folder_id, agent_type, title).await
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

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_conversation_title(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
    title: String,
) -> Result<(), AppCommandError> {
    conversation_service::update_title(&db.conn, conversation_id, title)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_conversation_external_id(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
    external_id: String,
) -> Result<(), AppCommandError> {
    conversation_service::update_external_id(&db.conn, conversation_id, external_id)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
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
