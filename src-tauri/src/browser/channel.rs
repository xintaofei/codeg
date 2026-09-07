//! Page → host channel: the helper script (`src/browser-injected/helper.js`)
//! runs in an isolated world and posts JSON envelopes through a native
//! message handler; this module validates and routes them. Everything that
//! arrives here is page-controlled input: sizes are capped, unknown kinds
//! are dropped, and gestures are forwarded to the frontend flagged
//! `untrusted`.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Url};

use crate::web::event_bridge::{emit_event, EventEmitter};

use super::events;
use super::hooks;
use super::registry::BrowserRegistry;
use super::types::ChannelKind;

pub const HELPER_JS: &str = include_str!("../../../src/browser-injected/helper.js");
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const TELEMETRY_EVENT: &str = "browser://telemetry";

/// `(message, is_main_frame, source webview pointer)`. The pointer identifies
/// the webview the message came from: a popup created from an opener shares
/// the opener's user-content controller (and therefore its message handler),
/// so the handler cannot be bound to one tab.
pub type MessageSink = Arc<dyn Fn(String, bool, usize) + Send + Sync>;

/// Defines the send primitive the helper calls; injected before the helper,
/// in the same world, so the page never sees either.
#[cfg(target_os = "macos")]
pub const PREFIX_SCRIPT: &str = "globalThis.__codegSend = function (m) { window.webkit.messageHandlers.codegBrowser.postMessage(String(m)); };";

#[derive(Debug, Deserialize)]
pub struct Envelope {
    pub kind: String,
    #[serde(default)]
    pub payload: Value,
    /// Set by the helper when it runs in the top-level browsing context.
    #[serde(default)]
    pub top: bool,
}

pub fn parse_envelope(raw: &str) -> Option<Envelope> {
    if raw.len() > MAX_MESSAGE_BYTES {
        return None;
    }
    serde_json::from_str::<Envelope>(raw).ok()
}

pub fn handle_message(app: &AppHandle, tab_id: &str, raw: String, main_frame: bool) {
    let Some(envelope) = parse_envelope(&raw) else {
        tracing::warn!(
            "[browser] tab {tab_id}: dropped malformed or oversized channel message ({} bytes)",
            raw.len()
        );
        return;
    };
    let Some(registry) = app.try_state::<BrowserRegistry>() else {
        return;
    };
    match envelope.kind.as_str() {
        "hello" => {
            if !(main_frame && envelope.top) {
                return;
            }
            let state = registry.update_state(tab_id, |state| {
                if state.channel != ChannelKind::Legacy {
                    state.channel = ChannelKind::Native;
                }
            });
            if let Some(state) = state {
                events::emit_state(app, &state);
            }
        }
        "nav-state" => {
            if !(main_frame && envelope.top) {
                return;
            }
            let href = envelope
                .payload
                .get("href")
                .and_then(Value::as_str)
                .and_then(|h| Url::parse(h).ok());
            let title = envelope
                .payload
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string);
            let changed = registry.update(tab_id, |tab| {
                let mut changed = false;
                if let Some(url) = &href {
                    let text = url.to_string();
                    if tab.state.url != text {
                        tab.state.url = text;
                        tab.state.origin = hooks::origin_of(url);
                        changed = true;
                    }
                }
                if let Some(title) = title {
                    if tab.state.title != title {
                        tab.state.title = title;
                        changed = true;
                    }
                }
                changed.then(|| tab.state.clone())
            });
            if let Some(Some(state)) = changed {
                events::emit_state(app, &state);
            }
        }
        "gesture" => {
            registry.push_gesture(tab_id, envelope.payload.clone());
            emit_event(
                &EventEmitter::Tauri(app.clone()),
                TELEMETRY_EVENT,
                json!({
                    "tabId": tab_id,
                    "kind": "gesture",
                    "untrusted": true,
                    "mainFrame": main_frame,
                    "top": envelope.top,
                    "payload": envelope.payload,
                }),
            );
        }
        other => {
            tracing::debug!("[browser] tab {tab_id}: ignored channel message kind {other:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_parsing_is_strict_about_size_and_shape() {
        let ok = parse_envelope(r#"{"kind":"hello","payload":{"href":"x"},"top":true}"#).unwrap();
        assert_eq!(ok.kind, "hello");
        assert!(ok.top);
        let minimal = parse_envelope(r#"{"kind":"gesture"}"#).unwrap();
        assert!(!minimal.top);
        assert!(minimal.payload.is_null());
        assert!(parse_envelope("not json").is_none());
        assert!(parse_envelope(r#"{"payload":{}}"#).is_none());
        let huge = format!(r#"{{"kind":"x","payload":"{}"}}"#, "a".repeat(MAX_MESSAGE_BYTES));
        assert!(parse_envelope(&huge).is_none());
    }

    #[test]
    fn helper_is_bundled_and_self_contained() {
        assert!(HELPER_JS.contains("codegBrowserHelper"));
        assert!(!HELPER_JS.contains("import "));
        assert!(!HELPER_JS.contains("require("));
    }
}
