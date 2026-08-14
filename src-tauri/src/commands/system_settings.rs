use sea_orm::DatabaseConnection;
#[cfg(feature = "tauri-runtime")]
use tauri::State;

#[cfg(feature = "tauri-runtime")]
use crate::acp::manager::ConnectionManager;
use crate::acp::terminal_runtime::TerminalShellRuntimeConfig;
use crate::app_error::AppCommandError;
use crate::db::service::app_metadata_service;
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;
#[cfg(feature = "tauri-runtime")]
use crate::models::SystemRenderingSettings;
#[cfg(feature = "tauri-runtime")]
use crate::models::{CloseAction, SystemCloseSettings, SystemCloseSettingsInfo};
use crate::models::{
    AvailableTerminalShells, SystemLanguageSettings, SystemProxySettings, SystemTerminalSettings,
    TerminalShellOption,
};
#[cfg(feature = "tauri-runtime")]
use crate::network::proxy;
#[cfg(feature = "tauri-runtime")]
use crate::preferences;
use crate::terminal::manager::resolve_shell;

#[cfg(feature = "tauri-runtime")]
use tokio::sync::Mutex;
#[cfg(feature = "tauri-runtime")]
use std::sync::OnceLock;

pub(crate) const SYSTEM_PROXY_SETTINGS_KEY: &str = "system_proxy_settings";
pub(crate) const SYSTEM_LANGUAGE_SETTINGS_KEY: &str = "system_language_settings";
pub(crate) const SYSTEM_TERMINAL_SETTINGS_KEY: &str = "system_terminal_settings";
#[cfg(feature = "tauri-runtime")]
pub(crate) const SYSTEM_CLOSE_SETTINGS_KEY: &str = "system_close_settings";
pub(crate) const LANGUAGE_SETTINGS_UPDATED_EVENT: &str = "app://language-settings-updated";
pub(crate) const TERMINAL_SETTINGS_UPDATED_EVENT: &str = "app://terminal-settings-updated";

pub(crate) const TERMINAL_SHELL_OPTION_SYSTEM: &str = "system";
pub(crate) const TERMINAL_SHELL_OPTION_CUSTOM: &str = "custom";

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

/// Whether `value` resolves to an executable on the current host. Used to
/// drive the "not installed" badge in the picker; never used to *block* a
/// selection — users may legitimately preconfigure a shell before installing it.
fn shell_exists(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    let path = std::path::Path::new(trimmed);
    let looks_like_path = path.is_absolute()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || path.components().count() > 1;

    if looks_like_path {
        return path.is_file();
    }

    which::which(trimmed).is_ok()
}

/// Trim and drop empty-only. We deliberately do **not** filter by host
/// platform: the Settings UI's custom-path field lets users type any shell
/// they want, and silently rewriting their input is more confusing than
/// letting `terminal_spawn` surface the failure if the path is wrong.
pub(crate) fn normalize_terminal_settings(
    settings: SystemTerminalSettings,
) -> SystemTerminalSettings {
    SystemTerminalSettings {
        default_shell: settings
            .default_shell
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    }
}

/// Build the per-platform option list shown in the "default shell" picker.
/// The frontend renders these verbatim, looking each `label_key` up under its
/// `GeneralSettings` namespace — so adding a new shell here requires zero
/// frontend code changes (only a new translation key).
pub(crate) fn build_available_terminal_shells() -> AvailableTerminalShells {
    let mut options: Vec<TerminalShellOption> = Vec::new();

    options.push(TerminalShellOption {
        id: TERMINAL_SHELL_OPTION_SYSTEM.to_string(),
        label_key: "terminalSystemDefault".to_string(),
        value: None,
        // System default always "exists" — resolve_shell() has its own fallback chain.
        exists: true,
        accepts_custom_path: false,
    });

    if cfg!(target_os = "windows") {
        for (id, label_key) in [
            ("pwsh.exe", "terminalPowerShell7"),
            ("powershell.exe", "terminalWindowsPowerShell"),
            ("cmd.exe", "terminalCmd"),
        ] {
            options.push(TerminalShellOption {
                id: id.to_string(),
                label_key: label_key.to_string(),
                value: Some(id.to_string()),
                exists: shell_exists(id),
                accepts_custom_path: false,
            });
        }
    }

    options.push(TerminalShellOption {
        id: TERMINAL_SHELL_OPTION_CUSTOM.to_string(),
        label_key: "terminalShellCustom".to_string(),
        value: None,
        // The "custom" row itself is always available; the path the user
        // types is validated via probe_terminal_shell_path.
        exists: true,
        accepts_custom_path: true,
    });

    AvailableTerminalShells {
        options,
        resolved_shell: resolve_shell(),
    }
}

