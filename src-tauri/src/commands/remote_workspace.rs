#[cfg(feature = "tauri-runtime")]
use reqwest::StatusCode;
#[cfg(feature = "tauri-runtime")]
use serde::Deserialize;
#[cfg(feature = "tauri-runtime")]
use std::time::Duration;
#[cfg(feature = "tauri-runtime")]
use tauri::{AppHandle, Emitter, EventTarget, Manager, WebviewUrl, WebviewWindowBuilder};

#[cfg(feature = "tauri-runtime")]
use crate::app_error::AppCommandError;
#[cfg(feature = "tauri-runtime")]
use crate::db::service::remote_workspace_connection_service;
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
#[cfg(feature = "tauri-runtime")]
use crate::models::{RemoteWorkspaceConnectionInfo, RemoteWorkspaceHeader, ToHeaderMap};

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
    #[serde(default)]
    pub headers: Vec<RemoteWorkspaceHeader>,
}

#[cfg(feature = "tauri-runtime")]
async fn validate_remote_health(
    base_url: &str,
    token: &str,
    headers: &[RemoteWorkspaceHeader],
) -> Result<(), AppCommandError> {
    let normalized = remote_workspace_connection_service::normalize_base_url(base_url)?;
    let url = format!("{normalized}/api/health");
    let client = reqwest::Client::builder()
        .timeout(REMOTE_HEALTH_TIMEOUT)
        // The health check is the first request to carry the connection's
        // custom headers, and the one the user runs to prove the setup works.
        // It gets the same host pinning as every later request, or "test
        // succeeded" would mean something weaker than "save succeeded".
        .redirect(crate::commands::remote_proxy::connection_redirect_policy())
        .build()
        .map_err(|e| {
            AppCommandError::configuration_invalid("Failed to create remote health client")
                .with_detail(e.to_string())
        })?;
    let headers = remote_workspace_connection_service::validate_headers(headers)?;
    let response = client
        .post(url)
        .bearer_auth(token.trim())
        .headers(headers.to_header_map())
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| {
            AppCommandError::network("Unable to connect to remote workspace")
                .with_detail(crate::commands::remote_proxy::request_error_detail(&e))
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
    validate_remote_health(&input.base_url, &input.token, &input.headers).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_remote_workspace_connection(
    db: tauri::State<'_, AppDatabase>,
    input: RemoteWorkspaceConnectionInput,
) -> Result<RemoteWorkspaceConnectionInfo, AppCommandError> {
    validate_remote_health(&input.base_url, &input.token, &input.headers).await?;
    remote_workspace_connection_service::create(
        &db.conn,
        &input.name,
        &input.base_url,
        &input.token,
        &input.headers,
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
    validate_remote_health(&input.base_url, &input.token, &input.headers).await?;
    remote_workspace_connection_service::update(
        &db.conn,
        id,
        &input.name,
        &input.base_url,
        &input.token,
        &input.headers,
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

/// Tauri event used to hand an already-open remote workspace window a folder
/// to open. MUST match `REMOTE_OPEN_FOLDER_EVENT` in
/// `src/lib/remote-workspace.ts`.
#[cfg(feature = "tauri-runtime")]
const REMOTE_OPEN_FOLDER_EVENT: &str = "remote-open-folder";

/// Query param carrying the same request to a window that is being spawned
/// (an event can't reach a webview that doesn't exist yet). MUST match the
/// param read by `RemoteWorkspaceOpenFolderListener`.
#[cfg(feature = "tauri-runtime")]
const OPEN_FOLDER_PATH_PARAM: &str = "openFolderPath";

#[cfg(feature = "tauri-runtime")]
#[derive(Clone, serde::Serialize)]
struct RemoteOpenFolderPayload {
    path: String,
}

/// Spawn (and register) the window bound to a remote connection. `extra_query`
/// is appended verbatim to the workspace URL and must already be URL-encoded.
#[cfg(feature = "tauri-runtime")]
fn build_remote_workspace_window(
    app: &AppHandle,
    id: i32,
    name: &str,
    extra_query: &str,
) -> Result<tauri::WebviewWindow, AppCommandError> {
    let label = format!("remote-workspace-{id}");
    let window_instance_id = new_remote_window_instance_id();
    let url = WebviewUrl::App(
        format!(
            "workspace?remoteConnectionId={id}&remoteWindowId={window_instance_id}{extra_query}"
        )
        .into(),
    );
    let builder = WebviewWindowBuilder::new(app, &label, url)
        .title(format!("Codeg - {name}"))
        .inner_size(1260.0, 860.0)
        .min_inner_size(400.0, 600.0)
        .center();
    let builder = crate::commands::windows::apply_platform_window_style(builder);
    // Remote workspace windows load the same `/workspace` route with the taller
    // h-10 title bar, so they get the workspace traffic-light position (not the
    // shorter auxiliary-window default).
    #[cfg(target_os = "macos")]
    let builder = builder
        .traffic_light_position(crate::commands::windows::workspace_window_traffic_light_position());
    let window = builder
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
    Ok(window)
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

    validate_remote_health(&connection.base_url, &connection.token, &connection.headers).await?;

    build_remote_workspace_window(&app, id, &connection.name, "")?;
    Ok(())
}

/// Open a folder that lives on a remote workspace host.
///
/// Folders belong to the backend that owns their paths, so this never opens the
/// folder here — it raises (or spawns) the window bound to `connection_id` and
/// hands it the path. That window opens the folder through its own transport,
/// which keeps the folder in the workspace that owns it and lets any failure
/// surface where the user is looking.
///
/// A live window gets a Tauri event; a window we have to spawn gets the path as
/// a URL param, since an event can't reach a webview that doesn't exist yet.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_remote_workspace_folder(
    app: AppHandle,
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    path: String,
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
        app.emit_to(
            EventTarget::webview(&label),
            REMOTE_OPEN_FOLDER_EVENT,
            RemoteOpenFolderPayload { path },
        )
        .map_err(|e| {
            AppCommandError::window("Failed to open the folder in the remote workspace", e.to_string())
        })?;
        return Ok(());
    }

    validate_remote_health(&connection.base_url, &connection.token, &connection.headers).await?;

    let query = format!("&{}={}", OPEN_FOLDER_PATH_PARAM, urlencoding::encode(&path));
    build_remote_workspace_window(&app, id, &connection.name, &query)?;
    Ok(())
}
