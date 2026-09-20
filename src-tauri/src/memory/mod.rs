/// Memory graph backend: traits, implementations (Off, LocalSqlite, ExternalMcp),
/// and access layer for use within the delegation listener.
pub mod backend;
pub mod mcp_proxy;
pub mod sanitize;
pub mod sqlite;
pub mod tools;

#[cfg(test)]
mod tests;

pub use backend::{
    MemoryBackend, MemoryError, MemoryHit, MemoryNode, MemoryProvenance, NewMemoryNode,
};
pub use mcp_proxy::ExternalMcpBackend;
pub use sanitize::sanitize_secrets;
pub use sqlite::LocalSqliteBackend;
pub use tools::MemoryBackendAccess;

use std::sync::OnceLock;

use crate::db::AppDatabase;

static MEMORY_DB: OnceLock<AppDatabase> = OnceLock::new();

/// Register the database this process stores memory settings in.
///
/// The delegation listener is built before the database handle is available to
/// it, so the memory tools resolve their backend through this handle at call
/// time instead of capturing one at construction.
pub fn set_process_db(db: AppDatabase) {
    let _ = MEMORY_DB.set(db);
}

/// Check if memory backend is enabled: backend is not Off and there is at least
/// one enabled memory kind. Used for feature flag computation at connection setup.
/// Returns false if no database is registered (memory feature disabled).
pub async fn is_memory_enabled() -> bool {
    let Some(db) = MEMORY_DB.get() else {
        return false;
    };
    let Ok(settings) = crate::commands::memory::memory_settings_get_core(db).await else {
        return false;
    };
    if settings.backend == crate::models::MemoryBackendKind::Off {
        return false;
    }
    // Check if there is at least one enabled kind (not Off mode)
    match crate::db::service::memory_kind_service::list(&db.conn).await {
        Ok(kinds) => kinds
            .iter()
            .any(|k| k.enabled && k.mode != crate::models::MemoryMode::Off),
        Err(_) => false,
    }
}

/// Access layer for the memory tools, built from the current settings.
///
/// Returns `None` when this process has no database registered or the user has
/// memory switched off, which is what keeps an off backend from writing.
pub async fn process_access() -> Option<MemoryBackendAccess> {
    let db = MEMORY_DB.get()?;
    let settings = crate::commands::memory::memory_settings_get_core(db)
        .await
        .ok()?;
    let backend: Option<std::sync::Arc<dyn MemoryBackend>> = match settings.backend {
        crate::models::MemoryBackendKind::Off => return None,
        crate::models::MemoryBackendKind::LocalSqlite => {
            let path = crate::paths::codeg_memory_db_path();
            Some(std::sync::Arc::new(LocalSqliteBackend::new(path).ok()?))
        }
        crate::models::MemoryBackendKind::ExternalMcp => {
            let mapping = settings.external.clone()?;
            Some(std::sync::Arc::new(ExternalMcpBackend::new(
                crate::memory::mcp_proxy::ExternalMcpConfig {
                    server_id: mapping.server_id,
                    write_tool: mapping.write_tool,
                    search_tool: mapping.search_tool,
                    link_tool: mapping.link_tool,
                },
            )))
        }
    };
    Some(MemoryBackendAccess::with_db_and_backend(
        db.clone(),
        backend,
        Some(settings),
    ))
}
