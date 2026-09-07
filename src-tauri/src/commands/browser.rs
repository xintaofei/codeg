//! `browser_*` commands: thin parameter validation over the registry. The
//! `_core` functions are the real implementation and are also driven by the
//! dev-only smoke puppet, so every code path the frontend uses is the one the
//! P0/P1 checks exercised.

use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri::Url;

use crate::app_error::AppCommandError;
use crate::browser::registry::{BrowserRegistry, BrowserTab};
use crate::browser::surface::BrowserSurface;
use crate::browser::types::{
    Bounds, BrowserCapabilities, BrowserTabState, ChannelKind, SurfaceChoice, SurfaceKind,
};
use crate::browser::{events, policy, tab_label};

#[cfg(all(
    feature = "browser-child",
    any(target_os = "macos", target_os = "windows")
))]
const CHILD_SURFACE_COMPILED: bool = true;
#[cfg(not(all(
    feature = "browser-child",
    any(target_os = "macos", target_os = "windows")
)))]
const CHILD_SURFACE_COMPILED: bool = false;

fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

/// What this build can do on this machine. `channel` stays `degraded` until
/// the isolated-world channel installer lands; the frontend keys off
/// `available` and `surface`.
pub fn capabilities() -> BrowserCapabilities {
    let mut reasons = Vec::new();
    let surface = if CHILD_SURFACE_COMPILED {
        SurfaceKind::Child
    } else {
        reasons.push(if cfg!(target_os = "linux") {
            "linux: child webviews cannot be positioned; using owned windows".to_string()
        } else {
            "child surface not compiled; using owned windows".to_string()
        });
        SurfaceKind::Window
    };
    reasons.push("page channel not installed yet".to_string());
    BrowserCapabilities {
        available: true,
        surface: Some(surface),
        platform: platform_name().to_string(),
        channel: ChannelKind::Degraded,
        reasons,
    }
}

fn pick_surface(choice: SurfaceChoice) -> SurfaceKind {
    if CHILD_SURFACE_COMPILED && choice != SurfaceChoice::Window {
        SurfaceKind::Child
    } else {
        SurfaceKind::Window
    }
}

/// Tab ids become webview labels, and a label is also what the popup and
/// capability checks key on, so keep them to a safe alphabet.
fn validate_tab_id(tab_id: &str) -> Result<(), AppCommandError> {
    let ok = !tab_id.is_empty()
        && tab_id.len() <= 64
        && tab_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(AppCommandError::invalid_input(format!(
            "invalid browser tab id {tab_id:?}"
        )))
    }
}

fn parse_web_url(raw: &str) -> Result<Url, AppCommandError> {
    let url = Url::parse(raw.trim())
        .map_err(|e| AppCommandError::invalid_input(format!("invalid url {raw:?}: {e}")))?;
    if !policy::open_url_allowed(&url) {
        return Err(AppCommandError::invalid_input(format!(
            "url scheme not allowed in a browser tab: {raw:?}"
        )));
    }
    Ok(url)
}

/// `host[:port]` — an owned window's title never shows the path or query, so a
/// one-time token in an OAuth URL cannot leak through the window list.
pub fn origin_title(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => url.to_string(),
    }
}

fn window_err(what: &str, err: impl std::fmt::Display) -> AppCommandError {
    AppCommandError::window(what.to_string(), err.to_string())
}

pub struct OpenTabParams {
    pub tab_id: String,
    pub url: String,
    pub bounds: Bounds,
    pub background: bool,
    pub surface: SurfaceChoice,
    /// Build the surface with the web inspector available (user preference).
    pub devtools: bool,
}

