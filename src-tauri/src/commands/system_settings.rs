#[cfg(feature = "tauri-runtime")]
use std::path::{Component, Path, PathBuf};

use sea_orm::DatabaseConnection;
#[cfg(feature = "tauri-runtime")]
use tauri::State;

use crate::app_error::AppCommandError;
use crate::db::service::app_metadata_service;
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
#[cfg(feature = "tauri-runtime")]
use crate::models::{SystemOpenTarget, SystemRenderingSettings};
use crate::models::{SystemLanguageSettings, SystemOpenTargetSettings, SystemProxySettings};
#[cfg(feature = "tauri-runtime")]
use crate::network::proxy;
#[cfg(feature = "tauri-runtime")]
use crate::preferences;

const SYSTEM_PROXY_SETTINGS_KEY: &str = "system_proxy_settings";
const SYSTEM_LANGUAGE_SETTINGS_KEY: &str = "system_language_settings";
const SYSTEM_OPEN_TARGET_SETTINGS_KEY: &str = "system_open_target_settings";
#[cfg(feature = "tauri-runtime")]
const LANGUAGE_SETTINGS_UPDATED_EVENT: &str = "app://language-settings-updated";

fn normalize_proxy_settings(
    settings: SystemProxySettings,
) -> Result<SystemProxySettings, AppCommandError> {
    if !settings.enabled {
        let proxy_url = settings
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        return Ok(SystemProxySettings {
            enabled: false,
            proxy_url,
        });
    }

    let proxy_url = settings
        .proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppCommandError::configuration_missing("Proxy URL is required when proxy is enabled")
        })?;

    reqwest::Proxy::all(proxy_url).map_err(|e| {
        AppCommandError::configuration_invalid("Invalid proxy URL").with_detail(e.to_string())
    })?;

    Ok(SystemProxySettings {
        enabled: true,
        proxy_url: Some(proxy_url.to_string()),
    })
}

fn normalize_open_target_settings(settings: SystemOpenTargetSettings) -> SystemOpenTargetSettings {
    settings
}

#[cfg(feature = "tauri-runtime")]
struct ResolvedWorkspacePath {
    root: PathBuf,
    target: PathBuf,
}

#[cfg(feature = "tauri-runtime")]
fn resolve_workspace_relative_path(
    folder_path: &str,
    relative_path: &str,
) -> Result<ResolvedWorkspacePath, AppCommandError> {
    let root = PathBuf::from(folder_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppCommandError::not_found("Folder does not exist"));
    }

    let rel = Path::new(relative_path);
    if rel.is_absolute() {
        return Err(AppCommandError::invalid_input("Path must be relative"));
    }

    for component in rel.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(AppCommandError::invalid_input("Path cannot contain '..'"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(AppCommandError::invalid_input("Invalid path component"));
            }
        }
    }

    let target = root.join(rel);
    if !target.exists() {
        return Err(AppCommandError::not_found("File does not exist"));
    }
    if !target.is_file() {
        return Err(AppCommandError::invalid_input("Path is not a file"));
    }

    let canonical_root = std::fs::canonicalize(&root).map_err(AppCommandError::io)?;
    let canonical_target = std::fs::canonicalize(&target).map_err(AppCommandError::io)?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(AppCommandError::invalid_input(
            "Path is outside workspace root",
        ));
    }

    Ok(ResolvedWorkspacePath {
        root: canonical_root,
        target: canonical_target,
    })
}

#[cfg(feature = "tauri-runtime")]
fn spawn_code_cli(root: &Path, target: &Path) -> Result<(), std::io::Error> {
    let mut command = crate::process::std_command("code");
    command.arg("--new-window").arg(root).arg(target);
    command.spawn().map(|_| ())
}

#[cfg(all(feature = "tauri-runtime", target_os = "macos"))]
fn spawn_platform_vscode(root: &Path, target: &Path) -> Result<(), std::io::Error> {
    let app_cli = Path::new("/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code");
    if app_cli.exists() {
        let mut command = crate::process::std_command(app_cli);
        command.arg("--new-window").arg(root).arg(target);
        if command.spawn().is_ok() {
            return Ok(());
        }
    }

    let mut command = crate::process::std_command("open");
    command
        .arg("-n")
        .arg("-a")
        .arg("Visual Studio Code")
        .arg("--args")
        .arg("--new-window")
        .arg(root)
        .arg(target);
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("VS Code application could not be opened: {status}"),
        )),
        Err(err) => Err(err),
    }
}

