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
use crate::models::{SystemAutostartSettings, SystemRenderingSettings};
use crate::models::{
    AvailableTerminalShells, SystemLanguageSettings, SystemProxySettings, SystemTerminalSettings,
    TerminalShellOption,
};
#[cfg(feature = "tauri-runtime")]
use crate::network::proxy;
#[cfg(feature = "tauri-runtime")]
use crate::preferences;
use crate::terminal::manager::resolve_shell;

pub(crate) const SYSTEM_PROXY_SETTINGS_KEY: &str = "system_proxy_settings";
pub(crate) const SYSTEM_LANGUAGE_SETTINGS_KEY: &str = "system_language_settings";
pub(crate) const SYSTEM_TERMINAL_SETTINGS_KEY: &str = "system_terminal_settings";
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

/// Reach for the manager through `try_state` rather than the plugin's
/// `autolaunch()` extension trait, which is `state::<AutoLaunchManager>()` and
/// panics when the state is absent.
///
/// In a running app the state is always there, so this `Err` arm is unreachable
/// rather than a degraded mode the UI should expect: the plugin registers the
/// manager from its setup hook, `initialize_plugins` propagates a failing hook
/// out of `Builder::build`, and `run()` unwraps that — a `current_exe()` failure
/// kills the process at startup instead of leaving it running without autostart.
/// Keeping the `Result` costs nothing and holds if that contract ever changes.
/// The error paths users *can* reach are the `is_enabled` / `enable` / `disable`
/// calls below (e.g. a locked-down registry).
#[cfg(feature = "tauri-runtime")]
fn autolaunch_manager(
    app: &tauri::AppHandle,
) -> Result<tauri::State<'_, tauri_plugin_autostart::AutoLaunchManager>, AppCommandError> {
    use tauri::Manager;

    app.try_state::<tauri_plugin_autostart::AutoLaunchManager>()
        .ok_or_else(|| {
            AppCommandError::configuration_missing("Launch at login is unavailable on this system")
        })
}

/// The slice of the autostart plugin's manager the sequencing below needs.
///
/// It exists to make that sequencing testable. The ordering rules encode
/// platform behaviour — above all that Windows' `disable` errors on a Run value
/// that isn't there — which a macOS or Linux CI box can never exercise against
/// the real backend, and which the commands themselves can't be called into
/// without a live `tauri::AppHandle`.
///
/// Named `register`/`unregister` rather than mirroring the manager's
/// `enable`/`disable`/`is_enabled` on purpose: same-named trait and inherent
/// methods would let a later edit inside the impl below resolve to the trait
/// method and recurse forever.
#[cfg(feature = "tauri-runtime")]
trait AutostartBackend {
    fn is_registered(&self) -> Result<bool, String>;
    fn register(&self) -> Result<(), String>;
    fn unregister(&self) -> Result<(), String>;
}

#[cfg(feature = "tauri-runtime")]
impl AutostartBackend for tauri_plugin_autostart::AutoLaunchManager {
    fn is_registered(&self) -> Result<bool, String> {
        self.is_enabled().map_err(|err| err.to_string())
    }

    fn register(&self) -> Result<(), String> {
        self.enable().map_err(|err| err.to_string())
    }

    fn unregister(&self) -> Result<(), String> {
        self.disable().map_err(|err| err.to_string())
    }
}

#[cfg(feature = "tauri-runtime")]
fn read_autostart_setting(
    backend: &impl AutostartBackend,
) -> Result<SystemAutostartSettings, AppCommandError> {
    let enabled = backend.is_registered().map_err(|err| {
        AppCommandError::io_error("Failed to read the launch-at-login state").with_detail(err)
    })?;
    Ok(SystemAutostartSettings { enabled })
}