/// Probe whether a user-supplied shell path or command exists on the host.
/// Returns `false` for empty / whitespace-only input.
pub(crate) fn probe_terminal_shell_path_core(path: &str) -> bool {
    shell_exists(path)
}

pub(crate) async fn load_system_terminal_settings(
    conn: &DatabaseConnection,
) -> Result<SystemTerminalSettings, AppCommandError> {
    let raw = app_metadata_service::get_value(conn, SYSTEM_TERMINAL_SETTINGS_KEY)
        .await
        .map_err(AppCommandError::from)?;

    let Some(raw) = raw else {
        return Ok(SystemTerminalSettings::default());
    };

    let parsed = serde_json::from_str::<SystemTerminalSettings>(&raw).map_err(|e| {
        AppCommandError::configuration_invalid("Failed to parse stored terminal settings")
            .with_detail(e.to_string())
    })?;

    Ok(normalize_terminal_settings(parsed))
}

/// Load the persisted shell selection into the live ACP terminal runtime.
///
/// This runs during app startup; a failure leaves the runtime on its system
/// fallback so a malformed old preference cannot prevent agents from running.
pub async fn apply_persisted_terminal_shell_config(
    conn: &DatabaseConnection,
    config: &TerminalShellRuntimeConfig,
) {
    match load_system_terminal_settings(conn).await {
        Ok(settings) => config.set(settings.default_shell).await,
        Err(err) => tracing::warn!(
            "[settings] failed to load default terminal shell for ACP runtime: {err}"
        ),
    }
}

