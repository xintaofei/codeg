//! Fan-out of tab state to the frontend. One full `BrowserTabState` per
//! change on `browser://state`; the frontend filters by `tabId`.

use tauri::AppHandle;

use crate::web::event_bridge::{emit_event, EventEmitter};

use super::downloads::{BrowserDownload, DOWNLOAD_EVENT};
use super::types::{
    BrowserClosedPayload, BrowserNavigationBlockedPayload, BrowserOpenRequestPayload,
    BrowserPopupPayload, BrowserShortcutPayload, BrowserTabState, NavigationBlockReason,
    CLOSED_EVENT, NAVIGATION_BLOCKED_EVENT, OPEN_REQUEST_EVENT, POPUP_EVENT, SHORTCUT_EVENT,
    STATE_EVENT,
};

pub fn emit_state(app: &AppHandle, state: &BrowserTabState) {
    emit_event(&EventEmitter::Tauri(app.clone()), STATE_EVENT, state);
}

pub fn emit_closed(app: &AppHandle, tab_id: &str, owner_window: &str) {
    emit_event(
        &EventEmitter::Tauri(app.clone()),
        CLOSED_EVENT,
        BrowserClosedPayload {
            tab_id: tab_id.to_string(),
            owner_window: owner_window.to_string(),
        },
    );
}

pub fn emit_popup(app: &AppHandle, payload: &BrowserPopupPayload) {
    emit_event(&EventEmitter::Tauri(app.clone()), POPUP_EVENT, payload);
}

pub fn emit_open_request(app: &AppHandle, payload: &BrowserOpenRequestPayload) {
    emit_event(&EventEmitter::Tauri(app.clone()), OPEN_REQUEST_EVENT, payload);
}

pub fn emit_shortcut(app: &AppHandle, tab_id: &str, shortcut: &str) {
    emit_event(
        &EventEmitter::Tauri(app.clone()),
        SHORTCUT_EVENT,
        BrowserShortcutPayload {
            tab_id: tab_id.to_string(),
            shortcut: shortcut.to_string(),
        },
    );
}

pub fn emit_download(app: &AppHandle, download: &BrowserDownload) {
    emit_event(&EventEmitter::Tauri(app.clone()), DOWNLOAD_EVENT, download);
}

pub fn emit_navigation_blocked(app: &AppHandle, tab_id: &str, url: &str, reason: NavigationBlockReason) {
    emit_event(
        &EventEmitter::Tauri(app.clone()),
        NAVIGATION_BLOCKED_EVENT,
        BrowserNavigationBlockedPayload {
            tab_id: tab_id.to_string(),
            url: url.to_string(),
            reason,
        },
    );
}
