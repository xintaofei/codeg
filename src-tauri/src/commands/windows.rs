use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering as AtomicOrdering};
use std::sync::Mutex;

use sea_orm::DatabaseConnection;
use tauri::{
    window::{Effect, EffectState, EffectsBuilder},
    AppHandle, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
};

use crate::app_error::AppCommandError;
use crate::db::service::app_metadata_service;
use crate::db::AppDatabase;
use crate::models::FolderDetail;

/// Base traffic-light position (logical px) at 100 % zoom, tuned for the
/// standard h-8 (32px) overlay title bar shared by the auxiliary windows
/// (commit / merge / push / stash / settings / …).
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_X: f64 = 12.0;
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_Y: f64 = 17.0;
/// The workspace windows (local `main` + remote workspace) render the taller
/// h-10 (40px) `FolderTitleBar` that hosts the relocated conversation + file
/// tab strips, so their traffic lights sit ~4px lower to stay vertically
/// centred within the taller bar.
#[cfg(target_os = "macos")]
const WORKSPACE_TRAFFIC_LIGHT_Y: f64 = 21.0;

#[cfg(target_os = "macos")]
static CURRENT_ZOOM: AtomicU32 = AtomicU32::new(100);

#[cfg(target_os = "macos")]
fn traffic_light_position_at(base_y: f64) -> tauri::LogicalPosition<f64> {
    let zoom = CURRENT_ZOOM.load(AtomicOrdering::Relaxed) as f64;
    // Only Y scales with zoom: overlay content shifts vertically with
    // font-size changes, but the horizontal inset remains constant.
    tauri::LogicalPosition::new(TRAFFIC_LIGHT_X, base_y * zoom / 100.0)
}

/// Traffic-light position for the shared/auxiliary windows (standard bar).
#[cfg(target_os = "macos")]
fn traffic_light_position() -> tauri::LogicalPosition<f64> {
    traffic_light_position_at(TRAFFIC_LIGHT_Y)
}

/// Traffic-light position for the workspace windows (taller bar): the local
/// `main` window and every remote workspace window, both of which load the
/// `/workspace` route. macOS only; call sites are guarded with
/// `#[cfg(target_os = "macos")]`.
#[cfg(target_os = "macos")]
pub(crate) fn workspace_window_traffic_light_position() -> tauri::LogicalPosition<f64> {
    traffic_light_position_at(WORKSPACE_TRAFFIC_LIGHT_Y)
}

const ZOOM_LEVEL_DB_KEY: &str = "appearance_zoom_level";

/// Load saved zoom level from DB and initialize CURRENT_ZOOM.
/// Called once at startup before any window is created.
pub async fn load_saved_zoom(conn: &DatabaseConnection) {
    #[cfg(target_os = "macos")]
    {
        if let Ok(Some(raw)) = app_metadata_service::get_value(conn, ZOOM_LEVEL_DB_KEY).await {
            if let Ok(zoom) = raw.parse::<u32>() {
                let clamped = zoom.clamp(50, 300);
                CURRENT_ZOOM.store(clamped, AtomicOrdering::Relaxed);
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = conn;
    }
}

// ---------------------------------------------------------------------------
// Appearance mode persistence (dark / light / system)
// ---------------------------------------------------------------------------

const APPEARANCE_MODE_DB_KEY: &str = "appearance_mode";

/// Encoded appearance mode: 0 = system (default), 1 = dark, 2 = light.
static CACHED_APPEARANCE_MODE: AtomicU8 = AtomicU8::new(0);

const MODE_SYSTEM: u8 = 0;
const MODE_DARK: u8 = 1;
const MODE_LIGHT: u8 = 2;

fn mode_from_str(s: &str) -> u8 {
    match s {
        "dark" => MODE_DARK,
        "light" => MODE_LIGHT,
        _ => MODE_SYSTEM,
    }
}

/// Load saved appearance mode from DB. Called once at startup.
pub async fn load_saved_appearance_mode(conn: &DatabaseConnection) {
    if let Ok(Some(raw)) = app_metadata_service::get_value(conn, APPEARANCE_MODE_DB_KEY).await {
        CACHED_APPEARANCE_MODE.store(mode_from_str(&raw), AtomicOrdering::Relaxed);
    }
}

pub struct SettingsWindowState {
    owner_by_settings_label: Mutex<HashMap<String, String>>,
}

pub struct CommitWindowState {
    owner_by_commit_label: Mutex<HashMap<String, String>>,
}

/// Detect macOS system dark mode via `defaults read`.
/// Result is cached for the process lifetime via `OnceLock`.
#[cfg(target_os = "macos")]
fn is_system_dark_mode() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        crate::process::std_command("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|o| o.status.success()) // key exists only in dark mode
            .unwrap_or(false)
    })
}

/// Detect Windows system dark mode via registry query.
/// `AppsUseLightTheme`: 0 = dark, 1 = light.
/// Uses `crate::process::std_command` to avoid flashing a console window.
/// On pre-1809 Windows where the key is absent, defaults to light mode.
#[cfg(target_os = "windows")]
fn is_system_dark_mode() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        crate::process::std_command("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output()
            .ok()
            .and_then(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                // Output: "    AppsUseLightTheme    REG_DWORD    0x0"
                // Extract the last token on the matching line to avoid
                // substring false-positives (e.g. "0x00000001" contains "0x0").
                stdout
                    .lines()
                    .find(|l| l.contains("AppsUseLightTheme"))
                    .map(|line| {
                        line.split_whitespace()
                            .last()
                            .map(|val| val == "0x0" || val == "0x00000000")
                            .unwrap_or(false)
                    })
            })
            .unwrap_or(false)
    })
}

/// Detect Linux system dark mode via desktop environment settings.
/// Covers GNOME (gsettings) and KDE Plasma (kreadconfig5/6).
/// Falls back to light mode on unsupported desktops (XFCE, etc.).
#[cfg(target_os = "linux")]
fn is_system_dark_mode() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        // GNOME 42+: color-scheme = 'prefer-dark'
        if let Ok(output) = crate::process::std_command("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
        {
            let s = String::from_utf8_lossy(&output.stdout);
            if s.contains("prefer-dark") {
                return true;
            }
        }
        // Older GNOME / GTK: theme name contains "dark"
        if let Ok(output) = crate::process::std_command("gsettings")
            .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
            .output()
        {
            let s = String::from_utf8_lossy(&output.stdout).to_lowercase();
            if s.contains("dark") {
                return true;
            }
        }
        // KDE Plasma 5/6: ColorScheme name contains "dark"
        for cmd in ["kreadconfig6", "kreadconfig5"] {
            if let Ok(output) = crate::process::std_command(cmd)
                .args(["--group", "General", "--key", "ColorScheme"])
                .output()
            {
                let s = String::from_utf8_lossy(&output.stdout).to_lowercase();
                if s.contains("dark") {
                    return true;
                }
            }
        }
        false
    })
}

/// Determine whether the window should use a dark background, considering
/// both the user's explicit preference (from DB) and the OS appearance.
fn should_use_dark_background() -> bool {
    match CACHED_APPEARANCE_MODE.load(AtomicOrdering::Relaxed) {
        MODE_DARK => true,
        MODE_LIGHT => false,
        _ => is_system_dark_mode(), // "system" or unknown — follow OS
    }
}

