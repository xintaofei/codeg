//! Embedded surface for macOS / Windows: a wry child webview built straight
//! through tauri-runtime-wry's re-exported `wry` (`build_as_child` on the
//! owner window — the same call tauri-runtime-wry makes for its own child
//! webviews). tauri never learns about the view, so the workspace window stays
//! an ordinary `WebviewWindow` (see the `browser-child` note in Cargo.toml).
//!
//! wry's `WebView` is not `Send`; every instance lives in a thread-local on
//! the main thread and is only ever touched there. `ChildHandle` is the
//! `Send + Clone` stand-in the registry and the commands hold: each operation
//! hops to the main thread and waits for the answer, or runs inline when the
//! caller already is the main thread (wry handlers, window-event hooks).
//! Never call into a handle while holding the registry mutex — the main
//! thread may need that mutex to finish the very operation being waited on.
//!
//! Page-initiated new windows (`window.open`, `target=_blank`) are handled
//! here too: the engine asks for a webview, we build one from the opener's
//! configuration (which is what keeps `window.opener` alive) and hand it back
//! with `NewWindowResponse::Create`, registering it as a new tab next to the
//! opener. The host never navigates on the page's behalf.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::thread::ThreadId;
use std::time::Duration;

use tauri::{AppHandle, Manager, Url, WebviewWindow};
use tauri_runtime_wry::wry::{
    self, dpi, NewWindowFeatures, NewWindowResponse, PageLoadEvent, Rect, WebViewBuilder,
};

use super::channel::{self, MessageSink};
use super::events;
use super::hooks;
use super::policy;
use super::registry::{BrowserRegistry, BrowserTab};
use super::surface::BrowserSurface;
use super::types::{
    Bounds, BrowserOpenRequestPayload, BrowserPopupPayload, BrowserTabState, ChannelKind,
    PopupPresentation, SurfaceKind,
};
#[cfg(target_os = "macos")]
use super::shim::macos as shim;

thread_local! {
    static SURFACES: RefCell<HashMap<String, wry::WebView>> = RefCell::new(HashMap::new());
}

static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();
static POPUP_SEQ: AtomicU64 = AtomicU64::new(0);

/// How far back a page-initiated new-window request may look for a user
/// gesture before it counts as an unsolicited popup.
pub const POPUP_GESTURE_WINDOW: Duration = Duration::from_secs(1);

/// Must be called once from the main thread (tauri's `setup` hook) before any
/// surface is created; lets `ChildHandle` run inline instead of deadlocking
/// when it is used from a wry callback.
pub fn init_main_thread() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
}

fn on_main_thread() -> bool {
    MAIN_THREAD.get() == Some(&std::thread::current().id())
}

#[derive(Debug, thiserror::Error)]
pub enum ChildError {
    #[error("browser surface {0} is gone")]
    Gone(String),
    #[error("main-thread dispatch failed: {0}")]
    Dispatch(String),
    #[error("{0}")]
    Op(String),
}

fn run_on_main<R: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> R + Send + 'static,
) -> Result<R, ChildError> {
    if on_main_thread() {
        return Ok(f());
    }
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| ChildError::Dispatch(e.to_string()))?;
    rx.recv().map_err(|e| ChildError::Dispatch(e.to_string()))
}

fn rect(bounds: Bounds) -> Rect {
    Rect {
        position: dpi::Position::Logical(dpi::LogicalPosition::new(bounds.x, bounds.y)),
        size: dpi::Size::Logical(dpi::LogicalSize::new(bounds.width, bounds.height)),
    }
}

/// Main thread only: which tab owns the platform webview behind `pointer`
/// (see `channel::MessageSink`).
#[cfg(target_os = "macos")]
fn tab_id_for_webview(pointer: usize) -> Option<String> {
    SURFACES.with(|s| {
        s.borrow()
            .iter()
            .find(|(_, wv)| shim::webview_pointer(wv) == pointer)
            .map(|(id, _)| id.clone())
    })
}

