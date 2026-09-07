//! Owned-window surface: a top-level `WebviewWindow` attached to the owner
//! (`parent`). The only surface on Linux, the fallback everywhere else, and
//! the shape popups take when they cannot be adopted as tabs.

use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use super::events;
use super::hooks;
use super::policy;
use super::registry::BrowserRegistry;

pub fn create(
    app: &AppHandle,
    owner: &WebviewWindow,
    tab_id: &str,
    label: &str,
    title: &str,
    background: bool,
    devtools: bool,
) -> tauri::Result<WebviewWindow> {
    let blank = Url::parse("about:blank").expect("static url");
    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::External(blank))
        .title(title)
        .inner_size(1100.0, 760.0)
        .min_inner_size(480.0, 320.0)
        .focused(!background)
        .devtools(devtools)
        .on_navigation(policy::navigation_allowed)
        .on_page_load({
            let app = app.clone();
            let tab_id = tab_id.to_string();
            move |_window, payload| {
                hooks::page_load(
                    &app,
                    &tab_id,
                    payload.url(),
                    matches!(payload.event(), PageLoadEvent::Started),
                )
            }
        })
        .on_document_title_changed({
            let app = app.clone();
            let tab_id = tab_id.to_string();
            move |_window, title| hooks::title_changed(&app, &tab_id, title)
        })
        .on_download(|_webview, _event| false)
        .disable_drag_drop_handler()
        .zoom_hotkeys_enabled(true)
        .browser_extensions_enabled(false);
    let builder = builder.parent(owner)?;
    let window = builder.build()?;

    // The user can close an owned window directly; drop the tab and tell the
    // frontend so the tab strip follows.
    {
        let app = app.clone();
        let tab_id = tab_id.to_string();
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                if let Some(registry) = app.try_state::<BrowserRegistry>() {
                    if let Some(tab) = registry.remove(&tab_id) {
                        events::emit_closed(&app, &tab_id, &tab.state.owner_window);
                    }
                }
            }
        });
    }
    Ok(window)
}