pub(crate) fn apply_platform_window_style<'a, R, M>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M>
where
    R: tauri::Runtime,
    M: tauri::Manager<R>,
{
    #[cfg(target_os = "macos")]
    {
        let builder = if should_use_dark_background() {
            // oklch(0.145 0 0) ≈ rgb(9,9,11) — matches CSS --background in dark mode
            builder.background_color(tauri::window::Color(9, 9, 11, 255))
        } else {
            builder
        };
        builder
            .hidden_title(true)
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .traffic_light_position(traffic_light_position())
    }

    #[cfg(target_os = "windows")]
    {
        let builder = if should_use_dark_background() {
            builder.background_color(tauri::window::Color(9, 9, 11, 255))
        } else {
            builder
        };
        builder.decorations(false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux: drop the native GTK title bar so it doesn't stack on top of
        // the app's own toolbar (the "double title bar"). The frontend then
        // renders custom window controls + resize grips, mirroring Windows.
        let builder = if should_use_dark_background() {
            builder.background_color(tauri::window::Color(9, 9, 11, 255))
        } else {
            builder
        };
        builder.decorations(false)
    }
}

#[cfg(target_os = "windows")]
fn ensure_windows_undecorated(window: &tauri::WebviewWindow) {
    let _ = window.set_decorations(false);
}

#[cfg(not(target_os = "windows"))]
fn ensure_windows_undecorated(_window: &tauri::WebviewWindow) {}

/// Apply platform-specific post-creation setup.
pub(crate) fn post_window_setup(window: &tauri::WebviewWindow) {
    ensure_windows_undecorated(window);
}

impl SettingsWindowState {
    pub fn new() -> Self {
        Self {
            owner_by_settings_label: Mutex::new(HashMap::new()),
        }
    }

    fn set_owner(&self, settings_label: String, owner_label: String) {
        if let Ok(mut owners) = self.owner_by_settings_label.lock() {
            owners.insert(settings_label, owner_label);
        }
    }

    fn take_owner(&self, settings_label: &str) -> Option<String> {
        self.owner_by_settings_label
            .lock()
            .ok()
            .and_then(|mut owners| owners.remove(settings_label))
    }
}

impl Default for SettingsWindowState {
    fn default() -> Self {
        Self::new()
    }
}

impl CommitWindowState {
    pub fn new() -> Self {
        Self {
            owner_by_commit_label: Mutex::new(HashMap::new()),
        }
    }

    fn set_owner(&self, commit_label: String, owner_label: String) {
        if let Ok(mut owners) = self.owner_by_commit_label.lock() {
            owners.insert(commit_label, owner_label);
        }
    }

    fn take_owner(&self, commit_label: &str) -> Option<String> {
        self.owner_by_commit_label
            .lock()
            .ok()
            .and_then(|mut owners| owners.remove(commit_label))
    }
}

impl Default for CommitWindowState {
    fn default() -> Self {
        Self::new()
    }
}

fn resolve_settings_route(section: Option<&str>) -> &'static str {
    match section {
        Some("appearance") => "settings/appearance",
        Some("agents") => "settings/agents",
        Some("mcp") => "settings/mcp",
        Some("skills") => "settings/skills",
        Some("experts") => "settings/experts",
        Some("science") => "settings/science",
        Some("office-tools") => "settings/office-tools",
        Some("shortcuts") => "settings/shortcuts",
        Some("system") => "settings/system",
        _ => "settings/appearance",
    }
}

fn normalize_agent_query(agent_type: Option<&str>) -> Option<String> {
    let raw = agent_type?.trim();
    if raw.is_empty() {
        return None;
    }
    if raw
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        return Some(raw.to_string());
    }
    None
}

fn resolve_settings_target(section: Option<&str>, agent_type: Option<&str>) -> String {
    let route = resolve_settings_route(section);
    if route == "settings/agents" {
        if let Some(agent) = normalize_agent_query(agent_type) {
            return format!("{route}?agent={agent}");
        }
    }
    route.to_string()
}

fn append_query_param(route: String, key: &str, value: &str) -> String {
    if route.contains('?') {
        format!("{route}&{key}={value}")
    } else {
        format!("{route}?{key}={value}")
    }
}

fn append_remote_context(
    route: String,
    remote_connection_id: Option<i32>,
    remote_window_id: Option<&str>,
) -> String {
    let Some(id) = remote_connection_id else {
        return route;
    };
    let route = append_query_param(route, "remoteConnectionId", &id.to_string());
    match remote_window_id {
        Some(window_id) => append_query_param(route, "remoteWindowId", window_id),
        None => route,
    }
}

fn route_with_new_remote_window(
    route: String,
    remote_connection_id: Option<i32>,
) -> (String, Option<String>) {
    let remote_window_id = remote_connection_id
        .map(|_| crate::commands::remote_workspace::new_remote_window_instance_id());
    let route = append_remote_context(route, remote_connection_id, remote_window_id.as_deref());
    (route, remote_window_id)
}

fn remote_window_id_from_window(window: &tauri::WebviewWindow) -> Option<String> {
    window.url().ok()?.query_pairs().find_map(|(key, value)| {
        if key == "remoteWindowId" && !value.is_empty() {
            Some(value.into_owned())
        } else {
            None
        }
    })
}

fn register_remote_window_cleanup(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    remote_window_id: Option<&str>,
) {
    let Some(remote_window_id) = remote_window_id else {
        return;
    };
    if let Some(proxy) =
        app.try_state::<std::sync::Arc<crate::commands::remote_proxy::RemoteProxyState>>()
    {
        proxy
            .inner()
            .register_window_instance_cleanup(window, remote_window_id.to_string());
    }
}

// ---------------------------------------------------------------------------
// Window title localization
// ---------------------------------------------------------------------------
//
// Window titles are set at creation time and not refreshed on locale change,
// which mirrors the behavior of native OS dialogs across the app. Translations
// live in Rust (not the frontend i18n JSON) because the title is applied via
// Tauri's window builder before the webview boots.

struct WindowTitles {
    settings: &'static str,
    commit: &'static str,
    merge: &'static str,
    stash: &'static str,
    push: &'static str,
    project_boot: &'static str,
}

fn window_titles_for(locale: crate::models::system::AppLocale) -> WindowTitles {
    use crate::models::system::AppLocale;
    match locale {
        AppLocale::ZhCn => WindowTitles {
            settings: "设置",
            commit: "提交代码",
            merge: "解决冲突",
            stash: "储藏",
            push: "推送",
            project_boot: "项目启动器",
        },
        AppLocale::ZhTw => WindowTitles {
            settings: "設定",
            commit: "提交程式碼",
            merge: "解決衝突",
            stash: "暫存",
            push: "推送",
            project_boot: "專案啟動器",
        },
        AppLocale::Ja => WindowTitles {
            settings: "設定",
            commit: "コミット",
            merge: "コンフリクトの解決",
            stash: "スタッシュ",
            push: "プッシュ",
            project_boot: "プロジェクトブート",
        },
        AppLocale::Ko => WindowTitles {
            settings: "설정",
            commit: "커밋",
            merge: "충돌 해결",
            stash: "스태시",
            push: "푸시",
            project_boot: "프로젝트 부트",
        },
        AppLocale::Es => WindowTitles {
            settings: "Configuración",
            commit: "Confirmar",
            merge: "Resolver conflictos",
            stash: "Reserva",
            push: "Enviar",
            project_boot: "Inicio de Proyecto",
        },
        AppLocale::De => WindowTitles {
            settings: "Einstellungen",
            commit: "Commit",
            merge: "Konflikte lösen",
            stash: "Stash",
            push: "Push",
            project_boot: "Projekt-Starter",
        },
        AppLocale::Fr => WindowTitles {
            settings: "Paramètres",
            commit: "Valider",
            merge: "Résoudre les conflits",
            stash: "Réserve",
            push: "Pousser",
            project_boot: "Lanceur de projet",
        },
        AppLocale::Pt => WindowTitles {
            settings: "Configurações",
            commit: "Confirmar",
            merge: "Resolver conflitos",
            stash: "Stash",
            push: "Enviar",
            project_boot: "Inicializador de Projeto",
        },
        AppLocale::Ar => WindowTitles {
            settings: "الإعدادات",
            commit: "الالتزام",
            merge: "حل التعارضات",
            stash: "إخفاء",
            push: "دفع",
            project_boot: "مُنشئ المشروع",
        },
        AppLocale::En => WindowTitles {
            settings: "Settings",
            commit: "Commit",
            merge: "Resolve Conflicts",
            stash: "Stash",
            push: "Push",
            project_boot: "Project Boot",
        },
    }
}

