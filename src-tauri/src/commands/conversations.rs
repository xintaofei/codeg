use std::collections::{HashMap, HashSet};

use crate::app_error::AppCommandError;
use crate::db::entities::conversation;
use crate::db::entities::folder::FolderKind;
use crate::db::service::{conversation_service, folder_service, import_service, tab_service};
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
use crate::models::*;
use crate::parsers::claude::ClaudeParser;
use crate::parsers::cline::ClineParser;
use crate::parsers::codebuddy::CodeBuddyParser;
use crate::parsers::codex::CodexParser;
use crate::parsers::gemini::GeminiParser;
use crate::parsers::cursor::CursorParser;
use crate::parsers::grok::GrokParser;
use crate::parsers::hermes::HermesParser;
use crate::parsers::kimi_code::KimiCodeParser;
use crate::parsers::pi::PiParser;
use crate::parsers::openclaw::OpenClawParser;
use crate::parsers::opencode::OpenCodeParser;
use crate::parsers::{
    folder_name_from_path, normalize_path_for_matching, path_eq_for_matching, AgentParser,
    ParseError,
};
use crate::web::event_bridge::{
    emit_event, ConversationChange, ConversationsBulkChanged, EventEmitter, ImportScanProgress,
    TabsChanged, CONVERSATIONS_BULK_CHANGED_EVENT, CONVERSATION_CHANGED_EVENT,
    IMPORT_SCAN_PROGRESS_EVENT, TABS_CHANGED_EVENT,
};

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
) -> Result<OpenedTabsSnapshot, AppCommandError> {
    // Single-transaction snapshot: reading tabs and version separately could
    // tear under a concurrent save (old tabs stamped with the new version).
    let (items, version) = tab_service::snapshot_tabs(conn)
        .await
        .map_err(AppCommandError::from)?;
    Ok(OpenedTabsSnapshot { items, version })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_opened_tabs(
    db: tauri::State<'_, AppDatabase>,
) -> Result<OpenedTabsSnapshot, AppCommandError> {
    list_opened_tabs_core(&db.conn).await
}

/// Persist the open-tab set with compare-and-set on the workspace tab version,
/// then broadcast the new set on `tabs://changed` (echoing `origin` so the
/// originating client ignores its own change). A stale save (version mismatch —
/// another client committed first) is rejected without writing or emitting; the
/// caller gets `accepted: false` plus the current truth to reconcile.
pub async fn save_opened_tabs_core(
    conn: &sea_orm::DatabaseConnection,
    emitter: &EventEmitter,
    items: Vec<OpenedTab>,
    expected_version: i64,
    origin: String,
) -> Result<SaveTabsOutcome, AppCommandError> {
    let outcome = tab_service::save_all_tabs_cas(conn, items, expected_version)
        .await
        .map_err(AppCommandError::from)?;

    if outcome.accepted {
        emit_tabs_changed(emitter, outcome.version, outcome.tabs.clone(), origin);
    }

    Ok(SaveTabsOutcome {
        accepted: outcome.accepted,
        version: outcome.version,
        tabs: outcome.tabs,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn save_opened_tabs(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    items: Vec<OpenedTab>,
    expected_version: i64,
    origin: String,
) -> Result<SaveTabsOutcome, AppCommandError> {
    save_opened_tabs_core(
        &db.conn,
        &EventEmitter::Tauri(app),
        items,
        expected_version,
        origin,
    )
    .await
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
        (AgentType::Hermes, Box::new(HermesParser::new())),
        (AgentType::CodeBuddy, Box::new(CodeBuddyParser::new())),
        (AgentType::KimiCode, Box::new(KimiCodeParser::new())),
        (AgentType::Pi, Box::new(PiParser::new())),
        (AgentType::Grok, Box::new(GrokParser::new())),
        (AgentType::Cursor, Box::new(CursorParser::new())),
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
                tracing::error!("Error listing {} conversations: {}", at, e);
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
            AgentType::Hermes => Box::new(HermesParser::new()),
            AgentType::CodeBuddy => Box::new(CodeBuddyParser::new()),
            AgentType::KimiCode => Box::new(KimiCodeParser::new()),
            AgentType::Pi => Box::new(PiParser::new()),
            AgentType::Grok => Box::new(GrokParser::new()),
            AgentType::Cursor => Box::new(CursorParser::new()),
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
    emitter: &EventEmitter,
    folder_id: i32,
) -> Result<ImportResult, AppCommandError> {
    // Share IMPORT_GUARD with the batch importer: `(external_id, agent_type)`
    // has no DB unique index, so this legacy path racing a batch import (or a
    // second legacy call) could double-insert. try_lock rejects the overlap
    // rather than queueing — matching `import_selected_sessions_core`. (No UI
    // still calls this command; it is kept only for API/back-compat.)
    let _guard = IMPORT_GUARD
        .try_lock()
        .map_err(|_| AppCommandError::invalid_input("An import is already in progress"))?;

    let folder = folder_service::get_folder_by_id(conn, folder_id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| {
            AppCommandError::not_found("Folder not found")
                .with_detail(format!("folder_id={folder_id}"))
        })?;

    let (result, updated_ids) =
        import_service::import_local_conversations(conn, folder_id, &folder.path)
            .await
            .map_err(AppCommandError::from)?;

    // Broadcast a sidebar upsert for every title refreshed in place, so other
    // windows and web clients converge live. The importing client refetches the
    // list itself, which also covers the newly imported rows.
    for id in updated_ids {
        emit_conversation_upsert(emitter, conn, id).await;
    }

    Ok(result)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn import_local_conversations(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
) -> Result<ImportResult, AppCommandError> {
    import_local_conversations_core(&db.conn, &EventEmitter::Tauri(app), folder_id).await
}

/// Serializes concurrent batch imports: `(external_id, agent_type)` has no DB
/// unique index (and adding one now could fail on historical duplicates), so
/// two overlapping imports could double-insert the same session. `try_lock`
/// instead of queueing — a second import racing the first is a user mistake to
/// surface, not work to serialize.
static IMPORT_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The DB's stored string for an [`AgentType`] (its snake_case serde name) —
/// the same conversion `import_one` uses for the `agent_type` column.
fn agent_type_db_str(at: &AgentType) -> String {
    serde_json::to_value(at)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// Minimal projection of a `folder` row for scan/import reconciliation (keeps
/// `build_scan_result` constructible in tests without full SeaORM models).
struct ScanFolderRow {
    id: i32,
    path: String,
    name: String,
    deleted: bool,
}

async fn load_folder_rows(
    conn: &sea_orm::DatabaseConnection,
) -> Result<Vec<ScanFolderRow>, AppCommandError> {
    use sea_orm::EntityTrait;
    let rows = crate::db::entities::folder::Entity::find()
        .all(conn)
        .await
        .map_err(crate::db::error::DbError::from)
        .map_err(AppCommandError::from)?;
    Ok(rows
        .into_iter()
        .map(|f| ScanFolderRow {
            id: f.id,
            path: f.path,
            name: f.name,
            deleted: f.deleted_at.is_some(),
        })
        .collect())
}

/// Normalized path → folder row, preferring a live row when a soft-deleted
/// variant of the same normalized path also exists (both can coexist since
/// `UNIQUE(path)` is on the raw string).
fn index_folder_rows(rows: &[ScanFolderRow]) -> HashMap<String, &ScanFolderRow> {
    let mut index: HashMap<String, &ScanFolderRow> = HashMap::new();
    for row in rows {
        let slot = index.entry(normalize_path_for_matching(&row.path)).or_insert(row);
        if slot.deleted && !row.deleted {
            *slot = row;
        }
    }
    index
}

/// Pure grouping/reconciliation for the import-picker scan.
/// `imported_index` maps `(agent_type_db_str, external_id)` → "a live row
/// exists" (false = only soft-deleted rows).
fn build_scan_result(
    summaries: Vec<(AgentType, ConversationSummary)>,
    imported_index: &HashMap<(String, String), bool>,
    folder_rows: &[ScanFolderRow],
) -> ScanResult {
    struct GroupAcc {
        path: String,
        name: String,
        exists_in_codeg: bool,
        folder_id: Option<i32>,
        agent_types: Vec<AgentType>,
        sessions: Vec<ScanSession>,
    }

    let folder_index = index_folder_rows(folder_rows);
    let mut groups: HashMap<String, GroupAcc> = HashMap::new();
    let mut no_folder_count = 0u32;

    for (at, summary) in summaries {
        let raw_path = match summary.folder_path.as_deref().map(str::trim) {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => {
                no_folder_count += 1;
                continue;
            }
        };
        let key = normalize_path_for_matching(&raw_path);
        let entry = groups.entry(key.clone()).or_insert_with(|| {
            let row = folder_index.get(&key).copied();
            GroupAcc {
                // Reuse the stored row's exact path string so the import-side
                // add_folder upsert hits the same UNIQUE(path) key instead of
                // minting a near-duplicate from a trailing-slash/case variant.
                path: row.map(|r| r.path.clone()).unwrap_or_else(|| raw_path.clone()),
                name: row
                    .map(|r| r.name.clone())
                    .or_else(|| summary.folder_name.clone())
                    .unwrap_or_else(|| folder_name_from_path(&raw_path)),
                exists_in_codeg: row.map(|r| !r.deleted).unwrap_or(false),
                folder_id: row.map(|r| r.id),
                agent_types: Vec::new(),
                sessions: Vec::new(),
            }
        });
        if !entry.agent_types.contains(&at) {
            entry.agent_types.push(at);
        }
        let status = match imported_index.get(&(agent_type_db_str(&at), summary.id.clone())) {
            None => ScanSessionStatus::New,
            Some(true) => ScanSessionStatus::Imported,
            Some(false) => ScanSessionStatus::Deleted,
        };
        entry.sessions.push(ScanSession {
            external_id: summary.id,
            agent_type: at,
            title: summary.title,
            started_at: summary.started_at,
            ended_at: summary.ended_at,
            message_count: summary.message_count,
            model: summary.model,
            git_branch: summary.git_branch,
            status,
        });
    }

    let mut folders: Vec<ScanFolder> = groups
        .into_values()
        .map(|mut g| {
            g.sessions
                .sort_by_key(|s| std::cmp::Reverse(s.started_at));
            ScanFolder {
                path: g.path,
                name: g.name,
                exists_in_codeg: g.exists_in_codeg,
                folder_id: g.folder_id,
                agent_types: g.agent_types,
                sessions: g.sessions,
            }
        })
        .collect();

    fn importable(f: &ScanFolder) -> u32 {
        f.sessions
            .iter()
            .filter(|s| s.status == ScanSessionStatus::New)
            .count() as u32
    }
    folders.sort_by(|a, b| {
        importable(b)
            .cmp(&importable(a))
            .then_with(|| a.path.cmp(&b.path))
    });

    let total_sessions = folders.iter().map(|f| f.sessions.len() as u32).sum();
    let importable_count = folders.iter().map(importable).sum();

    ScanResult {
        folders,
        no_folder_count,
        total_sessions,
        importable_count,
    }
}

/// Scan every local agent's sessions and reconcile them against the DB for the
/// import-picker window. Emits [`IMPORT_SCAN_PROGRESS_EVENT`] once per parser
/// while the walk runs.
pub async fn scan_importable_sessions_core(
    conn: &sea_orm::DatabaseConnection,
    emitter: &EventEmitter,
) -> Result<ScanResult, AppCommandError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let progress_emitter = emitter.clone();
    let summaries =
        import_service::collect_local_summaries(move |agent_type, done, total, session_count| {
            emit_event(
                &progress_emitter,
                IMPORT_SCAN_PROGRESS_EVENT,
                ImportScanProgress {
                    agent_type,
                    done,
                    total,
                    session_count,
                },
            );
        })
        .await;

    let conv_rows = conversation::Entity::find()
        .filter(conversation::Column::ExternalId.is_not_null())
        .all(conn)
        .await
        .map_err(crate::db::error::DbError::from)
        .map_err(AppCommandError::from)?;
    let mut imported_index: HashMap<(String, String), bool> = HashMap::new();
    for row in conv_rows {
        let Some(external_id) = row.external_id else {
            continue;
        };
        let live = row.deleted_at.is_none();
        let entry = imported_index
            .entry((row.agent_type, external_id))
            .or_insert(live);
        *entry = *entry || live;
    }

    let folder_rows = load_folder_rows(conn).await?;
    Ok(build_scan_result(summaries, &imported_index, &folder_rows))
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn scan_importable_sessions(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
) -> Result<ScanResult, AppCommandError> {
    scan_importable_sessions_core(&db.conn, &EventEmitter::Tauri(app)).await
}

/// Batch-import the selected sessions, creating (or reopening) each target
/// folder as needed. Test seam for [`import_selected_sessions_core`]: takes the
/// scanned summaries as input instead of walking the filesystem.
pub(crate) async fn import_selected_from_summaries(
    conn: &sea_orm::DatabaseConnection,
    emitter: &EventEmitter,
    summaries: Vec<(AgentType, ConversationSummary)>,
    selections: Vec<SelectedSessionKey>,
) -> Result<ImportSelectedResult, AppCommandError> {
    const MAX_ERRORS: usize = 10;

    // Defense-in-depth mirror of collect_local_summaries' child filter: a
    // delegation child must never import as a root row, so a selection key
    // pointing at one resolves to not_found.
    let mut by_key: HashMap<(AgentType, String), (AgentType, ConversationSummary)> = summaries
        .into_iter()
        .filter(|(_, c)| c.parent_id.is_none())
        .map(|(at, c)| ((at, c.id.clone()), (at, c)))
        .collect();

    // Group the resolved selections by normalized cwd. Duplicate keys in the
    // request resolve once (the map entry is consumed); a key that no longer
    // resolves — vanished from disk since the scan, cwd-less, or bogus — counts
    // as not_found.
    let mut not_found = 0u32;
    let mut groups: HashMap<String, Vec<(AgentType, ConversationSummary)>> = HashMap::new();
    let mut seen_keys: HashSet<(AgentType, String)> = HashSet::new();
    for key in selections {
        if !seen_keys.insert((key.agent_type, key.external_id.clone())) {
            continue;
        }
        let Some((at, summary)) = by_key.remove(&(key.agent_type, key.external_id)) else {
            not_found += 1;
            continue;
        };
        let raw_path = match summary.folder_path.as_deref().map(str::trim) {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => {
                not_found += 1;
                continue;
            }
        };
        groups
            .entry(normalize_path_for_matching(&raw_path))
            .or_default()
            .push((at, summary));
    }

    let folder_rows = load_folder_rows(conn).await?;
    let folder_index = index_folder_rows(&folder_rows);

    let mut result = ImportSelectedResult {
        imported: 0,
        updated: 0,
        skipped: 0,
        not_found,
        failed: 0,
        created_folders: 0,
        folders: Vec::new(),
        errors: Vec::new(),
    };
    let mut touched_folder_ids: Vec<i32> = Vec::new();

    // Deterministic folder order (normalized path) so results and tests are
    // stable regardless of HashMap iteration.
    let mut ordered: Vec<(String, Vec<(AgentType, ConversationSummary)>)> =
        groups.into_iter().collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));

    for (norm_key, items) in ordered {
        let row = folder_index.get(&norm_key).copied();
        // Import into the stored row's exact path when one normalize-matches
        // (see build_scan_result); otherwise the parser cwd creates the folder.
        let target_path = row.map(|r| r.path.clone()).unwrap_or_else(|| {
            items[0]
                .1
                .folder_path
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .to_string()
        });
        let created = row.map(|r| r.deleted).unwrap_or(true);

        // `add_folder` is the only fallible step here — `import_summaries` is
        // resilient (per-row failures are counted, never aborting the group), so
        // a partial failure still commits and reports its good rows and still
        // broadcasts the folder it created.
        match folder_service::add_folder(conn, &target_path)
            .await
            .map_err(AppCommandError::from)
        {
            Ok(entry) => {
                let folder_id = entry.id;
                let (tally, _updated_ids, failed_in_group) =
                    import_service::import_summaries_resilient(conn, folder_id, &items).await;
                result.imported += tally.imported;
                result.updated += tally.updated;
                result.skipped += tally.skipped;
                result.failed += failed_in_group;
                if created {
                    result.created_folders += 1;
                }
                touched_folder_ids.push(folder_id);
                result.folders.push(ImportFolderOutcome {
                    path: target_path.clone(),
                    folder_id,
                    created,
                    imported: tally.imported,
                    updated: tally.updated,
                    skipped: tally.skipped,
                });
                if failed_in_group > 0 && result.errors.len() < MAX_ERRORS {
                    result
                        .errors
                        .push(format!("{target_path}: {failed_in_group} session(s) failed"));
                }
                // Broadcast every touched folder: even a pre-existing row may
                // have flipped is_open/deleted_at in add_folder, and clients
                // need the row to place the imported conversations — so this
                // fires even when some of the group's rows failed.
                if let Ok(Some(detail)) = folder_service::get_folder_by_id(conn, folder_id).await {
                    crate::commands::folders::emit_folder_upsert(emitter, detail);
                }
            }
            // The folder itself could not be created/reopened — the whole group
            // produced nothing, so there is no folder to broadcast.
            Err(e) => {
                result.failed += items.len() as u32;
                if result.errors.len() < MAX_ERRORS {
                    result.errors.push(format!("{target_path}: {e}"));
                }
            }
        }
    }

    // One nudge instead of per-row upserts: clients answer with a single full
    // refetch, which also covers refreshed titles (see the event's doc).
    if result.imported > 0 || result.updated > 0 {
        emit_event(
            emitter,
            CONVERSATIONS_BULK_CHANGED_EVENT,
            ConversationsBulkChanged {
                imported: result.imported,
                updated: result.updated,
                folder_ids: touched_folder_ids,
            },
        );
    }

    Ok(result)
}

/// Import the selected scanned sessions. Re-walks the parsers rather than
/// trusting client-echoed summaries — the scan is moments old and the disk is
/// the source of truth. Runs under [`IMPORT_GUARD`]; if the picker window is
/// closed mid-import the future still completes and events still broadcast.
pub async fn import_selected_sessions_core(
    conn: &sea_orm::DatabaseConnection,
    emitter: &EventEmitter,
    selections: Vec<SelectedSessionKey>,
) -> Result<ImportSelectedResult, AppCommandError> {
    if selections.is_empty() {
        return Err(AppCommandError::invalid_input("No sessions selected"));
    }
    let _guard = IMPORT_GUARD
        .try_lock()
        .map_err(|_| AppCommandError::invalid_input("An import is already in progress"))?;

    let summaries = import_service::collect_local_summaries(|_, _, _, _| {}).await;
    import_selected_from_summaries(conn, emitter, summaries, selections).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn import_selected_sessions(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    selections: Vec<SelectedSessionKey>,
) -> Result<ImportSelectedResult, AppCommandError> {
    import_selected_sessions_core(&db.conn, &EventEmitter::Tauri(app), selections).await
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
/// MCP-prefixed (`mcp__codeg-mcp__delegate_to_agent`) and bare forms
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
///
/// Returns the detail plus the title parsed from the session file this call
/// just read (`None` when no file matched). The live wrapper uses that title to
/// backfill the DB row's title when the user hasn't locked it — reusing this
/// already-happening per-turn parse rather than reading the file again.
pub async fn get_folder_conversation_core(
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
) -> Result<(DbConversationDetail, Option<String>), AppCommandError> {
    let summary = conversation_service::get_by_id(conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?;

    let (mut turns, session_stats, resolved_ext_id, parsed_title, transcript_watermark) =
        if let Some(ref ext_id) = summary.external_id {
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
                AgentType::Hermes => Box::new(HermesParser::new()),
                AgentType::CodeBuddy => Box::new(CodeBuddyParser::new()),
                AgentType::KimiCode => Box::new(KimiCodeParser::new()),
                AgentType::Pi => Box::new(PiParser::new()),
                AgentType::Grok => Box::new(GrokParser::new()),
                AgentType::Cursor => Box::new(CursorParser::new()),
            };
            match parser.get_conversation(&eid) {
                Ok(d) => Ok((
                    d.turns,
                    d.session_stats,
                    None,
                    d.summary.title,
                    d.transcript_watermark,
                )),
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
                                    return Ok((
                                        d.turns,
                                        d.session_stats,
                                        Some(new_ext_id),
                                        d.summary.title,
                                        d.transcript_watermark,
                                    ));
                                }
                            }
                        }
                    }
                    Ok((vec![], None, None, None, None))
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
        (vec![], None, None, None, None)
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

    Ok((
        DbConversationDetail {
            summary,
            turns,
            session_stats,
            transcript_watermark,
            in_flight_user_turn_id: None,
        },
        parsed_title,
    ))
}

/// A normalized, comparable view of a user turn's renderable content. Used to
/// match the live in-flight prompt (`UserMessageBlock`s) against a parser-built
/// user turn (`ContentBlock`s), whose two id namespaces never line up. Mirrors
/// the frontend `userTurnContentKey`: only text and image carry identity, text
/// is compared verbatim, images by `(mime_type, data)`, and block order is
/// preserved so a rearrangement of the same pieces is not a match.
#[derive(PartialEq)]
enum UserContentSig {
    Text(String),
    Image { mime_type: String, data: String },
}

fn sig_from_user_message_blocks(
    blocks: &[crate::acp::types::UserMessageBlock],
) -> Vec<UserContentSig> {
    blocks
        .iter()
        .map(|b| match b {
            crate::acp::types::UserMessageBlock::Text { text } => {
                UserContentSig::Text(text.clone())
            }
            crate::acp::types::UserMessageBlock::Image { data, mime_type } => {
                UserContentSig::Image {
                    mime_type: mime_type.clone(),
                    data: data.clone(),
                }
            }
        })
        .collect()
}

/// `Some(sig)` only for a plain user prompt (text/image blocks). Any other block
/// type means this isn't a prompt we can match by content, so we return `None`
/// and the caller leaves the turn untouched.
fn sig_from_turn_blocks(blocks: &[ContentBlock]) -> Option<Vec<UserContentSig>> {
    let mut sig = Vec::with_capacity(blocks.len());
    for b in blocks {
        match b {
            ContentBlock::Text { text } => sig.push(UserContentSig::Text(text.clone())),
            ContentBlock::Image {
                data, mime_type, ..
            } => sig.push(UserContentSig::Image {
                mime_type: mime_type.clone(),
                data: data.clone(),
            }),
            _ => return None,
        }
    }
    Some(sig)
}

/// Stamp the persisted in-flight user turn with the broadcast `message_id`.
///
/// A cross-client viewer renders the in-flight prompt from two sources that use
/// different ids: the live broadcast/snapshot keys it by `pending.message_id`,
/// while the reloaded transcript carries the same prompt under a parser-assigned
/// `turn-N` id. Rewriting the persisted turn's id to the broadcast id lets the
/// frontend's id-dedup collapse the two into one instead of showing the prompt
/// twice.
///
/// The in-flight prompt is located tail-bounded:
///   - the trailing user turn (Claude/Codex write the assistant turn only on
///     completion, so mid-stream the transcript ends exactly at the prompt); or
///   - the user turn immediately before a *single* trailing assistant turn
///     (OpenCode and Gemini persist a partial assistant turn mid-stream, so the
///     transcript tail is `[.., user X, partial assistant Y]`).
///
/// A recency check then disambiguates: the in-flight prompt was persisted by the
/// agent CLI at/after `started_at` (the agent — a local subprocess sharing this
/// machine's clock — writes the prompt on receiving it), whereas a *prior*
/// identical prompt was persisted during an earlier turn and so predates
/// `started_at`. Without it, a repeated identical prompt whose tail is
/// `[user X, COMPLETED assistant]` (the new copy not yet persisted) would be
/// mistaken for the in-flight prompt and stamped, which — combined with the
/// frontend's keep-first user dedup — would HIDE the genuinely new prompt.
/// Neither agent exposes a per-turn "still streaming" flag in its transcript
/// (OpenCode falls back to the creation timestamp and folds completed tool
/// rows; Gemini always stamps a completion time), so this wall-clock recency is
/// the reliable signal. `started_at` is captured when the backend broadcasts the
/// `UserMessage` event — strictly before the agent request is issued — so the
/// in-flight prompt is always persisted at/after it and no backward tolerance is
/// needed; allowing one would risk mis-stamping a fast prior identical prompt
/// and hiding the new one.
///
/// The match also requires identical content, so an unrelated prompt is never
/// stamped; on no match the turns are left untouched and the viewer keeps
/// showing its synthesized copy — a recoverable transient duplicate, never a
/// hidden prompt. When `started_at` is unknown the recency check can't run, so
/// nothing is stamped (the safe, keep-visible default).
///
/// Returns the stamped turn's (new) id when a stamp is applied, so the caller can
/// surface it on the detail response as `in_flight_user_turn_id`. The frontend
/// uses that to locate the in-flight prompt and, while the live reply is in hand,
/// hide the partial assistant turn OpenCode/Gemini persist after it mid-stream
/// (which would otherwise double-render against the live reply). Returning the id
/// rather than truncating here is deliberate: removing the partial server-side
/// could hide a *completed* reply in the end-of-turn race (the agent may persist
/// the final assistant row before the backend processes `TurnComplete` and clears
/// the live state, after which an attaching client's snapshot can't recover it).
fn apply_in_flight_message_id(
    turns: &mut [MessageTurn],
    pending: &crate::acp::session_state::PendingUserMessage,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<String> {
    let n = turns.len();
    if n == 0 {
        return None;
    }
    let started_at = started_at?;
    let target_idx = match turns[n - 1].role {
        TurnRole::User => n - 1,
        TurnRole::Assistant if n >= 2 && matches!(turns[n - 2].role, TurnRole::User) => n - 2,
        _ => return None,
    };
    // Recency gate. `started_at` is recorded when the backend broadcasts the
    // `UserMessage` event, which happens *before* the agent request is issued
    // (see `connection.rs`), so the agent — a local subprocess on this machine's
    // clock — necessarily persists the in-flight prompt at a wall-clock instant
    // at or after `started_at`. A *prior* identical prompt was persisted during
    // an earlier turn and is therefore strictly older. We allow no backward
    // tolerance: any window before `started_at` could admit a fast prior
    // identical prompt (a turn can complete and be re-sent in well under a
    // second), and stamping it would HIDE the genuinely new prompt via the
    // frontend's keep-first user dedup. Erring the other way only ever yields a
    // recoverable visible duplicate, so the strict bound is the safe one.
    if turns[target_idx].timestamp < started_at {
        return None;
    }
    let want = sig_from_user_message_blocks(&pending.blocks);
    if sig_from_turn_blocks(&turns[target_idx].blocks) == Some(want) {
        // Never create a duplicate id. The broadcast id is normally disjoint from
        // parser `turn-N` ids (and `is_reserved_turn_id` in the manager rejects a
        // client id of that shape), but defend the invariant here too: if the id
        // already exists on another turn, stamping would make two turns share an
        // id and the frontend's id-keyed dedup could hide one. Leave the turn
        // under its parser id — a recoverable visible duplicate, never a hidden
        // prompt — and report nothing.
        let collides = turns
            .iter()
            .enumerate()
            .any(|(i, t)| i != target_idx && t.id == pending.message_id);
        if collides {
            return None;
        }
        turns[target_idx].id = pending.message_id.clone();
        return Some(pending.message_id.clone());
    }
    None
}

/// `get_folder_conversation_core` plus live in-flight correlation: when a turn is
/// currently running on the conversation's connection, stamp the persisted
/// in-flight user turn with the broadcast `message_id` so a cross-client viewer
/// dedups it against its synthesized copy, and report that turn's id on the detail
/// as `in_flight_user_turn_id` so the frontend can hide the partial assistant
/// reply persisted after it mid-stream. A no-op (one cheap lock pass) when no turn
/// is in flight. Shared by the Tauri command and the web handler.
pub async fn get_folder_conversation_with_live_core(
    conn: &sea_orm::DatabaseConnection,
    manager: &crate::acp::manager::ConnectionManager,
    chat_channel_manager: &crate::chat_channel::manager::ChatChannelManager,
    emitter: &EventEmitter,
    conversation_id: i32,
) -> Result<DbConversationDetail, AppCommandError> {
    let (mut detail, parsed_title) = get_folder_conversation_core(conn, conversation_id).await?;

    // Per-turn auto-title backfill. The parse `get_folder_conversation_core`
    // just did already produced the session-file title; adopt it (and broadcast
    // a sidebar upsert) whenever the user hasn't renamed this conversation by
    // hand. `refresh_auto_title` re-checks the lock and equality, so once the
    // title converges this becomes a cheap no-op on every later turn. The
    // pre-check here just avoids the extra DB round-trip in the common case.
    if !detail.summary.title_locked {
        if let Some(parsed) = parsed_title.as_deref().map(str::trim) {
            if !parsed.is_empty() && detail.summary.title.as_deref() != Some(parsed) {
                match conversation_service::refresh_auto_title(
                    conn,
                    conversation_id,
                    parsed.to_string(),
                )
                .await
                {
                    Ok(true) => {
                        detail.summary.title = Some(parsed.to_string());
                        emit_conversation_upsert(emitter, conn, conversation_id).await;
                        chat_channel_manager
                            .sync_conversation_title(conn, conversation_id, parsed)
                            .await;
                    }
                    Ok(false) => {}
                    Err(e) => tracing::error!(
                        "[conversations] auto-title refresh failed for {conversation_id}: {e}"
                    ),
                }
            }
        }
    }

    if let Some((pending, started_at)) = manager
        .pending_user_message_for_conversation(conversation_id)
        .await
    {
        detail.in_flight_user_turn_id =
            apply_in_flight_message_id(&mut detail.turns, &pending, started_at);
    }
    Ok(detail)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_folder_conversation(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    manager: tauri::State<'_, crate::acp::manager::ConnectionManager>,
    chat_channel_manager: tauri::State<'_, crate::chat_channel::manager::ChatChannelManager>,
    conversation_id: i32,
) -> Result<DbConversationDetail, AppCommandError> {
    get_folder_conversation_with_live_core(
        &db.conn,
        &manager,
        &chat_channel_manager,
        &EventEmitter::Tauri(app),
        conversation_id,
    )
    .await
}

/// Emit a `conversation://changed` Upsert for `conversation_id` so every
/// client's sidebar inserts-or-replaces the row in real time. Re-fetches the
/// fresh summary via `get_by_id`, which filters out soft-deleted rows — so an
/// upsert racing a delete is silently dropped (no row resurrection).
/// Best-effort: the DB write already succeeded; on fetch failure clients
/// reconcile on the next refresh / WS reconnect.
///
/// Lives at the wrapper layer (not inside the `_core` fns) so the many
/// internal/test callers of `create_conversation_core` don't fire sidebar
/// events, and so `_core` stays a pure DB primitive.
pub(crate) async fn emit_conversation_upsert(
    emitter: &EventEmitter,
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
) {
    match conversation_service::get_by_id(conn, conversation_id).await {
        Ok(summary) => {
            // Broadcast EVERY conversation, root or delegation child. The
            // sidebar's root list still drops children (the frontend keeps
            // `parent_id != null` out of its root array via `applyConversationUpsert`);
            // a separate subscriber routes child upserts into the expanded
            // sub-session subtree by `parent_id`. The summary carries `parent_id`
            // (serialized for children only) and a fresh `child_count`, so a
            // newly-spawned child can appear live and bump its parent's chevron.
            emit_event(
                emitter,
                CONVERSATION_CHANGED_EVENT,
                ConversationChange::Upsert {
                    summary: Box::new(summary),
                },
            )
        }
        Err(e) => tracing::warn!(
            "[conversations] upsert emit skipped (get_by_id {conversation_id} failed): {e}"
        ),
    }
}

/// Emit a `conversation://changed` Deleted for `conversation_id` so every
/// client removes the row. No re-fetch: the row is already soft-deleted.
pub(crate) fn emit_conversation_deleted(emitter: &EventEmitter, conversation_id: i32) {
    emit_event(
        emitter,
        CONVERSATION_CHANGED_EVENT,
        ConversationChange::Deleted {
            id: conversation_id,
        },
    );
}

/// Broadcast a `tabs://changed` snapshot so every client converges its open-tab
/// set. `origin` is the originating client's id (echoed so it can ignore its own
/// change) or the sentinel `"server"` for cascade-originated changes that every
/// client applies.
pub(crate) fn emit_tabs_changed(
    emitter: &EventEmitter,
    version: i64,
    tabs: Vec<OpenedTab>,
    origin: String,
) {
    emit_event(
        emitter,
        TABS_CHANGED_EVENT,
        TabsChanged {
            version,
            origin,
            tabs,
        },
    );
}

/// Invalidate any open tabs pointing at a just-deleted conversation. Conversation
/// deletion is a SOFT delete, so the FK CASCADE never removes the tab row — we do
/// it explicitly. The tab version is ALWAYS advanced as a barrier (so a
/// concurrent stale save can't re-add a tab for the deleted conversation), but we
/// only broadcast when a persisted tab actually changed — a zero-row deletion
/// needs no broadcast (an in-flight saver reconciles via its rejected CAS). Lives
/// at the wrapper layer (not in `delete_conversation_core`) so internal/test
/// callers don't fire tab events.
pub(crate) async fn cleanup_tabs_for_deleted_conversation(
    emitter: &EventEmitter,
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
) {
    match tab_service::delete_conversation_tabs_and_bump(conn, conversation_id).await {
        Ok(inv) => {
            if let Some(tabs) = inv.emit {
                emit_tabs_changed(emitter, inv.version, tabs, "server".to_string());
            }
        }
        Err(e) => tracing::error!(
            "[conversations] tab cleanup failed (delete tabs for conversation {conversation_id}): {e}"
        ),
    }
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
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    agent_type: AgentType,
    title: Option<String>,
) -> Result<i32, AppCommandError> {
    let id = create_conversation_core(&db.conn, folder_id, agent_type, title).await?;
    emit_conversation_upsert(&EventEmitter::Tauri(app), &db.conn, id).await;
    Ok(id)
}

/// Result of [`create_chat_conversation_core`]: the new conversation id plus the
/// hidden chat folder backing it, so the frontend can drop the folder straight
/// into `allFolders` (resolving cwd / active-folder) without a refetch.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateChatConversationResult {
    pub conversation_id: i32,
    pub folder_id: i32,
    pub folder: FolderDetail,
}

/// Result of [`create_chat_dir`]: the freshly created scratch directory path.
/// Handed to the frontend so a chat draft can point its ACP connection at a real
/// cwd *before* any conversation row exists.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateChatDirResult {
    pub path: String,
}

/// Create a fresh dated scratch directory for a chat-mode conversation and
/// return its absolute path. Mirrors Codex's date-grouped session dirs:
/// `<data_dir>/chat-sessions/<YYYY-MM-DD>/<uuid>/`.
///
/// This is a pure filesystem operation — it writes NO database rows — so it can
/// run eagerly the moment the user picks "no-folder mode" (giving the ACP
/// connection a cwd to spawn in) without breaching the lazy-conversation
/// invariant. The row-creating [`create_chat_conversation_core`] later reuses
/// this directory via its `existing_dir` parameter, so the connection's cwd
/// never moves across the first send.
pub fn create_chat_dir_core(data_dir: &std::path::Path) -> Result<String, AppCommandError> {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let unique = uuid::Uuid::new_v4().simple().to_string();
    let dir = data_dir.join("chat-sessions").join(date).join(unique);
    std::fs::create_dir_all(&dir).map_err(AppCommandError::io)?;
    Ok(dir.to_string_lossy().to_string())
}

/// How long a scratch dir must have sat untouched before the GC may reclaim it.
/// Spares a directory that an in-flight chat draft in another window just minted
/// (it has no conversation row yet, so it would otherwise look orphaned).
const CHAT_SCRATCH_STALE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Layout-invariant key for a chat scratch dir: its trailing `(<date>, <uuid>)`
/// path components. The GC matches live dirs by this tail rather than the full
/// path string, so a different *spelling* of the same data_dir (e.g. a symlinked
/// vs canonical `CODEG_DATA_DIR` naming the same storage) still matches — a live
/// dir must never be misclassified as an orphan and deleted. `<uuid>` is a v4
/// UUID (globally unique), so the tail is collision-free in practice. Returns
/// `None` if the path lacks a leaf or parent component.
fn chat_dir_key(path: &std::path::Path) -> Option<(String, String)> {
    let uuid = path.file_name()?.to_string_lossy().to_string();
    let date = path.parent()?.file_name()?.to_string_lossy().to_string();
    Some((date, uuid))
}

/// Reclaim orphaned chat scratch directories under
/// `<data_dir>/chat-sessions/<date>/<uuid>/`. A chat draft eagerly mints a
/// scratch dir (see [`create_chat_dir_core`]) the moment "no-folder mode" is
/// picked, *before* any DB row exists; quitting before the first send — or
/// deleting a chat conversation, which intentionally leaves the dir on disk —
/// orphans it forever. This startup sweep removes the leak.
///
/// A `<uuid>` dir is reclaimed iff it is NOT bound to a live chat folder AND it
/// is older than [`CHAT_SCRATCH_STALE`]. "Live" excludes both pre-send drafts
/// (no row) and post-delete dirs (soft-deleted row), so both are reclaimed while
/// bound chats are spared. Returns the number of `<uuid>` dirs removed. Never
/// fatal: every filesystem error is logged and skipped.
pub async fn gc_orphan_chat_dirs_core(
    conn: &sea_orm::DatabaseConnection,
    data_dir: &std::path::Path,
) -> Result<usize, AppCommandError> {
    gc_orphan_chat_dirs_core_with_threshold(conn, data_dir, CHAT_SCRATCH_STALE).await
}

/// [`gc_orphan_chat_dirs_core`] with the staleness threshold injected, for tests.
/// A zero `stale` forces every dir to count as stale (deterministic, independent
/// of clock/mtime resolution); the production entry point always passes
/// [`CHAT_SCRATCH_STALE`].
pub(crate) async fn gc_orphan_chat_dirs_core_with_threshold(
    conn: &sea_orm::DatabaseConnection,
    data_dir: &std::path::Path,
    stale: std::time::Duration,
) -> Result<usize, AppCommandError> {
    let root = data_dir.join("chat-sessions");
    if !root.is_dir() {
        return Ok(0);
    }

    // Dirs bound to a live chat conversation, keyed by their layout-invariant
    // `(<date>, <uuid>)` tail (see `chat_dir_key`) rather than the full path
    // string. This survives a data_dir spelled differently across runs (e.g. a
    // symlinked vs canonical `CODEG_DATA_DIR` pointing at the same storage),
    // which a full-string compare would miss — misclassifying the live dir as an
    // orphan and deleting it. We deliberately do NOT canonicalize (it fails on
    // missing paths and could itself alias two distinct dirs); keying by the tail
    // makes the worst case a missed deletion (a leak), never data loss.
    let live: HashSet<(String, String)> = folder_service::list_live_chat_folder_paths(conn)
        .await
        .map_err(AppCommandError::from)?
        .iter()
        .filter_map(|p| chat_dir_key(std::path::Path::new(p)))
        .collect();

    let now = std::time::SystemTime::now();
    let mut removed = 0usize;

    let date_dirs = match std::fs::read_dir(&root) {
        Ok(rd) => rd,
        Err(err) => {
            tracing::error!(
                "[conversations] chat-dir GC: read {} failed: {err}",
                root.display()
            );
            return Ok(0);
        }
    };

    for date_entry in date_dirs.filter_map(Result::ok) {
        let date_path = date_entry.path();
        if !date_path.is_dir() {
            continue;
        }
        let date_key = match date_path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => continue,
        };
        let uuid_dirs = match std::fs::read_dir(&date_path) {
            Ok(rd) => rd,
            Err(err) => {
                tracing::error!(
                    "[conversations] chat-dir GC: read {} failed: {err}",
                    date_path.display()
                );
                continue;
            }
        };
        for uuid_entry in uuid_dirs.filter_map(Result::ok) {
            let uuid_path = uuid_entry.path();
            if !uuid_path.is_dir() {
                continue;
            }
            // Match by the layout-invariant `(<date>, <uuid>)` tail, not the full
            // path — see the `live` set above.
            let uuid_key = uuid_entry.file_name().to_string_lossy().to_string();
            if live.contains(&(date_key.clone(), uuid_key)) {
                continue;
            }
            // Old enough to reclaim? Unknown age (mtime unreadable / in the
            // future) → treat as fresh and spare it (a GC should leak before it
            // deletes something possibly in use). A zero threshold short-circuits
            // to "always stale" so tests don't race the filesystem clock.
            let stale_enough = stale.is_zero()
                || uuid_path
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|m| now.duration_since(m).ok())
                    .is_some_and(|age| age >= stale);
            if !stale_enough {
                continue;
            }
            match std::fs::remove_dir_all(&uuid_path) {
                Ok(()) => removed += 1,
                Err(err) => tracing::error!(
                    "[conversations] chat-dir GC: remove {} failed: {err}",
                    uuid_path.display()
                ),
            }
        }
        // Best-effort: drop the date bucket if it is now empty (`remove_dir` only
        // succeeds on an empty dir, so this never touches a bucket with survivors).
        let _ = std::fs::remove_dir(&date_path);
    }

    Ok(removed)
}

/// Core logic for creating a folderless "chat mode" conversation. Mirrors
/// Codex's date-grouped session dirs: each chat conversation gets its own
/// scratch directory under `<data_dir>/chat-sessions/<YYYY-MM-DD>/<uuid>/` plus a
/// dedicated hidden chat folder (`folder.kind = 'chat'`) pointing at it, so the
/// NOT-NULL `folder_id` FK stays satisfied. Called lazily on first prompt send — never before — so
/// merely selecting "no-folder mode" writes nothing to the DB. Shared by the
/// Tauri command and the web handler.
///
/// `existing_dir`: when the frontend already eagerly created a scratch dir (to
/// connect ACP before sending), pass it here so this reuses it instead of
/// minting a second one — keeping the connection's cwd put across the lazy
/// create. `None` mints a fresh dir (the send-before-dir-ready fallback).
/// `create_dir_all` is idempotent, so re-ensuring an existing dir is harmless.
pub async fn create_chat_conversation_core(
    conn: &sea_orm::DatabaseConnection,
    data_dir: &std::path::Path,
    agent_type: AgentType,
    title: Option<String>,
    existing_dir: Option<&str>,
) -> Result<CreateChatConversationResult, AppCommandError> {
    let path = match existing_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).map_err(AppCommandError::io)?;
            dir.to_string()
        }
        None => create_chat_dir_core(data_dir)?,
    };

    let folder = folder_service::add_chat_folder(conn, &path)
        .await
        .map_err(AppCommandError::from)?;

    // A fresh empty scratch dir has no git repo, so skip branch detection — this
    // also keeps the composer/top-bar branch pickers hidden in chat mode. No
    // transaction spans the folder + conversation inserts (the service calls take
    // a plain connection), so if the conversation insert fails, compensate by
    // soft-deleting the just-created hidden folder — otherwise it would linger as
    // an orphan (active, conversation-less, never reached by the delete path) and
    // pollute the active-folder scope.
    let model =
        match conversation_service::create_chat(conn, folder.id, agent_type, title, None).await {
            Ok(model) => model,
            Err(create_err) => {
                if let Err(cleanup_err) = folder_service::remove_folder(conn, &folder.path).await {
                    tracing::error!(
                        "[conversations] failed to clean up orphan chat folder {} after conversation create error: {cleanup_err}",
                        folder.id
                    );
                }
                return Err(AppCommandError::from(create_err));
            }
        };

    Ok(CreateChatConversationResult {
        conversation_id: model.id,
        folder_id: folder.id,
        folder,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_chat_conversation(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    agent_type: AgentType,
    title: Option<String>,
    existing_dir: Option<String>,
) -> Result<CreateChatConversationResult, AppCommandError> {
    use tauri::Manager;
    let data_dir = app
        .path()
        .app_data_dir()
        .map(|p| crate::paths::resolve_effective_data_dir(&p))
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let result = create_chat_conversation_core(
        &db.conn,
        &data_dir,
        agent_type,
        title,
        existing_dir.as_deref(),
    )
    .await?;
    emit_conversation_upsert(&EventEmitter::Tauri(app), &db.conn, result.conversation_id).await;
    Ok(result)
}

/// Eagerly create a chat-mode scratch directory (no DB rows) and return its
/// path, so the frontend can connect ACP at a real cwd the instant the user
/// selects "no-folder mode" — before any first prompt. The hidden folder +
/// conversation are still created lazily on first send (reusing this dir).
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_chat_dir(app: tauri::AppHandle) -> Result<CreateChatDirResult, AppCommandError> {
    use tauri::Manager;
    let data_dir = app
        .path()
        .app_data_dir()
        .map(|p| crate::paths::resolve_effective_data_dir(&p))
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let path = create_chat_dir_core(&data_dir)?;
    Ok(CreateChatDirResult { path })
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
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
    status: String,
) -> Result<(), AppCommandError> {
    update_conversation_status_core(&db.conn, conversation_id, status).await?;
    emit_conversation_upsert(&EventEmitter::Tauri(app), &db.conn, conversation_id).await;
    Ok(())
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

/// Re-read the persisted conversation title and best-effort sync it to any
/// bound chat-channel threads (e.g. Telegram forum topics). Lives in
/// `commands/` so web handlers route through a `_core` helper instead of
/// calling the db service layer directly.
pub async fn sync_conversation_title_to_channels_core(
    conn: &sea_orm::DatabaseConnection,
    chat_channel_manager: &crate::chat_channel::manager::ChatChannelManager,
    conversation_id: i32,
) {
    if let Ok(conv) = conversation_service::get_by_id(conn, conversation_id).await {
        if let Some(title) = conv.title.as_deref() {
            chat_channel_manager
                .sync_conversation_title(conn, conversation_id, title)
                .await;
        }
    }
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_conversation_title(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    chat_channel_manager: tauri::State<'_, crate::chat_channel::manager::ChatChannelManager>,
    conversation_id: i32,
    title: String,
) -> Result<(), AppCommandError> {
    update_conversation_title_core(&db.conn, conversation_id, title).await?;
    emit_conversation_upsert(&EventEmitter::Tauri(app), &db.conn, conversation_id).await;
    sync_conversation_title_to_channels_core(&db.conn, &chat_channel_manager, conversation_id).await;
    Ok(())
}

pub async fn update_conversation_pinned_core(
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
    pinned: bool,
) -> Result<(), AppCommandError> {
    conversation_service::update_pin(conn, conversation_id, pinned)
        .await
        .map_err(AppCommandError::from)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_conversation_pinned(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
    pinned: bool,
) -> Result<(), AppCommandError> {
    update_conversation_pinned_core(&db.conn, conversation_id, pinned).await?;
    emit_conversation_upsert(&EventEmitter::Tauri(app), &db.conn, conversation_id).await;
    Ok(())
}

pub async fn delete_conversation_core(
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
) -> Result<(), AppCommandError> {
    conversation_service::soft_delete(conn, conversation_id)
        .await
        .map_err(AppCommandError::from)
}

/// When the deleted conversation was backed by a dedicated hidden chat folder,
/// soft-delete that folder too so it stops counting toward `list_all`'s active
/// folder scope. The per-conversation scratch dir on disk is intentionally left
/// in place (symmetric with conversation soft-delete keeping session files; a
/// future GC can prune dirs whose folder is soft-deleted). Best effort —
/// failures are logged, never propagated. `folder_id` must be captured BEFORE
/// the conversation soft-delete.
pub async fn cleanup_chat_folder_for_deleted_conversation(
    conn: &sea_orm::DatabaseConnection,
    folder_id: i32,
) {
    match folder_service::get_folder_by_id(conn, folder_id).await {
        Ok(Some(folder)) if folder.kind == FolderKind::Chat => {
            // Only retire the hidden folder once it backs no remaining
            // (non-deleted) conversations, so deleting one chat conversation can
            // never hide another that happens to share the folder. (Normally a
            // chat folder backs exactly one conversation, but this keeps the
            // delete path safe regardless.)
            match conversation_service::list_by_folder(conn, folder_id, None, None, None, None).await
            {
                Ok(remaining) if remaining.is_empty() => {
                    if let Err(e) = folder_service::remove_folder(conn, &folder.path).await {
                        tracing::error!(
                            "[conversations] chat folder cleanup failed (folder {folder_id}): {e}"
                        );
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::error!(
                    "[conversations] chat folder conversation check failed (folder {folder_id}): {e}"
                ),
            }
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!("[conversations] chat folder lookup failed (folder {folder_id}): {e}")
        }
    }
}

/// Full conversation-delete orchestration shared by the Tauri command and the web
/// handler: capture the backing folder BEFORE the soft-delete (so a hidden chat
/// folder can be retired afterward), soft-delete, broadcast the deletion, then run
/// the tab + chat-folder cleanups. The thin `delete_conversation_core` primitive
/// stays event-free for internal/test callers, so the orchestration lives here.
pub async fn delete_conversation_with_cleanup_core(
    emitter: &EventEmitter,
    conn: &sea_orm::DatabaseConnection,
    conversation_id: i32,
) -> Result<(), AppCommandError> {
    // Capture the backing folder AND parent before the soft-delete: a hidden
    // chat folder is retired afterward, and a deleted delegation child must
    // re-broadcast its parent so the parent's child_count (hence its chevron)
    // converges from the DB aggregate.
    let pre = conversation_service::get_by_id(conn, conversation_id)
        .await
        .ok();
    let folder_id = pre.as_ref().map(|c| c.folder_id);
    let parent_id = pre.as_ref().and_then(|c| c.parent_id);
    delete_conversation_core(conn, conversation_id).await?;
    emit_conversation_deleted(emitter, conversation_id);
    // A removed delegation child drops its parent's child_count (→ 0 hides the
    // chevron). Re-emit the parent from the authoritative aggregate so every
    // client converges — symmetric with the create-time parent re-emit.
    if let Some(parent_id) = parent_id {
        emit_conversation_upsert(emitter, conn, parent_id).await;
    }
    cleanup_tabs_for_deleted_conversation(emitter, conn, conversation_id).await;
    if let Some(folder_id) = folder_id {
        cleanup_chat_folder_for_deleted_conversation(conn, folder_id).await;
    }
    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn delete_conversation(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    conversation_id: i32,
) -> Result<(), AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    delete_conversation_with_cleanup_core(&emitter, &db.conn, conversation_id).await
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
            title_locked: false,
            agent_type: AgentType::Codex,
            status: status.into(),
            kind: conversation::ConversationKind::Delegate,
            model: None,
            git_branch: None,
            external_id: None,
            message_count: 0,
            child_count: 0,
            created_at: now,
            updated_at: now,
            pinned_at: None,
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

    // ──────────────────────────────────────────────────────────────────────
    // In-flight user-turn stamping (cross-client viewer dedup). See
    // `apply_in_flight_message_id`.
    // ──────────────────────────────────────────────────────────────────────

    // A fixed reference instant for the in-flight turn's start, and a helper for
    // building turn timestamps relative to it (positive = after the turn began,
    // negative = a turn that started earlier).
    fn turn_started() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-05-28T00:01:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn at(offset_secs: i64) -> chrono::DateTime<chrono::Utc> {
        turn_started() + chrono::Duration::seconds(offset_secs)
    }

    fn user_text_turn(id: &str, text: &str, ts: chrono::DateTime<chrono::Utc>) -> MessageTurn {
        MessageTurn {
            id: id.into(),
            role: TurnRole::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            timestamp: ts,
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
        }
    }

    fn assistant_text_turn(
        id: &str,
        text: &str,
        ts: chrono::DateTime<chrono::Utc>,
        completed: bool,
    ) -> MessageTurn {
        MessageTurn {
            id: id.into(),
            role: TurnRole::Assistant,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            timestamp: ts,
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: completed.then_some(ts),
        }
    }

    fn pending_text(message_id: &str, text: &str) -> crate::acp::session_state::PendingUserMessage {
        crate::acp::session_state::PendingUserMessage {
            message_id: message_id.into(),
            blocks: vec![crate::acp::types::UserMessageBlock::Text { text: text.into() }],
        }
    }

    #[test]
    fn stamps_trailing_user_turn() {
        // Claude/Codex mid-stream: the transcript ends exactly at the in-flight
        // prompt (the assistant turn is written only on completion).
        let mut turns = vec![
            user_text_turn("turn-0", "first", at(-30)),
            assistant_text_turn("turn-1", "reply", at(-29), true),
            user_text_turn("turn-2", "hello", at(1)),
        ];
        let stamped =
            apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "hello"), Some(turn_started()));
        assert_eq!(stamped.as_deref(), Some("msg-live"), "reports the stamped id");
        assert_eq!(turns[2].id, "msg-live");
        assert_eq!(turns[0].id, "turn-0", "earlier identical-position turn intact");
        assert_eq!(turns[1].id, "turn-1");
    }

    #[test]
    fn stamps_user_turn_before_partial_trailing_assistant_regardless_of_completion() {
        // OpenCode/Gemini mid-stream: a partial assistant turn is persisted, so
        // the tail is [user X, partial assistant Y]. The recency of the user turn
        // — not the assistant's completion flag — is what identifies the prompt,
        // so it stamps even when the trailing assistant carries a completion time
        // (as Gemini's partial always does). The partial reply is left in place
        // and its id reported: dropping it on the backend could hide a
        // just-completed reply in the end-of-turn race, so the frontend hides the
        // duplicate at render time (keyed off the reported id) while the live
        // stream is in hand instead.
        let mut turns = vec![
            user_text_turn("turn-0", "hello", at(1)),
            assistant_text_turn("turn-1", "partial...", at(2), true),
        ];
        let stamped =
            apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "hello"), Some(turn_started()));
        assert_eq!(stamped.as_deref(), Some("msg-live"));
        assert_eq!(turns[0].id, "msg-live");
        assert_eq!(turns.len(), 2, "the partial reply is preserved (not dropped)");
        assert_eq!(turns[1].id, "turn-1", "the partial reply is untouched");
    }

    #[test]
    fn does_not_stamp_when_content_differs() {
        let mut turns = vec![
            user_text_turn("turn-0", "hello", at(1)),
            assistant_text_turn("turn-1", "partial...", at(2), false),
        ];
        let stamped = apply_in_flight_message_id(
            &mut turns,
            &pending_text("msg-live", "something else"),
            Some(turn_started()),
        );
        assert_eq!(stamped, None, "no match → nothing reported");
        assert_eq!(turns[0].id, "turn-0", "no match → left untouched");
    }

    #[test]
    fn does_not_stamp_when_message_id_collides_with_another_turn() {
        // Defense in depth: an (untrusted) broadcast id equal to an existing
        // parser turn id must not be stamped onto the in-flight prompt — two turns
        // sharing an id could let the frontend's id-keyed dedup hide one. Here the
        // broadcast id "turn-0" already names the first turn, so the in-flight
        // prompt is left under its parser id and nothing is reported.
        let mut turns = vec![
            user_text_turn("turn-0", "earlier", at(-30)),
            assistant_text_turn("turn-1", "reply", at(-29), true),
            user_text_turn("turn-2", "hello", at(1)),
        ];
        let stamped =
            apply_in_flight_message_id(&mut turns, &pending_text("turn-0", "hello"), Some(turn_started()));
        assert_eq!(stamped, None, "colliding broadcast id → no stamp");
        assert_eq!(turns[2].id, "turn-2", "the in-flight prompt keeps its parser id");
        assert_eq!(turns[0].id, "turn-0", "the colliding turn is untouched");
    }

    #[test]
    fn does_not_reach_back_past_the_last_two_turns() {
        // The matching prompt sits buried before another full user/assistant
        // round; only the trailing user turn or the user-before-trailing-
        // assistant are eligible, so it is never stamped.
        let mut turns = vec![
            user_text_turn("turn-0", "hello", at(-30)),
            assistant_text_turn("turn-1", "a", at(-29), true),
            user_text_turn("turn-2", "ok", at(1)),
            assistant_text_turn("turn-3", "b", at(2), false),
        ];
        apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "hello"), Some(turn_started()));
        assert_eq!(turns[0].id, "turn-0");
        assert_eq!(turns[2].id, "turn-2", "non-matching tail user turn untouched");
    }

    #[test]
    fn does_not_stamp_with_two_trailing_assistant_turns() {
        // Bounded to a single trailing assistant: a deeper assistant tail means
        // we can't be sure the user prompt is the in-flight one, so bail.
        let mut turns = vec![
            user_text_turn("turn-0", "hello", at(1)),
            assistant_text_turn("turn-1", "a", at(2), false),
            assistant_text_turn("turn-2", "b", at(3), false),
        ];
        apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "hello"), Some(turn_started()));
        assert_eq!(turns[0].id, "turn-0", "left untouched");
    }

    #[test]
    fn stamps_image_user_turn_only_on_exact_match() {
        let image_turn = |id: &str, data: &str| MessageTurn {
            id: id.into(),
            role: TurnRole::User,
            blocks: vec![ContentBlock::Image {
                data: data.into(),
                mime_type: "image/png".into(),
                uri: Some("file:///shot.png".into()),
            }],
            timestamp: at(1),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
        };
        let pending_image = |message_id: &str, data: &str| {
            crate::acp::session_state::PendingUserMessage {
                message_id: message_id.into(),
                blocks: vec![crate::acp::types::UserMessageBlock::Image {
                    data: data.into(),
                    mime_type: "image/png".into(),
                }],
            }
        };

        let mut turns = vec![image_turn("turn-0", "AAAA")];
        apply_in_flight_message_id(&mut turns, &pending_image("msg-live", "AAAA"), Some(turn_started()));
        assert_eq!(turns[0].id, "msg-live", "uri difference is ignored, data matches");

        let mut turns = vec![image_turn("turn-0", "AAAA")];
        apply_in_flight_message_id(&mut turns, &pending_image("msg-live", "BBBB"), Some(turn_started()));
        assert_eq!(turns[0].id, "turn-0", "different image bytes → no stamp");
    }

    #[test]
    fn empty_turns_is_a_noop() {
        let mut turns: Vec<MessageTurn> = vec![];
        let stamped =
            apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "hello"), Some(turn_started()));
        assert_eq!(stamped, None);
        assert!(turns.is_empty());
    }

    #[test]
    fn does_not_stamp_a_prior_identical_prompt_by_recency() {
        // The repeated-identical-prompt case: a prior 'continue' is already
        // answered, and a new identical 'continue' is in flight but not yet
        // persisted. The prior prompt predates the turn start, so the recency
        // gate refuses to stamp it — otherwise the new prompt (whose optimistic
        // copy shares the broadcast id) would be hidden by the frontend's
        // keep-first user dedup. A completed trailing reply makes no difference;
        // recency, not completion, is the signal.
        let mut turns = vec![
            user_text_turn("turn-0", "continue", at(-60)),
            assistant_text_turn("turn-1", "done", at(-58), true),
        ];
        apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "continue"), Some(turn_started()));
        assert_eq!(turns[0].id, "turn-0", "older identical prompt → untouched");
    }

    #[test]
    fn does_not_stamp_when_started_at_is_unknown() {
        // Without a turn-start reference the recency gate can't run, so nothing
        // is stamped (keep-visible default).
        let mut turns = vec![user_text_turn("turn-0", "hello", at(1))];
        apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "hello"), None);
        assert_eq!(turns[0].id, "turn-0");
    }

    #[test]
    fn stamps_user_turn_persisted_at_turn_start() {
        // The in-flight prompt is persisted at/after the recorded turn start (the
        // backend broadcasts `UserMessage` before issuing the agent request), so
        // a turn exactly at the start qualifies — the boundary is inclusive.
        let mut turns = vec![user_text_turn("turn-0", "hello", at(0))];
        apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "hello"), Some(turn_started()));
        assert_eq!(turns[0].id, "msg-live", "persisted exactly at the start is in-flight");
    }

    #[test]
    fn does_not_stamp_user_turn_persisted_before_turn_start() {
        // Strict gate, no backward tolerance: a turn even one second before the
        // start belongs to an earlier turn, never the in-flight prompt.
        let mut turns = vec![user_text_turn("turn-0", "hello", at(-1))];
        apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "hello"), Some(turn_started()));
        assert_eq!(turns[0].id, "turn-0", "one second before the start is not in-flight");
    }

    #[test]
    fn does_not_stamp_fast_prior_prompt_before_completed_trailing_reply() {
        // The dangerous repeated-prompt race: a prior 'continue' completed within
        // a second, the user re-sends 'continue', and a refetch lands before the
        // new copy is persisted — so the tail is [prior user, completed assistant]
        // (the OpenCode/Gemini n-2 shape). The prior user turn predates the turn
        // start, so it is left alone; stamping it would let the frontend's
        // keep-first dedup hide the genuinely new prompt. A backward tolerance
        // would reopen exactly this hole.
        let mut turns = vec![
            user_text_turn("turn-0", "continue", at(-1)),
            assistant_text_turn("turn-1", "done", at(0), true),
        ];
        let stamped =
            apply_in_flight_message_id(&mut turns, &pending_text("msg-live", "continue"), Some(turn_started()));
        assert_eq!(stamped, None, "fast prior identical prompt → nothing reported");
        assert_eq!(turns[0].id, "turn-0", "fast prior identical prompt → untouched");
        assert_eq!(turns.len(), 2, "the prior completed reply is preserved");
    }

    #[test]
    fn inject_delegation_meta_populates_completed_child() {
        let mut turns = vec![tool_use_turn(
            Some("tu-1"),
            "mcp__codeg-mcp__delegate_to_agent",
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
        let (detail, _parsed_title) = get_folder_conversation_core(&db.conn, parent_id)
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
    async fn create_chat_conversation_core_creates_dir_folder_and_conversation() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        let result = create_chat_conversation_core(
            &db.conn,
            data_dir.path(),
            AgentType::ClaudeCode,
            Some("hello chat".into()),
            None,
        )
        .await
        .expect("create chat conversation");

        // The backing folder is a hidden, top-level chat folder.
        assert_eq!(
            result.folder.kind,
            FolderKind::Chat,
            "folder must be a chat folder"
        );
        assert_eq!(result.folder.parent_id, None);
        assert_eq!(result.folder_id, result.folder.id);
        assert!(
            result
                .folder
                .path
                .starts_with(&*data_dir.path().to_string_lossy()),
            "scratch path under data dir: {}",
            result.folder.path
        );
        // The dated scratch dir exists on disk.
        assert!(
            std::path::Path::new(&result.folder.path).is_dir(),
            "scratch dir created"
        );

        // The conversation points at the hidden folder, with no git branch.
        let summary = conversation_service::get_by_id(&db.conn, result.conversation_id)
            .await
            .expect("read back");
        assert_eq!(summary.folder_id, result.folder_id);
        assert_eq!(summary.agent_type, AgentType::ClaudeCode);
        assert!(summary.git_branch.is_none());

        // It surfaces in the default sidebar query (active-folder scope).
        let rows =
            list_all_conversations_core(&db.conn, None, None, None, None, None, false)
                .await
                .expect("list");
        assert!(rows.iter().any(|c| c.id == result.conversation_id));
    }

    #[tokio::test]
    async fn create_chat_dir_core_creates_dated_dir_without_db_rows() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let path = create_chat_dir_core(data_dir.path()).expect("create chat dir");

        assert!(std::path::Path::new(&path).is_dir(), "scratch dir exists");
        assert!(
            path.starts_with(&*data_dir.path().to_string_lossy()),
            "under data dir: {path}"
        );
        assert!(
            path.contains("chat-sessions"),
            "date-grouped under chat-sessions: {path}"
        );
        // Two calls mint distinct directories (uuid segment).
        let other = create_chat_dir_core(data_dir.path()).expect("second chat dir");
        assert_ne!(path, other, "each prepare gets its own dir");
    }

    #[tokio::test]
    async fn create_chat_conversation_core_reuses_existing_dir() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        // Eager step: mint the scratch dir first (as the frontend does on select).
        let prepared = create_chat_dir_core(data_dir.path()).expect("prepare dir");

        let result = create_chat_conversation_core(
            &db.conn,
            data_dir.path(),
            AgentType::ClaudeCode,
            None,
            Some(prepared.as_str()),
        )
        .await
        .expect("create chat conversation reusing dir");

        // The conversation's hidden folder points at the SAME pre-created dir —
        // no second directory was minted, so the ACP cwd never moved.
        assert_eq!(
            result.folder.path, prepared,
            "reuses the eagerly-created scratch dir"
        );

        // Exactly one uuid dir exists under that date bucket.
        let date_dir = std::path::Path::new(&prepared)
            .parent()
            .expect("date dir")
            .to_path_buf();
        let count = std::fs::read_dir(&date_dir)
            .expect("read date dir")
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .count();
        assert_eq!(count, 1, "no duplicate scratch dir created");
    }

    #[tokio::test]
    async fn cleanup_chat_folder_soft_deletes_hidden_folder() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        let res =
            create_chat_conversation_core(&db.conn, data_dir.path(), AgentType::Codex, None, None)
                .await
                .expect("create");

        // Before cleanup the hidden folder is active.
        assert!(folder_service::get_folder_by_id(&db.conn, res.folder_id)
            .await
            .unwrap()
            .is_some());

        delete_conversation_core(&db.conn, res.conversation_id)
            .await
            .expect("delete conversation");
        cleanup_chat_folder_for_deleted_conversation(&db.conn, res.folder_id).await;

        // After cleanup the hidden folder is soft-deleted (no longer returned),
        // so it stops counting toward the active-folder scope. The on-disk dir is
        // intentionally left in place.
        assert!(folder_service::get_folder_by_id(&db.conn, res.folder_id)
            .await
            .unwrap()
            .is_none());
        assert!(
            std::path::Path::new(&res.folder.path).is_dir(),
            "scratch dir is intentionally retained on delete"
        );
    }

    // ── Orphan chat scratch-dir GC ────────────────────────────────────────────
    // The GC walks the real `chat-sessions` tree under a tempdir; the in-memory
    // DB only supplies the live-chat-folder path set (matching the chat tests
    // above). `Duration::ZERO` forces "always stale" so removal is deterministic.

    #[tokio::test]
    async fn gc_removes_pre_send_orphan_scratch_dir() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        // Eager pre-send dir: minted, but never bound to a conversation/folder.
        let orphan = create_chat_dir_core(data_dir.path()).expect("prepare dir");
        assert!(std::path::Path::new(&orphan).is_dir());

        let removed = gc_orphan_chat_dirs_core_with_threshold(
            &db.conn,
            data_dir.path(),
            std::time::Duration::ZERO,
        )
        .await
        .expect("gc");

        assert_eq!(removed, 1, "the unbound pre-send dir is reclaimed");
        assert!(
            !std::path::Path::new(&orphan).exists(),
            "orphan scratch dir removed"
        );
        // Emptied date bucket is cleaned up too.
        let date_dir = std::path::Path::new(&orphan).parent().expect("date dir");
        assert!(!date_dir.exists(), "emptied date bucket removed");
    }

    #[tokio::test]
    async fn gc_spares_live_chat_dir() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        let res =
            create_chat_conversation_core(&db.conn, data_dir.path(), AgentType::Codex, None, None)
                .await
                .expect("create");

        let removed = gc_orphan_chat_dirs_core_with_threshold(
            &db.conn,
            data_dir.path(),
            std::time::Duration::ZERO,
        )
        .await
        .expect("gc");

        assert_eq!(removed, 0, "a dir bound to a live chat folder is spared");
        assert!(
            std::path::Path::new(&res.folder.path).is_dir(),
            "live chat dir retained"
        );
    }

    #[tokio::test]
    async fn gc_reclaims_soft_deleted_chat_dir() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        let res =
            create_chat_conversation_core(&db.conn, data_dir.path(), AgentType::Codex, None, None)
                .await
                .expect("create");
        delete_conversation_core(&db.conn, res.conversation_id)
            .await
            .expect("delete conversation");
        cleanup_chat_folder_for_deleted_conversation(&db.conn, res.folder_id).await;
        // Cleanup soft-deletes the folder row but intentionally leaves the dir.
        assert!(std::path::Path::new(&res.folder.path).is_dir());

        let removed = gc_orphan_chat_dirs_core_with_threshold(
            &db.conn,
            data_dir.path(),
            std::time::Duration::ZERO,
        )
        .await
        .expect("gc");

        assert_eq!(removed, 1, "the soft-deleted (not live) dir is reclaimed");
        assert!(
            !std::path::Path::new(&res.folder.path).exists(),
            "post-delete scratch dir removed"
        );
    }

    #[tokio::test]
    async fn gc_spares_fresh_dir_below_threshold() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        let fresh = create_chat_dir_core(data_dir.path()).expect("prepare dir");

        // A 10-minute threshold spares a dir an in-flight draft just minted.
        let removed = gc_orphan_chat_dirs_core_with_threshold(
            &db.conn,
            data_dir.path(),
            std::time::Duration::from_secs(600),
        )
        .await
        .expect("gc");

        assert_eq!(removed, 0, "a fresh dir below the staleness threshold is spared");
        assert!(
            std::path::Path::new(&fresh).is_dir(),
            "fresh dir retained (anti-race)"
        );
    }

    #[tokio::test]
    async fn gc_missing_root_is_noop() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        // No `chat-sessions` dir exists at all.
        let removed = gc_orphan_chat_dirs_core_with_threshold(
            &db.conn,
            data_dir.path(),
            std::time::Duration::ZERO,
        )
        .await
        .expect("gc");

        assert_eq!(removed, 0, "absent chat-sessions root is a no-op");
    }

    #[tokio::test]
    async fn gc_removes_orphan_but_spares_live_dir_in_same_bucket() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        // A live chat conversation — its scratch path is recorded in the DB via
        // the real create path (`add_chat_folder`), the exact string the GC
        // compares against ...
        let live =
            create_chat_conversation_core(&db.conn, data_dir.path(), AgentType::Codex, None, None)
                .await
                .expect("create live");
        // ... alongside an unbound orphan dir in the same `chat-sessions` tree
        // (same day → same date bucket).
        let orphan = create_chat_dir_core(data_dir.path()).expect("orphan dir");
        assert_ne!(live.folder.path, orphan);

        let removed = gc_orphan_chat_dirs_core_with_threshold(
            &db.conn,
            data_dir.path(),
            std::time::Duration::ZERO,
        )
        .await
        .expect("gc");

        // The predicate discriminates by exact stored path: only the orphan goes.
        assert_eq!(removed, 1, "only the orphan is reclaimed");
        assert!(
            std::path::Path::new(&live.folder.path).is_dir(),
            "the live chat dir is spared even with an orphan beside it"
        );
        assert!(
            !std::path::Path::new(&orphan).exists(),
            "the orphan is removed"
        );
    }

    // A live dir must survive even when this GC run's data_dir is a different
    // *spelling* (here a symlink) of the storage that created it — full-path
    // matching would misclassify it as an orphan and delete it (data loss). The
    // layout-invariant `(<date>, <uuid>)` keying is what prevents that.
    #[cfg(unix)]
    #[tokio::test]
    async fn gc_spares_live_dir_under_aliased_data_dir() {
        use std::os::unix::fs::symlink;
        let db = fresh_in_memory_db().await;
        let real = tempfile::tempdir().expect("tempdir");
        // DB records the live path under the REAL data_dir spelling.
        let live =
            create_chat_conversation_core(&db.conn, real.path(), AgentType::Codex, None, None)
                .await
                .expect("create live");
        // A second spelling of the same storage: a symlink pointing at it.
        let link_parent = tempfile::tempdir().expect("link parent");
        let link = link_parent.path().join("data-link");
        symlink(real.path(), &link).expect("symlink");

        // GC runs under the symlinked spelling; the live dir must still be spared.
        let removed = gc_orphan_chat_dirs_core_with_threshold(
            &db.conn,
            &link,
            std::time::Duration::ZERO,
        )
        .await
        .expect("gc");

        assert_eq!(
            removed, 0,
            "live dir spared despite an aliased data_dir spelling"
        );
        assert!(
            std::path::Path::new(&live.folder.path).is_dir(),
            "live chat dir retained under data_dir aliasing"
        );
    }

    #[tokio::test]
    async fn cleanup_chat_folder_keeps_folder_with_remaining_conversations() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        let res =
            create_chat_conversation_core(&db.conn, data_dir.path(), AgentType::Codex, None, None)
                .await
                .expect("create");
        // Simulate a second conversation that happens to share the hidden folder.
        let second =
            conversation_service::create(&db.conn, res.folder_id, AgentType::Codex, None, None)
                .await
                .expect("second conversation");

        // Deleting the first must NOT retire the folder — the second remains.
        delete_conversation_core(&db.conn, res.conversation_id)
            .await
            .expect("delete first");
        cleanup_chat_folder_for_deleted_conversation(&db.conn, res.folder_id).await;
        assert!(
            folder_service::get_folder_by_id(&db.conn, res.folder_id)
                .await
                .unwrap()
                .is_some(),
            "folder retained while a sibling conversation remains"
        );

        // Deleting the last one retires the now-empty folder.
        delete_conversation_core(&db.conn, second.id)
            .await
            .expect("delete second");
        cleanup_chat_folder_for_deleted_conversation(&db.conn, res.folder_id).await;
        assert!(
            folder_service::get_folder_by_id(&db.conn, res.folder_id)
                .await
                .unwrap()
                .is_none(),
            "folder retired once empty"
        );
    }

    #[tokio::test]
    async fn chat_folders_excluded_from_user_facing_lists_but_in_all_details() {
        let db = fresh_in_memory_db().await;
        let data_dir = tempfile::tempdir().expect("tempdir");
        let normal_id = seed_folder(&db, "/tmp/codeg-chat-list-test").await;
        let chat_id =
            create_chat_conversation_core(&db.conn, data_dir.path(), AgentType::Codex, None, None)
                .await
                .expect("chat")
                .folder_id;

        // Folder history excludes the hidden chat folder, keeps the normal one.
        let history = folder_service::list_folders(&db.conn).await.unwrap();
        assert!(history.iter().any(|f| f.id == normal_id));
        assert!(!history.iter().any(|f| f.id == chat_id));

        // Open-folder surfaces exclude it too.
        let open_details = folder_service::list_open_folder_details(&db.conn)
            .await
            .unwrap();
        assert!(!open_details.iter().any(|f| f.id == chat_id));
        let open_entries = folder_service::list_open_folders(&db.conn).await.unwrap();
        assert!(!open_entries.iter().any(|f| f.id == chat_id));

        // But the full set keeps it (internal cwd / active-folder resolution).
        let all = folder_service::list_all_folder_details(&db.conn)
            .await
            .unwrap();
        assert!(all
            .iter()
            .any(|f| f.id == chat_id && f.kind == FolderKind::Chat));
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
        let snap = list_opened_tabs_core(&db.conn).await.expect("list");
        assert!(snap.items.is_empty());
        assert_eq!(snap.version, 0, "fresh db starts at version 0");
    }

    fn conv_tab(folder_id: i32, conversation_id: i32, agent_type: AgentType) -> OpenedTab {
        OpenedTab {
            id: 0,
            folder_id,
            conversation_id: Some(conversation_id),
            agent_type,
            position: 0,
            is_active: false,
            is_pinned: true,
        }
    }

    #[tokio::test]
    async fn save_opened_tabs_core_persists_only_conversation_tabs_and_bumps_version() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-tabs-test").await;
        let c1 = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("c1");
        let c2 = create_conversation_core(&db.conn, folder_id, AgentType::Codex, None)
            .await
            .expect("c2");
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();

        let items = vec![
            conv_tab(folder_id, c1, AgentType::ClaudeCode),
            conv_tab(folder_id, c2, AgentType::Codex),
            // A draft (conversation_id == None) — must NOT persist.
            OpenedTab {
                id: 0,
                folder_id,
                conversation_id: None,
                agent_type: AgentType::Gemini,
                position: 2,
                is_active: true,
                is_pinned: true,
            },
        ];
        let outcome = save_opened_tabs_core(&db.conn, &emitter, items, 0, "win-a".into())
            .await
            .expect("save");
        assert!(outcome.accepted);
        assert_eq!(outcome.version, 1);
        assert_eq!(outcome.tabs.len(), 2, "draft tab must be stripped");

        let evt = rx.try_recv().expect("accepted save should broadcast");
        assert_eq!(evt.channel, TABS_CHANGED_EVENT);
        assert_eq!(evt.payload["version"], 1);
        assert_eq!(evt.payload["origin"], "win-a");
        assert_eq!(evt.payload["tabs"].as_array().unwrap().len(), 2);

        let snap = list_opened_tabs_core(&db.conn).await.expect("list");
        assert_eq!(snap.items.len(), 2);
        assert_eq!(snap.version, 1);
    }

    #[tokio::test]
    async fn save_opened_tabs_core_rejects_stale_version_without_emitting() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-tabs-stale").await;
        let c1 = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("c1");

        // First save at v0 → v1.
        let first = save_opened_tabs_core(
            &db.conn,
            &EventEmitter::Noop,
            vec![conv_tab(folder_id, c1, AgentType::ClaudeCode)],
            0,
            "a".into(),
        )
        .await
        .expect("first save");
        assert!(first.accepted);
        assert_eq!(first.version, 1);

        // Second save built from the now-stale v0 must be rejected, no emit.
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        let stale = save_opened_tabs_core(
            &db.conn,
            &emitter,
            vec![], // would have cleared all tabs — must NOT take effect
            0,
            "b".into(),
        )
        .await
        .expect("stale save returns Ok with accepted=false");
        assert!(!stale.accepted);
        assert_eq!(stale.version, 1, "rejected save reports current version");
        assert!(
            rx.try_recv().is_err(),
            "a stale (rejected) save must not broadcast"
        );

        // The original tab survived — the stale empty save did not clobber it.
        let snap = list_opened_tabs_core(&db.conn).await.expect("list");
        assert_eq!(snap.items.len(), 1);
        assert_eq!(snap.version, 1);
    }

    #[tokio::test]
    async fn cleanup_tabs_for_deleted_conversation_removes_tab_and_emits() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-tab-conv-del").await;
        let c1 = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("c1");
        save_opened_tabs_core(
            &db.conn,
            &EventEmitter::Noop,
            vec![conv_tab(folder_id, c1, AgentType::ClaudeCode)],
            0,
            "a".into(),
        )
        .await
        .expect("save");

        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        delete_conversation_core(&db.conn, c1).await.expect("delete");
        cleanup_tabs_for_deleted_conversation(&emitter, &db.conn, c1).await;

        let snap = list_opened_tabs_core(&db.conn).await.expect("list");
        assert!(
            snap.items.is_empty(),
            "tab for a soft-deleted conversation must be removed (no ghost tab)"
        );
        let evt = rx.try_recv().expect("cleanup should broadcast");
        assert_eq!(evt.channel, TABS_CHANGED_EVENT);
        assert_eq!(evt.payload["origin"], "server");
        assert_eq!(evt.payload["tabs"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn cleanup_tabs_for_deleted_conversation_bumps_barrier_without_emitting_when_no_open_tab() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-tab-conv-del-noop").await;
        let c1 = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("c1");
        let before = list_opened_tabs_core(&db.conn).await.expect("list").version;
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        cleanup_tabs_for_deleted_conversation(&emitter, &db.conn, c1).await;
        assert!(
            rx.try_recv().is_err(),
            "no persisted tab → no broadcast (in-flight savers reconcile via rejected CAS)"
        );
        let after = list_opened_tabs_core(&db.conn).await.expect("list").version;
        assert_eq!(
            after,
            before + 1,
            "deletion still advances the version as a barrier against stale saves"
        );
    }

    #[tokio::test]
    async fn remove_folder_from_workspace_cleans_tabs_and_emits() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-folder-remove-tabs").await;
        let c1 = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("c1");
        save_opened_tabs_core(
            &db.conn,
            &EventEmitter::Noop,
            vec![conv_tab(folder_id, c1, AgentType::ClaudeCode)],
            0,
            "a".into(),
        )
        .await
        .expect("save");

        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        crate::commands::folders::remove_folder_from_workspace_core(&emitter, &db, folder_id)
            .await
            .expect("remove folder");

        let snap = list_opened_tabs_core(&db.conn).await.expect("list");
        assert!(snap.items.is_empty(), "folder removal must drop its tabs");
        let evt = rx
            .try_recv()
            .expect("folder removal should broadcast a tab change");
        assert_eq!(evt.channel, TABS_CHANGED_EVENT);
        assert_eq!(evt.payload["origin"], "server");
    }

    #[tokio::test]
    async fn stale_save_after_conversation_cleanup_is_rejected_no_resurrection() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-tab-cleanup-race").await;
        let c1 = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("c1");
        let c2 = create_conversation_core(&db.conn, folder_id, AgentType::Codex, None)
            .await
            .expect("c2");

        // Both tabs open at v0 → v1.
        let saved = save_opened_tabs_core(
            &db.conn,
            &EventEmitter::Noop,
            vec![
                conv_tab(folder_id, c1, AgentType::ClaudeCode),
                conv_tab(folder_id, c2, AgentType::Codex),
            ],
            0,
            "a".into(),
        )
        .await
        .expect("save");
        assert_eq!(saved.version, 1);

        // Server deletes c1 and atomically cleans its tab → v2 (only c2 remains).
        delete_conversation_core(&db.conn, c1).await.expect("delete c1");
        cleanup_tabs_for_deleted_conversation(&EventEmitter::Noop, &db.conn, c1).await;

        // A client still on the pre-cleanup version re-saves the OLD set (with c1
        // present). The version bump must reject it — and c1 must NOT resurrect.
        let stale = save_opened_tabs_core(
            &db.conn,
            &EventEmitter::Noop,
            vec![
                conv_tab(folder_id, c1, AgentType::ClaudeCode),
                conv_tab(folder_id, c2, AgentType::Codex),
            ],
            1,
            "b".into(),
        )
        .await
        .expect("stale save returns Ok");
        assert!(
            !stale.accepted,
            "a save built on the pre-cleanup version must be rejected"
        );
        assert_eq!(stale.version, 2);

        let snap = list_opened_tabs_core(&db.conn).await.expect("list");
        assert_eq!(snap.items.len(), 1, "c1 must not be resurrected");
        assert_eq!(snap.items[0].conversation_id, Some(c2));
        assert_eq!(snap.version, 2);
    }

    #[tokio::test]
    async fn stale_save_after_folder_removal_is_rejected_no_resurrection() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-folder-remove-race").await;
        let c1 = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("c1");
        let saved = save_opened_tabs_core(
            &db.conn,
            &EventEmitter::Noop,
            vec![conv_tab(folder_id, c1, AgentType::ClaudeCode)],
            0,
            "a".into(),
        )
        .await
        .expect("save");
        assert_eq!(saved.version, 1);

        // Removing the folder atomically drops its tabs + bumps to v2.
        crate::commands::folders::remove_folder_from_workspace_core(
            &EventEmitter::Noop,
            &db,
            folder_id,
        )
        .await
        .expect("remove folder");

        // A stale re-add of the folder's tab (still on v1) must be rejected.
        let stale = save_opened_tabs_core(
            &db.conn,
            &EventEmitter::Noop,
            vec![conv_tab(folder_id, c1, AgentType::ClaudeCode)],
            1,
            "b".into(),
        )
        .await
        .expect("stale save returns Ok");
        assert!(!stale.accepted, "save on the pre-removal version must be rejected");

        let snap = list_opened_tabs_core(&db.conn).await.expect("list");
        assert!(
            snap.items.is_empty(),
            "folder removal's version bump must block the stale re-add"
        );
    }

    #[tokio::test]
    async fn stale_save_referencing_deleted_conversation_is_rejected_no_ghost() {
        // The zero-row cleanup race: client A opened c1 but its save is still
        // debouncing (no persisted c1 tab yet). c1 is deleted — cleanup removes
        // zero rows but still advances the version barrier. A's in-flight save
        // (built on the pre-deletion version, still listing c1) is then rejected,
        // so a tab for the soft-deleted conversation is never persisted.
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-tab-zero-row-race").await;
        let c1 = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("c1");
        let c2 = create_conversation_core(&db.conn, folder_id, AgentType::Codex, None)
            .await
            .expect("c2");

        // Only c2 is persisted as a tab (v0 → v1); c1 is open on A but unsaved.
        let saved = save_opened_tabs_core(
            &db.conn,
            &EventEmitter::Noop,
            vec![conv_tab(folder_id, c2, AgentType::Codex)],
            0,
            "init".into(),
        )
        .await
        .expect("save");
        assert_eq!(saved.version, 1);

        // c1 deleted with no persisted c1 tab → zero rows removed, but the
        // version barrier still advances (v1 → v2) and nothing is broadcast.
        delete_conversation_core(&db.conn, c1).await.expect("delete c1");
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        cleanup_tabs_for_deleted_conversation(&emitter, &db.conn, c1).await;
        assert!(rx.try_recv().is_err(), "zero-row cleanup must not broadcast");

        // A's debounced save (built on v1, still including the now-deleted c1) is
        // rejected by the barrier — c1 must not be persisted as a ghost.
        let stale = save_opened_tabs_core(
            &db.conn,
            &EventEmitter::Noop,
            vec![
                conv_tab(folder_id, c1, AgentType::ClaudeCode),
                conv_tab(folder_id, c2, AgentType::Codex),
            ],
            1,
            "a".into(),
        )
        .await
        .expect("stale save returns Ok");
        assert!(
            !stale.accepted,
            "a save built before the deletion barrier must be rejected"
        );
        assert_eq!(stale.version, 2);

        let snap = list_opened_tabs_core(&db.conn).await.expect("list");
        assert_eq!(snap.items.len(), 1, "no ghost tab for the deleted c1");
        assert_eq!(snap.items[0].conversation_id, Some(c2));
    }

    #[tokio::test]
    async fn import_local_conversations_core_missing_folder_errors() {
        let db = fresh_in_memory_db().await;
        let err = import_local_conversations_core(&db.conn, &EventEmitter::Noop, 999_999)
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

        // Two delegation children — both should come back, newest-first.
        let mut child_ids = Vec::new();
        for (i, tool_use) in ["tu-A", "tu-B"].iter().enumerate() {
            let link = DelegationLink {
                parent_conversation_id: parent_id,
                parent_tool_use_id: (*tool_use).into(),
                delegation_call_id: format!("call-{i}"),
            };
            let child = conversation_service::create_with_delegation(
                &db.conn,
                folder_id,
                AgentType::Codex,
                Some(format!("child-{i}")),
                None,
                Some(link),
            )
            .await
            .expect("create child");
            child_ids.push(child.id);
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
        // Newest-first (created_at DESC): the later-created child leads, matching
        // the sidebar's newest-on-top sub-session ordering.
        let ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![child_ids[1], child_ids[0]],
            "children must be newest-first"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // Phase 1 — cross-client list/status sync. The wrapper-layer emit helpers
    // broadcast `conversation://changed` so every client's sidebar stays in
    // sync regardless of which transport made the change. Drive the helpers
    // directly against a test broadcaster and assert the emitted JSON.
    // ──────────────────────────────────────────────────────────────────────

    fn sync_test_emitter() -> (
        std::sync::Arc<crate::web::event_bridge::WebEventBroadcaster>,
        EventEmitter,
    ) {
        let broadcaster = std::sync::Arc::new(crate::web::event_bridge::WebEventBroadcaster::new());
        let emitter = EventEmitter::test_web_only(broadcaster.clone());
        (broadcaster, emitter)
    }

    #[tokio::test]
    async fn emit_conversation_upsert_broadcasts_full_root_summary() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-sync-upsert").await;
        let id = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("create");
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        emit_conversation_upsert(&emitter, &db.conn, id).await;
        let evt = rx.try_recv().expect("upsert should broadcast");
        let p = &*evt.payload;
        assert_eq!(evt.channel, CONVERSATION_CHANGED_EVENT);
        assert_eq!(p["kind"], "upsert");
        assert_eq!(p["summary"]["id"], id);
        // Root conversation → parent_id omitted (serde skip_serializing_if), so
        // the frontend keeps it in the sidebar.
        assert!(
            p["summary"].get("parent_id").is_none(),
            "root summary must omit parent_id"
        );
    }

    #[tokio::test]
    async fn emit_conversation_deleted_broadcasts_id_only() {
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        emit_conversation_deleted(&emitter, 4242);
        let evt = rx.try_recv().expect("deleted should broadcast");
        let p = &*evt.payload;
        assert_eq!(evt.channel, CONVERSATION_CHANGED_EVENT);
        assert_eq!(p["kind"], "deleted");
        assert_eq!(p["id"], 4242);
    }

    #[tokio::test]
    async fn emit_conversation_upsert_carries_new_status_after_update() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-sync-status").await;
        let id = create_conversation_core(&db.conn, folder_id, AgentType::Codex, None)
            .await
            .expect("create");
        update_conversation_status_core(&db.conn, id, "pending_review".to_string())
            .await
            .expect("status update");
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        emit_conversation_upsert(&emitter, &db.conn, id).await;
        let evt = rx.try_recv().expect("upsert should broadcast");
        assert_eq!(evt.payload["summary"]["status"], "pending_review");
    }

    #[tokio::test]
    async fn emit_conversation_upsert_on_soft_deleted_row_is_silent() {
        // Anti-resurrection: get_by_id filters deleted_at, so an upsert that
        // races a delete emits nothing instead of re-inserting a tombstone.
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-sync-deleted-silent").await;
        let id = create_conversation_core(&db.conn, folder_id, AgentType::Gemini, None)
            .await
            .expect("create");
        delete_conversation_core(&db.conn, id)
            .await
            .expect("delete");
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        emit_conversation_upsert(&emitter, &db.conn, id).await;
        assert!(
            rx.try_recv().is_err(),
            "upsert for a soft-deleted row must not broadcast (no resurrection)"
        );
    }

    #[tokio::test]
    async fn emit_conversation_upsert_broadcasts_delegation_child_with_parent() {
        // Delegation children now broadcast too: a dedicated frontend subscriber
        // routes them into their parent's expanded sub-session subtree by
        // `parent_id`. The payload must therefore carry `parent_id` (the routing
        // key) and a fresh `child_count` (so a grandchild bumps the nested
        // chevron), unlike a root whose `parent_id` is omitted.
        use crate::acp::delegation::spawner::DelegationLink;
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-sync-child-broadcast").await;
        let parent_id = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("parent");
        let child = conversation_service::create_with_delegation(
            &db.conn,
            folder_id,
            AgentType::Codex,
            Some("child".into()),
            None,
            Some(DelegationLink {
                parent_conversation_id: parent_id,
                parent_tool_use_id: "tu-1".into(),
                delegation_call_id: "call-1".into(),
            }),
        )
        .await
        .expect("child");
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        emit_conversation_upsert(&emitter, &db.conn, child.id).await;
        let evt = rx
            .try_recv()
            .expect("delegation child should broadcast an upsert");
        let p = &*evt.payload;
        assert_eq!(p["kind"], "upsert");
        assert_eq!(p["summary"]["id"], child.id);
        assert_eq!(
            p["summary"]["parent_id"], parent_id,
            "child summary must carry parent_id so the frontend can route it"
        );
        assert_eq!(
            p["summary"]["child_count"], 0,
            "leaf child carries child_count 0"
        );
    }

    #[tokio::test]
    async fn delete_child_re_emits_parent_for_child_count_convergence() {
        // Deleting a delegation child must re-broadcast its parent so every
        // client's child_count (and chevron) converges from the DB aggregate —
        // symmetric with the create-time parent re-emit.
        use crate::acp::delegation::spawner::DelegationLink;
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/codeg-delete-child-reemit").await;
        let parent_id = create_conversation_core(&db.conn, folder_id, AgentType::ClaudeCode, None)
            .await
            .expect("parent");
        let child = conversation_service::create_with_delegation(
            &db.conn,
            folder_id,
            AgentType::Codex,
            Some("child".into()),
            None,
            Some(DelegationLink {
                parent_conversation_id: parent_id,
                parent_tool_use_id: "tu-1".into(),
                delegation_call_id: "call-1".into(),
            }),
        )
        .await
        .expect("child");
        let (broadcaster, emitter) = sync_test_emitter();
        let mut rx = broadcaster.subscribe();
        delete_conversation_with_cleanup_core(&emitter, &db.conn, child.id)
            .await
            .expect("delete child");
        let mut saw_deleted = false;
        let mut saw_parent_upsert = false;
        while let Ok(evt) = rx.try_recv() {
            if evt.channel != CONVERSATION_CHANGED_EVENT {
                continue;
            }
            let p = &*evt.payload;
            if p["kind"] == "deleted" && p["id"] == child.id {
                saw_deleted = true;
            }
            if p["kind"] == "upsert" && p["summary"]["id"] == parent_id {
                saw_parent_upsert = true;
                assert_eq!(
                    p["summary"]["child_count"], 0,
                    "parent count drops to 0 once its only child is gone"
                );
            }
        }
        assert!(saw_deleted, "child deletion must broadcast a Deleted");
        assert!(
            saw_parent_upsert,
            "parent must re-broadcast an Upsert for child_count convergence"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // Import-picker scan reconciliation (`build_scan_result`) and batch
    // import (`import_selected_from_summaries`).
    // ──────────────────────────────────────────────────────────────────────

    fn scan_summary(
        id: &str,
        agent: AgentType,
        cwd: Option<&str>,
        ts: chrono::DateTime<chrono::Utc>,
    ) -> (AgentType, ConversationSummary) {
        (
            agent,
            ConversationSummary {
                id: id.into(),
                agent_type: agent,
                folder_path: cwd.map(String::from),
                folder_name: cwd.map(folder_name_from_path),
                title: Some(format!("title-{id}")),
                started_at: ts,
                ended_at: None,
                message_count: 1,
                model: None,
                git_branch: None,
                parent_id: None,
                parent_tool_use_id: None,
                delegation_call_id: None,
            },
        )
    }

    fn key_of(agent: AgentType, id: &str) -> SelectedSessionKey {
        SelectedSessionKey {
            agent_type: agent,
            external_id: id.into(),
        }
    }

    #[test]
    fn scan_groups_normalized_path_variants_into_one_folder() {
        // A trailing-slash cwd variant must land in the same group as the bare
        // path — otherwise the picker shows one folder twice and an import
        // could mint a near-duplicate folder row.
        let summaries = vec![
            scan_summary("s1", AgentType::ClaudeCode, Some("/tmp/proj"), at(0)),
            scan_summary("s2", AgentType::Codex, Some("/tmp/proj/"), at(10)),
        ];
        let result = build_scan_result(summaries, &HashMap::new(), &[]);

        assert_eq!(result.folders.len(), 1);
        let folder = &result.folders[0];
        assert_eq!(folder.path, "/tmp/proj");
        assert!(!folder.exists_in_codeg);
        assert_eq!(folder.folder_id, None);
        assert_eq!(
            folder.agent_types,
            vec![AgentType::ClaudeCode, AgentType::Codex]
        );
        // Sessions sort newest-first inside the group.
        assert_eq!(folder.sessions[0].external_id, "s2");
        assert_eq!(result.total_sessions, 2);
        assert_eq!(result.importable_count, 2);
    }

    #[test]
    fn scan_marks_status_new_imported_deleted() {
        let summaries = vec![
            scan_summary("new", AgentType::ClaudeCode, Some("/tmp/p"), at(0)),
            scan_summary("live", AgentType::ClaudeCode, Some("/tmp/p"), at(1)),
            scan_summary("gone", AgentType::ClaudeCode, Some("/tmp/p"), at(2)),
        ];
        let mut imported_index = HashMap::new();
        imported_index.insert(("claude_code".to_string(), "live".to_string()), true);
        imported_index.insert(("claude_code".to_string(), "gone".to_string()), false);

        let result = build_scan_result(summaries, &imported_index, &[]);
        let by_id: HashMap<&str, ScanSessionStatus> = result.folders[0]
            .sessions
            .iter()
            .map(|s| (s.external_id.as_str(), s.status))
            .collect();

        assert_eq!(by_id["new"], ScanSessionStatus::New);
        assert_eq!(by_id["live"], ScanSessionStatus::Imported);
        assert_eq!(by_id["gone"], ScanSessionStatus::Deleted);
        assert_eq!(result.total_sessions, 3);
        assert_eq!(result.importable_count, 1, "only New counts as importable");
    }

    #[test]
    fn scan_counts_sessions_without_folder_path_instead_of_listing_them() {
        let summaries = vec![
            scan_summary("has", AgentType::Codex, Some("/tmp/p"), at(0)),
            scan_summary("none", AgentType::Codex, None, at(1)),
            scan_summary("blank", AgentType::Codex, Some("   "), at(2)),
        ];
        let result = build_scan_result(summaries, &HashMap::new(), &[]);

        assert_eq!(result.folders.len(), 1);
        assert_eq!(result.no_folder_count, 2);
        assert_eq!(result.total_sessions, 1);
    }

    #[test]
    fn scan_prefers_stored_row_path_and_live_row_over_deleted_variant() {
        // The DB stores raw strings (UNIQUE on the exact bytes), so a live and
        // a soft-deleted variant of the same normalized path can coexist. The
        // scan must surface the LIVE row's exact path, or a later add_folder
        // would resurrect the deleted variant instead.
        let rows = vec![
            ScanFolderRow {
                id: 1,
                path: "/tmp/proj/".into(),
                name: "proj-deleted".into(),
                deleted: true,
            },
            ScanFolderRow {
                id: 2,
                path: "/tmp/proj".into(),
                name: "proj".into(),
                deleted: false,
            },
        ];
        let summaries = vec![scan_summary(
            "s1",
            AgentType::ClaudeCode,
            Some("/tmp/proj///"),
            at(0),
        )];
        let result = build_scan_result(summaries, &HashMap::new(), &rows);

        let folder = &result.folders[0];
        assert_eq!(folder.path, "/tmp/proj", "live row's stored path wins");
        assert_eq!(folder.name, "proj");
        assert!(folder.exists_in_codeg);
        assert_eq!(folder.folder_id, Some(2));
    }

    #[test]
    fn scan_soft_deleted_folder_reports_not_exists_but_keeps_id() {
        let rows = vec![ScanFolderRow {
            id: 9,
            path: "/tmp/gone".into(),
            name: "gone".into(),
            deleted: true,
        }];
        let summaries = vec![scan_summary(
            "s1",
            AgentType::Codex,
            Some("/tmp/gone"),
            at(0),
        )];
        let result = build_scan_result(summaries, &HashMap::new(), &rows);

        let folder = &result.folders[0];
        assert!(
            !folder.exists_in_codeg,
            "a soft-deleted row is not a live folder — import will reopen it"
        );
        assert_eq!(folder.folder_id, Some(9));
    }

    #[test]
    fn scan_sorts_folders_by_importable_count_then_path() {
        let mut imported_index = HashMap::new();
        imported_index.insert(("codex".to_string(), "b1".to_string()), true);
        let summaries = vec![
            scan_summary("b1", AgentType::Codex, Some("/tmp/b"), at(0)),
            scan_summary("a1", AgentType::Codex, Some("/tmp/a"), at(1)),
            scan_summary("a2", AgentType::Codex, Some("/tmp/a"), at(2)),
        ];
        let result = build_scan_result(summaries, &imported_index, &[]);

        let paths: Vec<&str> = result.folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["/tmp/a", "/tmp/b"], "2 importable before 0");
    }

    #[tokio::test]
    async fn batch_import_creates_missing_folder_and_imports() {
        use sea_orm::EntityTrait;
        let db = fresh_in_memory_db().await;

        let summaries = vec![
            scan_summary("s1", AgentType::ClaudeCode, Some("/tmp/proj-a"), at(0)),
            scan_summary("s2", AgentType::Codex, Some("/tmp/proj-a"), at(1)),
        ];
        let result = import_selected_from_summaries(
            &db.conn,
            &EventEmitter::Noop,
            summaries,
            vec![
                key_of(AgentType::ClaudeCode, "s1"),
                key_of(AgentType::Codex, "s2"),
            ],
        )
        .await
        .expect("batch import");

        assert_eq!(result.imported, 2);
        assert_eq!(result.created_folders, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.folders.len(), 1);
        assert!(result.folders[0].created);

        let folder_rows = crate::db::entities::folder::Entity::find()
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(folder_rows.len(), 1);
        assert_eq!(folder_rows[0].path, "/tmp/proj-a");
        assert!(folder_rows[0].is_open, "created folder must open in sidebar");

        let convs = conversation::Entity::find().all(&db.conn).await.unwrap();
        assert_eq!(convs.len(), 2);
        assert!(convs.iter().all(|c| c.folder_id == folder_rows[0].id));
    }

    #[tokio::test]
    async fn batch_import_reuses_stored_path_for_trailing_slash_variant() {
        use sea_orm::EntityTrait;
        let db = fresh_in_memory_db().await;
        let seeded_id = seed_folder(&db, "/tmp/proj-b").await;

        let summaries = vec![scan_summary(
            "s1",
            AgentType::ClaudeCode,
            Some("/tmp/proj-b/"),
            at(0),
        )];
        let result = import_selected_from_summaries(
            &db.conn,
            &EventEmitter::Noop,
            summaries,
            vec![key_of(AgentType::ClaudeCode, "s1")],
        )
        .await
        .expect("batch import");

        assert_eq!(result.imported, 1);
        assert_eq!(result.created_folders, 0);
        assert!(!result.folders[0].created);
        assert_eq!(result.folders[0].folder_id, seeded_id);

        let folder_rows = crate::db::entities::folder::Entity::find()
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(
            folder_rows.len(),
            1,
            "the trailing-slash cwd must NOT mint a near-duplicate folder row"
        );
    }

    #[tokio::test]
    async fn batch_import_reopens_soft_deleted_folder_without_duplicate() {
        use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set};
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/proj-c").await;

        let row = crate::db::entities::folder::Entity::find_by_id(folder_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let mut active = row.into_active_model();
        active.deleted_at = Set(Some(chrono::Utc::now()));
        active.is_open = Set(false);
        active.update(&db.conn).await.unwrap();

        let summaries = vec![scan_summary(
            "s1",
            AgentType::ClaudeCode,
            Some("/tmp/proj-c"),
            at(0),
        )];
        let result = import_selected_from_summaries(
            &db.conn,
            &EventEmitter::Noop,
            summaries,
            vec![key_of(AgentType::ClaudeCode, "s1")],
        )
        .await
        .expect("batch import");

        assert_eq!(result.imported, 1);
        assert_eq!(
            result.created_folders, 1,
            "reopening a soft-deleted row counts as creating a folder"
        );

        let folder_rows = crate::db::entities::folder::Entity::find()
            .all(&db.conn)
            .await
            .unwrap();
        assert_eq!(folder_rows.len(), 1, "reopened in place, not duplicated");
        assert!(folder_rows[0].deleted_at.is_none());
        assert!(folder_rows[0].is_open);
    }

    #[tokio::test]
    async fn batch_import_skips_already_imported_and_counts_missing_keys() {
        let db = fresh_in_memory_db().await;

        let make = || {
            vec![scan_summary(
                "s1",
                AgentType::ClaudeCode,
                Some("/tmp/proj-d"),
                at(0),
            )]
        };
        let first = import_selected_from_summaries(
            &db.conn,
            &EventEmitter::Noop,
            make(),
            vec![key_of(AgentType::ClaudeCode, "s1")],
        )
        .await
        .unwrap();
        assert_eq!(first.imported, 1);

        let second = import_selected_from_summaries(
            &db.conn,
            &EventEmitter::Noop,
            make(),
            vec![
                key_of(AgentType::ClaudeCode, "s1"),
                key_of(AgentType::Codex, "does-not-exist"),
            ],
        )
        .await
        .unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped, 1, "re-import of an existing row skips");
        assert_eq!(second.not_found, 1, "unresolvable key counts as not_found");
    }

    #[tokio::test]
    async fn batch_import_never_resurrects_a_deleted_conversation() {
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter,
            Set};
        let db = fresh_in_memory_db().await;

        let make = || {
            vec![scan_summary(
                "s1",
                AgentType::ClaudeCode,
                Some("/tmp/proj-e"),
                at(0),
            )]
        };
        import_selected_from_summaries(
            &db.conn,
            &EventEmitter::Noop,
            make(),
            vec![key_of(AgentType::ClaudeCode, "s1")],
        )
        .await
        .unwrap();

        let row = conversation::Entity::find()
            .filter(conversation::Column::ExternalId.eq("s1"))
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let mut active = row.into_active_model();
        active.deleted_at = Set(Some(chrono::Utc::now()));
        active.update(&db.conn).await.unwrap();

        let again = import_selected_from_summaries(
            &db.conn,
            &EventEmitter::Noop,
            make(),
            vec![key_of(AgentType::ClaudeCode, "s1")],
        )
        .await
        .unwrap();
        assert_eq!(again.imported, 0);
        assert_eq!(again.skipped, 1);

        let rows = conversation::Entity::find().all(&db.conn).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].deleted_at.is_some(),
            "a deleted conversation stays deleted"
        );
    }

    #[tokio::test]
    async fn batch_import_selection_of_delegation_child_counts_not_found() {
        let db = fresh_in_memory_db().await;

        let (agent, mut child) =
            scan_summary("child", AgentType::Hermes, Some("/tmp/proj-f"), at(0));
        child.parent_id = Some("root".into());

        let result = import_selected_from_summaries(
            &db.conn,
            &EventEmitter::Noop,
            vec![(agent, child)],
            vec![key_of(AgentType::Hermes, "child")],
        )
        .await
        .unwrap();

        assert_eq!(result.imported, 0);
        assert_eq!(
            result.not_found, 1,
            "a delegation child must never import as a root row"
        );
    }

    #[tokio::test]
    async fn batch_import_emits_folder_upserts_and_one_bulk_event() {
        use crate::web::event_bridge::{
            WebEventBroadcaster, CONVERSATIONS_BULK_CHANGED_EVENT, FOLDER_CHANGED_EVENT,
        };
        use std::sync::Arc;

        let db = fresh_in_memory_db().await;
        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let mut rx = broadcaster.subscribe();
        let emitter = EventEmitter::test_web_only(broadcaster.clone());

        let summaries = vec![
            scan_summary("s1", AgentType::ClaudeCode, Some("/tmp/proj-g"), at(0)),
            scan_summary("s2", AgentType::Codex, Some("/tmp/proj-h"), at(1)),
        ];
        let result = import_selected_from_summaries(
            &db.conn,
            &emitter,
            summaries,
            vec![
                key_of(AgentType::ClaudeCode, "s1"),
                key_of(AgentType::Codex, "s2"),
            ],
        )
        .await
        .unwrap();
        assert_eq!(result.imported, 2);

        let mut folder_events = 0;
        let mut bulk_events = 0;
        while let Ok(evt) = rx.try_recv() {
            match evt.channel.as_str() {
                FOLDER_CHANGED_EVENT => folder_events += 1,
                CONVERSATIONS_BULK_CHANGED_EVENT => {
                    bulk_events += 1;
                    let p = &*evt.payload;
                    assert_eq!(p["imported"], 2);
                    assert_eq!(p["folder_ids"].as_array().unwrap().len(), 2);
                }
                _ => {}
            }
        }
        assert_eq!(folder_events, 2, "one folder upsert per touched folder");
        assert_eq!(bulk_events, 1, "exactly one bulk nudge, never per-row spam");
    }

    #[tokio::test]
    async fn import_selected_sessions_core_rejects_concurrent_and_empty() {
        let db = fresh_in_memory_db().await;

        assert!(
            import_selected_sessions_core(&db.conn, &EventEmitter::Noop, vec![])
                .await
                .is_err(),
            "empty selection is invalid input"
        );

        let _held = IMPORT_GUARD.try_lock().expect("guard free in test");
        assert!(
            import_selected_sessions_core(
                &db.conn,
                &EventEmitter::Noop,
                vec![key_of(AgentType::ClaudeCode, "x")],
            )
            .await
            .is_err(),
            "a second import racing the guard must be rejected"
        );
    }

    #[tokio::test]
    async fn legacy_import_shares_the_guard_with_batch_import() {
        // The retained legacy command must NOT bypass IMPORT_GUARD — otherwise a
        // legacy import racing a batch import could double-insert on a DB with no
        // unique index. With the guard held it is rejected BEFORE the folder
        // lookup, so even a valid folder id surfaces the guard error, not a hit.
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/legacy-guard").await;

        let _held = IMPORT_GUARD.try_lock().expect("guard free in test");
        let err = import_local_conversations_core(&db.conn, &EventEmitter::Noop, folder_id)
            .await
            .expect_err("legacy import must be rejected while an import is in progress");
        let msg = format!("{err:?}").to_lowercase();
        assert!(
            msg.contains("already in progress"),
            "expected the guard error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn import_summaries_counts_row_failures_without_aborting() {
        // A row whose insert fails (here: a non-existent folder_id → FK violation
        // with PRAGMA foreign_keys=ON) is logged and counted as `failed`, never
        // aborting the batch or stranding the good rows — so a mid-group DB error
        // can't lose the committed tally or the folder broadcast.
        let db = fresh_in_memory_db().await;
        let items = vec![
            scan_summary("s1", AgentType::ClaudeCode, Some("/tmp/x"), at(0)),
            scan_summary("s2", AgentType::Codex, Some("/tmp/x"), at(1)),
        ];

        let (tally, updated_ids, failed) =
            import_service::import_summaries_resilient(&db.conn, 999_999, &items).await;
        assert_eq!(failed, 2, "both rows fail the folder FK and are counted");
        assert_eq!(tally.imported, 0);
        assert_eq!(tally.updated, 0);
        assert!(updated_ids.is_empty());

        // Same items into a real folder import cleanly — the resilient loop did
        // not corrupt state or leave a half-open transaction.
        let folder_id = seed_folder(&db, "/tmp/x").await;
        let (tally2, _ids, failed2) =
            import_service::import_summaries_resilient(&db.conn, folder_id, &items).await;
        assert_eq!(failed2, 0);
        assert_eq!(tally2.imported, 2);
    }

    #[tokio::test]
    async fn legacy_strict_import_summaries_propagates_row_failure() {
        // The legacy per-folder importer keeps its strict contract: a DB error
        // propagates as Err rather than being swallowed into a 0/0/0 tally, so
        // its back-compat command still surfaces failures. (The batch path uses
        // the resilient variant instead.)
        let db = fresh_in_memory_db().await;
        let items = vec![scan_summary(
            "s1",
            AgentType::ClaudeCode,
            Some("/tmp/x"),
            at(0),
        )];
        assert!(
            import_service::import_summaries(&db.conn, 999_999, &items)
                .await
                .is_err(),
            "a row FK violation must propagate through the strict importer"
        );
    }
}
