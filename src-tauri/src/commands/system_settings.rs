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
use crate::models::{
    CloseWindowBehavior, SystemAutostartSettings, SystemCloseBehaviorSettings,
    SystemCloseBehaviorSettingsView, SystemRenderingSettings,
};
use crate::models::{
    AvailableTerminalShells, SystemLanguageSettings, SystemProxySettings, SystemTerminalSettings,
    TerminalShellOption,
};
use crate::network::proxy;
#[cfg(feature = "tauri-runtime")]
use crate::preferences;
use crate::terminal::manager::resolve_shell;

pub(crate) const SYSTEM_PROXY_SETTINGS_KEY: &str = "system_proxy_settings";
pub(crate) const SYSTEM_LANGUAGE_SETTINGS_KEY: &str = "system_language_settings";
pub(crate) const SYSTEM_TERMINAL_SETTINGS_KEY: &str = "system_terminal_settings";
#[cfg(feature = "tauri-runtime")]
pub(crate) const SYSTEM_CLOSE_BEHAVIOR_SETTINGS_KEY: &str = "system_close_behavior_settings";
#[cfg(feature = "tauri-runtime")]
pub(crate) const CLOSE_REQUEST_EVENT: &str = "app://close-request";
pub(crate) const LANGUAGE_SETTINGS_UPDATED_EVENT: &str = "app://language-settings-updated";
pub(crate) const TERMINAL_SETTINGS_UPDATED_EVENT: &str = "app://terminal-settings-updated";

pub(crate) const TERMINAL_SHELL_OPTION_SYSTEM: &str = "system";
pub(crate) const TERMINAL_SHELL_OPTION_CUSTOM: &str = "custom";