// When the frontend passes an explicit `locale`, use it — that's the
// authoritative effective locale (see lib/i18n.ts::getCurrentEffectiveAppLocale).
// Falling back to the DB only matters for callers that bypass the JS wrappers
// (e.g. a future HTTP client, internal tests).
async fn resolve_window_titles(
    conn: &DatabaseConnection,
    explicit: Option<crate::models::system::AppLocale>,
) -> WindowTitles {
    if let Some(locale) = explicit {
        return window_titles_for(locale);
    }
    let locale = crate::commands::system_settings::load_system_language_settings(conn)
        .await
        .map(|settings| settings.language)
        .unwrap_or_default();
    window_titles_for(locale)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_folder_window(
    app: AppHandle,
    db: tauri::State<'_, AppDatabase>,
    path: String,
) -> Result<FolderDetail, AppCommandError> {
    // Single-window workspace: upsert the folder (is_open = true), close any
    // legacy project-boot window, and return the full detail for the frontend
    // to add to its workspace state.
    let entry = crate::db::service::folder_service::add_folder(&db.conn, &path)
        .await
        .map_err(AppCommandError::from)?;

    if let Some(w) = app.get_webview_window("project-boot") {
        let _ = w.close();
    }

    let folder = crate::db::service::folder_service::get_folder_by_id(&db.conn, entry.id)
        .await
        .map_err(AppCommandError::from)?
        .ok_or_else(|| AppCommandError::not_found("Folder not found after add"))?;

    // Bring the main window to focus if it exists
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.set_focus();
    }

    Ok(folder)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_commit_window(
    app: AppHandle,
    window: tauri::WebviewWindow,
    db: tauri::State<'_, AppDatabase>,
    state: tauri::State<'_, CommitWindowState>,
    folder_id: i32,
    locale: Option<crate::models::system::AppLocale>,
    remote_connection_id: Option<i32>,
) -> Result<(), AppCommandError> {
    let owner_label = window.label().to_string();
    let label = match remote_connection_id {
        Some(remote_id) => format!("remote-commit-{remote_id}-{folder_id}"),
        None => format!("commit-{folder_id}"),
    };

    if let Some(existing) = app.get_webview_window(&label) {
        state.set_owner(label.clone(), owner_label);
        let _ = existing.unminimize();
        existing
            .set_focus()
            .map_err(|e| AppCommandError::window("Failed to focus commit window", e.to_string()))?;
        return Ok(());
    }

    let titles = resolve_window_titles(&db.conn, locale).await;
    let window_title = if remote_connection_id.is_some() {
        titles.commit.to_string()
    } else {
        let folder = crate::db::service::folder_service::get_folder_by_id(&db.conn, folder_id)
            .await
            .map_err(AppCommandError::from)?
            .ok_or_else(|| {
                AppCommandError::not_found(format!("Folder {folder_id} not found"))
                    .with_detail(format!("folder_id={folder_id}"))
            })?;
        format!("{} - {}", titles.commit, folder.name)
    };
    let (url_str, remote_window_id) =
        route_with_new_remote_window(format!("commit?folderId={folder_id}"), remote_connection_id);
    let url = WebviewUrl::App(url_str.into());
    let builder = WebviewWindowBuilder::new(&app, &label, url)
        .title(window_title)
        .inner_size(1220.0, 820.0)
        .min_inner_size(980.0, 620.0)
        .center();
    let builder = builder.parent(&window).map_err(|e| {
        AppCommandError::window("Failed to attach commit window to parent", e.to_string())
    })?;
    let commit_window = apply_platform_window_style(builder)
        .build()
        .map_err(|e| AppCommandError::window("Failed to open commit window", e.to_string()))?;
    register_remote_window_cleanup(&app, &commit_window, remote_window_id.as_deref());
    post_window_setup(&commit_window);
    state.set_owner(label, owner_label);
    commit_window
        .set_focus()
        .map_err(|e| AppCommandError::window("Failed to focus commit window", e.to_string()))?;

    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
#[allow(clippy::too_many_arguments)]
pub async fn open_settings_window(
    app: AppHandle,
    window: tauri::WebviewWindow,
    db: tauri::State<'_, AppDatabase>,
    section: Option<String>,
    agent_type: Option<String>,
    locale: Option<crate::models::system::AppLocale>,
    remote_connection_id: Option<i32>,
    state: tauri::State<'_, SettingsWindowState>,
) -> Result<(), AppCommandError> {
    let settings_label = match remote_connection_id {
        Some(remote_id) => format!("remote-settings-{remote_id}"),
        None => "settings".to_string(),
    };
    let owner_label = window.label().to_string();
    if let Some(existing) = app.get_webview_window(&settings_label) {
        post_window_setup(&existing);
        if section.is_some() || agent_type.is_some() || remote_connection_id.is_some() {
            let existing_remote_window_id = remote_window_id_from_window(&existing);
            let generated_remote_window_id = remote_connection_id
                .filter(|_| existing_remote_window_id.is_none())
                .map(|_| crate::commands::remote_workspace::new_remote_window_instance_id());
            let remote_window_id = existing_remote_window_id
                .as_deref()
                .or(generated_remote_window_id.as_deref());
            if generated_remote_window_id.is_some() {
                register_remote_window_cleanup(&app, &existing, remote_window_id);
            }
            let target_route = append_remote_context(
                resolve_settings_target(section.as_deref(), agent_type.as_deref()),
                remote_connection_id,
                remote_window_id,
            );
            let target_path = format!("/{target_route}");
            let target_json = serde_json::to_string(&target_path).map_err(|e| {
                AppCommandError::window("Failed to build settings navigation target", e.to_string())
            })?;
            let nav_script = format!("window.location.replace({target_json});");
            existing.eval(&nav_script).map_err(|e| {
                AppCommandError::window("Failed to navigate settings window", e.to_string())
            })?;
        }
        let _ = state.take_owner(&settings_label);
        state.set_owner(settings_label, owner_label);
        let _ = existing.unminimize();
        existing.set_focus().map_err(|e| {
            AppCommandError::window("Failed to focus settings window", e.to_string())
        })?;
        return Ok(());
    }

    let titles = resolve_window_titles(&db.conn, locale).await;
    let (target_route, remote_window_id) = route_with_new_remote_window(
        resolve_settings_target(section.as_deref(), agent_type.as_deref()),
        remote_connection_id,
    );
    let url = WebviewUrl::App(target_route.into());
    let builder = WebviewWindowBuilder::new(&app, &settings_label, url)
        .title(titles.settings)
        .inner_size(1080.0, 700.0)
        .min_inner_size(1080.0, 600.0)
        .center();
    // Intentionally NOT a child of the caller window: on macOS `.parent()`
    // attaches the window via `addChildWindow`, which makes settings move and
    // minimize together with the main window. Keep it an independent top-level
    // window; focus returns to the owner on close via
    // `restore_windows_after_settings` (the SettingsWindowState owner tracking
    // is independent of any parent/child relationship).
    let settings_window = apply_platform_window_style(builder)
        .build()
        .map_err(|e| AppCommandError::window("Failed to open settings window", e.to_string()))?;
    register_remote_window_cleanup(&app, &settings_window, remote_window_id.as_deref());
    post_window_setup(&settings_window);
    state.set_owner(settings_label, owner_label);
    settings_window
        .set_focus()
        .map_err(|e| AppCommandError::window("Failed to focus settings window", e.to_string()))?;
    Ok(())
}

pub fn restore_windows_after_settings(
    app: &AppHandle,
    state: &SettingsWindowState,
    settings_window_label: &str,
) {
    if let Some(owner_label) = state.take_owner(settings_window_label) {
        if let Some(window) = app.get_webview_window(&owner_label) {
            let _ = window.set_focus();
        }
    }
}

pub fn restore_window_after_commit(
    app: &AppHandle,
    state: &CommitWindowState,
    commit_window_label: &str,
) {
    if let Some(owner_label) = state.take_owner(commit_window_label) {
        if let Some(window) = app.get_webview_window(&owner_label) {
            let _ = window.set_focus();
        }
    }
}

pub struct MergeWindowState {
    owner_by_merge_label: Mutex<HashMap<String, String>>,
}

impl MergeWindowState {
    pub fn new() -> Self {
        Self {
            owner_by_merge_label: Mutex::new(HashMap::new()),
        }
    }

    fn set_owner(&self, merge_label: String, owner_label: String) {
        if let Ok(mut owners) = self.owner_by_merge_label.lock() {
            owners.insert(merge_label, owner_label);
        }
    }

    fn take_owner(&self, merge_label: &str) -> Option<String> {
        self.owner_by_merge_label
            .lock()
            .ok()
            .and_then(|mut owners| owners.remove(merge_label))
    }
}

impl Default for MergeWindowState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
#[allow(clippy::too_many_arguments)]
pub async fn open_merge_window(
    app: AppHandle,
    window: tauri::WebviewWindow,
    db: tauri::State<'_, AppDatabase>,
    state: tauri::State<'_, MergeWindowState>,
    folder_id: i32,
    operation: String,
    upstream_commit: Option<String>,
    locale: Option<crate::models::system::AppLocale>,
    remote_connection_id: Option<i32>,
) -> Result<(), AppCommandError> {
    let owner_label = window.label().to_string();
    let label = match remote_connection_id {
        Some(remote_id) => format!("remote-merge-{remote_id}-{folder_id}"),
        None => format!("merge-{folder_id}"),
    };

    if let Some(existing) = app.get_webview_window(&label) {
        state.set_owner(label.clone(), owner_label);
        let _ = existing.unminimize();
        existing
            .set_focus()
            .map_err(|e| AppCommandError::window("Failed to focus merge window", e.to_string()))?;
        return Ok(());
    }

    let titles = resolve_window_titles(&db.conn, locale).await;
    let window_title = if remote_connection_id.is_some() {
        titles.merge.to_string()
    } else {
        let folder = crate::db::service::folder_service::get_folder_by_id(&db.conn, folder_id)
            .await
            .map_err(AppCommandError::from)?
            .ok_or_else(|| {
                AppCommandError::not_found(format!("Folder {folder_id} not found"))
                    .with_detail(format!("folder_id={folder_id}"))
            })?;
        format!("{} - {}", titles.merge, folder.name)
    };
    let mut url_str = format!("merge?folderId={folder_id}&operation={operation}");
    if let Some(ref commit) = upstream_commit {
        url_str.push_str(&format!("&upstreamCommit={commit}"));
    }
    let (url_str, remote_window_id) = route_with_new_remote_window(url_str, remote_connection_id);
    let url = WebviewUrl::App(url_str.into());
    let builder = WebviewWindowBuilder::new(&app, &label, url)
        .title(window_title)
        .inner_size(1400.0, 900.0)
        .min_inner_size(1100.0, 650.0)
        .center();
    let builder = builder.parent(&window).map_err(|e| {
        AppCommandError::window("Failed to attach merge window to parent", e.to_string())
    })?;
    let merge_window = apply_platform_window_style(builder)
        .build()
        .map_err(|e| AppCommandError::window("Failed to open merge window", e.to_string()))?;
    register_remote_window_cleanup(&app, &merge_window, remote_window_id.as_deref());
    post_window_setup(&merge_window);
    state.set_owner(label, owner_label);
    merge_window
        .set_focus()
        .map_err(|e| AppCommandError::window("Failed to focus merge window", e.to_string()))?;

    Ok(())
}

pub fn restore_window_after_merge(
    app: &AppHandle,
    state: &MergeWindowState,
    merge_window_label: &str,
) {
    if let Some(owner_label) = state.take_owner(merge_window_label) {
        if let Some(window) = app.get_webview_window(&owner_label) {
            let _ = window.set_focus();
        }
    }
}

/// Clean up dangling merge state when a merge window is closed without
/// completing or aborting. Checks if MERGE_HEAD exists, aborts the merge,
/// and notifies the parent window.
pub async fn cleanup_dangling_merge(app: &AppHandle, merge_window_label: &str) {
    let folder_id: i32 = match merge_window_label
        .strip_prefix("merge-")
        .and_then(|s| s.parse().ok())
    {
        Some(id) => id,
        None => return,
    };

    let db = match app.try_state::<AppDatabase>() {
        Some(db) => db,
        None => return,
    };

    let folder =
        match crate::db::service::folder_service::get_folder_by_id(&db.conn, folder_id).await {
            Ok(Some(f)) => f,
            _ => return,
        };

    // Check if MERGE_HEAD exists
    let check = crate::process::tokio_command("git")
        .args(["rev-parse", "--verify", "MERGE_HEAD"])
        .current_dir(&folder.path)
        .output()
        .await;
    let has_merge_head = check.map(|o| o.status.success()).unwrap_or(false);

    if has_merge_head {
        let _ = crate::process::tokio_command("git")
            .args(["merge", "--abort"])
            .current_dir(&folder.path)
            .output()
            .await;

        let emitter = crate::web::event_bridge::EventEmitter::Tauri(app.clone());
        crate::web::event_bridge::emit_event(
            &emitter,
            "folder://merge-aborted",
            serde_json::json!({ "folder_id": folder_id }),
        );
    }
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_stash_window(
    app: AppHandle,
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    locale: Option<crate::models::system::AppLocale>,
    remote_connection_id: Option<i32>,
) -> Result<(), AppCommandError> {
    let label = match remote_connection_id {
        Some(remote_id) => format!("remote-stash-{remote_id}-{folder_id}"),
        None => format!("stash-{folder_id}"),
    };

    if let Some(existing) = app.get_webview_window(&label) {
        post_window_setup(&existing);
        let _ = existing.unminimize();
        existing
            .set_focus()
            .map_err(|e| AppCommandError::window("Failed to focus stash window", e.to_string()))?;
        return Ok(());
    }

    let titles = resolve_window_titles(&db.conn, locale).await;
    let window_title = if remote_connection_id.is_some() {
        titles.stash.to_string()
    } else {
        let folder = crate::db::service::folder_service::get_folder_by_id(&db.conn, folder_id)
            .await
            .map_err(AppCommandError::from)?
            .ok_or_else(|| {
                AppCommandError::not_found(format!("Folder {folder_id} not found"))
                    .with_detail(format!("folder_id={folder_id}"))
            })?;
        format!("{} - {}", titles.stash, folder.name)
    };
    let (url_str, remote_window_id) =
        route_with_new_remote_window(format!("stash?folderId={folder_id}"), remote_connection_id);
    let url = WebviewUrl::App(url_str.into());
    let builder = WebviewWindowBuilder::new(&app, &label, url)
        .title(window_title)
        .inner_size(1100.0, 700.0)
        .min_inner_size(800.0, 500.0)
        .center();
    let stash_window = apply_platform_window_style(builder)
        .build()
        .map_err(|e| AppCommandError::window("Failed to open stash window", e.to_string()))?;
    register_remote_window_cleanup(&app, &stash_window, remote_window_id.as_deref());
    post_window_setup(&stash_window);

    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_push_window(
    app: AppHandle,
    window: tauri::WebviewWindow,
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    locale: Option<crate::models::system::AppLocale>,
    remote_connection_id: Option<i32>,
) -> Result<(), AppCommandError> {
    let label = match remote_connection_id {
        Some(remote_id) => format!("remote-push-{remote_id}-{folder_id}"),
        None => format!("push-{folder_id}"),
    };

    if let Some(existing) = app.get_webview_window(&label) {
        post_window_setup(&existing);
        let _ = existing.unminimize();
        existing
            .set_focus()
            .map_err(|e| AppCommandError::window("Failed to focus push window", e.to_string()))?;
        return Ok(());
    }

    let titles = resolve_window_titles(&db.conn, locale).await;
    let window_title = if remote_connection_id.is_some() {
        titles.push.to_string()
    } else {
        let folder = crate::db::service::folder_service::get_folder_by_id(&db.conn, folder_id)
            .await
            .map_err(AppCommandError::from)?
            .ok_or_else(|| {
                AppCommandError::not_found(format!("Folder {folder_id} not found"))
                    .with_detail(format!("folder_id={folder_id}"))
            })?;
        format!("{} - {}", titles.push, folder.name)
    };
    let (url_str, remote_window_id) =
        route_with_new_remote_window(format!("push?folderId={folder_id}"), remote_connection_id);
    let url = WebviewUrl::App(url_str.into());
    let builder = WebviewWindowBuilder::new(&app, &label, url)
        .title(window_title)
        .inner_size(1100.0, 700.0)
        .min_inner_size(800.0, 500.0)
        .center();
    let builder = builder.parent(&window).map_err(|e| {
        AppCommandError::window("Failed to attach push window to parent", e.to_string())
    })?;
    let push_window = apply_platform_window_style(builder)
        .build()
        .map_err(|e| AppCommandError::window("Failed to open push window", e.to_string()))?;
    register_remote_window_cleanup(&app, &push_window, remote_window_id.as_deref());
    post_window_setup(&push_window);

    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_project_boot_window(
    app: AppHandle,
    db: tauri::State<'_, AppDatabase>,
    source: Option<String>,
    locale: Option<crate::models::system::AppLocale>,
    remote_connection_id: Option<i32>,
) -> Result<(), AppCommandError> {
    let _ = source;
    let label = match remote_connection_id {
        Some(id) => format!("remote-project-boot-{id}"),
        None => "project-boot".to_string(),
    };
    if let Some(existing) = app.get_webview_window(&label) {
        post_window_setup(&existing);
        let _ = existing.unminimize();
        existing.set_focus().map_err(|e| {
            AppCommandError::window("Failed to focus project boot window", e.to_string())
        })?;
        return Ok(());
    }

    let titles = resolve_window_titles(&db.conn, locale).await;
    let (url_str, remote_window_id) =
        route_with_new_remote_window("project-boot".to_string(), remote_connection_id);
    let url = WebviewUrl::App(url_str.into());
    let builder = WebviewWindowBuilder::new(&app, &label, url)
        .title(titles.project_boot)
        .inner_size(1400.0, 900.0)
        .min_inner_size(1100.0, 700.0)
        .center();
    let window = apply_platform_window_style(builder).build().map_err(|e| {
        AppCommandError::window("Failed to open project boot window", e.to_string())
    })?;
    register_remote_window_cleanup(&app, &window, remote_window_id.as_deref());
    post_window_setup(&window);

    Ok(())
}

// ─── Desktop pet window ─────────────────────────────────────────────────

const PET_WINDOW_LABEL: &str = "pet";
const PET_HOVER_ENTER_EVENT: &str = "pet://hover-enter";
const PET_HOVER_LEAVE_EVENT: &str = "pet://hover-leave";
/// Single-frame logical pixel dimensions, locked to the Codex sprite-sheet
/// contract. The window is sized as one frame × user scale, with no extra
/// chrome — DPR handling lives inside the webview.
const PET_BASE_WIDTH: f64 = 192.0;
const PET_BASE_HEIGHT: f64 = 208.0;

/// Process-global "cursor is currently inside the pet window" flag, owned by
/// the hover watcher but readable/writable by the context-menu command so
/// that dismissing the native menu can force a fresh `enter` event. Without
/// this, the cursor never appears to "leave" while the menu is up — the
/// watcher's transition detector then misses the post-dismiss enter and
/// the user has to wiggle off-pet-and-back to re-trigger waving.
static PET_HOVER_WAS_INSIDE: AtomicBool = AtomicBool::new(false);

/// Apply the pet-window-specific platform style. Deliberately separate from
/// `apply_platform_window_style`: that helper sets a solid background color
/// for the main / settings / git windows, which would defeat the
/// transparent + chromeless pet window. The pet builder needs only
/// borderless decoration; transparency itself is set by the caller.
fn apply_pet_window_style<'a, R, M>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M>
where
    R: tauri::Runtime,
    M: tauri::Manager<R>,
{
    #[cfg(target_os = "macos")]
    {
        builder
            .title_bar_style(tauri::TitleBarStyle::Transparent)
            .hidden_title(true)
    }

    #[cfg(target_os = "windows")]
    {
        builder.decorations(false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        builder
    }
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_pet_window(
    app: AppHandle,
    db: tauri::State<'_, AppDatabase>,
) -> Result<(), AppCommandError> {
    let mut config = crate::commands::pet::pet_get_settings_core(&db.conn).await?;
    let pet_id = config
        .active_pet_id
        .clone()
        .ok_or_else(|| AppCommandError::configuration_missing("No active pet selected."))?;

    // Validate the pet still exists; otherwise fail loudly so the caller
    // can route the user to the picker rather than open an empty window.
    {
        let id = pet_id.clone();
        tokio::task::spawn_blocking(move || crate::pets::get_pet(&id))
            .await
            .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))??;
    }

    if let Some(existing) = app.get_webview_window(PET_WINDOW_LABEL) {
        let _ = existing.unminimize();
        existing
            .set_focus()
            .map_err(|e| AppCommandError::window("Failed to focus pet window", e.to_string()))?;
        return Ok(());
    }

    let scale = config.scale.clamp(0.5, 3.0);
    config.scale = scale;
    config.enabled = true;
    crate::commands::pet::pet_save_window_state_core(
        &db.conn,
        crate::models::pet::PetWindowStatePatch {
            x: None,
            y: None,
            scale: Some(scale),
            always_on_top: None,
            enabled: Some(true),
        },
    )
    .await?;

    let url = WebviewUrl::App(format!("pet?petId={pet_id}").into());
    let mut builder = WebviewWindowBuilder::new(&app, PET_WINDOW_LABEL, url)
        .title("codeg pet")
        .inner_size(PET_BASE_WIDTH * scale, PET_BASE_HEIGHT * scale)
        .min_inner_size(PET_BASE_WIDTH * 0.5, PET_BASE_HEIGHT * 0.5)
        .max_inner_size(PET_BASE_WIDTH * 3.0, PET_BASE_HEIGHT * 3.0)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(config.always_on_top)
        .skip_taskbar(true)
        .shadow(false)
        // Don't steal focus from the user's IDE/terminal on summon, and let
        // the first click on an inactive pet window hit the webview directly
        // (so drag works without a "click once to activate" cycle).
        .focused(false)
        .accept_first_mouse(true);

    builder = builder.center();

    apply_pet_window_style(builder)
        .build()
        .map_err(|e| AppCommandError::window("Failed to open pet window", e.to_string()))?;

    spawn_pet_hover_watcher(app.clone());

    Ok(())
}

/// Polls the global cursor position and emits `pet://hover-enter` whenever
/// the cursor crosses into the pet window's bounds. Native webviews on
/// macOS don't reliably deliver mouse events to non-key windows, so we
/// detect "cursor over the pet" in Rust and let the frontend trigger the
/// waving animation in response. The task ends when the pet window is
/// closed.
fn spawn_pet_hover_watcher(app: AppHandle) {
    use std::time::Duration;
    use tauri::Emitter;

    // Bounds change only on drag or scale; refreshing every N ticks cuts
    // `outer_position`/`outer_size` IPC by ~80% in the steady state. The
    // false hover-enter that cache staleness produces during a drag is
    // suppressed on the JS side via a pointer-down guard (see PetWindow).
    const BOUNDS_REFRESH_TICKS: u8 = 5;

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(80));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Start fresh: prior pet sessions may have left the flag set to
        // true, and we must guarantee an enter event next time the cursor
        // actually crosses the bounds.
        PET_HOVER_WAS_INSIDE.store(false, AtomicOrdering::Relaxed);
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        let mut ticks_since_refresh: u8 = BOUNDS_REFRESH_TICKS;
        loop {
            interval.tick().await;
            let Some(window) = app.get_webview_window(PET_WINDOW_LABEL) else {
                break;
            };

            if ticks_since_refresh >= BOUNDS_REFRESH_TICKS {
                let Ok(pos) = window.outer_position() else {
                    continue;
                };
                let Ok(size) = window.outer_size() else {
                    continue;
                };
                let x_min = pos.x as f64;
                let y_min = pos.y as f64;
                bounds = Some((
                    x_min,
                    x_min + size.width as f64,
                    y_min,
                    y_min + size.height as f64,
                ));
                ticks_since_refresh = 0;
            } else {
                ticks_since_refresh += 1;
            }

            let Some((x_min, x_max, y_min, y_max)) = bounds else {
                continue;
            };
            let Ok(cursor) = app.cursor_position() else {
                continue;
            };
            let inside =
                cursor.x >= x_min && cursor.x < x_max && cursor.y >= y_min && cursor.y < y_max;
            let was_inside = PET_HOVER_WAS_INSIDE.load(AtomicOrdering::Relaxed);
            if inside && !was_inside {
                let _ = app.emit(PET_HOVER_ENTER_EVENT, ());
            } else if !inside && was_inside {
                let _ = app.emit(PET_HOVER_LEAVE_EVENT, ());
            }
            PET_HOVER_WAS_INSIDE.store(inside, AtomicOrdering::Relaxed);
        }
    });
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn close_pet_window(
    app: AppHandle,
    db: tauri::State<'_, AppDatabase>,
) -> Result<(), AppCommandError> {
    if let Some(existing) = app.get_webview_window(PET_WINDOW_LABEL) {
        let _ = existing.close();
    }
    let _ = crate::commands::pet::pet_save_window_state_core(
        &db.conn,
        crate::models::pet::PetWindowStatePatch {
            x: None,
            y: None,
            scale: None,
            always_on_top: None,
            enabled: Some(false),
        },
    )
    .await?;
    Ok(())
}

/// Persist the pet window's last-known position. Called by the pet renderer
/// when the user finishes dragging.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pet_window_record_position(
    db: tauri::State<'_, AppDatabase>,
    x: f64,
    y: f64,
) -> Result<(), AppCommandError> {
    crate::commands::pet::pet_save_window_state_core(
        &db.conn,
        crate::models::pet::PetWindowStatePatch {
            x: Some(x),
            y: Some(y),
            scale: None,
            always_on_top: None,
            enabled: None,
        },
    )
    .await?;
    Ok(())
}

// ─── Pet session panel (click-to-open companion window) ─────────────────
//
// A second, focusable window anchored next to the sprite. The sprite window
// itself is transparent / non-focusing / exact-fit and hostile to a scrollable
// interactive list, so the list + inline permission actions live here. Tapping
// the pet toggles it; clicking away (blur) dismisses it.

pub const PET_PANEL_LABEL: &str = "pet-panel";
const PET_PANEL_WIDTH: f64 = 300.0;
/// First-frame window height (logical px). The panel reports its real content
/// height via `resize_pet_panel` right after it mounts, so this is only the
/// open-time size — tuned to the rendered empty-state card (header + "no active
/// sessions" message + padding). Keeping it at the common (empty) height means
/// the common path opens already-correct, with no resize flash.
const PET_PANEL_DEFAULT_HEIGHT: f64 = 132.0;
/// Floor for `resize_pet_panel`'s clamp — never collapse below a usable header.
const PET_PANEL_MIN_HEIGHT: f64 = 80.0;
const PET_PANEL_GAP: f64 = 8.0;

/// Guards the toggle-vs-blur race. When the panel auto-closes on blur because
/// the user clicked the pet, that same click also fires `toggle_pet_panel`;
/// without this guard the toggle would immediately reopen the just-closed
/// panel. The blur handler stamps the close instant here and `toggle_pet_panel`
/// skips the reopen while the stamp is fresh.
static PET_PANEL_BLUR_CLOSED_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
const PET_PANEL_REOPEN_SUPPRESS_MS: u128 = 300;

/// Close the panel on blur (click-away dismiss) and record the time so a
/// paired pet click doesn't reopen it. Invoked from the global window-event
/// handler in `lib.rs`.
#[cfg(feature = "tauri-runtime")]
pub fn close_pet_panel_on_blur(app: &AppHandle) {
    if let Some(panel) = app.get_webview_window(PET_PANEL_LABEL) {
        if let Ok(mut guard) = PET_PANEL_BLUR_CLOSED_AT.lock() {
            *guard = Some(std::time::Instant::now());
        }
        let _ = panel.close();
    }
}

/// True (consuming the stamp) if the panel was blur-closed within the suppress
/// window — i.e. the current toggle is the back half of a click that already
/// dismissed the panel, so it must not reopen.
fn pet_panel_reopen_suppressed() -> bool {
    if let Ok(mut guard) = PET_PANEL_BLUR_CLOSED_AT.lock() {
        if let Some(t) = *guard {
            if t.elapsed().as_millis() < PET_PANEL_REOPEN_SUPPRESS_MS {
                *guard = None;
                return true;
            }
        }
    }
    false
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn toggle_pet_panel(app: AppHandle) -> Result<(), AppCommandError> {
    // Already open → toggle off. (Covers the race where the pet click reaches
    // this command before the panel's blur event has fired.)
    if let Some(existing) = app.get_webview_window(PET_PANEL_LABEL) {
        let _ = existing.close();
        return Ok(());
    }
    // The click that closed it via blur must not reopen it.
    if pet_panel_reopen_suppressed() {
        return Ok(());
    }
    open_pet_panel_window(&app)
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn close_pet_panel(app: AppHandle) -> Result<(), AppCommandError> {
    if let Some(existing) = app.get_webview_window(PET_PANEL_LABEL) {
        let _ = existing.close();
    }
    Ok(())
}

/// Compute the panel's top-left origin (logical px) from the pet window's
/// logical rect (`px,py,pw,ph`), the current monitor's logical rect (`mon_*`),
/// and the panel size. Prefers placement above the pet, drops below if that
/// would clip the monitor's top edge, then clamps vertically into the monitor;
/// horizontally aligns the panel's right edge with the pet's, clamped into the
/// monitor. Pure (no Tauri handles) so the initial open and `resize_pet_panel`
/// re-anchor identically — and so it's unit-testable. Re-running it with a
/// larger `panel_h` is what keeps the panel attached as it grows.
#[allow(clippy::too_many_arguments)]
fn compute_pet_panel_origin(
    px: f64,
    py: f64,
    pw: f64,
    ph: f64,
    mon_x: f64,
    mon_y: f64,
    mon_w: f64,
    mon_h: f64,
    panel_w: f64,
    panel_h: f64,
) -> (f64, f64) {
    // Prefer above the pet; drop below if it would clip the top edge, then
    // clamp into the monitor either way.
    let mut panel_y = py - panel_h - PET_PANEL_GAP;
    if panel_y < mon_y {
        panel_y = py + ph + PET_PANEL_GAP;
    }
    let max_y = mon_y + mon_h - panel_h;
    if panel_y > max_y {
        panel_y = max_y.max(mon_y);
    }

    // Align the panel's right edge with the pet's, clamped horizontally.
    let mut panel_x = (px + pw) - panel_w;
    let max_x = mon_x + mon_w - panel_w;
    if panel_x > max_x {
        panel_x = max_x;
    }
    if panel_x < mon_x {
        panel_x = mon_x;
    }

    (panel_x, panel_y)
}

/// Pet + monitor logical rects, the shared input to [`compute_pet_panel_origin`]:
/// `(px, py, pw, ph, mon_x, mon_y, mon_w, mon_h)`.
type PetAnchorGeometry = (f64, f64, f64, f64, f64, f64, f64, f64);

/// Read the pet window's logical rect and its monitor's logical rect — the
/// shared input to [`compute_pet_panel_origin`]. Returns `None` if the pet
/// window isn't open. A missing monitor falls back to a generous default so
/// placement still resolves. All math is in logical pixels for DPI independence.
#[cfg(feature = "tauri-runtime")]
fn read_pet_anchor_geometry(app: &AppHandle) -> Option<PetAnchorGeometry> {
    let pet = app.get_webview_window(PET_WINDOW_LABEL)?;

    let sf = pet.scale_factor().unwrap_or(1.0);
    let (px, py, pw, ph) = match (pet.outer_position(), pet.outer_size()) {
        (Ok(pos), Ok(size)) => (
            pos.x as f64 / sf,
            pos.y as f64 / sf,
            size.width as f64 / sf,
            size.height as f64 / sf,
        ),
        _ => (0.0, 0.0, PET_BASE_WIDTH, PET_BASE_HEIGHT),
    };

    let (mon_x, mon_y, mon_w, mon_h) = pet
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let msf = m.scale_factor();
            let mp = m.position();
            let ms = m.size();
            (
                mp.x as f64 / msf,
                mp.y as f64 / msf,
                ms.width as f64 / msf,
                ms.height as f64 / msf,
            )
        })
        .unwrap_or((px, py - PET_PANEL_DEFAULT_HEIGHT, 1920.0, 1080.0));

    Some((px, py, pw, ph, mon_x, mon_y, mon_w, mon_h))
}

/// Create the panel anchored to the sprite at the default (empty-state) height.
/// The renderer measures its real content and calls `resize_pet_panel` to fit.
#[cfg(feature = "tauri-runtime")]
fn open_pet_panel_window(app: &AppHandle) -> Result<(), AppCommandError> {
    let (px, py, pw, ph, mon_x, mon_y, mon_w, mon_h) = read_pet_anchor_geometry(app)
        .ok_or_else(|| AppCommandError::window("Pet window not open", String::new()))?;

    let (panel_x, panel_y) = compute_pet_panel_origin(
        px,
        py,
        pw,
        ph,
        mon_x,
        mon_y,
        mon_w,
        mon_h,
        PET_PANEL_WIDTH,
        PET_PANEL_DEFAULT_HEIGHT,
    );

    let url = WebviewUrl::App("pet-panel".into());
    let builder = WebviewWindowBuilder::new(app, PET_PANEL_LABEL, url)
        .title("codeg sessions")
        .inner_size(PET_PANEL_WIDTH, PET_PANEL_DEFAULT_HEIGHT)
        .position(panel_x, panel_y)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .visible(false)
        .effects(
            EffectsBuilder::new()
                .effect(Effect::Popover)
                .state(EffectState::Active)
                .build(),
        )
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(true)
        .accept_first_mouse(true);

    apply_pet_window_style(builder)
        .build()
        .map_err(|e| AppCommandError::window("Failed to open pet panel", e.to_string()))?;

    Ok(())
}

/// Resize the open session panel to fit its measured content height (logical
/// px, reported by the panel renderer after layout) and re-anchor it to the pet
/// so it stays attached as the list grows or shrinks. No-op if the panel isn't
/// open — it can race a blur / Esc close. `height` is clamped to a usable floor
/// and a monitor-derived ceiling (never taller than the screen); the practical
/// upper bound is the panel's own scrollable list, so this clamp is just a
/// safety net for tiny displays.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn resize_pet_panel(app: AppHandle, height: f64) -> Result<(), AppCommandError> {
    let Some(panel) = app.get_webview_window(PET_PANEL_LABEL) else {
        return Ok(());
    };
    let Some((px, py, pw, ph, mon_x, mon_y, mon_w, mon_h)) = read_pet_anchor_geometry(&app) else {
        return Ok(());
    };

    let max_h = (mon_h - PET_PANEL_GAP).max(PET_PANEL_MIN_HEIGHT);
    let panel_h = height.clamp(PET_PANEL_MIN_HEIGHT, max_h);

    let (panel_x, panel_y) = compute_pet_panel_origin(
        px, py, pw, ph, mon_x, mon_y, mon_w, mon_h, PET_PANEL_WIDTH, panel_h,
    );

    // Size before reposition so the re-anchor uses the final height. Errors are
    // non-fatal: the caller fires this and forgets, and a failed resize just
    // leaves the panel at its previous size.
    let _ = panel.set_size(LogicalSize::new(PET_PANEL_WIDTH, panel_h));
    let _ = panel.set_position(LogicalPosition::new(panel_x, panel_y));
    let _ = panel.show();
    let _ = panel.set_focus();
    Ok(())
}

/// Bring the main workspace to the foreground and ask it to focus a specific
/// conversation. Uses an event (not a URL reload) so the in-memory tab/session
/// state survives — `PetFocusBridge` in the main window calls `openTab`.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn focus_conversation(
    app: AppHandle,
    folder_id: i32,
    conversation_id: i32,
    agent: String,
) -> Result<(), AppCommandError> {
    use tauri::Emitter;
    show_main_window(&app);
    let payload = serde_json::json!({
        "folderId": folder_id,
        "conversationId": conversation_id,
        "agent": agent,
    });
    app.emit_to("main", "workspace://focus-conversation", payload)
        .map_err(|e| {
            AppCommandError::window("Failed to signal main window", e.to_string())
        })?;
    Ok(())
}