pub fn open_tab_core(
    app: &AppHandle,
    owner: &WebviewWindow,
    registry: &BrowserRegistry,
    params: OpenTabParams,
) -> Result<BrowserTabState, AppCommandError> {
    validate_tab_id(&params.tab_id)?;
    if registry.contains(&params.tab_id) {
        return Err(AppCommandError::already_exists(format!(
            "browser tab {} is already open",
            params.tab_id
        )));
    }
    let url = parse_web_url(&params.url)?;
    let label = tab_label(&params.tab_id);

    let surface = match pick_surface(params.surface) {
        #[cfg(all(
            feature = "browser-child",
            any(target_os = "macos", target_os = "windows")
        ))]
        SurfaceKind::Child => BrowserSurface::Child(
            crate::browser::surface_child::create(
                app,
                owner,
                &params.tab_id,
                &label,
                params.bounds,
                params.background,
                params.devtools,
            )
            .map_err(|e| window_err("Failed to create browser webview", e))?,
        ),
        _ => BrowserSurface::Window(Box::new(
            crate::browser::surface_window::create(
                app,
                owner,
                &params.tab_id,
                &label,
                &origin_title(&url),
                params.background,
                params.devtools,
            )
            .map_err(|e| window_err("Failed to create browser window", e))?,
        )),
    };

    let state = BrowserTabState {
        tab_id: params.tab_id.clone(),
        owner_window: owner.label().to_string(),
        surface: surface.kind(),
        channel: ChannelKind::Degraded,
        url: String::new(),
        requested_url: url.to_string(),
        title: String::new(),
        favicon: None,
        loading: true,
        can_go_back: false,
        can_go_forward: false,
        origin: None,
        zoom: 1.0,
        error: None,
        remote_host: None,
        opener_tab_id: None,
    };
    if let Err(err) = registry.insert(BrowserTab::new(
        state.clone(),
        surface.clone(),
        params.bounds,
        !params.background,
        params.devtools,
    )) {
        let _ = surface.close();
        return Err(err);
    }
    // The helper must be in place before the first real document loads;
    // `about:blank` is still showing at this point. A failed install is not
    // fatal: the tab works, only the page channel is missing.
    let mut state = state;
    if surface.is_embedded() {
        match surface.install_channel() {
            // Stays `degraded` until the helper's `hello` proves the round trip.
            Ok(true) => {}
            Ok(false) => {
                if let Some(next) =
                    registry.update_state(&params.tab_id, |s| s.channel = ChannelKind::Legacy)
                {
                    state = next;
                }
            }
            Err(err) => {
                tracing::warn!(
                    "[browser] tab {}: page channel unavailable ({err}); continuing degraded",
                    params.tab_id
                );
            }
        }
    }
    if params.background && surface.is_embedded() {
        let _ = surface.hide();
    }
    if let Err(err) = surface.navigate(url) {
        registry.remove(&params.tab_id);
        let _ = surface.close();
        return Err(window_err("Failed to navigate browser tab", err));
    }
    events::emit_state(app, &state);
    Ok(state)
}

/// Wipe cookies, caches and every other kind of stored site data. All tabs
/// share one persistent store, so this is app-wide; open pages keep running
/// (nothing is reloaded, as in a browser).
pub async fn clear_data_core(app: &AppHandle, registry: &BrowserRegistry) -> Result<(), AppCommandError> {
    #[cfg(target_os = "macos")]
    {
        // Straight at the shared default store: works with no tab open and
        // reports completion, which a surface's `clear_all_browsing_data`
        // cannot.
        let _ = registry;
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
        let finish = move |result: Result<(), String>| {
            if let Some(tx) = tx.lock().unwrap_or_else(|p| p.into_inner()).take() {
                let _ = tx.send(result);
            }
        };
        app.run_on_main_thread(move || {
            let on_done = finish.clone();
            if let Err(err) = crate::browser::shim::macos::clear_default_data_store(move || on_done(Ok(()))) {
                finish(Err(err));
            }
        })
        .map_err(|e| window_err("Failed to clear browsing data", e))?;
        match tokio::time::timeout(std::time::Duration::from_secs(15), rx).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(err))) => Err(window_err("Failed to clear browsing data", err)),
            Ok(Err(_)) => Err(window_err("Failed to clear browsing data", "the request was dropped")),
            Err(_) => Err(window_err("Failed to clear browsing data", "timed out waiting for WebKit")),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Until the Windows / Linux shims land, clearing goes through a live
        // surface (they all share the store); with none open there is nothing
        // to call into.
        let _ = app;
        let Some(state) = registry.list().into_iter().next() else {
            return Err(AppCommandError::invalid_input(
                "open a page in the built-in browser first, then clear its data",
            ));
        };
        surface_of(registry, &state.tab_id)?
            .clear_browsing_data()
            .map_err(|e| window_err("Failed to clear browsing data", e))
    }
}

fn surface_of(registry: &BrowserRegistry, tab_id: &str) -> Result<BrowserSurface, AppCommandError> {
    registry
        .surface(tab_id)
        .ok_or_else(|| AppCommandError::not_found(format!("browser tab {tab_id} not found")))
}