/// Trim, validate, and canonicalize proxy settings. Shared by the save path,
/// the load path, and the web handler so all three agree on what gets stored.
///
/// Enabling the proxy rewrites the address into one that carries an explicit
/// scheme (see [`proxy::normalize_proxy_url`]) — the stored value, the value
/// echoed back to the settings page, and the value exported to child processes
/// are then the same string. Because the load path normalizes too, a row saved
/// by an older build with a bare `host:port` heals on read; no migration.
pub(crate) fn normalize_proxy_settings(
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

    Ok(SystemProxySettings {
        enabled: true,
        proxy_url: Some(proxy::normalize_proxy_url(proxy_url)?),
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
        // Nothing to canonicalize on a bool; carried explicitly so adding a
        // field here can never silently drop it back to the default.
        colorize_command_output: settings.colorize_command_output,
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

/// Load the persisted terminal settings into the live runtimes: the shell
/// selection into the ACP terminal runtime, and the command-color opt-in into
/// the launch env (`crate::acp::connection::set_force_command_color`).
///
/// This runs during app startup; a failure leaves the runtime on its system
/// fallback so a malformed old preference cannot prevent agents from running.
/// Both live values are applied from ONE load — they share a stored row, and
/// reading it twice would let a save land between the two reads.
pub async fn apply_persisted_terminal_settings(
    conn: &DatabaseConnection,
    config: &TerminalShellRuntimeConfig,
) {
    match load_system_terminal_settings(conn).await {
        Ok(settings) => {
            crate::acp::connection::set_force_command_color(settings.colorize_command_output);
            config.set(settings.default_shell).await;
        }
        // Both live values stay on their process defaults — system shell, and
        // color off. Naming only the shell here would send whoever reads this
        // log looking for a second, non-existent failure when the colored
        // transcript they opted into also fails to show up.
        Err(err) => tracing::warn!(
            "[settings] failed to load terminal settings (default shell, command color) for ACP runtime: {err}"
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

    // Update the shared handles before notifying the frontend, so an already
    // connected model can issue its next terminal request with the new shell.
    // The color flag only reaches a launch's env, so it lands on the NEXT
    // connection rather than the running one.
    crate::acp::connection::set_force_command_color(normalized.colorize_command_output);
    config.set(normalized.default_shell.clone()).await;
    crate::web::event_bridge::emit_event(
        emitter,
        TERMINAL_SETTINGS_UPDATED_EVENT,
        normalized.clone(),
    );

    Ok(normalized)
}

// --- Close window behavior ---

/// Mirror of [`CloseWindowBehavior`] for the atomic cache. The close handler is
/// a synchronous window callback with no runtime to await a query on, so the
/// preference has to be readable without touching the database.
#[cfg(feature = "tauri-runtime")]
mod close_behavior_code {
    pub const ASK: u8 = 0;
    pub const MINIMIZE: u8 = 1;
    pub const EXIT: u8 = 2;
}

#[cfg(feature = "tauri-runtime")]
static CLOSE_BEHAVIOR_CACHE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(close_behavior_code::ASK);

/// Whether a close prompt is already on screen. The close button stays clickable
/// while the dialog is up, and every click re-enters `CloseRequested` — without
/// this the user stacks a dialog per click and has to dismiss all of them.
#[cfg(feature = "tauri-runtime")]
static CLOSE_PROMPT_OPEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// When the outstanding claim was taken, so an unanswered one can expire.
#[cfg(feature = "tauri-runtime")]
static CLOSE_PROMPT_CLAIMED_AT: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

/// How long an unanswered claim keeps suppressing the close button.
///
/// Nothing in the protocol can prove a dialog actually appeared. `Emitter::emit*`
/// answers for the bus, not for a listener, and wry runs an eval inline when it
/// is issued from the main thread — which is where the close handler lives — so
/// an emit can even overtake a listener registration that is still queued on the
/// event-loop proxy. [`CLOSE_PROMPT_LISTENER_READY`] makes that rare; this makes
/// it recoverable, and covers the cases readiness cannot see at all: JS that died
/// after mounting, a dialog wedged mid-render, an emit dropped in flight.
///
/// Ten seconds is chosen against the two ways a second press reads: a
/// double-click or a moment's hesitation still means "the dialog is up, ignore
/// me", while a press this long after nothing visibly happened means the user is
/// asking again and deserves an answer.
#[cfg(feature = "tauri-runtime")]
const CLOSE_PROMPT_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether the main webview has a close-prompt listener up.
///
/// `main` is built visible, so the close button is clickable from the first
/// frame — seconds before React mounts `CloseRequestDialog` and subscribes.
/// Emitting into that gap is indistinguishable from a successful emit
/// (`Emitter::emit*` reports delivery to the bus, not to a listener), so the
/// press would land in a window that simply does not react. Until the dialog
/// has proved it exists, the close button behaves the way it did before the
/// preference existed.
#[cfg(feature = "tauri-runtime")]
static CLOSE_PROMPT_LISTENER_READY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Payload of [`CLOSE_REQUEST_EVENT`]. One event covers both prompts so the
/// frontend has a single listener and the backend a single de-dup flag.
#[cfg(feature = "tauri-runtime")]
#[derive(Debug, Clone, serde::Serialize)]
pub struct CloseRequestPayload {
    /// `"ask"` — offer both actions plus "remember my choice".
    /// `"confirm_terminals"` — the action is already decided; confirm the loss.
    pub mode: &'static str,
    pub running_terminals: usize,
}

#[cfg(feature = "tauri-runtime")]
pub(crate) fn cached_close_behavior() -> CloseWindowBehavior {
    match CLOSE_BEHAVIOR_CACHE.load(std::sync::atomic::Ordering::Relaxed) {
        close_behavior_code::MINIMIZE => CloseWindowBehavior::Minimize,
        close_behavior_code::EXIT => CloseWindowBehavior::Exit,
        _ => CloseWindowBehavior::Ask,
    }
}

#[cfg(feature = "tauri-runtime")]
pub(crate) fn store_close_behavior_cache(behavior: CloseWindowBehavior) {
    let code = match behavior {
        CloseWindowBehavior::Ask => close_behavior_code::ASK,
        CloseWindowBehavior::Minimize => close_behavior_code::MINIMIZE,
        CloseWindowBehavior::Exit => close_behavior_code::EXIT,
    };
    CLOSE_BEHAVIOR_CACHE.store(code, std::sync::atomic::Ordering::Relaxed);
}

/// Never returns an error. The close button is the user's last exit; a row this
/// build cannot parse must degrade to asking, not to a window that refuses to
/// close.
#[cfg(feature = "tauri-runtime")]
pub(crate) async fn load_system_close_behavior_settings(
    conn: &DatabaseConnection,
) -> SystemCloseBehaviorSettings {
    let raw = match app_metadata_service::get_value(conn, SYSTEM_CLOSE_BEHAVIOR_SETTINGS_KEY).await {
        Ok(Some(raw)) => raw,
        Ok(None) => return SystemCloseBehaviorSettings::default(),
        Err(err) => {
            tracing::warn!("[settings] failed to read close behavior, defaulting to ask: {err}");
            return SystemCloseBehaviorSettings::default();
        }
    };

    match serde_json::from_str::<SystemCloseBehaviorSettings>(&raw) {
        Ok(settings) => settings,
        Err(err) => {
            tracing::warn!("[settings] failed to parse close behavior, defaulting to ask: {err}");
            SystemCloseBehaviorSettings::default()
        }
    }
}

/// Writes the row, then the cache. In that order: a failed write must not leave
/// the running process obeying a preference the next launch will not remember.
#[cfg(feature = "tauri-runtime")]
pub(crate) async fn save_system_close_behavior_settings(
    conn: &DatabaseConnection,
    behavior: CloseWindowBehavior,
) -> Result<SystemCloseBehaviorSettings, AppCommandError> {
    let settings = SystemCloseBehaviorSettings { behavior };
    let serialized = serde_json::to_string(&settings).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize close behavior settings")
            .with_detail(e.to_string())
    })?;

    app_metadata_service::upsert_value(conn, SYSTEM_CLOSE_BEHAVIOR_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    store_close_behavior_cache(behavior);
    Ok(settings)
}

/// Seeds the atomic at startup. The cache starts at its default every launch,
/// so without this a user who picked "exit" months ago would be asked again.
#[cfg(feature = "tauri-runtime")]
pub async fn apply_persisted_close_behavior(conn: &DatabaseConnection) {
    store_close_behavior_cache(load_system_close_behavior_settings(conn).await.behavior);
}

/// Gives the claim back when the prompt could not be delivered. Without it a
/// failed emit would leave the flag set and the close button permanently dead.
#[cfg(feature = "tauri-runtime")]
pub(crate) fn release_close_prompt() {
    *CLOSE_PROMPT_CLAIMED_AT.lock().unwrap() = None;
    CLOSE_PROMPT_OPEN.store(false, std::sync::atomic::Ordering::Release);
}

/// Whether a prompt emitted now would reach a dialog.
///
/// See [`CLOSE_PROMPT_LISTENER_READY`]. One-way: nothing lowers it, because a
/// listener that answered once is the best evidence available that the webview
/// is alive, and a false negative costs the user the prompt they asked for.
#[cfg(feature = "tauri-runtime")]
pub(crate) fn close_prompt_listener_ready() -> bool {
    CLOSE_PROMPT_LISTENER_READY.load(std::sync::atomic::Ordering::Acquire)
}

/// Raised by [`resolve_close_request`] — the only call the dialog makes, and
/// one it makes on mount as well as on every answer.
#[cfg(feature = "tauri-runtime")]
pub(crate) fn mark_close_prompt_listener_ready() {
    CLOSE_PROMPT_LISTENER_READY.store(true, std::sync::atomic::Ordering::Release);
}

/// What one close press found when it went to open a prompt.
#[cfg(feature = "tauri-runtime")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClosePromptClaim {
    /// Nothing was outstanding; this press owns the prompt.
    Granted,
    /// A prompt is up and the user has not answered yet. This press is the
    /// second click on a button whose dialog is already on screen.
    AlreadyOpen,
    /// The outstanding claim went unanswered past [`CLOSE_PROMPT_GRACE`], so the
    /// prompt it belonged to never reached anyone. The claim has been dropped;
    /// the caller should act on the preference rather than wait for a dialog
    /// that is not coming.
    Expired,
}

/// Claims the right to show one close prompt.
///
/// The expiry is what makes the close button impossible to wedge: whatever goes
/// wrong between the emit and the dialog, the press after the grace acts.
#[cfg(feature = "tauri-runtime")]
pub(crate) fn try_open_close_prompt() -> ClosePromptClaim {
    let mut claimed_at = CLOSE_PROMPT_CLAIMED_AT.lock().unwrap();

    if CLOSE_PROMPT_OPEN
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_ok()
    {
        *claimed_at = Some(std::time::Instant::now());
        return ClosePromptClaim::Granted;
    }

    // A claim with no timestamp is one already being torn down by
    // `release_close_prompt`; treat it as live rather than racing it.
    let expired = claimed_at
        .map(|at| at.elapsed() >= CLOSE_PROMPT_GRACE)
        .unwrap_or(false);
    if !expired {
        return ClosePromptClaim::AlreadyOpen;
    }

    tracing::warn!(
        "[close] close prompt went unanswered for {}s; treating it as undelivered",
        CLOSE_PROMPT_GRACE.as_secs()
    );
    *claimed_at = None;
    CLOSE_PROMPT_OPEN.store(false, std::sync::atomic::Ordering::Release);
    ClosePromptClaim::Expired
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_system_close_behavior_settings(
    db: State<'_, AppDatabase>,
) -> Result<SystemCloseBehaviorSettingsView, AppCommandError> {
    let settings = load_system_close_behavior_settings(&db.conn).await;
    Ok(SystemCloseBehaviorSettingsView {
        behavior: settings.behavior,
        tray_available: crate::commands::windows::can_hide_to_tray(),
    })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_system_close_behavior_settings(
    behavior: CloseWindowBehavior,
    db: State<'_, AppDatabase>,
) -> Result<SystemCloseBehaviorSettingsView, AppCommandError> {
    let settings = save_system_close_behavior_settings(&db.conn, behavior).await?;
    Ok(SystemCloseBehaviorSettingsView {
        behavior: settings.behavior,
        tray_available: crate::commands::windows::can_hide_to_tray(),
    })
}

/// Carries out what the user picked in the close prompt.
///
/// `action` is `"minimize"`, `"exit"`, or `"cancel"`. `remember` pins the
/// choice as the preference; it is ignored for `"cancel"`, which expresses no
/// preference about future closes.
///
/// Releasing the prompt flag is the FIRST thing this does, so a persistence
/// failure below cannot leave the flag stuck and the close button dead.
///
/// Doubles as the dialog's "I am listening" signal — it is the one call the
/// dialog makes, and it makes it on mount (with `"cancel"`, to clear a flag a
/// webview reload left behind) as well as on every answer. Reaching this
/// function at all therefore proves a listener exists, which is what
/// [`close_prompt_listener_ready`] reports and the close handler needs before
/// it is willing to hand a press to a dialog.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn resolve_close_request(
    action: String,
    remember: bool,
    db: State<'_, AppDatabase>,
    app: tauri::AppHandle,
) -> Result<(), AppCommandError> {
    // Release first, as above: a press racing in between then falls back to the
    // preference (readiness is still whatever it was) instead of being dropped
    // as a duplicate of a claim nobody holds any more.
    release_close_prompt();
    mark_close_prompt_listener_ready();

    let behavior = match action.as_str() {
        "minimize" => Some(CloseWindowBehavior::Minimize),
        "exit" => Some(CloseWindowBehavior::Exit),
        "cancel" => None,
        other => {
            return Err(AppCommandError::invalid_input(format!(
                "Unknown close action: {other}"
            )))
        }
    };

    let Some(behavior) = behavior else {
        return Ok(());
    };

    if remember {
        save_system_close_behavior_settings(&db.conn, behavior).await?;
    }

    // `orderOut:` and process exit both leave a macOS native-fullscreen
    // Space standing (issue #507), and this is a second entry point into
    // both: the press that raised the dialog may have arrived windowed and
    // the user can go fullscreen while it is up, so the answer cannot
    // assume the close path already drained.
    match behavior {
        CloseWindowBehavior::Minimize => {
            if let Some(window) = tauri::Manager::get_webview_window(&app, "main") {
                crate::commands::windows::with_macos_fullscreen_drained(&app, move || {
                    let _ = window.hide();
                });
            }
        }
        // Reuses the tray-quit path: `exit` triggers `ExitRequested`, which
        // sets `APP_QUITTING` and runs the ACP-disconnect / terminal-reclaim
        // cleanup already wired there.
        CloseWindowBehavior::Exit => {
            let quit = tauri::Manager::app_handle(&app).clone();
            crate::commands::windows::with_macos_fullscreen_drained(&app, move || quit.exit(0));
        }
        CloseWindowBehavior::Ask => {}
    }

    Ok(())
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
    app: tauri::AppHandle,
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
    crate::browser::profile::proxy_settings_changed(&app);
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
    use std::collections::BTreeMap;

    /// Every terminal-settings save writes `FORCE_COMMAND_COLOR`, a PROCESS
    /// global — so two of these tests running concurrently (the default) would
    /// have one clobber the flag the other is about to assert on. The clobber
    /// is not hypothetical: `set_system_terminal_settings_core` awaits between
    /// storing the flag and returning, which is exactly where the other test's
    /// store lands. Anything that saves or applies terminal settings holds this
    /// first.
    static TERMINAL_SETTINGS_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Restores `FORCE_COMMAND_COLOR` on the way out, including on a panic —
    /// a test that left it set would make the *next* run of the "off by
    /// default" assertion fail for reasons that have nothing to do with the
    /// code under test.
    struct RestoreCommandColor(bool);

    impl RestoreCommandColor {
        fn capture() -> Self {
            Self(crate::acp::connection::force_command_color_enabled())
        }
    }

    impl Drop for RestoreCommandColor {
        fn drop(&mut self) {
            crate::acp::connection::set_force_command_color(self.0);
        }
    }

    /// The command-color variables a REAL launch env carries right now.
    ///
    /// The setting only matters if it survives the trip from the stored row
    /// through the process global into the env a spawn actually gets, and the
    /// step joining those — `merge_agent_env` reading the global — is the one
    /// place the pure-function tests in `acp::connection` cannot reach. Any
    /// launch would do; Antigravity's is the one exposed as a `pub fn`, and it
    /// merges through the same helper as every other agent.
    ///
    /// Returns the whole set rather than a yes/no so both directions are exact:
    /// "on" has to produce every variable (they cover disjoint decisions —
    /// `CLICOLOR` enables the BSD family, `CLICOLOR_FORCE` waives its `isatty`
    /// check, `FORCE_COLOR` covers the npm one, `TERM` feeds the terminfo lookup
    /// — so a launch carrying only some of them is a failure, not a partial
    /// success), and "off" has to produce none. A boolean over `all()` would let
    /// the off case pass while leaking one of them.
    fn launch_env_color_vars() -> BTreeMap<String, String> {
        crate::acp::connection::antigravity_launch_env(&BTreeMap::new(), None)
            .into_iter()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "CLICOLOR" | "CLICOLOR_FORCE" | "FORCE_COLOR" | "TERM"
                )
            })
            .collect()
    }

    fn expected_color_vars() -> BTreeMap<String, String> {
        [
            ("CLICOLOR", "1"),
            ("CLICOLOR_FORCE", "1"),
            ("FORCE_COLOR", "1"),
            ("TERM", "xterm-256color"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
    }

    fn enabled_proxy(url: &str) -> SystemProxySettings {
        SystemProxySettings {
            enabled: true,
            proxy_url: Some(url.to_string()),
        }
    }

    fn normalized_url(url: &str) -> String {
        normalize_proxy_settings(enabled_proxy(url))
            .expect("proxy url should be accepted")
            .proxy_url
            .expect("enabled proxy keeps its url")
    }

    /// A scheme-less address is what a user actually types, and what used to
    /// reach `HTTP_PROXY` verbatim — killing every npm-based agent install with
    /// `ERR_INVALID_URL` while codeg's own reqwest calls kept working.
    #[test]
    fn scheme_less_proxy_addresses_gain_an_http_scheme() {
        assert_eq!(normalized_url("127.0.0.1:7890"), "http://127.0.0.1:7890");
        // Parses as a URL whose *scheme* is `localhost` — so "did it parse" is
        // not a usable test; only the missing host gives it away.
        assert_eq!(normalized_url("localhost:7890"), "http://localhost:7890");
        assert_eq!(
            normalized_url("proxy.corp.com:8080"),
            "http://proxy.corp.com:8080"
        );
        assert_eq!(
            normalized_url("user:pass@127.0.0.1:7890"),
            "http://user:pass@127.0.0.1:7890"
        );
        // An IPv6 literal keeps its brackets — without them the address is
        // indistinguishable from a host and a port.
        assert_eq!(normalized_url("[::1]:7890"), "http://[::1]:7890");
        assert_eq!(normalized_url("  127.0.0.1:7890  "), "http://127.0.0.1:7890");
    }

    /// The repaired value is the user's own string with a prefix, never the
    /// `url` crate's re-serialization — which would append a trailing slash and
    /// churn what the settings field shows back.
    #[test]
    fn normalizing_does_not_reserialize_the_address() {
        assert_eq!(normalized_url("127.0.0.1:7890/"), "http://127.0.0.1:7890/");
        assert_eq!(
            normalized_url("proxy.corp.com:8080/gateway"),
            "http://proxy.corp.com:8080/gateway"
        );
        assert_eq!(normalized_url("http://127.0.0.1:7890"), "http://127.0.0.1:7890");
    }

    /// Re-running normalization over its own output must be a no-op: the value
    /// is normalized once on save and again on env export.
    #[test]
    fn normalizing_is_idempotent() {
        for url in ["127.0.0.1:7890", "[::1]:7890", "socks5://127.0.0.1:1080"] {
            let once = normalized_url(url);
            assert_eq!(normalized_url(&once), once, "{url} re-normalized");
        }
    }

    /// An address that already names a scheme must survive untouched — most of
    /// all a socks proxy, which would stop working if rewritten to `http://`.
    #[test]
    fn proxy_addresses_with_a_scheme_are_left_alone() {
        for url in [
            "http://127.0.0.1:7890",
            "https://proxy.corp.com:8443",
            "socks5://127.0.0.1:1080",
            "socks5h://127.0.0.1:1080",
        ] {
            assert_eq!(normalized_url(url), url);
        }
    }

    /// The repair must not launder a malformed address into a parseable one:
    /// `http://` has a scheme separator but no host, so prefixing it would
    /// produce `http://http://` and quietly accept it.
    #[test]
    fn malformed_addresses_are_still_rejected() {
        for url in ["http://", "socks5://", "http://:7890"] {
            assert!(
                normalize_proxy_settings(enabled_proxy(url)).is_err(),
                "{url} should not be accepted"
            );
        }
    }

    #[test]
    fn enabling_the_proxy_still_requires_an_address() {
        for settings in [
            SystemProxySettings {
                enabled: true,
                proxy_url: None,
            },
            enabled_proxy("   "),
        ] {
            assert!(normalize_proxy_settings(settings).is_err());
        }
    }

    /// Disabling keeps the stored address as typed: it is only a remembered
    /// value at that point, not something exported to a child process.
    #[test]
    fn disabled_proxy_keeps_its_address_unchanged() {
        let normalized = normalize_proxy_settings(SystemProxySettings {
            enabled: false,
            proxy_url: Some("  127.0.0.1:7890  ".to_string()),
        })
        .expect("disabled settings never validate the url");

        assert!(!normalized.enabled);
        assert_eq!(normalized.proxy_url.as_deref(), Some("127.0.0.1:7890"));
    }

    /// What actually reaches `HTTP_PROXY` and every spawned agent.
    #[test]
    fn exported_env_value_carries_the_scheme() {
        assert_eq!(
            proxy::proxy_env_value(&enabled_proxy("127.0.0.1:7890")).expect("normalizes"),
            Some("http://127.0.0.1:7890".to_string())
        );
        assert_eq!(
            proxy::proxy_env_value(&SystemProxySettings {
                enabled: false,
                proxy_url: Some("127.0.0.1:7890".to_string()),
            })
            .expect("disabled is not an error"),
            None
        );
    }

    /// Rows written by a build that stored the address verbatim must heal on
    /// read, so startup exports a usable value without a migration.
    #[tokio::test]
    async fn stored_scheme_less_proxy_heals_on_load() {
        let db = fresh_in_memory_db().await;
        app_metadata_service::upsert_value(
            &db.conn,
            SYSTEM_PROXY_SETTINGS_KEY,
            r#"{"enabled":true,"proxy_url":"127.0.0.1:7890"}"#,
        )
        .await
        .expect("seed legacy proxy row");

        let loaded = load_system_proxy_settings(&db.conn)
            .await
            .expect("load proxy settings");

        assert!(loaded.enabled);
        assert_eq!(
            loaded.proxy_url.as_deref(),
            Some("http://127.0.0.1:7890"),
            "a legacy bare host:port must be repaired on read"
        );
    }

    #[tokio::test]
    async fn terminal_shell_setting_persists_and_updates_live_runtime() {
        let _serial = TERMINAL_SETTINGS_SERIAL.lock().await;
        let _restore = RestoreCommandColor::capture();
        let db = fresh_in_memory_db().await;
        let config = TerminalShellRuntimeConfig::new();

        let saved = set_system_terminal_settings_core(
            &db.conn,
            &config,
            &EventEmitter::Noop,
            SystemTerminalSettings {
                default_shell: Some("  pwsh.exe  ".to_string()),
                colorize_command_output: false,
            },
        )
        .await
        .expect("save terminal setting");

        assert_eq!(saved.default_shell.as_deref(), Some("pwsh.exe"));
        assert_eq!(config.snapshot().await.as_deref(), Some("pwsh.exe"));

        let restarted_config = TerminalShellRuntimeConfig::new();
        apply_persisted_terminal_settings(&db.conn, &restarted_config).await;
        assert_eq!(
            restarted_config.snapshot().await.as_deref(),
            Some("pwsh.exe")
        );
    }

    /// The command-color opt-in survives a save AND a restart, and reaches the
    /// launch env both times — a value that only persisted would leave every
    /// connection made before the next restart on the wrong setting.
    #[tokio::test]
    async fn colorize_command_output_persists_and_reaches_the_launch_env() {
        let _serial = TERMINAL_SETTINGS_SERIAL.lock().await;
        let _restore = RestoreCommandColor::capture();
        let db = fresh_in_memory_db().await;
        let config = TerminalShellRuntimeConfig::new();

        // Off is the default, and the whole point of the change — assert it
        // before anything writes, so a regression to "forced on" fails here.
        assert!(!crate::acp::connection::force_command_color_enabled());
        assert!(
            launch_env_color_vars().is_empty(),
            "a default launch must not force color"
        );

        let saved = set_system_terminal_settings_core(
            &db.conn,
            &config,
            &EventEmitter::Noop,
            SystemTerminalSettings {
                default_shell: None,
                colorize_command_output: true,
            },
        )
        .await
        .expect("save terminal setting");

        assert!(saved.colorize_command_output);
        assert!(crate::acp::connection::force_command_color_enabled());
        assert_eq!(
            launch_env_color_vars(),
            expected_color_vars(),
            "the save must reach a launch"
        );

        // A fresh process would start with the global at its `false` default;
        // the startup load is what has to put it back.
        crate::acp::connection::set_force_command_color(false);
        apply_persisted_terminal_settings(&db.conn, &config).await;
        assert!(crate::acp::connection::force_command_color_enabled());
        assert_eq!(
            launch_env_color_vars(),
            expected_color_vars(),
            "the restart must reach a launch"
        );

        let reloaded = load_system_terminal_settings(&db.conn)
            .await
            .expect("load terminal settings");
        assert!(reloaded.colorize_command_output);

        // `_restore` puts the process global back on the way out — it is
        // shared by every test in this binary, and a bare store at the end
        // would be skipped by any assertion above it that fails.
    }

    /// A row stored before the field existed must load as "off" rather than
    /// failing to parse (which would strand the user's shell choice too).
    #[tokio::test]
    async fn terminal_settings_row_without_the_color_field_loads_as_off() {
        let db = fresh_in_memory_db().await;
        app_metadata_service::upsert_value(
            &db.conn,
            SYSTEM_TERMINAL_SETTINGS_KEY,
            r#"{"default_shell":"pwsh.exe"}"#,
        )
        .await
        .expect("seed legacy terminal row");

        let loaded = load_system_terminal_settings(&db.conn)
            .await
            .expect("load terminal settings");

        assert_eq!(loaded.default_shell.as_deref(), Some("pwsh.exe"));
        assert!(!loaded.colorize_command_output);
    }
}

#[cfg(all(test, feature = "tauri-runtime"))]
mod close_behavior_tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    /// `CLOSE_BEHAVIOR_CACHE` is a PROCESS global. Two of these running
    /// concurrently (the default) would have one clobber the value the other
    /// is about to assert on.
    static CLOSE_BEHAVIOR_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Restores the cache on the way out, including on a panic — a test that
    /// left `Exit` behind would make the next run of the "defaults to ask"
    /// assertion fail for reasons unrelated to the code under test.
    struct RestoreCloseBehavior(CloseWindowBehavior);

    impl RestoreCloseBehavior {
        fn capture() -> Self {
            Self(cached_close_behavior())
        }
    }

    impl Drop for RestoreCloseBehavior {
        fn drop(&mut self) {
            store_close_behavior_cache(self.0);
        }
    }

    #[tokio::test]
    async fn close_behavior_defaults_to_ask() {
        let db = fresh_in_memory_db().await;

        let loaded = load_system_close_behavior_settings(&db.conn).await;

        assert_eq!(loaded.behavior, CloseWindowBehavior::Ask);
    }

    #[tokio::test]
    async fn close_behavior_roundtrips() {
        let db = fresh_in_memory_db().await;

        save_system_close_behavior_settings(&db.conn, CloseWindowBehavior::Exit)
            .await
            .expect("save close behavior");
        let loaded = load_system_close_behavior_settings(&db.conn).await;

        assert_eq!(loaded.behavior, CloseWindowBehavior::Exit);
    }

    /// The close button is the user's last exit. A row this build cannot parse
    /// — hand-edited, written by a newer build, or truncated — must degrade to
    /// "ask", never to an error that leaves the window unclosable.
    #[tokio::test]
    async fn corrupt_close_behavior_falls_back_to_ask() {
        for raw in [r#"not json"#, r#"{"behavior":"boom"}"#, r#"{}"#] {
            let db = fresh_in_memory_db().await;
            app_metadata_service::upsert_value(&db.conn, SYSTEM_CLOSE_BEHAVIOR_SETTINGS_KEY, raw)
                .await
                .expect("seed corrupt row");

            let loaded = load_system_close_behavior_settings(&db.conn).await;

            assert_eq!(
                loaded.behavior,
                CloseWindowBehavior::Ask,
                "corrupt row {raw} should fall back to ask"
            );
        }
    }

    /// The close handler is a synchronous callback and reads the atomic, not
    /// the database — so a save that updates only the row would be invisible
    /// until the next launch.
    #[tokio::test]
    async fn cache_reflects_update() {
        let _serial = CLOSE_BEHAVIOR_SERIAL.lock().await;
        let _restore = RestoreCloseBehavior::capture();
        let db = fresh_in_memory_db().await;

        store_close_behavior_cache(CloseWindowBehavior::Ask);
        save_system_close_behavior_settings(&db.conn, CloseWindowBehavior::Minimize)
            .await
            .expect("save close behavior");

        assert_eq!(cached_close_behavior(), CloseWindowBehavior::Minimize);
    }

    /// Seeding is what makes the preference survive a restart: the atomic
    /// starts at its default every launch, so a missing seed would silently
    /// serve "ask" to a user who picked "exit" months ago.
    #[tokio::test]
    async fn startup_seeding_loads_persisted_behavior() {
        let _serial = CLOSE_BEHAVIOR_SERIAL.lock().await;
        let _restore = RestoreCloseBehavior::capture();
        let db = fresh_in_memory_db().await;
        save_system_close_behavior_settings(&db.conn, CloseWindowBehavior::Exit)
            .await
            .expect("save close behavior");
        store_close_behavior_cache(CloseWindowBehavior::Ask);

        apply_persisted_close_behavior(&db.conn).await;

        assert_eq!(cached_close_behavior(), CloseWindowBehavior::Exit);
    }

    /// `CLOSE_PROMPT_OPEN` / `CLOSE_PROMPT_LISTENER_READY` are process globals
    /// too, and the readiness one is deliberately one-way in production — so a
    /// test that raises it has to put it back by hand or every later assertion
    /// about the un-booted state passes for free.
    struct RestoreClosePromptFlags {
        open: bool,
        ready: bool,
    }

    impl RestoreClosePromptFlags {
        fn capture() -> Self {
            Self {
                open: CLOSE_PROMPT_OPEN.load(std::sync::atomic::Ordering::Acquire),
                ready: close_prompt_listener_ready(),
            }
        }
    }

    impl Drop for RestoreClosePromptFlags {
        fn drop(&mut self) {
            *CLOSE_PROMPT_CLAIMED_AT.lock().unwrap() = None;
            CLOSE_PROMPT_OPEN.store(self.open, std::sync::atomic::Ordering::Release);
            CLOSE_PROMPT_LISTENER_READY.store(self.ready, std::sync::atomic::Ordering::Release);
        }
    }

    /// The de-dup flag: one prompt at a time, and the claim is reusable only
    /// after it is given back. Without this the close button — which stays
    /// clickable while the dialog is up — stacks one dialog per press.
    #[tokio::test]
    async fn close_prompt_claim_is_exclusive_until_released() {
        let _serial = CLOSE_BEHAVIOR_SERIAL.lock().await;
        let _restore = RestoreClosePromptFlags::capture();
        release_close_prompt();

        assert_eq!(
            try_open_close_prompt(),
            ClosePromptClaim::Granted,
            "first press claims the prompt"
        );
        assert_eq!(
            try_open_close_prompt(),
            ClosePromptClaim::AlreadyOpen,
            "a press while the dialog is up is a duplicate"
        );

        release_close_prompt();

        assert_eq!(
            try_open_close_prompt(),
            ClosePromptClaim::Granted,
            "the next press claims it again once the dialog has answered"
        );
    }

    /// The backstop for every way a prompt can fail to reach a dialog that the
    /// readiness flag cannot see — including the one it provably cannot rule
    /// out, where wry runs the close handler's emit inline on the main thread
    /// ahead of a listener registration still queued on the event-loop proxy.
    /// Whatever the cause, the press after the grace has to act.
    #[tokio::test]
    async fn an_unanswered_close_prompt_expires_and_hands_the_press_back() {
        let _serial = CLOSE_BEHAVIOR_SERIAL.lock().await;
        let _restore = RestoreClosePromptFlags::capture();
        release_close_prompt();

        assert_eq!(try_open_close_prompt(), ClosePromptClaim::Granted);
        // Age the claim past the grace rather than sleeping through it.
        *CLOSE_PROMPT_CLAIMED_AT.lock().unwrap() =
            Some(std::time::Instant::now() - CLOSE_PROMPT_GRACE);

        assert_eq!(
            try_open_close_prompt(),
            ClosePromptClaim::Expired,
            "a prompt nobody answered within the grace never arrived"
        );
        // And the expiry hands the claim back rather than eating it, so the
        // press after that one is an ordinary first press again.
        assert_eq!(
            try_open_close_prompt(),
            ClosePromptClaim::Granted,
            "expiring releases the claim instead of wedging on it"
        );
    }

    /// A claim younger than the grace is a dialog the user is still reading.
    /// Expiring it would exit (or hide) out from under them.
    #[tokio::test]
    async fn a_fresh_close_prompt_is_never_expired() {
        let _serial = CLOSE_BEHAVIOR_SERIAL.lock().await;
        let _restore = RestoreClosePromptFlags::capture();
        release_close_prompt();

        assert_eq!(try_open_close_prompt(), ClosePromptClaim::Granted);
        *CLOSE_PROMPT_CLAIMED_AT.lock().unwrap() = Some(
            std::time::Instant::now() - (CLOSE_PROMPT_GRACE - std::time::Duration::from_secs(1)),
        );

        assert_eq!(try_open_close_prompt(), ClosePromptClaim::AlreadyOpen);
    }

    /// `main` is visible before its webview has a listener, and an emit into
    /// that gap looks successful. The close handler asks this first, so the
    /// press falls through to the preference instead of disappearing.
    #[tokio::test]
    async fn listener_readiness_starts_false_and_is_raised_by_the_dialog() {
        let _serial = CLOSE_BEHAVIOR_SERIAL.lock().await;
        let _restore = RestoreClosePromptFlags::capture();
        CLOSE_PROMPT_LISTENER_READY.store(false, std::sync::atomic::Ordering::Release);

        assert!(
            !close_prompt_listener_ready(),
            "nothing is listening until the dialog says so"
        );

        // What the dialog does on mount — `resolve_close_request` is its only
        // call, and this is the half of it that does not need a Tauri handle.
        mark_close_prompt_listener_ready();

        assert!(close_prompt_listener_ready());
    }
}