// ─── Pet right-click context menu (native) ──────────────────────────────
//
// The pet window is intentionally tiny (a single sprite frame × user scale,
// e.g. 144×156 logical px at 0.75x). An HTML-rendered popup gets clipped to
// those bounds — items don't fit, and the user can't click "outside" because
// there is no outside inside the window. Popping a real OS menu via Tauri's
// `popup_menu_at` sidesteps the clip entirely and gets us native dismiss
// (Escape, click-elsewhere) for free. Item ids carry the action; the global
// `on_menu_event` listener wired up in `lib.rs` dispatches them.

/// Stable id namespace for pet menu items.
pub const PET_MENU_ID_PREFIX: &str = "pet:";
pub const PET_MENU_ID_OPEN_MANAGER: &str = "pet:open_manager";
pub const PET_MENU_ID_CLOSE: &str = "pet:close";
pub const PET_MENU_SCALE_PREFIX: &str = "pet:scale:";
/// Selectable scale steps. Display label is locale-independent (just digits +
/// "×"), so we don't translate it. The id `suffix` survives a round-trip
/// through the OS menu and back into our event dispatcher.
const PET_MENU_SCALE_STEPS: &[(f64, &str, &str)] = &[
    (0.5, "0.5×", "05"),
    (0.75, "0.75×", "075"),
    (1.0, "1×", "1"),
    (1.5, "1.5×", "15"),
    (2.0, "2×", "2"),
];

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetMenuLabels {
    pub scale: String,
    pub open_manager: String,
    pub close: String,
}

