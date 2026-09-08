//! Owned-window surface: a top-level `WebviewWindow` attached to the owner
//! (`parent`). The only surface on Linux, the fallback everywhere else, and
//! the shape popups take when they cannot be adopted as tabs.

use tauri::webview::{DownloadEvent, PageLoadEvent};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use super::downloads;
use super::events;
use super::hooks;
use super::policy;
use super::profile;
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
        .on_download({
            let app = app.clone();
            let tab_id = tab_id.to_string();
            move |_webview, event| match event {
                DownloadEvent::Requested { url, destination } => {
                    downloads::requested(&app, &tab_id, url.as_str(), destination)
                }
                DownloadEvent::Finished { url, path, success } => {
                    downloads::finished(&app, url.as_str(), path, success);
                    true
                }
                // `DownloadEvent` is non-exhaustive: a variant added upstream
                // must not silently become "allowed".
                _ => false,
            }
        })
        .disable_drag_drop_handler()
        .zoom_hotkeys_enabled(true)
        .browser_extensions_enabled(false);
    // Same container and proxy as the embedded tabs, so a page behaves the
    // same whichever surface hosts it.
    #[cfg(target_os = "macos")]
    // wry falls back to the default store below macOS 14, exactly like the
    // shim does for embedded tabs; the proxy is a property of the store and
    // is in place once `profile::prepare` has run.
    let builder = builder.data_store_identifier(profile::DEFAULT_DATA_STORE_IDENTIFIER);
    #[cfg(target_os = "windows")]
    let builder = builder
        .data_directory(profile::directory(profile::DEFAULT_PROFILE_ID))
        .additional_browser_args(&profile::windows_browser_args(
            profile::frozen_proxy().as_ref(),
        ));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let builder = {
        let builder = builder.data_directory(profile::directory(profile::DEFAULT_PROFILE_ID));
        match profile::current_proxy() {
            Ok(Some(proxy)) => match Url::parse(&proxy.to_url_string()) {
                Ok(url) => builder.proxy_url(url),
                Err(_) => builder,
            },
            Ok(None) => builder,
            Err(reason) => {
                tracing::warn!("[browser] ignoring the configured proxy: {reason}");
                builder
            }
        }
    };
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
