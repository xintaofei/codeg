use serde::Serialize;
#[cfg(feature = "tauri-runtime")]
use tauri::State;

use crate::acp::manager::{ConnectionLimitSnapshot, ConnectionManager};
use crate::acp::types::ConnectionInfo;
use crate::app_error::AppCommandError;
use crate::build_info::{BuildConsistencyInfo, RuntimeSecurityInfo};
use crate::runtime_monitor::{RuntimeLogEntry, RuntimeMonitor};
use crate::terminal::manager::TerminalManager;
use crate::terminal::types::TerminalInfo;
use crate::web::client_owner::{WebClientInfo, WebClientRegistry};
#[cfg(feature = "tauri-runtime")]
use {
    std::sync::Arc,
    crate::acp::binary_cache::{self, AcpCacheInventory},
};

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeDiagnostics {
    pub build: Option<BuildConsistencyInfo>,
    pub security: Option<RuntimeSecurityInfo>,
    pub connections: Vec<ConnectionInfo>,
    pub terminals: Vec<TerminalInfo>,
    pub web_clients: Vec<WebClientInfo>,
    pub recent_logs: Vec<RuntimeLogEntry>,
    pub connection_limits: ConnectionLimitSnapshot,
}

pub async fn get_runtime_diagnostics_core(
    connection_manager: &ConnectionManager,
    terminal_manager: &TerminalManager,
    web_client_registry: &WebClientRegistry,
    runtime_monitor: &RuntimeMonitor,
) -> Result<RuntimeDiagnostics, AppCommandError> {
    let connections = connection_manager.list_connections().await;
    let connection_limits = connection_manager.limits_snapshot().await;
    let terminals = terminal_manager.list_with_exit_check(None);
    let web_clients = web_client_registry.list_clients().await;
    let recent_logs = runtime_monitor.recent_entries(100);
    let build = runtime_monitor.build_consistency();
    let security = runtime_monitor.security();

    Ok(RuntimeDiagnostics {
        build,
        security,
        connections,
        terminals,
        web_clients,
        recent_logs,
        connection_limits,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_runtime_diagnostics(
    connection_manager: State<'_, ConnectionManager>,
    terminal_manager: State<'_, TerminalManager>,
    web_client_registry: State<'_, Arc<WebClientRegistry>>,
    runtime_monitor: State<'_, Arc<RuntimeMonitor>>,
) -> Result<RuntimeDiagnostics, AppCommandError> {
    get_runtime_diagnostics_core(
        connection_manager.inner(),
        terminal_manager.inner(),
        web_client_registry.inner(),
        runtime_monitor.inner(),
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_acp_cache_inventory() -> Result<AcpCacheInventory, AppCommandError> {
    binary_cache::inventory_agent_caches()
        .map_err(|error| AppCommandError::task_execution_failed(error.to_string()))
}