/// One sink for every tab: messages are attributed by source webview, because
/// an adopted popup shares its opener's user-content controller.
#[cfg(target_os = "macos")]
fn message_sink(app: &AppHandle) -> MessageSink {
    let app = app.clone();
    Arc::new(move |raw, main_frame, source| match tab_id_for_webview(source) {
        Some(tab_id) => channel::handle_message(&app, &tab_id, raw, main_frame),
        None => tracing::debug!("[browser] channel message from an unknown webview dropped"),
    })
}

#[derive(Clone, Debug)]
pub struct ChildHandle {
    tab_id: String,
    label: String,
    app: AppHandle,
}

impl ChildHandle {
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Run `f` against the live webview on the main thread.
    fn with<R: Send + 'static>(
        &self,
        f: impl FnOnce(&wry::WebView) -> R + Send + 'static,
    ) -> Result<R, ChildError> {
        let id = self.tab_id.clone();
        run_on_main(&self.app, move || {
            SURFACES.with(|s| s.borrow().get(&id).map(f))
        })?
        .ok_or_else(|| ChildError::Gone(self.tab_id.clone()))
    }

    fn op(
        &self,
        f: impl FnOnce(&wry::WebView) -> wry::Result<()> + Send + 'static,
    ) -> Result<(), ChildError> {
        self.with(move |wv| f(wv).map_err(|e| e.to_string()))?
            .map_err(ChildError::Op)
    }

    pub fn load_url(&self, url: &str) -> Result<(), ChildError> {
        let url = url.to_string();
        self.op(move |wv| wv.load_url(&url))
    }

    pub fn reload(&self) -> Result<(), ChildError> {
        self.op(|wv| wv.reload())
    }

    pub fn url(&self) -> Result<String, ChildError> {
        self.with(|wv| wv.url().map_err(|e| e.to_string()))?
            .map_err(ChildError::Op)
    }

    pub fn evaluate_script(&self, js: &str) -> Result<(), ChildError> {
        let js = js.to_string();
        self.op(move |wv| wv.evaluate_script(&js))
    }

    pub fn evaluate_script_with_callback(
        &self,
        js: &str,
        callback: impl Fn(String) + Send + 'static,
    ) -> Result<(), ChildError> {
        let js = js.to_string();
        self.op(move |wv| wv.evaluate_script_with_callback(&js, callback))
    }

    pub fn set_bounds(&self, bounds: Bounds) -> Result<(), ChildError> {
        self.op(move |wv| wv.set_bounds(rect(bounds)))
    }

    pub fn set_visible(&self, visible: bool) -> Result<(), ChildError> {
        self.op(move |wv| wv.set_visible(visible))
    }

    pub fn focus(&self) -> Result<(), ChildError> {
        self.op(|wv| wv.focus())
    }

    pub fn zoom(&self, factor: f64) -> Result<(), ChildError> {
        self.op(move |wv| wv.zoom(factor))
    }

    pub fn open_devtools(&self) -> Result<(), ChildError> {
        self.with(|wv| wv.open_devtools())
    }

    pub fn clear_all_browsing_data(&self) -> Result<(), ChildError> {
        self.op(|wv| wv.clear_all_browsing_data())
    }

    /// Install the isolated-world helper and the native message handler.
    /// `Ok(true)` = isolated world, `Ok(false)` = page-world fallback.
    pub fn install_channel(&self) -> Result<bool, ChildError> {
        #[cfg(target_os = "macos")]
        {
            let sink = message_sink(&self.app);
            self.with(move |wv| {
                shim::install_world(wv, &[channel::PREFIX_SCRIPT, channel::HELPER_JS], sink)
            })?
            .map_err(ChildError::Op)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(ChildError::Op(
                "page channel is not implemented on this platform yet".into(),
            ))
        }
    }

    /// Evaluate an expression in the helper's world; see `shim::eval_in_world`
    /// for the result envelope.
    pub fn eval_in_world(
        &self,
        expression: &str,
        callback: impl Fn(Result<String, String>) + Send + 'static,
    ) -> Result<(), ChildError> {
        #[cfg(target_os = "macos")]
        {
            let expression = expression.to_string();
            self.with(move |wv| shim::eval_in_world(wv, &expression, callback))?
                .map_err(ChildError::Op)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (expression, callback);
            Err(ChildError::Op(
                "world evaluation is not implemented on this platform yet".into(),
            ))
        }
    }

    pub fn snapshot_png(
        &self,
        callback: impl Fn(Result<Vec<u8>, String>) + Send + 'static,
    ) -> Result<(), ChildError> {
        #[cfg(target_os = "macos")]
        {
            self.with(move |wv| shim::snapshot_png(wv, callback))?
                .map_err(ChildError::Op)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = callback;
            Err(ChildError::Op(
                "snapshots are not implemented on this platform yet".into(),
            ))
        }
    }

    pub fn go_back(&self) -> Result<(), ChildError> {
        #[cfg(target_os = "macos")]
        {
            self.with(shim::go_back)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(ChildError::Op(
                "history navigation is not implemented on this platform yet".into(),
            ))
        }
    }

    pub fn go_forward(&self) -> Result<(), ChildError> {
        #[cfg(target_os = "macos")]
        {
            self.with(shim::go_forward)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(ChildError::Op(
                "history navigation is not implemented on this platform yet".into(),
            ))
        }
    }

    pub fn can_go_back(&self) -> Result<bool, ChildError> {
        #[cfg(target_os = "macos")]
        {
            self.with(shim::can_go_back)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(false)
        }
    }

    pub fn can_go_forward(&self) -> Result<bool, ChildError> {
        #[cfg(target_os = "macos")]
        {
            self.with(shim::can_go_forward)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(false)
        }
    }

    pub fn stop(&self) -> Result<(), ChildError> {
        #[cfg(target_os = "macos")]
        {
            self.with(shim::stop_loading)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(ChildError::Op(
                "stop is not implemented on this platform yet".into(),
            ))
        }
    }

    /// Dev puppet only: native-side state for a tab.
    pub fn debug_view(&self) -> Result<serde_json::Value, ChildError> {
        #[cfg(target_os = "macos")]
        {
            self.with(|wv| {
                let mut v = shim::debug_view(wv);
                v["wryBounds"] = wv
                    .bounds()
                    .map(|b| serde_json::json!(format!("{b:?}")))
                    .unwrap_or(serde_json::Value::Null);
                v
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(serde_json::Value::Null)
        }
    }

    /// Detach and drop the webview (on the main thread; wry removes the
    /// native view from the window when the `WebView` drops).
    pub fn close(&self) -> Result<(), ChildError> {
        let id = self.tab_id.clone();
        let removed = run_on_main(&self.app, move || {
            SURFACES.with(|s| s.borrow_mut().remove(&id)).is_some()
        })?;
        if removed {
            Ok(())
        } else {
            Err(ChildError::Gone(self.tab_id.clone()))
        }
    }
}

/// Opener-provided platform configuration for a popup webview.
#[cfg(target_os = "macos")]
type OpenerConfiguration = objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration>;
#[cfg(not(target_os = "macos"))]
type OpenerConfiguration = ();

/// Main thread only. Builds the child webview at `bounds` with every hook
/// attached and **no URL**: a regular tab is navigated by the caller once the
/// page channel is installed, a popup is navigated by the engine itself.
fn build_child(
    app: &AppHandle,
    owner: &WebviewWindow,
    tab_id: &str,
    label: &str,
    bounds: Bounds,
    visible: bool,
    configuration: Option<OpenerConfiguration>,
) -> Result<wry::WebView, String> {
    let nav_id = tab_id.to_string();
    let nav_app = app.clone();
    let nav_owner = owner.label().to_string();
    #[allow(unused_mut)]
    let mut builder = WebViewBuilder::new()
        .with_id(label)
        .with_bounds(rect(bounds))
        .with_visible(visible)
        .with_focused(false)
        .with_devtools(true)
        .with_hotkeys_zoom(true)
        .with_navigation_handler(move |url| {
            let allowed = Url::parse(&url)
                .map(|u| policy::navigation_allowed(&u))
                .unwrap_or(false);
            if !allowed {
                tracing::info!("[browser] tab {nav_id} blocked navigation to {url}");
                return false;
            }
            // ⌘/Ctrl-click on a plain anchor: the page did not prevent the
            // default, so the engine is about to navigate this tab. Browsers
            // open a background tab instead; so do we — the host cancels the
            // in-place navigation and asks the frontend for a new tab. (No
            // JS involved: a page can always add a later listener, so only
            // the navigation itself is a reliable signal.)
            if let Some(registry) = nav_app.try_state::<BrowserRegistry>() {
                if registry.take_modifier_click(&nav_id, &url, POPUP_GESTURE_WINDOW) {
                    events::emit_open_request(
                        &nav_app,
                        &BrowserOpenRequestPayload {
                            url: url.clone(),
                            source: "modifier-click".to_string(),
                            activate: false,
                            owner_window: Some(nav_owner.clone()),
                            opener_tab_id: Some(nav_id.clone()),
                        },
                    );
                    return false;
                }
            }
            true
        })
        .with_on_page_load_handler({
            let app = app.clone();
            let id = tab_id.to_string();
            move |event, url| {
                if let Ok(url) = Url::parse(&url) {
                    hooks::page_load(&app, &id, &url, matches!(event, PageLoadEvent::Started));
                }
            }
        })
        .with_document_title_changed_handler({
            let app = app.clone();
            let id = tab_id.to_string();
            move |title| hooks::title_changed(&app, &id, title)
        })
        // Downloads are refused until the download UI exists (P2).
        .with_download_started_handler(|_url, _destination| false)
        .with_new_window_req_handler(new_window_handler(app.clone(), owner.clone(), tab_id.to_string()));
    #[cfg(target_os = "macos")]
    if let Some(configuration) = configuration {
        use tauri_runtime_wry::wry::WebViewBuilderExtMacos;
        builder = builder.with_webview_configuration(configuration);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = configuration;
    builder.build_as_child(owner).map_err(|e| e.to_string())
}

/// Build the child webview for a regular tab. The caller navigates afterwards,
/// once the page ↔ host channel is installed.
pub fn create(
    app: &AppHandle,
    owner: &WebviewWindow,
    tab_id: &str,
    label: &str,
    bounds: Bounds,
    background: bool,
) -> Result<ChildHandle, ChildError> {
    let handle = ChildHandle {
        tab_id: tab_id.to_string(),
        label: label.to_string(),
        app: app.clone(),
    };
    let app = app.clone();
    let owner = owner.clone();
    let id = tab_id.to_string();
    let label = label.to_string();
    run_on_main(&app.clone(), move || -> Result<(), String> {
        let webview = build_child(&app, &owner, &id, &label, bounds, !background, None)?;
        SURFACES.with(|s| s.borrow_mut().insert(id, webview));
        Ok(())
    })?
    .map_err(ChildError::Op)?;
    Ok(handle)
}

fn deny(app: &AppHandle, opener_tab_id: &str, url: &str, features: &NewWindowFeatures, reason: &str) -> NewWindowResponse {
    tracing::info!("[browser] tab {opener_tab_id}: new-window request for {url} denied ({reason})");
    events::emit_popup(
        app,
        &BrowserPopupPayload {
            presentation: PopupPresentation::Denied,
            opener_tab_id: opener_tab_id.to_string(),
            tab_id: None,
            url: url.to_string(),
            requested_size: features.size.map(|s| [s.width, s.height]),
            reason: Some(reason.to_string()),
        },
    );
    NewWindowResponse::Deny
}

/// wry calls this on the main thread for every page-initiated new window.
/// Policy (v3 §5.3): scheme allow-list, then a user gesture within
/// `POPUP_GESTURE_WINDOW` (the popup blocker), then `Create` — the engine
/// navigates its own webview, so `window.opener`, `Referer` and `noopener`
/// semantics are exactly what the page asked for; the host only decides how
/// to present it, and for now every popup is adopted as a tab beside its
/// opener.
fn new_window_handler(
    app: AppHandle,
    owner: WebviewWindow,
    opener_tab_id: String,
) -> impl Fn(String, NewWindowFeatures) -> NewWindowResponse + 'static {
    move |url, features| {
        let Some(registry) = app.try_state::<BrowserRegistry>() else {
            return NewWindowResponse::Deny;
        };
        let parsed = match Url::parse(&url) {
            Ok(u) if policy::navigation_allowed(&u) => u,
            _ => return deny(&app, &opener_tab_id, &url, &features, "blocked-scheme"),
        };
        let has_gesture = registry
            .recent_gestures(&opener_tab_id)
            .iter()
            .any(|g| g.received.elapsed() <= POPUP_GESTURE_WINDOW);
        if !has_gesture {
            return deny(&app, &opener_tab_id, &url, &features, "no-gesture");
        }

        #[cfg(target_os = "macos")]
        {
            let seq = POPUP_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
            let tab_id = format!("{opener_tab_id}-p{seq}");
            let label = super::tab_label(&tab_id);
            let bounds = registry
                .update(&opener_tab_id, |tab| tab.last_bounds)
                .unwrap_or_default();
            let configuration = features.opener.target_configuration.clone();
            let webview = match build_child(&app, &owner, &tab_id, &label, bounds, true, Some(configuration)) {
                Ok(webview) => webview,
                Err(err) => {
                    tracing::warn!("[browser] popup webview creation failed: {err}");
                    return deny(&app, &opener_tab_id, &url, &features, "create-failed");
                }
            };
            // The platform object WebKit will load the request into.
            let platform = objc2::rc::Retained::into_super(
                tauri_runtime_wry::wry::WebViewExtMacOS::webview(&webview),
            );
            SURFACES.with(|s| s.borrow_mut().insert(tab_id.clone(), webview));
            let handle = ChildHandle {
                tab_id: tab_id.clone(),
                label,
                app: app.clone(),
            };
            // Same controller as the opener in practice, so this is a no-op
            // that still reports the channel kind; a fresh controller gets the
            // full install. Either way it happens before WebKit loads anything.
            let channel = match handle.install_channel() {
                Ok(true) => ChannelKind::Degraded, // native once `hello` arrives
                Ok(false) => ChannelKind::Legacy,
                Err(err) => {
                    tracing::warn!("[browser] popup {tab_id}: page channel unavailable ({err})");
                    ChannelKind::Degraded
                }
            };
            let state = BrowserTabState {
                tab_id: tab_id.clone(),
                owner_window: owner.label().to_string(),
                surface: SurfaceKind::Child,
                channel,
                url: String::new(),
                requested_url: parsed.to_string(),
                title: String::new(),
                favicon: None,
                loading: true,
                can_go_back: false,
                can_go_forward: false,
                origin: None,
                zoom: 1.0,
                error: None,
                remote_host: None,
                opener_tab_id: Some(opener_tab_id.clone()),
            };
            if let Err(err) = registry.insert(BrowserTab::new(
                state.clone(),
                BrowserSurface::Child(handle),
                bounds,
                true,
            )) {
                tracing::warn!("[browser] popup registry insert failed: {err}");
                SURFACES.with(|s| s.borrow_mut().remove(&tab_id));
                return deny(&app, &opener_tab_id, &url, &features, "registry");
            }
            events::emit_state(&app, &state);
            events::emit_popup(
                &app,
                &BrowserPopupPayload {
                    presentation: PopupPresentation::Adopted,
                    opener_tab_id: opener_tab_id.clone(),
                    tab_id: Some(tab_id),
                    url: parsed.to_string(),
                    requested_size: features.size.map(|s| [s.width, s.height]),
                    reason: None,
                },
            );
            NewWindowResponse::Create { webview: platform }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (&owner, parsed);
            deny(&app, &opener_tab_id, &url, &features, "unsupported-platform")
        }
    }
}
