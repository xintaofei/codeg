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

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread::ThreadId;

use tauri::{AppHandle, Url, WebviewWindow};
use tauri_runtime_wry::wry::{self, dpi, PageLoadEvent, Rect, WebViewBuilder};

use super::hooks;
use super::policy;
use super::types::Bounds;

thread_local! {
    static SURFACES: RefCell<HashMap<String, wry::WebView>> = RefCell::new(HashMap::new());
}

static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();

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

/// Build the child webview on `about:blank` at `bounds`. The caller navigates
/// afterwards, once the page ↔ host channel is installed.
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
    let owner = owner.clone();
    let app_for_hooks = app.clone();
    let id = tab_id.to_string();
    let label = label.to_string();
    run_on_main(app, move || -> Result<(), String> {
        let nav_id = id.clone();
        let builder = WebViewBuilder::new()
            .with_id(&label)
            .with_url("about:blank")
            .with_bounds(rect(bounds))
            .with_visible(!background)
            .with_focused(false)
            .with_devtools(true)
            .with_hotkeys_zoom(true)
            .with_navigation_handler(move |url| {
                let allowed = Url::parse(&url)
                    .map(|u| policy::navigation_allowed(&u))
                    .unwrap_or(false);
                if !allowed {
                    tracing::info!("[browser] tab {nav_id} blocked navigation to {url}");
                }
                allowed
            })
            .with_on_page_load_handler({
                let app = app_for_hooks.clone();
                let id = id.clone();
                move |event, url| {
                    if let Ok(url) = Url::parse(&url) {
                        hooks::page_load(&app, &id, &url, matches!(event, PageLoadEvent::Started));
                    }
                }
            })
            .with_document_title_changed_handler({
                let app = app_for_hooks.clone();
                let id = id.clone();
                move |title| hooks::title_changed(&app, &id, title)
            })
            // Downloads are refused until the download UI exists (P2).
            .with_download_started_handler(|_url, _destination| false);
        let webview = builder
            .build_as_child(&owner)
            .map_err(|e| e.to_string())?;
        SURFACES.with(|s| s.borrow_mut().insert(id, webview));
        Ok(())
    })?
    .map_err(ChildError::Op)?;
    Ok(handle)
}
