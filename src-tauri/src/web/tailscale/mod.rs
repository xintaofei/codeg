//! Tailscale Funnel sidecar control plane.
//!
//! Codeg never talks to system Tailscale. All Funnel lifecycle is owned by the
//! pure-Go `codeg-tsnet` userspace sidecar under an independent state directory.

pub mod binary;
pub mod controller;
pub mod protocol;
pub mod status;

pub use binary::{default_hostname, default_state_dir, locate_codeg_tsnet_binary};
pub use controller::{EnableFunnelOpts, TailscaleController};
pub use protocol::{SidecarBootstrap, SidecarStatus};
pub use status::TailscaleFunnelStatus;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::web::{load_web_service_config, update_web_service_config_core, DEFAULT_WEB_SERVICE_PORT};

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetTailscaleFunnelEnabledParams {
    pub enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTailscaleLoginResult {
    pub login_url: Option<String>,
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn get_tailscale_funnel_status(
    controller: tauri::State<'_, std::sync::Arc<TailscaleController>>,
) -> Result<TailscaleFunnelStatus, AppCommandError> {
    Ok(controller.status().await)
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn set_tailscale_funnel_enabled(
    app: tauri::AppHandle,
    controller: tauri::State<'_, std::sync::Arc<TailscaleController>>,
    ws: tauri::State<'_, crate::web::WebServerState>,
    params: SetTailscaleFunnelEnabledParams,
) -> Result<TailscaleFunnelStatus, AppCommandError> {
    use tauri::Manager;
    let db = app.state::<crate::db::AppDatabase>();
    let data_dir = crate::paths::resolve_effective_data_dir(
        &app.path().app_data_dir().unwrap_or_default(),
    );
    let port = ws.current_port();
    set_funnel_enabled_parts(
        controller.inner().clone(),
        &db.conn,
        data_dir,
        port,
        params.enabled,
        false,
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn open_tailscale_login(
    controller: tauri::State<'_, std::sync::Arc<TailscaleController>>,
) -> Result<OpenTailscaleLoginResult, AppCommandError> {
    let login_url = controller.open_login_hint().await?;
    Ok(OpenTailscaleLoginResult { login_url })
}

/// Resolve the localhost port for Funnel.
///
/// Desktop Tauri embeds a placeholder `WebServerState` inside the router
/// `AppState`, so `current_port()` may be 0 even while the managed listener
/// is live. Fall back to the persisted config port in that case.
async fn resolve_funnel_port(
    conn: &sea_orm::DatabaseConnection,
    current_port: u16,
) -> u16 {
    if current_port != 0 {
        return current_port;
    }
    match load_web_service_config(conn).await {
        Ok(cfg) => cfg.port.unwrap_or(DEFAULT_WEB_SERVICE_PORT),
        Err(_) => DEFAULT_WEB_SERVICE_PORT,
    }
}

pub async fn set_funnel_enabled_core(
    state: std::sync::Arc<AppState>,
    enabled: bool,
) -> Result<TailscaleFunnelStatus, AppCommandError> {
    let port = state.web_server_state.current_port();
    set_funnel_enabled_parts(
        state.tailscale.clone(),
        &state.db.conn,
        state.data_dir.clone(),
        port,
        enabled,
        false,
    )
    .await
}

pub async fn set_funnel_enabled_parts(
    controller: std::sync::Arc<TailscaleController>,
    conn: &sea_orm::DatabaseConnection,
    data_dir: std::path::PathBuf,
    current_port: u16,
    enabled: bool,
    require_auth_key: bool,
) -> Result<TailscaleFunnelStatus, AppCommandError> {
    if !enabled {
        // Persist off first so a later restart doesn't re-enable.
        if let Ok(mut cfg) = load_web_service_config(conn).await {
            if cfg.funnel_enabled {
                cfg.funnel_enabled = false;
                let _ = update_web_service_config_core(conn, cfg).await;
            }
        }
        controller.disable_funnel().await?;
        return Ok(controller.status().await);
    }

    let port = resolve_funnel_port(conn, current_port).await;
    if port == 0 {
        return Err(AppCommandError::invalid_input(
            "Web Service must be running before enabling Funnel",
        ));
    }

    // Persist on.
    if let Ok(mut cfg) = load_web_service_config(conn).await {
        if !cfg.funnel_enabled {
            cfg.funnel_enabled = true;
            let _ = update_web_service_config_core(conn, cfg).await;
        }
    }

    controller
        .enable_funnel(EnableFunnelOpts {
            data_dir,
            localhost_port: port,
            auth_key: std::env::var("CODEG_TS_AUTHKEY")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            require_auth_key,
            state_dir_override: std::env::var_os("CODEG_TS_STATE_DIR").map(std::path::PathBuf::from),
            hostname_override: std::env::var("CODEG_TS_HOSTNAME")
                .ok()
                .filter(|s| !s.trim().is_empty()),
        })
        .await
}

/// Best-effort Funnel enable used after a successful web bind.
pub async fn maybe_enable_funnel_after_web_start(
    state: &AppState,
    port: u16,
    require_auth_key: bool,
) {
    let enabled = match load_web_service_config(&state.db.conn).await {
        Ok(cfg) => cfg.funnel_enabled,
        Err(err) => {
            tracing::warn!(%err, "[tailscale] failed to load funnel config");
            false
        }
    };
    let env_enabled = matches!(
        std::env::var("CODEG_TS_FUNNEL").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    );
    if !(enabled || env_enabled) {
        return;
    }

    match state
        .tailscale
        .enable_funnel(EnableFunnelOpts {
            data_dir: state.data_dir.clone(),
            localhost_port: port,
            auth_key: std::env::var("CODEG_TS_AUTHKEY")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            require_auth_key,
            state_dir_override: std::env::var_os("CODEG_TS_STATE_DIR").map(std::path::PathBuf::from),
            hostname_override: std::env::var("CODEG_TS_HOSTNAME")
                .ok()
                .filter(|s| !s.trim().is_empty()),
        })
        .await
    {
        Ok(st) => {
            tracing::info!(
                state = %st.state,
                funnel_url = ?st.funnel_url,
                "[tailscale] Funnel status after web start"
            );
        }
        Err(err) => {
            tracing::warn!(%err, "[tailscale] Funnel not enabled");
        }
    }
}
