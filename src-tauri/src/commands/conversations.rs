use std::collections::{HashMap, HashSet};

use crate::app_error::AppCommandError;
use crate::db::entities::conversation;
use crate::db::service::{conversation_service, folder_service, import_service, tab_service};
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

pub async fn list_all_conversations_core(
    conn: &sea_orm::DatabaseConnection,
    folder_ids: Option<Vec<i32>>,
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    status: Option<String>,
    include_children: bool,
) -> Result<Vec<DbConversationSummary>, AppCommandError> {
    conversation_service::list_all(
        conn,
        folder_ids,
        agent_type,
        search,
        sort_by,
        status,
        include_children,
    )
    .await
    .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_all_conversations(
    db: tauri::State<'_, AppDatabase>,
    folder_ids: Option<Vec<i32>>,
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    status: Option<String>,
    include_children: Option<bool>,
) -> Result<Vec<DbConversationSummary>, AppCommandError> {
    list_all_conversations_core(
        &db.conn,
        folder_ids,
        agent_type,
        search,
        sort_by,
        status,
        include_children.unwrap_or(false),
    )
    .await
}

pub async fn list_child_conversations_core(
    conn: &sea_orm::DatabaseConnection,
    parent_conversation_id: i32,
) -> Result<Vec<DbConversationSummary>, AppCommandError> {
    conversation_service::list_children(conn, parent_conversation_id)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_child_conversations(
    db: tauri::State<'_, AppDatabase>,
    parent_conversation_id: i32,
) -> Result<Vec<DbConversationSummary>, AppCommandError> {
    list_child_conversations_core(&db.conn, parent_conversation_id).await
}

pub async fn list_opened_tabs_core(
    conn: &sea_orm::DatabaseConnection,
) -> Result<Vec<OpenedTab>, AppCommandError> {
    tab_service::list_all_tabs(conn)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_opened_tabs(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<OpenedTab>, AppCommandError> {
    list_opened_tabs_core(&db.conn).await
}

pub async fn save_opened_tabs_core(
    conn: &sea_orm::DatabaseConnection,
    items: Vec<OpenedTab>,
) -> Result<(), AppCommandError> {
    tab_service::save_all_tabs(conn, items)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn save_opened_tabs(
    db: tauri::State<'_, AppDatabase>,
    items: Vec<OpenedTab>,
) -> Result<(), AppCommandError> {
    save_opened_tabs_core(&db.conn, items).await
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
        Some("oldest") => all_conversations.sort_by_key(|a| a.started_at),
        Some("messages") => {
            all_conversations.sort_by_key(|b| std::cmp::Reverse(b.message_count));
        }
        _ => all_conversations.sort_by_key(|b| std::cmp::Reverse(b.started_at)), // default: newest first
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
    folders.sort_by_key(|b| std::cmp::Reverse(b.conversation_count));
    folders
}

pub async fn import_local_conversations_core(
    conn: &sea_orm::DatabaseConnection,
    folder_id: i32,
) -> Result<ImportResult, AppCommandError> {
    let folder = folder_service::get_folder_by_id(conn, folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| {
            AppCommandError::not_found("Folder not found")
                .with_detail(format!("folder_id={folder_id}"))
        })?;

    import_service::import_local_conversations(conn, folder_id, &folder.path)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn import_local_conversations(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<ImportResult, AppCommandError> {
    import_local_conversations_core(&db.conn, folder_id).await
}

/// Build the `meta["codeg.delegation"]` value for a delegation child loaded
/// from the DB. Mirrors the shape produced at runtime by
/// `acp::delegation::meta_writer::build_delegation_meta`, but only includes
/// the fields the DB can vouch for: `status` and `child_conversation_id`.
/// `child_connection_id` is omitted (no live connection for a historical
/// view; the frontend's parser treats it as optional).
///
/// Status mapping:
///  - `in_progress` → `running` (still streaming or about to)
///  - `pending_review` → `completed` (set by `TurnComplete { stop_reason:
///    "end_turn" }` — the success path; the live broker writes `completed`
///    for this same outcome, see `acp/delegation/broker.rs` Ok arm).
///  - `completed` → `completed`
///  - `cancelled` → `failed` with NO `error_code`. The DB's `Cancelled`
///    variant covers both user-cancel and turn-failure modes (refusal,
///    max_tokens, max_turn_requests, empty, unknown — see
///    `acp/lifecycle.rs` TurnComplete branch), and the broker writes a
///    distinct `error_code` per failure mode at runtime. Since the DB
///    persists only the bucket and not the specific code, we cannot
///    truthfully label the failure here — emit `failed` without a code
///    rather than mislabel non-cancel failures as `"canceled"`.
///  - other (defensive) → `running`
fn build_historical_delegation_meta(child: &DbConversationSummary) -> serde_json::Value {
    let status: &str = match child.status.as_str() {
        "in_progress" => "running",
        "pending_review" | "completed" => "completed",
        "cancelled" => "failed",
        _ => "running",
    };
    let mut obj = serde_json::Map::new();
    obj.insert("status".into(), serde_json::Value::String(status.into()));
    obj.insert(
        "child_conversation_id".into(),
        serde_json::Value::Number(child.id.into()),
    );
    serde_json::Value::Object(obj)
}

/// Walk every `delegate_to_agent` ToolUse block in `turns` and, when its
/// `tool_use_id` matches a child conversation in `children`, set
/// `meta["codeg.delegation"]` to the DB-derived snapshot. Skips blocks
/// whose meta is already populated so the live-broker write (when present)
/// always wins. Tool-name match is by substring to cover the
/// MCP-prefixed (`mcp__codeg-delegate__delegate_to_agent`) and bare forms
/// the host may have emitted.
fn inject_delegation_meta(turns: &mut [MessageTurn], children: &[DbConversationSummary]) {
    if children.is_empty() {
        return;
    }
    let by_parent_tool_use_id: HashMap<&str, &DbConversationSummary> = children
        .iter()
        .filter_map(|c| c.parent_tool_use_id.as_deref().map(|tu| (tu, c)))
        .collect();
    for turn in turns.iter_mut() {
        for block in turn.blocks.iter_mut() {
            if let ContentBlock::ToolUse {
                tool_use_id: Some(tu),
                tool_name,
                meta,
                ..
            } = block
            {
                if meta.is_some() {
                    continue;
                }
                if !tool_name.contains("delegate_to_agent") {
                    continue;
                }
                if let Some(child) = by_parent_tool_use_id.get(tu.as_str()) {
                    *meta = Some(serde_json::json!({
                        "codeg.delegation": build_historical_delegation_meta(child),
                    }));
                }
            }
        }
    }
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

    let (mut turns, session_stats, resolved_ext_id) = if let Some(ref ext_id) = summary.external_id
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
                    if matches!(
                        at,
                        AgentType::OpenClaw | AgentType::Cline | AgentType::Gemini
                    ) {
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
                                    let diff =
                                        (c.started_at - db_created_at).num_seconds().unsigned_abs();
                                    diff < 300
                                });
                            if let Some(conv) = matched {
                                let new_ext_id = conv.id.clone();
                                if let Ok(d) = parser.get_conversation(&new_ext_id) {
                                    return Ok((d.turns, d.session_stats, Some(new_ext_id)));
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
        let _ = conversation_service::update_external_id(conn, conversation_id, new_ext_id).await;
    }

    let mut summary = summary;
    summary.message_count = turns.len() as u32;

    // Historical recovery for the read-only sub-agent viewer: JSONL parsers
    // don't carry `meta["codeg.delegation"]`, so a reloaded conversation
    // can't drive the parent UI's child-conversation lookup. Join on
    // `parent_id = summary.id` to repopulate it from the DB. Failure to
    // fetch children silently degrades to "no button on the card" (the
    // pre-fix behavior), never to a failed detail load.
    let children = conversation_service::list_children(conn, conversation_id)
        .await
        .unwrap_or_default();
    inject_delegation_meta(&mut turns, &children);

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

pub async fn update_conversation_status_core(
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
    status: String,
) -> Result<(), AppCommandError> {
    let status_enum: conversation::ConversationStatus =
        serde_json::from_value(serde_json::Value::String(status)).map_err(|e| {
            AppCommandError::invalid_input("Invalid conversation status").with_detail(e.to_string())
        })?;
    conversation_service::update_status(conn, conversation_id, status_enum)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_conversation_status(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
    status: String,
) -> Result<(), AppCommandError> {
    update_conversation_status_core(&db.conn, conversation_id, status).await
}

pub async fn update_conversation_title_core(
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
    title: String,
) -> Result<(), AppCommandError> {
    conversation_service::update_title(conn, conversation_id, title)
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
    update_conversation_title_core(&db.conn, conversation_id, title).await
}

pub async fn delete_conversation_core(
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
) -> Result<(), AppCommandError> {
    conversation_service::soft_delete(conn, conversation_id)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn delete_conversation(
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
) -> Result<(), AppCommandError> {
    delete_conversation_core(&db.conn, conversation_id).await
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
    by_agent.sort_by_key(|b| std::cmp::Reverse(b.conversation_count));

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};

    // ──────────────────────────────────────────────────────────────────────
    // Delegation meta injection for historical reload. Parsers always emit
    // `ContentBlock::ToolUse { meta: None }`; without this helper, a
    // conversation reloaded from JSONL has no way to surface its
    // sub-agent children to the parent UI's read-only viewer.
    // ──────────────────────────────────────────────────────────────────────

    fn summary_child(id: i32, parent_tool_use_id: &str, status: &str) -> DbConversationSummary {
        let now = chrono::Utc::now();
        DbConversationSummary {
            id,
            folder_id: 1,
            title: None,
            agent_type: AgentType::Codex,
            status: status.into(),
            model: None,
            git_branch: None,
            external_id: None,
            message_count: 0,
            created_at: now,
            updated_at: now,
            parent_id: Some(1),
            parent_tool_use_id: Some(parent_tool_use_id.into()),
            delegation_call_id: Some("call-1".into()),
        }
    }

    fn tool_use_turn(tool_use_id: Option<&str>, tool_name: &str) -> MessageTurn {
        MessageTurn {
            id: "t1".into(),
            role: TurnRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                tool_use_id: tool_use_id.map(String::from),
                tool_name: tool_name.into(),
                input_preview: None,
                meta: None,
            }],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
        }
    }

    fn first_block_meta(turn: &MessageTurn) -> Option<&serde_json::Value> {
        turn.blocks.first().and_then(|b| match b {
            ContentBlock::ToolUse { meta, .. } => meta.as_ref(),
            _ => None,
        })
    }

    #[test]
    fn inject_delegation_meta_populates_completed_child() {
        let mut turns = vec![tool_use_turn(
            Some("tu-1"),
            "mcp__codeg-delegate__delegate_to_agent",
        )];
        let children = vec![summary_child(42, "tu-1", "completed")];
        inject_delegation_meta(&mut turns, &children);
        let meta = first_block_meta(&turns[0]).expect("meta should be set");
        let inner = meta.get("codeg.delegation").expect("codeg.delegation key");
        assert_eq!(inner["status"], "completed");
        assert_eq!(inner["child_conversation_id"], 42);
        assert!(
            inner.get("error_code").is_none(),
            "completed has no error_code"
        );
    }

    #[test]
    fn inject_delegation_meta_maps_in_progress_to_running() {
        let mut turns = vec![tool_use_turn(Some("tu-1"), "delegate_to_agent")];
        let children = vec![summary_child(7, "tu-1", "in_progress")];
        inject_delegation_meta(&mut turns, &children);
        let inner = first_block_meta(&turns[0])
            .unwrap()
            .get("codeg.delegation")
            .unwrap();
        assert_eq!(inner["status"], "running");
        assert_eq!(inner["child_conversation_id"], 7);
    }

    #[test]
    fn inject_delegation_meta_maps_pending_review_to_completed() {
        // `pending_review` is the DB status written after a successful
        // `TurnComplete { stop_reason: "end_turn" }` (see acp/lifecycle.rs).
        // The live broker maps that same child outcome to delegation meta
        // `status: "completed"` (see broker.rs Ok arm). Historical reload
        // must agree, otherwise a finished sub-agent shows a stale
        // "running" badge until the user reloads again.
        let mut turns = vec![tool_use_turn(Some("tu-1"), "delegate_to_agent")];
        let children = vec![summary_child(11, "tu-1", "pending_review")];
        inject_delegation_meta(&mut turns, &children);
        let inner = first_block_meta(&turns[0])
            .unwrap()
            .get("codeg.delegation")
            .unwrap();
        assert_eq!(inner["status"], "completed");
        assert_eq!(inner["child_conversation_id"], 11);
    }

    #[test]
    fn inject_delegation_meta_maps_cancelled_to_failed_without_error_code() {
        // `Cancelled` covers both user-cancel and turn-failure outcomes
        // (refusal, max_tokens, max_turn_requests, empty, unknown — see
        // acp/lifecycle.rs TurnComplete branch). The DB does not persist
        // the broker's distinct `error_code` per failure mode, so a
        // hard-coded `"canceled"` would mislabel every non-cancel failure
        // as user-cancel. Emit `failed` without a code instead.
        let mut turns = vec![tool_use_turn(Some("tu-1"), "delegate_to_agent")];
        let children = vec![summary_child(9, "tu-1", "cancelled")];
        inject_delegation_meta(&mut turns, &children);
        let inner = first_block_meta(&turns[0])
            .unwrap()
            .get("codeg.delegation")
            .unwrap();
        assert_eq!(inner["status"], "failed");
        assert!(
            inner.get("error_code").is_none(),
            "DB cannot distinguish cancel from other failures, must not claim 'canceled'"
        );
    }

    #[test]
    fn inject_delegation_meta_skips_non_delegation_tool_calls() {
        let mut turns = vec![tool_use_turn(Some("tu-1"), "bash")];
        let children = vec![summary_child(42, "tu-1", "completed")];
        inject_delegation_meta(&mut turns, &children);
        assert!(
            first_block_meta(&turns[0]).is_none(),
            "non-delegation tool_name must not get meta even on tool_use_id match"
        );
    }

    #[test]
    fn inject_delegation_meta_skips_blocks_without_tool_use_id() {
        let mut turns = vec![tool_use_turn(None, "delegate_to_agent")];
        let children = vec![summary_child(42, "tu-1", "completed")];
        inject_delegation_meta(&mut turns, &children);
        assert!(first_block_meta(&turns[0]).is_none());
    }

    #[test]
    fn inject_delegation_meta_preserves_live_broker_meta() {
        // Defensive: even though parsers always emit `meta: None`, a future
        // snapshot path could carry a live broker write. Don't clobber it.
        let pre_existing = serde_json::json!({ "codeg.delegation": { "status": "running", "child_conversation_id": 999 } });
        let mut turns = vec![MessageTurn {
            id: "t1".into(),
            role: TurnRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                tool_use_id: Some("tu-1".into()),
                tool_name: "delegate_to_agent".into(),
                input_preview: None,
                meta: Some(pre_existing.clone()),
            }],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
        }];
        let children = vec![summary_child(42, "tu-1", "completed")];
        inject_delegation_meta(&mut turns, &children);
        // The 999 (broker-written) survives — DB-derived 42 is not used here.
        let inner = first_block_meta(&turns[0])
            .unwrap()
            .get("codeg.delegation")
            .unwrap();
        assert_eq!(inner["child_conversation_id"], 999);
        assert_eq!(inner["status"], "running");
    }

    #[test]
    fn inject_delegation_meta_no_op_when_children_empty() {
        let mut turns = vec![tool_use_turn(Some("tu-1"), "delegate_to_agent")];
        inject_delegation_meta(&mut turns, &[]);
        assert!(first_block_meta(&turns[0]).is_none());
    }

    #[test]
    fn inject_delegation_meta_unmatched_tool_use_id_left_alone() {
        let mut turns = vec![tool_use_turn(Some("tu-other"), "delegate_to_agent")];
        let children = vec![summary_child(42, "tu-1", "completed")];
        inject_delegation_meta(&mut turns, &children);
        assert!(first_block_meta(&turns[0]).is_none());
    }

    #[tokio::test]
    async fn get_folder_conversation_core_injects_meta_for_real_child() {
        // Seed a parent and a delegation child; the parent has no external_id
        // (no JSONL on disk), so `turns` returns empty — but we still want to
        // exercise the children-fetch + injection short-circuit cleanly.
        // The richer end-to-end (with parser turns) is covered by the unit
        // tests above; here we just verify the wiring inside the _core fn
        // doesn't error on the join path.
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-inject-test").await;
        let parent_id = create_conversation_core(
            &db.conn,
            folder_id,
            AgentType::ClaudeCode,
            Some("parent".into()),
        )
        .await
        .expect("parent");
        // Attach a child to this parent via the delegation-link path.
        let link = crate::acp::delegation::spawner::DelegationLink {
            parent_conversation_id: parent_id,
            parent_tool_use_id: "tu-historical".into(),
            delegation_call_id: "call-historical".into(),
        };
        conversation_service::create_with_delegation(
            &db.conn,
            folder_id,
            AgentType::Codex,
            Some("child".into()),
            None,
            Some(link),
        )
        .await
        .expect("child");
        // Parent has no external_id → no JSONL → no turns to inject into.
        // The call must still succeed without error.
        let detail = get_folder_conversation_core(&db.conn, parent_id)
            .await
            .expect("load");
        assert_eq!(detail.summary.id, parent_id);
        assert!(detail.turns.is_empty());
    }

    #[tokio::test]
    async fn create_conversation_core_happy_path() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-conv-test-1").await;
        let id = create_conversation_core(
            &db.conn,
            folder_id,
            AgentType::ClaudeCode,
            Some("hello".into()),
        )
        .await
        .expect("create");
        assert!(id > 0, "expected positive conversation id, got {id}");

        let summary = conversation_service::get_by_id(&db.conn, id)
            .await
            .expect("read back");
        assert_eq!(summary.folder_id, folder_id);
        assert_eq!(summary.agent_type, AgentType::ClaudeCode);
    }

    #[tokio::test]
    async fn create_conversation_core_non_git_path_yields_no_branch() {
        let db = fresh_in_memory_db().await;
        // Use a tempdir that's guaranteed not a git repo (no .git).
        let temp = tempfile::tempdir().expect("tempdir");
        let folder_id = seed_folder(&db, &temp.path().to_string_lossy()).await;
        let id = create_conversation_core(&db.conn, folder_id, AgentType::Codex, None)
            .await
            .expect("create succeeds even without git");
        let summary = conversation_service::get_by_id(&db.conn, id)
            .await
            .expect("read back");
        assert!(
            summary.git_branch.is_none(),
            "non-git path should produce no branch, got: {:?}",
            summary.git_branch
        );
    }

    #[tokio::test]
    async fn create_conversation_core_missing_folder_still_creates() {
        // FK on folder_id is not enforced (no FK constraint in schema/PRAGMA),
        // so creating a conversation against an unknown folder_id should not
        // panic. detect_git_branch is skipped because folder lookup returns None.
        let db = fresh_in_memory_db().await;
        let result = create_conversation_core(&db.conn, 999_999, AgentType::Gemini, None).await;
        // Behavior contract: either success (current FK-loose behavior) or a
        // database error — never panic. Accept both.
        match result {
            Ok(id) => assert!(id > 0),
            Err(err) => {
                let msg = format!("{err:?}");
                assert!(
                    msg.to_lowercase().contains("foreign")
                        || msg.to_lowercase().contains("constraint")
                        || msg.to_lowercase().contains("999999"),
                    "unexpected error shape: {msg}"
                );
            }
        }
    }

    #[tokio::test]
    async fn get_folder_conversation_core_missing_id_errors() {
        let db = fresh_in_memory_db().await;
        let err = get_folder_conversation_core(&db.conn, 999_999)
            .await
            .expect_err("missing conversation must error, not panic");
        let msg = format!("{err:?}");
        assert!(
            msg.to_lowercase().contains("not found") || msg.to_lowercase().contains("999999"),
            "expected not-found-shaped error, got: {msg}"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // Phase 8 — _core wrappers around DB-only service calls. These were
    // extracted from the web handlers so HTTP and Tauri callers share one
    // implementation. Tests pin the boundary contract: empty-state shape,
    // roundtrip behavior, and how the wrappers surface error conditions.
    // ──────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_all_conversations_core_empty_db_returns_empty() {
        let db = fresh_in_memory_db().await;
        let rows = list_all_conversations_core(&db.conn, None, None, None, None, None, false)
            .await
            .expect("list");
        assert!(rows.is_empty(), "fresh db must have zero conversations");
    }

    #[tokio::test]
    async fn list_opened_tabs_core_empty_db_returns_empty() {
        let db = fresh_in_memory_db().await;
        let tabs = list_opened_tabs_core(&db.conn).await.expect("list");
        assert!(tabs.is_empty());
    }

    #[tokio::test]
    async fn save_opened_tabs_core_roundtrip() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-tabs-test").await;
        let items = vec![
            OpenedTab {
                id: 0,
                folder_id,
                conversation_id: None,
                agent_type: AgentType::ClaudeCode,
                position: 0,
                is_active: true,
                is_pinned: false,
            },
            OpenedTab {
                id: 0,
                folder_id,
                conversation_id: None,
                agent_type: AgentType::Codex,
                position: 1,
                is_active: false,
                is_pinned: false,
            },
        ];
        save_opened_tabs_core(&db.conn, items).await.expect("save");
        let tabs = list_opened_tabs_core(&db.conn).await.expect("list");
        assert_eq!(
            tabs.len(),
            2,
            "expected 2 tabs roundtrip, got {}",
            tabs.len()
        );
    }

    #[tokio::test]
    async fn import_local_conversations_core_missing_folder_errors() {
        let db = fresh_in_memory_db().await;
        let err = import_local_conversations_core(&db.conn, 999_999)
            .await
            .expect_err("missing folder must surface as error");
        let msg = format!("{err:?}");
        assert!(
            msg.to_lowercase().contains("not found") || msg.to_lowercase().contains("999999"),
            "expected not-found-shaped error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn update_conversation_status_core_invalid_string_errors() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-status-test").await;
        let conv_id = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("create");
        let err =
            update_conversation_status_core(&db.conn, conv_id, "not-a-real-status".to_string())
                .await
                .expect_err("garbage status must error before touching the DB");
        let msg = format!("{err:?}");
        assert!(
            msg.to_lowercase().contains("invalid"),
            "expected invalid-input error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn update_conversation_title_core_roundtrip() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-title-test").await;
        let conv_id = create_conversation_core(&db.conn, folder_id, AgentType::Gemini, None)
            .await
            .expect("create");
        update_conversation_title_core(&db.conn, conv_id, "Renamed".into())
            .await
            .expect("update");
        let summary = conversation_service::get_by_id(&db.conn, conv_id)
            .await
            .expect("read back");
        assert_eq!(summary.title.as_deref(), Some("Renamed"));
    }

    #[tokio::test]
    async fn delete_conversation_core_soft_deletes() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-delete-test").await;
        let conv_id = create_conversation_core(&db.conn, folder_id, AgentType::Codex, None)
            .await
            .expect("create");
        delete_conversation_core(&db.conn, conv_id)
            .await
            .expect("delete");
        // After soft delete the row should no longer show up in list_all.
        let remaining = list_all_conversations_core(&db.conn, None, None, None, None, None, false)
            .await
            .expect("list");
        assert!(
            remaining.iter().all(|c| c.id != conv_id),
            "soft-deleted conversation must not appear in list_all"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // Phase 7 — delegation list filter + child lookup wrappers.
    // ──────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_child_conversations_core_returns_empty_for_no_parent() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-list-children-empty").await;
        let parent_id = create_conversation_core(&db.conn, folder_id, AgentType::Codex, None)
            .await
            .expect("create parent");
        let rows = list_child_conversations_core(&db.conn, parent_id)
            .await
            .expect("list");
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn list_child_conversations_core_returns_only_matching_children() {
        use crate::acp::delegation::spawner::DelegationLink;
        use crate::db::service::conversation_service;

        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-list-children-match").await;
        let parent_id = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("create parent");

        // Two delegation children — both should come back, oldest-first.
        for (i, tool_use) in ["tu-A", "tu-B"].iter().enumerate() {
            let link = DelegationLink {
                parent_conversation_id: parent_id,
                parent_tool_use_id: (*tool_use).into(),
                delegation_call_id: format!("call-{i}"),
            };
            conversation_service::create_with_delegation(
                &db.conn,
                folder_id,
                AgentType::Codex,
                Some(format!("child-{i}")),
                None,
                Some(link),
            )
            .await
            .expect("create child");
        }
        // Sibling root conversation that must NOT appear.
        let _other = create_conversation_core(&db.conn, folder_id, AgentType::Gemini, None)
            .await
            .expect("unrelated root");

        let rows = list_child_conversations_core(&db.conn, parent_id)
            .await
            .expect("list");
        assert_eq!(rows.len(), 2, "expected 2 children, got {}", rows.len());
        assert!(rows.iter().all(|r| r.parent_id == Some(parent_id)));
        // Oldest-first ordering (created_at ascending).
        assert!(rows[0].created_at <= rows[1].created_at);
    }
}
