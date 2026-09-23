use axum::{extract::Extension, Json};
/// HTTP handlers for memory API.
use std::sync::Arc;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::memory as core;
use crate::memory::MemoryHit;
use crate::models::{MemoryKind, MemorySettings};

/// Get memory settings.
pub async fn get_memory_settings(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<MemorySettings>, AppCommandError> {
    Ok(Json(core::memory_settings_get_core(&state.db).await?))
}

/// Set memory settings.
pub async fn set_memory_settings(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::MemorySettingsSetParams>,
) -> Result<Json<MemorySettings>, AppCommandError> {
    Ok(Json(
        core::memory_settings_set_core(&state.emitter, &state.db, params.settings).await?,
    ))
}

/// List memory kinds.
pub async fn list_memory_kinds(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<MemoryKind>>, AppCommandError> {
    Ok(Json(core::memory_kind_list_core(&state.db).await?))
}

/// Create a custom memory kind.
pub async fn create_memory_kind(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::MemoryKindCreateParams>,
) -> Result<Json<MemoryKind>, AppCommandError> {
    Ok(Json(
        core::memory_kind_create_core(&state.emitter, &state.db, params.draft).await?,
    ))
}

/// Update a memory kind.
pub async fn update_memory_kind(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::MemoryKindUpdateParams>,
) -> Result<Json<MemoryKind>, AppCommandError> {
    Ok(Json(
        core::memory_kind_update_core(&state.emitter, &state.db, params.id, params.draft).await?,
    ))
}

/// Set enabled flag for a memory kind.
pub async fn set_enabled_memory_kind(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::MemoryKindSetEnabledParams>,
) -> Result<Json<MemoryKind>, AppCommandError> {
    Ok(Json(
        core::memory_kind_set_enabled_core(&state.emitter, &state.db, params.id, params.enabled)
            .await?,
    ))
}

/// Delete a memory kind.
pub async fn delete_memory_kind(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::MemoryKindIdParams>,
) -> Result<Json<()>, AppCommandError> {
    core::memory_kind_delete_core(&state.emitter, &state.db, params.id).await?;
    Ok(Json(()))
}

/// Search memory.
pub async fn search_memory(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::MemorySearchParams>,
) -> Result<Json<Vec<MemoryHit>>, AppCommandError> {
    Ok(Json(
        core::memory_search_core(&state.db, params.query, params.limit).await?,
    ))
}

/// Delete a memory node.
pub async fn delete_memory_node(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<core::MemoryNodeIdParams>,
) -> Result<Json<()>, AppCommandError> {
    core::memory_node_delete_core(&state.emitter, &state.db, params.id).await?;
    Ok(Json(()))
}