#[cfg(feature = "tauri-runtime")]
fn apply_autostart_setting(
    backend: &impl AutostartBackend,
    desired: bool,
) -> Result<SystemAutostartSettings, AppCommandError> {
    if desired {
        // Unconditional: `register` overwrites the entry on every platform, so
        // re-running it also repairs a registration left pointing at a stale
        // executable path (app moved or reinstalled elsewhere).
        backend.register().map_err(|err| {
            AppCommandError::io_error("Failed to enable launch at login").with_detail(err)
        })?;
    } else if read_autostart_setting(backend)?.enabled {
        // Guarded: on Windows `unregister` deletes the registry value and
        // errors when it isn't there, so turning off an already-off entry
        // would fail.
        backend.unregister().map_err(|err| {
            AppCommandError::io_error("Failed to disable launch at login").with_detail(err)
        })?;
    }

    // Report what the OS ends up holding rather than what was asked for: on
    // Windows the Task Manager Startup tab can veto the registry entry.
    read_autostart_setting(backend)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_autostart_settings(
    app: tauri::AppHandle,
) -> Result<SystemAutostartSettings, AppCommandError> {
    read_autostart_setting(&*autolaunch_manager(&app)?)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_autostart_settings(
    settings: SystemAutostartSettings,
    app: tauri::AppHandle,
) -> Result<SystemAutostartSettings, AppCommandError> {
    apply_autostart_setting(&*autolaunch_manager(&app)?, settings.enabled)
}

#[cfg(all(test, feature = "tauri-runtime"))]
mod autostart_tests {
    use std::cell::RefCell;

    use super::{apply_autostart_setting, AutostartBackend};

    /// Records the calls the sequencing makes, and lets a test pretend the OS
    /// disagreed with the request — which is what Windows does when the Task
    /// Manager Startup tab has vetoed the Run entry.
    #[derive(Default)]
    struct FakeBackend {
        registered: RefCell<bool>,
        calls: RefCell<Vec<&'static str>>,
        /// `register` succeeds but leaves the entry off, as a veto would.
        veto_register: bool,
        fail: Option<&'static str>,
    }

    impl FakeBackend {
        fn new(registered: bool) -> Self {
            Self {
                registered: RefCell::new(registered),
                ..Default::default()
            }
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    impl AutostartBackend for FakeBackend {
        fn is_registered(&self) -> Result<bool, String> {
            self.calls.borrow_mut().push("is_registered");
            if self.fail == Some("is_registered") {
                return Err("registry unreadable".into());
            }
            Ok(*self.registered.borrow())
        }

        fn register(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("register");
            if self.fail == Some("register") {
                return Err("access denied".into());
            }
            if !self.veto_register {
                *self.registered.borrow_mut() = true;
            }
            Ok(())
        }

        fn unregister(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("unregister");
            if self.fail == Some("unregister") {
                return Err("value not found".into());
            }
            *self.registered.borrow_mut() = false;
            Ok(())
        }
    }

    /// Turning it on always rewrites the entry. That is what repairs a
    /// registration still pointing at an executable the user has since moved,
    /// so "already on" must not short-circuit into doing nothing.
    #[test]
    fn enabling_rewrites_the_entry_even_when_already_enabled() {
        let backend = FakeBackend::new(true);

        let result = apply_autostart_setting(&backend, true).expect("enable");

        assert!(result.enabled);
        assert!(backend.calls().contains(&"register"));
    }

    /// The Windows guard: `disable` there deletes the Run value and errors when
    /// it is absent, so turning off an already-off entry must not call it. This
    /// is the assertion no macOS or Linux CI box can make against the real
    /// backend, which is the whole reason the trait exists.
    #[test]
    fn disabling_an_already_disabled_entry_never_calls_unregister() {
        let backend = FakeBackend::new(false);

        let result = apply_autostart_setting(&backend, false).expect("disable");

        assert!(!result.enabled);
        assert!(!backend.calls().contains(&"unregister"));
    }

    #[test]
    fn disabling_an_enabled_entry_unregisters_it() {
        let backend = FakeBackend::new(true);

        let result = apply_autostart_setting(&backend, false).expect("disable");

        assert!(!result.enabled);
        assert!(backend.calls().contains(&"unregister"));
    }

    /// The reply is the OS's answer, not an echo of the request: a vetoed
    /// registration has to come back as `false` so the switch shows what will
    /// actually happen at login.
    #[test]
    fn reports_the_state_the_os_settled_on_not_the_request() {
        let backend = FakeBackend {
            veto_register: true,
            ..FakeBackend::new(false)
        };

        let result = apply_autostart_setting(&backend, true).expect("enable");

        assert!(backend.calls().contains(&"register"));
        assert!(!result.enabled, "a vetoed entry must not report as enabled");
    }

    #[test]
    fn a_failing_backend_surfaces_an_error_with_the_os_detail() {
        let backend = FakeBackend {
            fail: Some("register"),
            ..FakeBackend::new(false)
        };

        let err = apply_autostart_setting(&backend, true).expect_err("should fail");

        assert_eq!(err.detail.as_deref(), Some("access denied"));
    }
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