pub fn close_core(app: &AppHandle, registry: &BrowserRegistry, tab_id: &str) -> Result<(), AppCommandError> {
    if let Some(tab) = registry.remove(tab_id) {
        let _ = tab.surface.close();
        events::emit_closed(app, tab_id, &tab.state.owner_window);
    }
    Ok(())
}

/// Called from the window-event hook when a window is destroyed: its tabs go
/// with it. Errors are ignored — a child webview of a destroyed window is
/// already gone.
pub fn close_all_for_owner(app: &AppHandle, owner_window: &str) {
    if let Some(registry) = app.try_state::<BrowserRegistry>() {
        for tab in registry.remove_by_owner(owner_window) {
            let _ = tab.surface.close();
        }
    }
}

pub fn set_bounds_core(
    registry: &BrowserRegistry,
    tab_id: &str,
    bounds: Bounds,
) -> Result<(), AppCommandError> {
    let surface = surface_of(registry, tab_id)?;
    let visible = registry
        .update(tab_id, |tab| {
            tab.last_bounds = bounds;
            tab.visible
        })
        .unwrap_or(false);
    // A hidden surface picks the bounds up again when it is shown; moving it
    // while hidden is wasted main-thread work on every split-pane drag.
    if visible && surface.is_embedded() {
        surface
            .set_bounds(bounds)
            .map_err(|e| window_err("Failed to move browser webview", e))?;
    }
    Ok(())
}

pub fn set_visible_core(
    owner: &WebviewWindow,
    registry: &BrowserRegistry,
    tab_id: &str,
    visible: bool,
    handoff_focus: bool,
) -> Result<(), AppCommandError> {
    let surface = surface_of(registry, tab_id)?;
    let bounds = registry
        .update(tab_id, |tab| {
            tab.visible = visible;
            tab.last_bounds
        })
        .unwrap_or_default();
    if visible {
        if surface.is_embedded() {
            surface
                .set_bounds(bounds)
                .map_err(|e| window_err("Failed to move browser webview", e))?;
        }
        surface
            .show()
            .map_err(|e| window_err("Failed to show browser surface", e))?;
    } else {
        // Keyboard focus must not stay inside a hidden native view: the
        // overlay that caused the hide would never receive Esc / Tab.
        if handoff_focus {
            let _ = owner.set_focus();
        }
        surface
            .hide()
            .map_err(|e| window_err("Failed to hide browser surface", e))?;
    }
    Ok(())
}

pub fn navigate_core(
    app: &AppHandle,
    registry: &BrowserRegistry,
    tab_id: &str,
    raw_url: &str,
) -> Result<BrowserTabState, AppCommandError> {
    let url = parse_web_url(raw_url)?;
    let surface = surface_of(registry, tab_id)?;
    let state = registry
        .update_state(tab_id, |state| {
            state.requested_url = url.to_string();
            state.loading = true;
            state.error = None;
        })
        .ok_or_else(|| AppCommandError::not_found(format!("browser tab {tab_id} not found")))?;
    surface
        .navigate(url)
        .map_err(|e| window_err("Failed to navigate browser tab", e))?;
    events::emit_state(app, &state);
    Ok(state)
}

pub fn reload_core(registry: &BrowserRegistry, tab_id: &str) -> Result<(), AppCommandError> {
    surface_of(registry, tab_id)?
        .reload()
        .map_err(|e| window_err("Failed to reload browser tab", e))
}

pub fn go_back_core(registry: &BrowserRegistry, tab_id: &str) -> Result<(), AppCommandError> {
    surface_of(registry, tab_id)?
        .go_back()
        .map_err(|e| window_err("Failed to go back", e))
}

pub fn go_forward_core(registry: &BrowserRegistry, tab_id: &str) -> Result<(), AppCommandError> {
    surface_of(registry, tab_id)?
        .go_forward()
        .map_err(|e| window_err("Failed to go forward", e))
}

pub fn stop_core(app: &AppHandle, registry: &BrowserRegistry, tab_id: &str) -> Result<(), AppCommandError> {
    surface_of(registry, tab_id)?
        .stop()
        .map_err(|e| window_err("Failed to stop loading", e))?;
    if let Some(state) = registry.update_state(tab_id, |s| s.loading = false) {
        events::emit_state(app, &state);
    }
    Ok(())
}

pub fn state_core(registry: &BrowserRegistry, tab_id: &str) -> Result<BrowserTabState, AppCommandError> {
    registry
        .state(tab_id)
        .ok_or_else(|| AppCommandError::not_found(format!("browser tab {tab_id} not found")))
}

