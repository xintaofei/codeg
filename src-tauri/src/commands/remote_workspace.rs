#[cfg(feature = "tauri-runtime")]
use reqwest::StatusCode;
#[cfg(feature = "tauri-runtime")]
use serde::Deserialize;
#[cfg(feature = "tauri-runtime")]
use std::time::Duration;
#[cfg(feature = "tauri-runtime")]
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

#[cfg(feature = "tauri-runtime")]
use crate::app_error::AppCommandError;
#[cfg(feature = "tauri-runtime")]
use crate::db::service::remote_workspace_connection_service;
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
#[cfg(feature = "tauri-runtime")]
use crate::models::RemoteWorkspaceConnectionInfo;

#[cfg(feature = "tauri-runtime")]
const REMOTE_HEALTH_TIMEOUT: Duration = Duration::from_secs(8);

#[cfg(feature = "tauri-runtime")]
pub(crate) fn new_remote_window_instance_id() -> String {
    format!("rw-{}", uuid::Uuid::new_v4().simple())
}

#[cfg(feature = "tauri-runtime")]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkspaceConnectionInput {
    pub name: String,
    #[serde(alias = "baseUrl")]
    pub base_url: String,
    pub token: String,
}

#[cfg(feature = "tauri-runtime")]
async fn validate_remote_health(base_url: &str, token: &str) -> Result<(), AppCommandError> {
    let normalized = remote_workspace_connection_service::normalize_base_url(base_url)?;
    let url = format!("{normalized}/api/health");
    let client = reqwest::Client::builder()
        .timeout(REMOTE_HEALTH_TIMEOUT)
        .build()
        .map_err(|e| {
            AppCommandError::configuration_invalid("Failed to create remote health client")
                .with_detail(e.to_string())
        })?;
    let response = client
        .post(url)
        .bearer_auth(token.trim())
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| {
            AppCommandError::network("Unable to connect to remote workspace")
                .with_detail(e.to_string())
        })?;

    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(AppCommandError::authentication_failed(
            "Remote Workspace token is invalid",
        ));
    }

    if !response.status().is_success() {
        return Err(
            AppCommandError::network("Remote Workspace health check failed")
                .with_detail(format!("HTTP {}", response.status())),
        );
    }

    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_remote_workspace_connections(
    db: tauri::State<'_, AppDatabase>,
) -> Result<Vec<RemoteWorkspaceConnectionInfo>, AppCommandError> {
    remote_workspace_connection_service::list(&db.conn)
        .await
        .map_err(AppCommandError::db)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_remote_workspace_connection(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<RemoteWorkspaceConnectionInfo, AppCommandError> {
    remote_workspace_connection_service::get(&db.conn, id)
        .await
        .map_err(AppCommandError::db)?
        .ok_or_else(|| AppCommandError::not_found(format!("Remote connection {id} not found")))
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn test_remote_workspace_connection(
    input: RemoteWorkspaceConnectionInput,
) -> Result<(), AppCommandError> {
    validate_remote_health(&input.base_url, &input.token).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_remote_workspace_connection(
    db: tauri::State<'_, AppDatabase>,
    input: RemoteWorkspaceConnectionInput,
) -> Result<RemoteWorkspaceConnectionInfo, AppCommandError> {
    validate_remote_health(&input.base_url, &input.token).await?;
    remote_workspace_connection_service::create(
        &db.conn,
        &input.name,
        &input.base_url,
        &input.token,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_remote_workspace_connection(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    input: RemoteWorkspaceConnectionInput,
) -> Result<RemoteWorkspaceConnectionInfo, AppCommandError> {
    validate_remote_health(&input.base_url, &input.token).await?;
    remote_workspace_connection_service::update(
        &db.conn,
        id,
        &input.name,
        &input.base_url,
        &input.token,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn delete_remote_workspace_connection(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), AppCommandError> {
    remote_workspace_connection_service::delete(&db.conn, id)
        .await
        .map_err(AppCommandError::db)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn reorder_remote_workspace_connections(
    db: tauri::State<'_, AppDatabase>,
    ids: Vec<i32>,
) -> Result<(), AppCommandError> {
    remote_workspace_connection_service::reorder(&db.conn, ids).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_remote_workspace(
    app: AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<(), AppCommandError> {
    let connection = remote_workspace_connection_service::get(&db.conn, id)
        .await
        .map_err(AppCommandError::db)?
        .ok_or_else(|| AppCommandError::not_found(format!("Remote connection {id} not found")))?;

    let label = format!("remote-workspace-{id}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.unminimize();
        existing.set_focus().map_err(|e| {
            AppCommandError::window("Failed to focus remote workspace", e.to_string())
        })?;
        return Ok(());
    }

    validate_remote_health(&connection.base_url, &connection.token).await?;

    let window_instance_id = new_remote_window_instance_id();
    let url = WebviewUrl::App(
        format!("workspace?remoteConnectionId={id}&remoteWindowId={window_instance_id}").into(),
    );
    let builder = WebviewWindowBuilder::new(&app, &label, url)
        .title(format!("Codeg - {}", connection.name))
        .inner_size(1260.0, 860.0)
        .min_inner_size(400.0, 600.0)
        .center();
    let window = crate::commands::windows::apply_platform_window_style(builder)
        .build()
        .map_err(|e| AppCommandError::window("Failed to open remote workspace", e.to_string()))?;
    if let Some(proxy) =
        app.try_state::<std::sync::Arc<crate::commands::remote_proxy::RemoteProxyState>>()
    {
        proxy
            .inner()
            .register_window_instance_cleanup(&window, window_instance_id);
    }
    crate::commands::windows::post_window_setup(&window);
    Ok(())
}