/// Map a menu item id back to its scale value. Used by the global menu event
/// dispatcher in `lib.rs` so the suffix→value table lives in one place.
pub fn pet_menu_scale_from_id(id: &str) -> Option<f64> {
    let suffix = id.strip_prefix(PET_MENU_SCALE_PREFIX)?;
    PET_MENU_SCALE_STEPS
        .iter()
        .find_map(|(value, _, s)| if *s == suffix { Some(*value) } else { None })
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn pet_show_context_menu(
    app: AppHandle,
    db: tauri::State<'_, AppDatabase>,
    labels: PetMenuLabels,
    x: f64,
    y: f64,
) -> Result<(), AppCommandError> {
    use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItem, PredefinedMenuItem};

    let pet_window = app
        .get_webview_window(PET_WINDOW_LABEL)
        .ok_or_else(|| AppCommandError::window("Pet window not open", String::new()))?;

    let config = crate::commands::pet::pet_get_settings_core(&db.conn).await?;
    let current = config.scale;

    let menu_err = |stage: &str, e: tauri::Error| AppCommandError::window(stage, e.to_string());

    // Disabled header acts as a "Scale" section label. macOS renders this as
    // dimmed gray text, Linux/Windows as a non-clickable item — close enough
    // to a section heading without depending on platform-specific section
    // APIs that don't exist in Tauri's cross-platform menu wrapper.
    let header = MenuItem::with_id(
        &app,
        format!("{PET_MENU_ID_PREFIX}header"),
        &labels.scale,
        false,
        None::<&str>,
    )
    .map_err(|e| menu_err("Failed to build pet menu header", e))?;
    let sep1 = PredefinedMenuItem::separator(&app)
        .map_err(|e| menu_err("Failed to build pet menu separator", e))?;
    let sep2 = PredefinedMenuItem::separator(&app)
        .map_err(|e| menu_err("Failed to build pet menu separator", e))?;

    let mut scale_items = Vec::with_capacity(PET_MENU_SCALE_STEPS.len());
    for (value, label, suffix) in PET_MENU_SCALE_STEPS {
        let id = format!("{PET_MENU_SCALE_PREFIX}{suffix}");
        let checked = (current - *value).abs() < 0.01;
        let item = CheckMenuItem::with_id(&app, id, *label, true, checked, None::<&str>)
            .map_err(|e| menu_err("Failed to build pet menu scale item", e))?;
        scale_items.push(item);
    }

    let open_mgr = MenuItem::with_id(
        &app,
        PET_MENU_ID_OPEN_MANAGER,
        &labels.open_manager,
        true,
        None::<&str>,
    )
    .map_err(|e| menu_err("Failed to build pet menu manager item", e))?;
    let close_item = MenuItem::with_id(&app, PET_MENU_ID_CLOSE, &labels.close, true, None::<&str>)
        .map_err(|e| menu_err("Failed to build pet menu close item", e))?;

    let mut builder = MenuBuilder::new(&app).item(&header).item(&sep1);
    for item in &scale_items {
        builder = builder.item(item);
    }
    let menu = builder
        .item(&sep2)
        .item(&open_mgr)
        .item(&close_item)
        .build()
        .map_err(|e| menu_err("Failed to build pet menu", e))?;

    pet_window
        .popup_menu_at(&menu, LogicalPosition::new(x, y))
        .map_err(|e| menu_err("Failed to popup pet menu", e))?;

    // Hover transition state needs a manual reset after the menu
    // dismisses — see `reset_pet_hover_after_native_menu`'s docs.
    reset_pet_hover_after_native_menu();

    Ok(())
}