#[tauri::command]
pub async fn browser_capabilities() -> Result<BrowserCapabilities, AppCommandError> {
    Ok(capabilities())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn browser_open_tab(
    app: AppHandle,
    window: WebviewWindow,
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
    url: String,
    bounds: Bounds,
    background: Option<bool>,
    surface: Option<SurfaceChoice>,
    folder_id: Option<i64>,
    devtools: Option<bool>,
) -> Result<BrowserTabState, AppCommandError> {
    // Folder scoping is a frontend concern (tab strip grouping); the backend
    // only needs the owner window.
    let _ = folder_id;
    open_tab_core(
        &app,
        &window,
        &registry,
        OpenTabParams {
            tab_id,
            url,
            bounds,
            background: background.unwrap_or(false),
            surface: surface.unwrap_or_default(),
            devtools: devtools.unwrap_or(false),
        },
    )
}

#[tauri::command]
pub async fn browser_clear_data(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
) -> Result<(), AppCommandError> {
    clear_data_core(&app, &registry).await
}

#[tauri::command]
pub async fn browser_close(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
) -> Result<(), AppCommandError> {
    close_core(&app, &registry, &tab_id)
}

#[tauri::command]
pub async fn browser_set_bounds(
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
    bounds: Bounds,
) -> Result<(), AppCommandError> {
    set_bounds_core(&registry, &tab_id, bounds)
}

#[tauri::command]
pub async fn browser_set_visible(
    window: WebviewWindow,
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
    visible: bool,
    handoff_focus: Option<bool>,
) -> Result<(), AppCommandError> {
    set_visible_core(
        &window,
        &registry,
        &tab_id,
        visible,
        handoff_focus.unwrap_or(false),
    )
}

#[tauri::command]
pub async fn browser_navigate(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
    url: String,
) -> Result<BrowserTabState, AppCommandError> {
    navigate_core(&app, &registry, &tab_id, &url)
}

#[tauri::command]
pub async fn browser_reload(
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
) -> Result<(), AppCommandError> {
    reload_core(&registry, &tab_id)
}

#[tauri::command]
pub async fn browser_go_back(
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
) -> Result<(), AppCommandError> {
    go_back_core(&registry, &tab_id)
}

#[tauri::command]
pub async fn browser_go_forward(
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
) -> Result<(), AppCommandError> {
    go_forward_core(&registry, &tab_id)
}

#[tauri::command]
pub async fn browser_stop(
    app: AppHandle,
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
) -> Result<(), AppCommandError> {
    stop_core(&app, &registry, &tab_id)
}

#[tauri::command]
pub async fn browser_get_state(
    registry: State<'_, BrowserRegistry>,
    tab_id: String,
) -> Result<BrowserTabState, AppCommandError> {
    state_core(&registry, &tab_id)
}

#[tauri::command]
pub async fn browser_list_tabs(
    window: WebviewWindow,
    registry: State<'_, BrowserRegistry>,
) -> Result<Vec<BrowserTabState>, AppCommandError> {
    Ok(registry.list_for_owner(window.label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_ids_are_label_safe() {
        assert!(validate_tab_id("b7e2c1d0-1a2b").is_ok());
        assert!(validate_tab_id("tab_1").is_ok());
        for bad in ["", "a b", "a/b", "a:b", "é", &"x".repeat(65)] {
            assert!(validate_tab_id(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn origin_title_hides_path_and_query() {
        let url = Url::parse("https://accounts.example.com/o/oauth2?state=SECRET").unwrap();
        assert_eq!(origin_title(&url), "accounts.example.com");
        let url = Url::parse("http://localhost:3000/app#x").unwrap();
        assert_eq!(origin_title(&url), "localhost:3000");
    }

    #[test]
    fn open_url_must_be_a_web_page() {
        assert!(parse_web_url(" https://example.com ").is_ok());
        assert!(parse_web_url("about:blank").is_ok());
        assert!(parse_web_url("file:///etc/hosts").is_err());
        assert!(parse_web_url("javascript:1").is_err());
        assert!(parse_web_url("not a url").is_err());
    }

    #[test]
    fn capabilities_report_a_surface() {
        let caps = capabilities();
        assert!(caps.available);
        assert!(caps.surface.is_some());
        assert!(!caps.platform.is_empty());
    }
}
