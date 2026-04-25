use std::sync::Arc;

use axum::{extract::Extension, Json};

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::commands::runtime as runtime_commands;

pub async fn get_runtime_diagnostics(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<runtime_commands::RuntimeDiagnostics>, AppCommandError> {
    let result = runtime_commands::get_runtime_diagnostics_core(
        &state.connection_manager,
        &state.terminal_manager,
        &state.web_client_registry,
        &state.runtime_monitor,
    )
    .await?;
    Ok(Json(result))
}

pub async fn get_acp_cache_inventory(
) -> Result<Json<crate::acp::binary_cache::AcpCacheInventory>, AppCommandError> {
    let result = crate::acp::binary_cache::inventory_agent_caches()
        .map_err(|error| AppCommandError::task_execution_failed(error.to_string()))?;
    Ok(Json(result))
}
