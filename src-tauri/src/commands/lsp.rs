use serde::Serialize;
use tauri::{Emitter, State};

use crate::db::service::lsp_server_setting_service;
use crate::db::AppDatabase;
use crate::lsp::binary_cache;
use crate::lsp::error::LspError;
use crate::lsp::preflight::{self, LspPreflightResult};
use crate::lsp::registry::{self, LspDistribution};
use crate::lsp::types::{LspServerInfo, LspServerStatus};

const LSP_SERVERS_UPDATED_EVENT: &str = "app://lsp-servers-updated";

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
struct LspServersUpdatedPayload {
    reason: &'static str,
    server_id: Option<String>,
}

fn emit_lsp_servers_updated(
    app: &tauri::AppHandle,
    reason: &'static str,
    server_id: Option<String>,
) {
    let _ = app.emit(
        LSP_SERVERS_UPDATED_EVENT,
        LspServersUpdatedPayload { reason, server_id },
    );
}

fn is_version_like(value: &str) -> bool {
    value.chars().any(|c| c.is_ascii_digit()) && value.contains('.')
}

fn normalize_version_candidate(value: &str) -> Option<String> {
    let normalized = value.trim().trim_start_matches('v');
    if is_version_like(normalized) {
        Some(normalized.to_string())
    } else {
        None
    }
}

fn package_name_from_spec(package: &str) -> String {
    let normalized = package.trim();
    if normalized.is_empty() {
        return String::new();
    }

    if let Some(index) = normalized.rfind('@') {
        if index > 0 {
            let version_part = normalized[index + 1..].trim();
            if !version_part.is_empty() {
                return normalized[..index].to_string();
            }
        }
    }

    normalized.to_string()
}

async fn detect_npm_global_version(package_name: &str) -> Option<String> {
    let npm_path = which::which("npm").ok()?;
    let output = crate::process::tokio_command(npm_path)
        .arg("list")
        .arg("-g")
        .arg(package_name)
        .arg("--json")
        .arg("--depth=0")
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).ok()?;
    let version = json
        .get("dependencies")?
        .get(package_name)?
        .get("version")?
        .as_str()?;
    normalize_version_candidate(version)
}

async fn install_npm_global_package(package: &str) -> Result<(), LspError> {
    let output = crate::process::tokio_command("npm")
        .arg("install")
        .arg("-g")
        .arg(package)
        .output()
        .await
        .map_err(|e| LspError::InstallFailed(format!("failed to run npm install -g: {e}")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if err.is_empty() {
            "failed to install npm package globally".to_string()
        } else {
            format!("failed to install npm package globally: {err}")
        };
        return Err(LspError::InstallFailed(msg));
    }

    Ok(())
}

async fn uninstall_npm_global_package(package: &str) -> Result<(), LspError> {
    let package_name = package_name_from_spec(package);

    if !package_name.is_empty() {
        let output = crate::process::tokio_command("npm")
            .arg("uninstall")
            .arg("-g")
            .arg(&package_name)
            .output()
            .await
            .map_err(|e| LspError::InstallFailed(format!("failed to run npm uninstall -g: {e}")))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if err.is_empty() {
                "failed to uninstall npm package globally".to_string()
            } else {
                format!("failed to uninstall npm package globally: {err}")
            };
            return Err(LspError::InstallFailed(msg));
        }
    }

    Ok(())
}

async fn install_cargo_package(
    crate_name: &str,
    version: &str,
    features: &[&str],
) -> Result<(), LspError> {
    let mut cmd = crate::process::tokio_command("cargo");
    cmd.arg("install").arg(crate_name);
    cmd.arg("--version").arg(version);
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| LspError::InstallFailed(format!("failed to run cargo install: {e}")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(LspError::InstallFailed(format!(
            "cargo install failed: {err}"
        )));
    }

    Ok(())
}