/// Force the hover watcher to re-emit `enter` next tick if the cursor
/// is still over the pet.
///
/// `popup_menu_at` is modal on macOS / Windows, so this runs strictly
/// after the user has dismissed the menu. The cursor almost certainly
/// never appeared to leave the pet window from the watcher's view —
/// right-click happens *at* the pet, and the OS menu is just an
/// overlay sitting on top, so the polled cursor stays inside the
/// window's bounds the whole time. Without this reset, the watcher's
/// `was_inside == true` flag suppresses the next genuine hover-enter
/// and the wave animation silently stops working until the user moves
/// off-pet and back. Storing `false` makes the very next 80 ms tick
/// re-emit `enter` if the cursor is still over the pet, restoring
/// waving on the spot.
fn reset_pet_hover_after_native_menu() {
    PET_HOVER_WAS_INSIDE.store(false, AtomicOrdering::Relaxed);
}

/// Store the current zoom level and persist it to DB so the next launch
/// creates windows with the correct traffic-light position.
/// Existing windows are NOT repositioned at runtime.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_traffic_light_position(
    app: AppHandle,
    db: tauri::State<'_, AppDatabase>,
    zoom: f64,
) -> Result<(), AppCommandError> {
    let clamped = zoom.clamp(50.0, 300.0) as u32;

    #[cfg(target_os = "macos")]
    CURRENT_ZOOM.store(clamped, AtomicOrdering::Relaxed);

    // Persist to DB so the next launch reads the correct value.
    let _ =
        app_metadata_service::upsert_value(&db.conn, ZOOM_LEVEL_DB_KEY, &clamped.to_string()).await;

    let _ = app;
    Ok(())
}