/// Persist, apply, and broadcast the default shell in one path shared by the
/// desktop command and web handler.
pub(crate) async fn set_system_terminal_settings_core(
    conn: &DatabaseConnection,
    config: &TerminalShellRuntimeConfig,
    emitter: &crate::web::event_bridge::EventEmitter,
    settings: SystemTerminalSettings,
) -> Result<SystemTerminalSettings, AppCommandError> {
    let normalized = normalize_terminal_settings(settings);
    let serialized = serde_json::to_string(&normalized).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize terminal settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(conn, SYSTEM_TERMINAL_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    // Update the shared handle before notifying the frontend, so an already
    // connected model can issue its next terminal request with the new shell.
    config.set(normalized.default_shell.clone()).await;
    crate::web::event_bridge::emit_event(
        emitter,
        TERMINAL_SETTINGS_UPDATED_EVENT,
        normalized.clone(),
    );

    Ok(normalized)
}

#[cfg(feature = "tauri-runtime")]
pub(crate) async fn load_system_close_settings(
    conn: &DatabaseConnection,
) -> Result<SystemCloseSettings, AppCommandError> {
    let raw = app_metadata_service::get_value(conn, SYSTEM_CLOSE_SETTINGS_KEY)
        .await
        .map_err(AppCommandError::from)?;

    let Some(raw) = raw else {
        return Ok(SystemCloseSettings::default());
    };

    serde_json::from_str::<SystemCloseSettings>(&raw).map_err(|e| {
        AppCommandError::configuration_invalid("Failed to parse stored close settings")
            .with_detail(e.to_string())
    })
}

/// Cached copy of the persisted `CloseAction`, so the window's `CloseRequested`
/// handler can decide synchronously. That callback runs on the GUI event-loop
/// thread, where a `block_on` DB read would hold the whole UI for as long as
/// SQLite takes to answer — a `busy_timeout` retry budget plus pool
/// `connect_timeout` — for a value that only changes when the user changes it.
///
/// Follows the same load-once-then-cache shape as `CACHED_APPEARANCE_MODE`:
/// primed at startup by `prime_close_settings_cache`, refreshed by
/// `update_system_close_settings`.
#[cfg(feature = "tauri-runtime")]
static CACHED_CLOSE_ACTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(
    // Pre-startup fallback: matches `CloseAction::default()` for this platform
    // in case a close somehow arrives before the cache is primed.
    !cfg!(target_os = "linux"),
);

#[cfg(feature = "tauri-runtime")]
fn store_cached_close_action(action: CloseAction) {
    CACHED_CLOSE_ACTION.store(
        action == CloseAction::HideToTray,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The persisted close action, readable from a synchronous context.
#[cfg(feature = "tauri-runtime")]
pub fn cached_close_action() -> CloseAction {
    if CACHED_CLOSE_ACTION.load(std::sync::atomic::Ordering::Relaxed) {
        CloseAction::HideToTray
    } else {
        CloseAction::Exit
    }
}

/// Prime the cache during setup, next to the other persisted-settings loads.
#[cfg(feature = "tauri-runtime")]
pub(crate) async fn prime_close_settings_cache(conn: &DatabaseConnection) {
    match load_system_close_settings(conn).await {
        Ok(settings) => store_cached_close_action(settings.action),
        Err(err) => {
            // Keep the platform default rather than failing startup; the user
            // can re-pick in settings, which rewrites the row.
            tracing::warn!("[Close] failed to load close settings, using default: {err}");
        }
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
pub async fn get_system_terminal_settings(
    db: State<'_, AppDatabase>,
) -> Result<SystemTerminalSettings, AppCommandError> {
    load_system_terminal_settings(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_close_settings(
    db: State<'_, AppDatabase>,
) -> Result<SystemCloseSettingsInfo, AppCommandError> {
    let settings = load_system_close_settings(&db.conn).await?;
    Ok(SystemCloseSettingsInfo {
        action: settings.action,
        // Probed per read rather than cached: the user may have installed a
        // tray extension or restarted their panel since launch, and opening
        // this page is exactly when a stale answer would mislead them.
        tray_available: crate::commands::windows::tray_probably_visible(),
    })
}

/// Serializes overlapping update_system_close_settings calls so the cache always
/// reflects the last successfully committed value, never an intermediate state.
#[cfg(feature = "tauri-runtime")]
static CLOSE_SETTINGS_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_close_settings(
    settings: SystemCloseSettings,
    db: State<'_, AppDatabase>,
) -> Result<SystemCloseSettingsInfo, AppCommandError> {
    let lock = CLOSE_SETTINGS_WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().await;

    let serialized = serde_json::to_string(&settings).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize close settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(&db.conn, SYSTEM_CLOSE_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    // Only after the write lands, so a failed write leaves the close handler
    // acting on the value that is actually stored. The mutex ensures concurrent
    // writes commit and update the cache in the same order.
    store_cached_close_action(settings.action);

    // Drop the lock before the tray probe, which can block for up to 2 seconds
    // on Linux. This prevents subsequent writes from being delayed by an
    // advisory check that doesn't affect correctness.
    drop(_guard);

    Ok(SystemCloseSettingsInfo {
        action: settings.action,
        tray_available: crate::commands::windows::tray_probably_visible(),
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_available_terminal_shells() -> Result<AvailableTerminalShells, AppCommandError> {
    Ok(build_available_terminal_shells())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn probe_terminal_shell_path(path: String) -> Result<bool, AppCommandError> {
    Ok(probe_terminal_shell_path_core(&path))
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
pub async fn update_system_terminal_settings(
    settings: SystemTerminalSettings,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
    manager: State<'_, ConnectionManager>,
) -> Result<SystemTerminalSettings, AppCommandError> {
    let config = manager.terminal_shell_config();
    let emitter = crate::web::event_bridge::EventEmitter::Tauri(app);
    set_system_terminal_settings_core(&db.conn, &config, &emitter, settings).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;
    use crate::web::event_bridge::EventEmitter;

    #[tokio::test]
    async fn terminal_shell_setting_persists_and_updates_live_runtime() {
        let db = fresh_in_memory_db().await;
        let config = TerminalShellRuntimeConfig::new();

        let saved = set_system_terminal_settings_core(
            &db.conn,
            &config,
            &EventEmitter::Noop,
            SystemTerminalSettings {
                default_shell: Some("  pwsh.exe  ".to_string()),
            },
        )
        .await
        .expect("save terminal setting");

        assert_eq!(saved.default_shell.as_deref(), Some("pwsh.exe"));
        assert_eq!(config.snapshot().await.as_deref(), Some("pwsh.exe"));

        let restarted_config = TerminalShellRuntimeConfig::new();
        apply_persisted_terminal_shell_config(&db.conn, &restarted_config).await;
        assert_eq!(
            restarted_config.snapshot().await.as_deref(),
            Some("pwsh.exe")
        );
    }
}

#[cfg(all(test, feature = "tauri-runtime"))]
mod close_settings_tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    /// A fresh install has no row, and must not error into "no preference".
    #[tokio::test]
    async fn missing_key_falls_back_to_the_platform_default() {
        let db = fresh_in_memory_db().await;

        let settings = load_system_close_settings(&db.conn)
            .await
            .expect("a missing key is not an error");

        assert_eq!(settings, SystemCloseSettings::default());
        // Linux users keep the pre-setting behaviour unless they opt in.
        if cfg!(target_os = "linux") {
            assert_eq!(settings.action, CloseAction::Exit);
        } else {
            assert_eq!(settings.action, CloseAction::HideToTray);
        }
    }

    /// Garbage in the row is surfaced rather than silently read as a default —
    /// the close handler falls back on its own, but a caller asking for the
    /// stored value deserves to know it could not be read.
    #[tokio::test]
    async fn malformed_json_is_an_error() {
        let db = fresh_in_memory_db().await;
        app_metadata_service::upsert_value(&db.conn, SYSTEM_CLOSE_SETTINGS_KEY, "{not json")
            .await
            .expect("seed the row");

        let err = load_system_close_settings(&db.conn)
            .await
            .expect_err("malformed JSON must not read as a default");

        assert!(
            err.to_string().contains("close settings"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn both_actions_round_trip_through_the_row() {
        let db = fresh_in_memory_db().await;

        for action in [CloseAction::Exit, CloseAction::HideToTray] {
            let serialized =
                serde_json::to_string(&SystemCloseSettings { action }).expect("serialize");
            app_metadata_service::upsert_value(&db.conn, SYSTEM_CLOSE_SETTINGS_KEY, &serialized)
                .await
                .expect("persist");

            let reloaded = load_system_close_settings(&db.conn).await.expect("reload");
            assert_eq!(reloaded.action, action);
        }
    }

    /// The wire form is what the TypeScript mirror declares.
    #[test]
    fn actions_serialize_in_snake_case() {
        assert_eq!(
            serde_json::to_string(&CloseAction::HideToTray).unwrap(),
            "\"hide_to_tray\""
        );
        assert_eq!(
            serde_json::to_string(&CloseAction::Exit).unwrap(),
            "\"exit\""
        );
    }

    /// The close handler reads this cache instead of the DB, so a stale value
    /// here means the close button ignores what the user picked.
    ///
    /// One test rather than several: the cache is process-global, and separate
    /// tests would race each other under the parallel test runner.
    #[tokio::test]
    async fn the_cache_tracks_writes_and_the_persisted_value() {
        for action in [CloseAction::Exit, CloseAction::HideToTray, CloseAction::Exit] {
            store_cached_close_action(action);
            assert_eq!(cached_close_action(), action);
        }

        let db = fresh_in_memory_db().await;
        let serialized = serde_json::to_string(&SystemCloseSettings {
            action: CloseAction::HideToTray,
        })
        .expect("serialize");
        app_metadata_service::upsert_value(&db.conn, SYSTEM_CLOSE_SETTINGS_KEY, &serialized)
            .await
            .expect("persist");

        store_cached_close_action(CloseAction::Exit);
        prime_close_settings_cache(&db.conn).await;
        assert_eq!(cached_close_action(), CloseAction::HideToTray);
    }
}
