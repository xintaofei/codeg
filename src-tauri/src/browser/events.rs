//! Fan-out of tab state to the frontend. One full `BrowserTabState` per
//! change on `browser://state`; the frontend filters by `tabId`.

use tauri::AppHandle;

use crate::web::event_bridge::{emit_event, EventEmitter};

use super::types::{BrowserClosedPayload, BrowserTabState, CLOSED_EVENT, STATE_EVENT};

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