#[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
fn spawn_platform_vscode(root: &Path, target: &Path) -> Result<(), std::io::Error> {
    let mut candidates = Vec::new();
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(base);
        candidates.push(
            base.join("Programs")
                .join("Microsoft VS Code")
                .join("Code.exe"),
        );
        candidates.push(base.join("Microsoft VS Code").join("Code.exe"));
    }
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(key) {
            candidates.push(
                PathBuf::from(base)
                    .join("Microsoft VS Code")
                    .join("Code.exe"),
            );
        }
    }

    let mut last_error = None;
    for candidate in candidates {
        if !candidate.exists() {
            continue;
        }
        let mut command = crate::process::std_command(&candidate);
        command.arg("--new-window").arg(root).arg(target);
        match command.spawn() {
            Ok(_) => return Ok(()),
            Err(err) => last_error = Some(err),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "VS Code executable not found")
    }))
}

#[cfg(all(
    feature = "tauri-runtime",
    not(any(target_os = "macos", target_os = "windows"))
))]
fn spawn_platform_vscode(_root: &Path, _target: &Path) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "VS Code executable not found",
    ))
}

#[cfg(feature = "tauri-runtime")]
fn open_path_in_vscode(root: &Path, target: &Path) -> Result<(), AppCommandError> {
    match spawn_code_cli(root, target) {
        Ok(()) => Ok(()),
        Err(code_err) => match spawn_platform_vscode(root, target) {
            Ok(()) => Ok(()),
            Err(fallback_err) => {
                let detail = format!("code: {code_err}; fallback: {fallback_err}");
                if code_err.kind() == std::io::ErrorKind::NotFound
                    && fallback_err.kind() == std::io::ErrorKind::NotFound
                {
                    Err(AppCommandError::dependency_missing(
                        "VS Code was not found. Install VS Code and enable the 'code' command in PATH.",
                    )
                    .with_detail(detail))
                } else {
                    Err(AppCommandError::external_command(
                        "Failed to open file in VS Code",
                        detail,
                    ))
                }
            }
        },
    }
}

pub(crate) async fn load_system_proxy_settings(
    conn: &DatabaseConnection,
) -> Result<SystemProxySettings, AppCommandError> {
    let raw = app_metadata_service::get_value(conn, SYSTEM_PROXY_SETTINGS_KEY)
        .await
        .map_err(AppCommandError::from)?;

    let Some(raw) = raw else {
        return Ok(SystemProxySettings::default());
    };

    let parsed = serde_json::from_str::<SystemProxySettings>(&raw).map_err(|e| {
        AppCommandError::configuration_invalid("Failed to parse stored proxy settings")
            .with_detail(e.to_string())
    })?;
    normalize_proxy_settings(parsed)
}

pub(crate) async fn load_system_language_settings(
    conn: &DatabaseConnection,
) -> Result<SystemLanguageSettings, AppCommandError> {
    let raw = app_metadata_service::get_value(conn, SYSTEM_LANGUAGE_SETTINGS_KEY)
        .await
        .map_err(AppCommandError::from)?;

    let Some(raw) = raw else {
        return Ok(SystemLanguageSettings::default());
    };

    serde_json::from_str::<SystemLanguageSettings>(&raw).map_err(|e| {
        AppCommandError::configuration_invalid("Failed to parse stored language settings")
            .with_detail(e.to_string())
    })
}

pub(crate) async fn load_system_open_target_settings(
    conn: &DatabaseConnection,
) -> Result<SystemOpenTargetSettings, AppCommandError> {
    let raw = app_metadata_service::get_value(conn, SYSTEM_OPEN_TARGET_SETTINGS_KEY)
        .await
        .map_err(AppCommandError::from)?;

    let Some(raw) = raw else {
        return Ok(SystemOpenTargetSettings::default());
    };

    let parsed = serde_json::from_str::<SystemOpenTargetSettings>(&raw).map_err(|e| {
        AppCommandError::configuration_invalid("Failed to parse stored open target settings")
            .with_detail(e.to_string())
    })?;
    Ok(normalize_open_target_settings(parsed))
}