/// Persist the user's appearance mode ("dark" / "light" / "system") to DB
/// and update the in-memory cache so that subsequent window creations use the
/// correct native background color.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_appearance_mode(
    db: tauri::State<'_, AppDatabase>,
    mode: String,
) -> Result<(), AppCommandError> {
    CACHED_APPEARANCE_MODE.store(mode_from_str(&mode), AtomicOrdering::Relaxed);

    let _ = app_metadata_service::upsert_value(&db.conn, APPEARANCE_MODE_DB_KEY, &mode).await;

    Ok(())
}

// ─── System tray icon ──────────────────────────────────────────────────

/// Monochrome template image for the macOS menu bar. AppKit treats an
/// `NSImage` with `isTemplate = true` as a mask: only the alpha channel
/// matters and the system tints it to match the menu-bar appearance
/// (light, dark, accent). The colored window icon won't get this
/// treatment and looks out of place next to other system icons.
#[cfg(all(feature = "tauri-runtime", target_os = "macos"))]
const MACOS_TRAY_TEMPLATE_PNG: &[u8] = include_bytes!("../../icons/tray-icon-template.png");

#[cfg(all(feature = "tauri-runtime", target_os = "macos"))]
fn load_macos_tray_template_icon() -> Result<tauri::image::Image<'static>, String> {
    let decoded = image::load_from_memory(MACOS_TRAY_TEMPLATE_PNG)
        .map_err(|e| format!("decode tray template png: {e}"))?
        .to_rgba8();
    let (w, h) = (decoded.width(), decoded.height());
    Ok(tauri::image::Image::new_owned(decoded.into_raw(), w, h))
}

/// Stable id namespace for tray menu items. Routed through the app-wide
/// `on_menu_event` handler in `lib.rs`.
pub const TRAY_MENU_ID_PREFIX: &str = "tray:";
pub const TRAY_MENU_ID_SHOW: &str = "tray:show";
pub const TRAY_MENU_ID_QUIT: &str = "tray:quit";
pub const TRAY_ICON_ID: &str = "codeg-tray";

/// True after `install_tray_icon` returns `Ok`. The hide-on-close path
/// in `lib.rs` consults this so we don't strand the user on systems
/// where the tray failed to install (Windows tray refused, etc.). On
/// Linux this is necessary-but-not-sufficient: the StatusNotifierWatcher
/// may be missing and the icon invisible even when build() returns Ok,
/// which is why `can_hide_to_tray()` reports false on Linux regardless.
#[cfg(feature = "tauri-runtime")]
static TRAY_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Whether hide-on-close is safe on this platform/session. When false,
/// the close handler in `lib.rs` forces a real app exit instead — both
/// `hide()` and `minimize()` would leave aux windows (pet, settings)
/// running without a recoverable workspace.
#[cfg(feature = "tauri-runtime")]
pub fn can_hide_to_tray() -> bool {
    // Linux: even with a successfully installed tray icon, modern GNOME
    // (45+) defaults ship without a StatusNotifierWatcher and the icon
    // is silently invisible. Refusing here forces the close to pass
    // through to a real exit on Linux — preferable to a phantom process
    // with no UI surface.
    if cfg!(target_os = "linux") {
        return false;
    }
    TRAY_AVAILABLE.load(AtomicOrdering::Relaxed)
}

