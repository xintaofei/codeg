/// Commands for memory settings and operations.
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::db::error::DbError;
use crate::db::service::{app_metadata_service, memory_kind_service};
use crate::db::AppDatabase;
use crate::memory::{ExternalMcpBackend, LocalSqliteBackend, MemoryBackend, MemoryHit};
use crate::models::{MemoryBackendKind, MemoryKind, MemoryKindDraft, MemoryScope, MemorySettings};
use crate::web::event_bridge::{emit_event, EventEmitter};

pub const MEMORY_CHANGED_EVENT: &str = "memory://changed";

const MEMORY_SETTINGS_KEY: &str = "memory.settings";

fn map_db(error: DbError) -> AppCommandError {
    match error {
        DbError::NotFound(message) => AppCommandError::not_found(message),
        DbError::Validation(message) => AppCommandError::invalid_input(message),
        DbError::Conflict(message) => AppCommandError::already_exists(message),
        other => AppCommandError::db(other),
    }
}

// Parameter structs for HTTP handlers
#[derive(Debug, Clone, Deserialize)]
pub struct MemorySettingsSetParams {
    pub settings: MemorySettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryKindCreateParams {
    pub draft: MemoryKindDraft,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryKindUpdateParams {
    pub id: i32,
    pub draft: MemoryKindDraft,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryKindSetEnabledParams {
    pub id: i32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryKindIdParams {
    pub id: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemorySearchParams {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryNodeIdParams {
    pub id: i32,
}

/// Get current memory settings.
pub async fn memory_settings_get_core(db: &AppDatabase) -> Result<MemorySettings, AppCommandError> {
    let raw = app_metadata_service::get_value(&db.conn, MEMORY_SETTINGS_KEY)
        .await
        .map_err(AppCommandError::from)?;

    match raw {
        Some(raw) => serde_json::from_str::<MemorySettings>(&raw).map_err(|e| {
            AppCommandError::configuration_invalid("Failed to parse stored memory settings")
                .with_detail(e.to_string())
        }),
        None => Ok(MemorySettings {
            backend: MemoryBackendKind::Off,
            scope: MemoryScope::Global,
            external: None,
        }),
    }
}

/// Set memory settings.
pub async fn memory_settings_set_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    settings: MemorySettings,
) -> Result<MemorySettings, AppCommandError> {
    let serialized = serde_json::to_string(&settings).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize memory settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(&db.conn, MEMORY_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    emit_event(
        emitter,
        MEMORY_CHANGED_EVENT,
        serde_json::json!({ "kind": "settings" }),
    );

    Ok(settings)
}

/// List all memory kinds (built-in + custom).
pub async fn memory_kind_list_core(db: &AppDatabase) -> Result<Vec<MemoryKind>, AppCommandError> {
    memory_kind_service::list(&db.conn).await.map_err(map_db)
}

/// Create a custom memory kind.
pub async fn memory_kind_create_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    draft: MemoryKindDraft,
) -> Result<MemoryKind, AppCommandError> {
    let kind = memory_kind_service::create(&db.conn, draft)
        .await
        .map_err(map_db)?;
    emit_event(
        emitter,
        MEMORY_CHANGED_EVENT,
        serde_json::json!({ "kind": "kinds" }),
    );
    Ok(kind)
}

/// Update a memory kind (mode, enabled, or instruction for built-in types).
pub async fn memory_kind_update_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    id: i32,
    draft: MemoryKindDraft,
) -> Result<MemoryKind, AppCommandError> {
    let kind = memory_kind_service::update(&db.conn, id, draft)
        .await
        .map_err(map_db)?;
    emit_event(
        emitter,
        MEMORY_CHANGED_EVENT,
        serde_json::json!({ "kind": "kinds" }),
    );
    Ok(kind)
}

/// Set enabled flag for a memory kind.
pub async fn memory_kind_set_enabled_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    id: i32,
    enabled: bool,
) -> Result<MemoryKind, AppCommandError> {
    let kind = memory_kind_service::set_enabled(&db.conn, id, enabled)
        .await
        .map_err(map_db)?;
    emit_event(
        emitter,
        MEMORY_CHANGED_EVENT,
        serde_json::json!({ "kind": "kinds" }),
    );
    Ok(kind)
}

/// Delete a custom memory kind (built-in cannot be deleted).
pub async fn memory_kind_delete_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    id: i32,
) -> Result<(), AppCommandError> {
    memory_kind_service::delete(&db.conn, id)
        .await
        .map_err(map_db)?;
    emit_event(
        emitter,
        MEMORY_CHANGED_EVENT,
        serde_json::json!({ "kind": "kinds" }),
    );
    Ok(())
}

/// Search the memory graph.
pub async fn memory_search_core(
    db: &AppDatabase,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<MemoryHit>, AppCommandError> {
    memory_search_scoped(db, query, limit, None).await
}

/// As [`memory_search_core`], but limited to one folder when the caller knows
/// which project it is working in. Without it a project-scoped search returns
/// every project's entries.
pub async fn memory_search_scoped(
    db: &AppDatabase,
    query: String,
    limit: Option<usize>,
    folder_id: Option<i32>,
) -> Result<Vec<MemoryHit>, AppCommandError> {
    let settings = memory_settings_get_core(db).await?;
    match settings.backend {
        MemoryBackendKind::Off => Ok(vec![]),
        MemoryBackendKind::LocalSqlite => {
            let db_path = crate::paths::codeg_memory_db_path();
            let backend = LocalSqliteBackend::new(db_path).map_err(|e| AppCommandError::database_error(e.0))?;
            backend
                .search(&query, settings.scope, folder_id, limit.unwrap_or(20))
                .await
                .map_err(|e| AppCommandError::database_error(e.0))
        }
        MemoryBackendKind::ExternalMcp => {
            if let Some(mapping) = settings.external {
                let config = crate::memory::mcp_proxy::ExternalMcpConfig {
                    server_id: mapping.server_id,
                    write_tool: mapping.write_tool,
                    search_tool: mapping.search_tool,
                    link_tool: mapping.link_tool,
                };
                let backend = ExternalMcpBackend::new(config);
                backend
                    .search(&query, settings.scope, folder_id, limit.unwrap_or(20))
                    .await
                    .map_err(|e| AppCommandError::network(e.0))
            } else {
                Ok(vec![])
            }
        }
    }
}

/// Delete a memory node.
pub async fn memory_node_delete_core(
    emitter: &EventEmitter,
    db: &AppDatabase,
    id: i32,
) -> Result<(), AppCommandError> {
    let settings = memory_settings_get_core(db).await?;
    match settings.backend {
        MemoryBackendKind::Off => Ok(()),
        MemoryBackendKind::LocalSqlite => {
            let db_path = crate::paths::codeg_memory_db_path();
            let backend = LocalSqliteBackend::new(db_path).map_err(|e| AppCommandError::database_error(e.0))?;
            backend
                .delete(id)
                .await
                .map_err(|e| AppCommandError::database_error(e.0))?;
            emit_event(
                emitter,
                MEMORY_CHANGED_EVENT,
                serde_json::json!({ "kind": "nodes" }),
            );
            Ok(())
        }
        MemoryBackendKind::ExternalMcp => {
            if let Some(mapping) = settings.external {
                let config = crate::memory::mcp_proxy::ExternalMcpConfig {
                    server_id: mapping.server_id,
                    write_tool: mapping.write_tool,
                    search_tool: mapping.search_tool,
                    link_tool: mapping.link_tool,
                };
                let backend = ExternalMcpBackend::new(config);
                backend
                    .delete(id)
                    .await
                    .map_err(|e| AppCommandError::network(e.0))?;
                emit_event(
                    emitter,
                    MEMORY_CHANGED_EVENT,
                    serde_json::json!({ "kind": "nodes" }),
                );
            }
            Ok(())
        }
    }
}

// ─── Tauri command wrappers ───

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn memory_settings_get(
    db: tauri::State<'_, AppDatabase>,
) -> Result<MemorySettings, AppCommandError> {
    memory_settings_get_core(&db).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn memory_settings_set(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    settings: MemorySettings,
) -> Result<MemorySettings, AppCommandError> {
    memory_settings_set_core(&EventEmitter::Tauri(app), &db, settings).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn memory_kind_list(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<MemoryKind>, AppCommandError> {
    memory_kind_list_core(&db).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn memory_kind_create(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    draft: MemoryKindDraft,
) -> Result<MemoryKind, AppCommandError> {
    memory_kind_create_core(&EventEmitter::Tauri(app), &db, draft).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn memory_kind_update(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    draft: MemoryKindDraft,
) -> Result<MemoryKind, AppCommandError> {
    memory_kind_update_core(&EventEmitter::Tauri(app), &db, id, draft).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn memory_kind_set_enabled(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    enabled: bool,
) -> Result<MemoryKind, AppCommandError> {
    memory_kind_set_enabled_core(&EventEmitter::Tauri(app), &db, id, enabled).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn memory_kind_delete(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), AppCommandError> {
    memory_kind_delete_core(&EventEmitter::Tauri(app), &db, id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn memory_search(
    db: tauri::State<'_, AppDatabase>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<MemoryHit>, AppCommandError> {
    memory_search_core(&db, query, limit).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn memory_node_delete(
    app: tauri::AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), AppCommandError> {
    memory_node_delete_core(&EventEmitter::Tauri(app), &db, id).await
}

/// Write an entry produced by the pipeline itself (not by an agent tool call).
///
/// Secrets are stripped first, and an Off backend drops the write, so turning
/// memory off really does mean nothing is stored.
pub async fn memory_write_auto_core(
    db: &AppDatabase,
    mut node: crate::memory::backend::NewMemoryNode,
) -> Result<Option<i32>, AppCommandError> {
    let settings = memory_settings_get_core(db).await?;
    if settings.backend == MemoryBackendKind::Off {
        return Ok(None);
    }
    let (clean_title, _) = crate::memory::sanitize_secrets(&node.title);
    let (clean_body, _) = crate::memory::sanitize_secrets(&node.body);
    node.title = clean_title;
    node.body = clean_body;
    node.scope = settings.scope;
    if settings.scope != crate::models::MemoryScope::Project {
        node.folder_id = None;
    }

    match settings.backend {
        MemoryBackendKind::Off => Ok(None),
        MemoryBackendKind::LocalSqlite => {
            let db_path = crate::paths::codeg_memory_db_path();
            let backend = LocalSqliteBackend::new(db_path)
                .map_err(|e| AppCommandError::database_error(e.0))?;
            let id = backend
                .write(node)
                .await
                .map_err(|e| AppCommandError::database_error(e.0))?;
            Ok(Some(id))
        }
        MemoryBackendKind::ExternalMcp => {
            let Some(mapping) = settings.external else {
                return Ok(None);
            };
            let config = crate::memory::mcp_proxy::ExternalMcpConfig {
                server_id: mapping.server_id,
                write_tool: mapping.write_tool,
                search_tool: mapping.search_tool,
                link_tool: mapping.link_tool,
            };
            let backend = ExternalMcpBackend::new(config);
            let id = backend
                .write(node)
                .await
                .map_err(|e| AppCommandError::network(e.0))?;
            Ok(Some(id))
        }
    }
}
