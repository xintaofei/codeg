use std::sync::Arc;

use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize)]
pub struct WebEvent {
    pub channel: String,
    pub payload: serde_json::Value,
}

pub struct WebEventBroadcaster {
    sender: broadcast::Sender<WebEvent>,
}

impl Default for WebEventBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl WebEventBroadcaster {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(4096);
        Self { sender }
    }

    pub fn send(&self, channel: &str, payload: &impl Serialize) {
        if self.sender.receiver_count() == 0 {
            return;
        }
        if let Ok(value) = serde_json::to_value(payload) {
            let _ = self.sender.send(WebEvent {
                channel: channel.to_string(),
                payload: value,
            });
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WebEvent> {
        self.sender.subscribe()
    }
}

/// Abstraction over event emission targets.
/// In Tauri mode, events go to both webview and WebSocket clients.
/// In standalone server mode, events only go to WebSocket clients.
#[derive(Clone)]
pub enum EventEmitter {
    #[cfg(feature = "tauri-runtime")]
    Tauri(tauri::AppHandle),
    WebOnly(Arc<WebEventBroadcaster>),
}

/// Unified event emission: sends to both Tauri webview and Web clients (if applicable).
pub fn emit_event(
    emitter: &EventEmitter,
    event: &str,
    payload: impl Serialize + Clone,
) {
    match emitter {
        #[cfg(feature = "tauri-runtime")]
        EventEmitter::Tauri(app) => {
            use tauri::{Emitter, Manager};
            let _ = app.emit(event, payload.clone());
            if let Some(web) = app.try_state::<Arc<WebEventBroadcaster>>() {
                web.send(event, &payload);
            }
        }
        EventEmitter::WebOnly(broadcaster) => {
            broadcaster.send(event, &payload);
        }
    }
}