pub(crate) async fn update_system_open_target_settings_core(
    conn: &DatabaseConnection,
    settings: SystemOpenTargetSettings,
) -> Result<SystemOpenTargetSettings, AppCommandError> {
    let normalized = normalize_open_target_settings(settings);
    let serialized = serde_json::to_string(&normalized).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize open target settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(conn, SYSTEM_OPEN_TARGET_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    Ok(normalized)
}

#[cfg(feature = "tauri-runtime")]
pub(crate) async fn open_path_with_target_core(
    folder_path: String,
    relative_path: String,
    target: Option<SystemOpenTarget>,
    conn: &DatabaseConnection,
) -> Result<(), AppCommandError> {
    let settings = match target {
        Some(target) => SystemOpenTargetSettings {
            target,
            ..SystemOpenTargetSettings::default()
        },
        None => load_system_open_target_settings(conn).await?,
    };
    let normalized = normalize_open_target_settings(settings);

    match normalized.target {
        SystemOpenTarget::Vscode => {
            let resolved_path = resolve_workspace_relative_path(&folder_path, &relative_path)?;
            open_path_in_vscode(&resolved_path.root, &resolved_path.target)?;
            Ok(())
        }
        SystemOpenTarget::FileManager => Err(AppCommandError::invalid_input(
            "The open_path_with_target command only supports VS Code. Use file manager actions from the file tree instead.",
        )),
        SystemOpenTarget::Terminal => Err(AppCommandError::invalid_input(
            "The open_path_with_target command does not support opening terminals.",
        )),
    }
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_proxy_settings(
    db: State<'_, AppDatabase>,
) -> Result<SystemProxySettings, AppCommandError> {
    load_system_proxy_settings(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_proxy_settings(
    settings: SystemProxySettings,
    db: State<'_, AppDatabase>,
) -> Result<SystemProxySettings, AppCommandError> {
    let normalized = normalize_proxy_settings(settings)?;
    let serialized = serde_json::to_string(&normalized).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize proxy settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(&db.conn, SYSTEM_PROXY_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    proxy::apply_system_proxy_settings(&normalized)?;
    Ok(normalized)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_language_settings(
    db: State<'_, AppDatabase>,
) -> Result<SystemLanguageSettings, AppCommandError> {
    load_system_language_settings(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_open_target_settings(
    db: State<'_, AppDatabase>,
) -> Result<SystemOpenTargetSettings, AppCommandError> {
    load_system_open_target_settings(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_open_target_settings(
    settings: SystemOpenTargetSettings,
    db: State<'_, AppDatabase>,
) -> Result<SystemOpenTargetSettings, AppCommandError> {
    update_system_open_target_settings_core(&db.conn, settings).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_path_with_target(
    folder_path: String,
    relative_path: String,
    target: Option<SystemOpenTarget>,
    db: State<'_, AppDatabase>,
) -> Result<(), AppCommandError> {
    open_path_with_target_core(folder_path, relative_path, target, &db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_language_settings(
    settings: SystemLanguageSettings,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<SystemLanguageSettings, AppCommandError> {
    let serialized = serde_json::to_string(&settings).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize language settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(&db.conn, SYSTEM_LANGUAGE_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    let emitter = crate::web::event_bridge::EventEmitter::Tauri(app);
    crate::web::event_bridge::emit_event(
        &emitter,
        LANGUAGE_SETTINGS_UPDATED_EVENT,
        settings.clone(),
    );

    Ok(settings)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_rendering_settings() -> Result<SystemRenderingSettings, AppCommandError> {
    let prefs = preferences::load();
    Ok(SystemRenderingSettings {
        disable_hardware_acceleration: prefs.disable_hardware_acceleration,
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_rendering_settings(
    settings: SystemRenderingSettings,
) -> Result<SystemRenderingSettings, AppCommandError> {
    let mut prefs = preferences::load();
    prefs.disable_hardware_acceleration = settings.disable_hardware_acceleration;
    preferences::save(&prefs).map_err(|err| {
        AppCommandError::io_error("Failed to persist rendering settings")
            .with_detail(err.to_string())
    })?;
    Ok(settings)
}