async fn uninstall_cargo_package(crate_name: &str) -> Result<(), LspError> {
    let output = crate::process::tokio_command("cargo")
        .arg("uninstall")
        .arg(crate_name)
        .output()
        .await
        .map_err(|e| LspError::InstallFailed(format!("failed to run cargo uninstall: {e}")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(LspError::InstallFailed(format!(
            "cargo uninstall failed: {err}"
        )));
    }

    Ok(())
}

async fn install_pip_package(package: &str, version: &str) -> Result<(), LspError> {
    let pip_cmd = if which::which("pip3").is_ok() {
        "pip3"
    } else {
        "pip"
    };

    let spec = format!("{package}=={version}");
    let output = crate::process::tokio_command(pip_cmd)
        .arg("install")
        .arg(&spec)
        .output()
        .await
        .map_err(|e| LspError::InstallFailed(format!("failed to run pip install: {e}")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(LspError::InstallFailed(format!(
            "pip install failed: {err}"
        )));
    }

    Ok(())
}

async fn uninstall_pip_package(package: &str) -> Result<(), LspError> {
    let pip_cmd = if which::which("pip3").is_ok() {
        "pip3"
    } else {
        "pip"
    };

    let output = crate::process::tokio_command(pip_cmd)
        .arg("uninstall")
        .arg("-y")
        .arg(package)
        .output()
        .await
        .map_err(|e| LspError::InstallFailed(format!("failed to run pip uninstall: {e}")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(LspError::InstallFailed(format!(
            "pip uninstall failed: {err}"
        )));
    }

    Ok(())
}

async fn detect_cargo_version(cmd: &str) -> Option<String> {
    let cmd_path = which::which(cmd).ok()?;
    let output = crate::process::tokio_command(cmd_path)
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse "taplo 0.9.3" or "tool-name 1.2.3" format
    stdout
        .split_whitespace()
        .find_map(|s| normalize_version_candidate(s))
}

async fn detect_pip_version(package: &str) -> Option<String> {
    let pip_cmd = if which::which("pip3").is_ok() {
        "pip3"
    } else if which::which("pip").is_ok() {
        "pip"
    } else {
        return None;
    };

    let output = crate::process::tokio_command(pip_cmd)
        .arg("show")
        .arg(package)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(version) = line.strip_prefix("Version:") {
            return normalize_version_candidate(version.trim());
        }
    }
    None
}

#[tauri::command]
pub async fn lsp_list_servers(
    db: State<'_, AppDatabase>,
) -> Result<Vec<LspServerInfo>, LspError> {
    let all_ids = registry::all_lsp_servers();
    let defaults: Vec<_> = all_ids
        .iter()
        .enumerate()
        .map(|(i, id)| lsp_server_setting_service::LspServerDefaultInput {
            server_id: id.to_string(),
            default_sort_order: i as i32,
        })
        .collect();
    lsp_server_setting_service::ensure_defaults(&db.conn, &defaults).await?;

    let settings = lsp_server_setting_service::list(&db.conn).await?;

    let mut servers = Vec::new();
    for setting in settings {
        if let Some(meta) = registry::get_lsp_meta(&setting.server_id) {
            servers.push(LspServerInfo {
                id: setting.server_id,
                name: meta.name.to_string(),
                description: meta.description.to_string(),
                language: meta.language.to_string(),
                distribution_type: meta.distribution_type().to_string(),
                registry_version: meta.registry_version().map(|v| v.to_string()),
                enabled: setting.enabled,
                sort_order: setting.sort_order,
                installed_version: setting.installed_version,
                config_json: setting.config_json,
            });
        }
    }

    Ok(servers)
}

#[tauri::command]
pub async fn lsp_get_server_status(server_id: String) -> Result<LspServerStatus, LspError> {
    let meta = registry::get_lsp_meta(&server_id)
        .ok_or_else(|| LspError::NotFound(server_id.clone()))?;

    let installed_version = detect_local_version(&server_id, &meta.distribution).await;
    let registry_version = meta.registry_version();

    let update_available = match (&installed_version, registry_version) {
        (Some(installed), Some(registry)) => installed != registry,
        _ => false,
    };

    Ok(LspServerStatus {
        id: server_id,
        installed: installed_version.is_some(),
        installed_version,
        update_available,
    })
}

async fn detect_local_version(server_id: &str, distribution: &LspDistribution) -> Option<String> {
    match distribution {
        LspDistribution::Npm { package, cmd, .. } => {
            if !which::which(cmd).is_ok() {
                return None;
            }
            let pkg_name = package_name_from_spec(package);
            if let Some(v) = detect_npm_global_version(&pkg_name).await {
                return Some(v);
            }
            None
        }
        LspDistribution::Binary { cmd, .. } => {
            binary_cache::detect_installed_version(server_id, cmd).ok().flatten()
        }
        LspDistribution::CargoInstall { cmd, .. } => detect_cargo_version(cmd).await,
        LspDistribution::PipInstall { package, .. } => detect_pip_version(package).await,
    }
}

#[tauri::command]
pub async fn lsp_preflight(server_id: String) -> Result<LspPreflightResult, LspError> {
    Ok(preflight::run_preflight(&server_id).await)
}

#[tauri::command]
pub async fn lsp_install_server(
    server_id: String,
    app: tauri::AppHandle,
    db: State<'_, AppDatabase>,
) -> Result<(), LspError> {
    let meta = registry::get_lsp_meta(&server_id)
        .ok_or_else(|| LspError::NotFound(server_id.clone()))?;

    match &meta.distribution {
        LspDistribution::Npm { package, .. } => {
            install_npm_global_package(package).await?;
        }
        LspDistribution::Binary {
            version,
            cmd,
            platforms,
        } => {
            let current = crate::acp::registry::current_platform();
            let platform = platforms
                .iter()
                .find(|p| p.platform == current)
                .ok_or_else(|| {
                    LspError::PlatformNotSupported(format!(
                        "No binary available for platform {current}"
                    ))
                })?;
            binary_cache::ensure_binary(&server_id, version, platform.url, cmd).await?;
        }
        LspDistribution::CargoInstall {
            version,
            crate_name,
            features,
            ..
        } => {
            install_cargo_package(crate_name, version, features).await?;
        }
        LspDistribution::PipInstall {
            version, package, ..
        } => {
            install_pip_package(package, version).await?;
        }
    }

    // Detect and persist installed version
    let installed = detect_local_version(&server_id, &meta.distribution).await;
    lsp_server_setting_service::set_installed_version(&db.conn, &server_id, installed).await?;

    emit_lsp_servers_updated(&app, "installed", Some(server_id));
    Ok(())
}

#[tauri::command]
pub async fn lsp_upgrade_server(
    server_id: String,
    app: tauri::AppHandle,
    db: State<'_, AppDatabase>,
) -> Result<(), LspError> {
    let meta = registry::get_lsp_meta(&server_id)
        .ok_or_else(|| LspError::NotFound(server_id.clone()))?;

    // For binary type, clear old cache first
    if matches!(meta.distribution, LspDistribution::Binary { .. }) {
        let _ = binary_cache::clear_server_cache(&server_id);
    }

    // Re-install with the registry version
    match &meta.distribution {
        LspDistribution::Npm { package, .. } => {
            install_npm_global_package(package).await?;
        }
        LspDistribution::Binary {
            version,
            cmd,
            platforms,
        } => {
            let current = crate::acp::registry::current_platform();
            let platform = platforms
                .iter()
                .find(|p| p.platform == current)
                .ok_or_else(|| {
                    LspError::PlatformNotSupported(format!(
                        "No binary available for platform {current}"
                    ))
                })?;
            binary_cache::ensure_binary(&server_id, version, platform.url, cmd).await?;
        }
        LspDistribution::CargoInstall {
            version,
            crate_name,
            features,
            ..
        } => {
            install_cargo_package(crate_name, version, features).await?;
        }
        LspDistribution::PipInstall {
            version, package, ..
        } => {
            install_pip_package(package, version).await?;
        }
    }

    let installed = detect_local_version(&server_id, &meta.distribution).await;
    lsp_server_setting_service::set_installed_version(&db.conn, &server_id, installed).await?;

    emit_lsp_servers_updated(&app, "upgraded", Some(server_id));
    Ok(())
}

#[tauri::command]
pub async fn lsp_uninstall_server(
    server_id: String,
    app: tauri::AppHandle,
    db: State<'_, AppDatabase>,
) -> Result<(), LspError> {
    let meta = registry::get_lsp_meta(&server_id)
        .ok_or_else(|| LspError::NotFound(server_id.clone()))?;

    match &meta.distribution {
        LspDistribution::Npm { package, .. } => {
            uninstall_npm_global_package(package).await?;
        }
        LspDistribution::Binary { .. } => {
            binary_cache::clear_server_cache(&server_id)?;
        }
        LspDistribution::CargoInstall { crate_name, .. } => {
            uninstall_cargo_package(crate_name).await?;
        }
        LspDistribution::PipInstall { package, .. } => {
            uninstall_pip_package(package).await?;
        }
    }

    lsp_server_setting_service::set_installed_version(&db.conn, &server_id, None).await?;

    emit_lsp_servers_updated(&app, "uninstalled", Some(server_id));
    Ok(())
}

#[tauri::command]
pub async fn lsp_detect_server_version(
    server_id: String,
) -> Result<Option<String>, LspError> {
    let meta = registry::get_lsp_meta(&server_id)
        .ok_or_else(|| LspError::NotFound(server_id.clone()))?;
    Ok(detect_local_version(&server_id, &meta.distribution).await)
}

#[tauri::command]
pub async fn lsp_update_server_preferences(
    server_id: String,
    enabled: bool,
    config_json: Option<String>,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), LspError> {
    lsp_server_setting_service::update(
        &db.conn,
        &server_id,
        lsp_server_setting_service::LspServerSettingsUpdate {
            enabled,
            config_json,
        },
    )
    .await?;

    emit_lsp_servers_updated(&app, "preferences_updated", Some(server_id));
    Ok(())
}

#[tauri::command]
pub async fn lsp_reorder_servers(
    server_ids: Vec<String>,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), LspError> {
    lsp_server_setting_service::reorder(&db.conn, &server_ids).await?;
    emit_lsp_servers_updated(&app, "reordered", None);
    Ok(())
}

#[tauri::command]
pub async fn lsp_clear_binary_cache(server_id: String) -> Result<(), LspError> {
    binary_cache::clear_server_cache(&server_id)
}