/// Bring the hidden / minimized main workspace window back to the
/// foreground. Used by:
///   * single-instance plugin (second launch)
///   * tray icon left-click and "Show Workspace" menu item
///   * macOS dock-icon reopen
#[cfg(feature = "tauri-runtime")]
pub fn show_main_window(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
}

#[cfg(feature = "tauri-runtime")]
struct TrayLabels {
    show_workspace: &'static str,
    quit: &'static str,
}

#[cfg(feature = "tauri-runtime")]
fn tray_labels_for(locale: crate::models::system::AppLocale) -> TrayLabels {
    use crate::models::system::AppLocale;
    match locale {
        AppLocale::ZhCn => TrayLabels {
            show_workspace: "显示工作台",
            quit: "退出 Codeg",
        },
        AppLocale::ZhTw => TrayLabels {
            show_workspace: "顯示工作臺",
            quit: "退出 Codeg",
        },
        AppLocale::Ja => TrayLabels {
            show_workspace: "ワークスペースを表示",
            quit: "Codeg を終了",
        },
        AppLocale::Ko => TrayLabels {
            show_workspace: "워크스페이스 표시",
            quit: "Codeg 종료",
        },
        AppLocale::Es => TrayLabels {
            show_workspace: "Mostrar el área de trabajo",
            quit: "Salir de Codeg",
        },
        AppLocale::De => TrayLabels {
            show_workspace: "Arbeitsbereich anzeigen",
            quit: "Codeg beenden",
        },
        AppLocale::Fr => TrayLabels {
            show_workspace: "Afficher l'espace de travail",
            quit: "Quitter Codeg",
        },
        AppLocale::Pt => TrayLabels {
            show_workspace: "Mostrar área de trabalho",
            quit: "Sair do Codeg",
        },
        AppLocale::Ar => TrayLabels {
            show_workspace: "إظهار مساحة العمل",
            quit: "إنهاء Codeg",
        },
        AppLocale::En => TrayLabels {
            show_workspace: "Show Workspace",
            quit: "Quit Codeg",
        },
    }
}

/// Install the system tray icon and its right-click menu. Left-click
/// (Linux/Windows) and dock-style activation behaviors map to
/// `show_main_window`. Menu wiring lives in the app-wide
/// `on_menu_event` callback in `lib.rs` so the tray and pet menus share
/// one dispatcher.
#[cfg(feature = "tauri-runtime")]
pub fn install_tray_icon(
    app: &AppHandle,
    locale: crate::models::system::AppLocale,
) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let labels = tray_labels_for(locale);
    let show_item = MenuItem::with_id(
        app,
        TRAY_MENU_ID_SHOW,
        labels.show_workspace,
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_ID_QUIT, labels.quit, true, None::<&str>)?;
    let menu = MenuBuilder::new(app)
        .items(&[&show_item, &separator, &quit_item])
        .build()?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ICON_ID)
        .tooltip("Codeg")
        .menu(&menu)
        // `false` is required for `on_tray_icon_event::Click` to fire on
        // every platform we ship: the default `true` causes the OS to
        // consume left-click to pop the menu (notably on macOS — see
        // tauri-apps/tauri#11413). Right-click still shows the menu
        // because that's the OS's job, not ours.
        .show_menu_on_left_click(false);

    // macOS menu bar expects a monochrome template image that adapts to
    // light/dark mode and the user's accent settings. Other platforms
    // (Windows tray, Linux indicators) want the regular colored icon.
    #[cfg(target_os = "macos")]
    {
        match load_macos_tray_template_icon() {
            Ok(icon) => {
                builder = builder.icon(icon).icon_as_template(true);
            }
            Err(err) => {
                tracing::warn!("[Tray] failed to load template icon, falling back: {err}");
                if let Some(icon) = app.default_window_icon() {
                    builder = builder.icon(icon.clone());
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(icon) = app.default_window_icon() {
            builder = builder.icon(icon.clone());
        }
    }

    builder
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    TRAY_AVAILABLE.store(true, AtomicOrdering::Relaxed);
    Ok(())
}

/// Rebuild the tray menu in the supplied locale and swap it onto the
/// existing tray icon. No-op if the tray hasn't been installed yet
/// (e.g. the language change races setup, or the platform refused the
/// initial install).
#[cfg(feature = "tauri-runtime")]
pub fn refresh_tray_menu(
    app: &AppHandle,
    locale: crate::models::system::AppLocale,
) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItem, PredefinedMenuItem};

    let Some(tray) = app.tray_by_id(TRAY_ICON_ID) else {
        return Ok(());
    };

    let labels = tray_labels_for(locale);
    let show_item = MenuItem::with_id(
        app,
        TRAY_MENU_ID_SHOW,
        labels.show_workspace,
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_ID_QUIT, labels.quit, true, None::<&str>)?;
    let menu = MenuBuilder::new(app)
        .items(&[&show_item, &separator, &quit_item])
        .build()?;

    tray.set_menu(Some(menu))?;
    Ok(())
}

/// Push the current effective UI locale to the system tray. Called by
/// the i18n provider whenever the resolved app locale changes — covers
/// both manual selection and OS-driven changes in system mode.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn set_tray_locale(
    app: AppHandle,
    locale: crate::models::system::AppLocale,
) -> Result<(), AppCommandError> {
    refresh_tray_menu(&app, locale)
        .map_err(|e| AppCommandError::window("Failed to refresh tray menu", e.to_string()))
}

#[cfg(test)]
mod pet_panel_geometry_tests {
    use super::{compute_pet_panel_origin, PET_PANEL_GAP, PET_PANEL_WIDTH};

    // A roomy 1920×1080 monitor at the origin, with the pet near the bottom-right
    // (the common resting spot). Sprite ≈ 144×156 logical px (0.75× of 192×208).
    const MON: (f64, f64, f64, f64) = (0.0, 0.0, 1920.0, 1080.0);
    const PET_W: f64 = 144.0;
    const PET_H: f64 = 156.0;

    fn origin(pet_x: f64, pet_y: f64, panel_h: f64) -> (f64, f64) {
        compute_pet_panel_origin(
            pet_x,
            pet_y,
            PET_W,
            PET_H,
            MON.0,
            MON.1,
            MON.2,
            MON.3,
            PET_PANEL_WIDTH,
            panel_h,
        )
    }

    #[test]
    fn places_above_and_aligns_right_edge() {
        // Pet low on screen: the panel sits above it, gap included.
        let (x, y) = origin(1000.0, 900.0, 380.0);
        assert_eq!(y, 900.0 - 380.0 - PET_PANEL_GAP, "panel bottom hugs pet top");
        // Right edges align: panel_x = pet_right - panel_w.
        assert_eq!(x, (1000.0 + PET_W) - PET_PANEL_WIDTH);
    }

    #[test]
    fn drops_below_when_above_would_clip_top() {
        // Pet near the top: above placement is off-screen, so drop below the pet.
        let (_x, y) = origin(500.0, 20.0, 380.0);
        assert_eq!(y, 20.0 + PET_H + PET_PANEL_GAP, "panel drops below the pet");
    }

    #[test]
    fn taller_panel_anchored_above_grows_upward() {
        // With an above-anchor, a taller panel's top moves further up while its
        // bottom stays pinned near the pet — i.e. it grows upward.
        let (_x, short) = origin(1000.0, 900.0, 200.0);
        let (_x2, tall) = origin(1000.0, 900.0, 380.0);
        assert!(tall < short, "taller panel has a higher (smaller-y) top");
    }

    #[test]
    fn clamps_right_edge_into_monitor() {
        // Pet at the far right: aligning right edges would push the panel off the
        // monitor, so it clamps to the right work-area edge.
        let (x, _y) = origin(1850.0, 900.0, 380.0);
        assert_eq!(x, MON.2 - PET_PANEL_WIDTH, "clamped to right edge");
    }

    #[test]
    fn clamps_left_edge_into_monitor() {
        // Pet at the far left: right-edge alignment would yield a negative x, so
        // it clamps to the left work-area edge.
        let (x, _y) = origin(0.0, 900.0, 380.0);
        assert_eq!(x, MON.0, "clamped to left edge");
    }

    #[test]
    fn clamps_bottom_on_short_monitor() {
        // Short monitor where neither above nor below fully fits: vertical clamp
        // pins the panel into the monitor (and never above its top edge).
        let short_mon = (0.0, 0.0, 1920.0, 300.0);
        let (_x, y) = compute_pet_panel_origin(
            500.0,
            20.0,
            PET_W,
            PET_H,
            short_mon.0,
            short_mon.1,
            short_mon.2,
            short_mon.3,
            PET_PANEL_WIDTH,
            380.0,
        );
        assert_eq!(y, short_mon.1, "clamped to the monitor top, not above it");
    }
}
