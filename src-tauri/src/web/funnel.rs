//! Official Tailscale Serve (private tailnet) and Funnel (public HTTPS).
//! Both terminate TLS on this PC and proxy HTTP to 127.0.0.1 only.
//! Serve is the both-devices privacy model. Funnel is a public door + token.

use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::timeout;

use crate::app_error::{AppCommandError, AppErrorCode};

const FUNNEL_DEADLINE: Duration = Duration::from_secs(20);
const FUNNEL_STOP_DEADLINE: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunnelStatus {
    pub enabled: bool,
    pub url: Option<String>,
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

fn tailscale_program() -> String {
    let windows = r"C:\Program Files\Tailscale\tailscale.exe";
    if cfg!(windows) && std::path::Path::new(windows).exists() {
        return windows.to_string();
    }
    "tailscale".to_string()
}

pub fn funnel_target(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

pub fn extract_funnel_url(status: &Value) -> Option<String> {
    let web = status.get("Web")?.as_object()?;
    for key in web.keys() {
        let host = key.split(':').next().unwrap_or(key);
        if host.contains('.') {
            return Some(format!("https://{host}"));
        }
    }
    None
}

pub fn extract_funnel_target(status: &Value) -> Option<String> {
    let web = status.get("Web")?.as_object()?;
    for value in web.values() {
        let handlers = value.get("Handlers")?.as_object()?;
        for handler in handlers.values() {
            if let Some(proxy) = handler.get("Proxy").and_then(Value::as_str) {
                return Some(proxy.to_string());
            }
        }
    }
    None
}

fn target_is_loopback(target: &str) -> bool {
    target.contains("127.0.0.1") || target.contains("localhost")
}

pub fn allow_funnel(status: &Value) -> bool {
    match status.get("AllowFunnel") {
        Some(Value::Object(map)) => map.values().any(|value| value.as_bool() == Some(true)),
        Some(Value::Bool(flag)) => *flag,
        _ => false,
    }
}

fn status_from_json(raw: &str, want_public: bool) -> FunnelStatus {
    let value: Value = serde_json::from_str(raw).unwrap_or(Value::Object(Default::default()));
    let url = extract_funnel_url(&value);
    let target = extract_funnel_target(&value);
    let public = allow_funnel(&value);
    let enabled = url.is_some() && public == want_public;
    FunnelStatus {
        enabled,
        url: if enabled { url } else { None },
        target: if enabled { target } else { None },
        login_url: None,
        unavailable_reason: None,
    }
}

async fn run_tailscale_with_deadline(
    args: &[&str],
    deadline: Duration,
) -> Result<String, AppCommandError> {
    let mut cmd = crate::process::tokio_command(tailscale_program());
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd.spawn().map_err(|err| {
        AppCommandError::new(
            AppErrorCode::DependencyMissing,
            "Tailscale CLI is not available",
        )
        .with_detail(err.to_string())
    })?;
    let output = timeout(deadline, child.wait_with_output())
        .await
        .map_err(|_| {
            AppCommandError::new(AppErrorCode::ExternalCommandFailed, "Tailscale timed out")
        })?
        .map_err(|err| {
            AppCommandError::new(AppErrorCode::ExternalCommandFailed, "Tailscale failed")
                .with_detail(err.to_string())
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppCommandError::new(
            AppErrorCode::ExternalCommandFailed,
            "Tailscale command failed",
        )
        .with_detail(stderr.chars().take(400).collect::<String>()));
    }
    Ok(stdout)
}

async fn run_tailscale(args: &[&str]) -> Result<String, AppCommandError> {
    run_tailscale_with_deadline(args, FUNNEL_DEADLINE).await
}

async fn expose_status_json() -> Result<String, AppCommandError> {
    match run_tailscale(&["serve", "status", "--json"]).await {
        Ok(raw) => Ok(raw),
        Err(_) => run_tailscale(&["funnel", "status", "--json"]).await,
    }
}

pub async fn serve_status_core() -> FunnelStatus {
    if let Some(status) = crate::web::tsnet_sidecar::sidecar_status(false) {
        return status;
    }
    match expose_status_json().await {
        Ok(raw) => status_from_json(&raw, false),
        Err(err) => FunnelStatus {
            enabled: false,
            url: None,
            target: None,
            login_url: None,
            unavailable_reason: Some(err.message),
        },
    }
}

pub async fn funnel_status_core() -> FunnelStatus {
    if let Some(status) = crate::web::tsnet_sidecar::sidecar_status(true) {
        return status;
    }
    match expose_status_json().await {
        Ok(raw) => status_from_json(&raw, true),
        Err(err) => FunnelStatus {
            enabled: false,
            url: None,
            target: None,
            login_url: None,
            unavailable_reason: Some(err.message),
        },
    }
}

pub fn require_running_web_port(
    running_port: Option<u16>,
    requested: u16,
) -> Result<(), AppCommandError> {
    match running_port {
        Some(port) if port == requested => Ok(()),
        Some(_) => Err(AppCommandError::new(
            AppErrorCode::InvalidInput,
            "Tailscale port must match the running Web Service",
        )),
        None => Err(AppCommandError::new(
            AppErrorCode::InvalidInput,
            "Start the Web Service before enabling Tailscale",
        )),
    }
}

async fn enable_expose(port: u16, public: bool) -> Result<FunnelStatus, AppCommandError> {
    if crate::web::tsnet_sidecar::sidecar_available() {
        return crate::web::tsnet_sidecar::sidecar_enable(port, public).await;
    }
    let target = funnel_target(port);
    if !target_is_loopback(&target) {
        return Err(AppCommandError::new(
            AppErrorCode::InvalidInput,
            "Tailscale target must be loopback",
        ));
    }
    // Same port cannot be Serve and Funnel. Reset both, then set the mode.
    let _ = run_tailscale(&["funnel", "reset"]).await;
    let _ = run_tailscale(&["serve", "reset"]).await;
    if public {
        run_tailscale(&["funnel", "--bg", "--yes", &target]).await?;
        Ok(funnel_status_core().await)
    } else {
        run_tailscale(&["serve", "--bg", "--yes", &target]).await?;
        Ok(serve_status_core().await)
    }
}

pub async fn serve_enable_core(port: u16) -> Result<FunnelStatus, AppCommandError> {
    enable_expose(port, false).await
}

pub async fn funnel_enable_core(port: u16) -> Result<FunnelStatus, AppCommandError> {
    enable_expose(port, true).await
}

pub async fn serve_disable_core() -> Result<FunnelStatus, AppCommandError> {
    expose_reset().await;
    Ok(serve_status_core().await)
}

pub async fn funnel_disable_core() -> Result<FunnelStatus, AppCommandError> {
    expose_reset().await;
    Ok(funnel_status_core().await)
}

async fn expose_reset() {
    crate::web::tsnet_sidecar::sidecar_disable().await;
    let _ = run_tailscale(&["funnel", "reset"]).await;
    let _ = run_tailscale(&["serve", "reset"]).await;
}

/// Tear down leftover Serve/Funnel URLs without blocking Stop / quit for 20s.
pub async fn funnel_disable_best_effort() {
    crate::web::tsnet_sidecar::sidecar_disable().await;
    let _ = run_tailscale_with_deadline(&["funnel", "reset"], FUNNEL_STOP_DEADLINE).await;
    let _ = run_tailscale_with_deadline(&["serve", "reset"], FUNNEL_STOP_DEADLINE).await;
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn tailscale_serve_status() -> Result<FunnelStatus, AppCommandError> {
    Ok(serve_status_core().await)
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn tailscale_serve_enable(
    state: tauri::State<'_, crate::web::WebServerState>,
    port: u16,
) -> Result<FunnelStatus, AppCommandError> {
    let running = crate::web::do_get_web_server_status(&state).map(|info| info.port);
    require_running_web_port(running, port)?;
    serve_enable_core(port).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn tailscale_serve_disable() -> Result<FunnelStatus, AppCommandError> {
    serve_disable_core().await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn tailscale_funnel_status() -> Result<FunnelStatus, AppCommandError> {
    Ok(funnel_status_core().await)
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn tailscale_funnel_enable(
    state: tauri::State<'_, crate::web::WebServerState>,
    port: u16,
) -> Result<FunnelStatus, AppCommandError> {
    let running = crate::web::do_get_web_server_status(&state).map(|info| info.port);
    require_running_web_port(running, port)?;
    funnel_enable_core(port).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn tailscale_funnel_disable() -> Result<FunnelStatus, AppCommandError> {
    funnel_disable_core().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_https_url_and_loopback_proxy() {
        let status = json!({
            "Web": {
                "codeg.tail123.ts.net:443": {
                    "Handlers": {
                        "/": { "Proxy": "http://127.0.0.1:3080" }
                    }
                }
            }
        });
        assert_eq!(
            extract_funnel_url(&status).as_deref(),
            Some("https://codeg.tail123.ts.net")
        );
        assert_eq!(
            extract_funnel_target(&status).as_deref(),
            Some("http://127.0.0.1:3080")
        );
        assert!(target_is_loopback("http://127.0.0.1:3080"));
        assert!(!target_is_loopback("http://0.0.0.0:3080"));
        assert!(require_running_web_port(Some(3080), 3080).is_ok());
        assert!(require_running_web_port(None, 3080).is_err());
        assert!(require_running_web_port(Some(3080), 4000).is_err());
        assert!(!allow_funnel(&status));
        let public = json!({
            "Web": {
                "codeg.tail123.ts.net:443": {
                    "Handlers": {
                        "/": { "Proxy": "http://127.0.0.1:3080" }
                    }
                }
            },
            "AllowFunnel": { "codeg.tail123.ts.net:443": true }
        });
        assert!(allow_funnel(&public));
        let serve = status_from_json(&public.to_string(), false);
        let funnel = status_from_json(&public.to_string(), true);
        assert!(!serve.enabled);
        assert!(funnel.enabled);
    }
}
