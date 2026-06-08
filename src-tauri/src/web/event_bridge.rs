use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{ser::SerializeStruct, Serialize, Serializer};
use tokio::sync::{broadcast, RwLock};

use crate::acp::{AcpEvent, EventBusMetrics, EventEnvelope, InternalEventBus, SessionState};

/// Broadcast-delivered event.
///
/// `payload` is wrapped in `Arc` so cloning across broadcast receivers is
/// refcount-only — avoids copying multi-MB JSON trees per subscriber.
#[derive(Clone, Debug)]
pub struct WebEvent {
    pub channel: String,
    pub payload: Arc<serde_json::Value>,
}

impl Serialize for WebEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("WebEvent", 2)?;
        state.serialize_field("channel", &self.channel)?;
        state.serialize_field("payload", self.payload.as_ref())?;
        state.end()
    }
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

    /// Serialize `payload` once and broadcast. Returns the serialized
    /// `Value` so Tauri callers can reuse it without serializing twice.
    pub fn send(&self, channel: &str, payload: &impl Serialize) -> Option<Arc<serde_json::Value>> {
        let value = Arc::new(serde_json::to_value(payload).ok()?);
        if self.sender.receiver_count() > 0 {
            let _ = self.sender.send(WebEvent {
                channel: channel.to_string(),
                payload: value.clone(),
            });
        }
        Some(value)
    }

    /// Broadcast a pre-serialized `Value` without re-serialization.
    pub fn send_value(&self, channel: &str, payload: Arc<serde_json::Value>) {
        if self.sender.receiver_count() == 0 {
            return;
        }
        let _ = self.sender.send(WebEvent {
            channel: channel.to_string(),
            payload,
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WebEvent> {
        self.sender.subscribe()
    }
}

/// Abstraction over event emission targets.
///
/// Three concerns layered together:
/// - **Tauri webview** (`Tauri` variant): events delivered to the desktop
///   webview via `app.emit`. Looked-up state (`Arc<WebEventBroadcaster>`,
///   `Arc<InternalEventBus>`) goes through `app.try_state`, registered in
///   `lib.rs::run` setup.
/// - **WS clients** (`WebOnly` variant): standalone server mode. Carries
///   the broadcaster directly because there's no AppHandle to look it up
///   from.
/// - **In-process consumers** (lifecycle / pet / chat-channel): receive
///   typed `Arc<EventEnvelope>` from `InternalEventBus`. Both `Tauri` and
///   `WebOnly` resolve to the same bus (via `acp_event_bus()`).
///
/// `Noop` drops everything — used for legacy non-streaming call paths and
/// in tests that don't observe events.
#[derive(Clone)]
pub enum EventEmitter {
    #[cfg(feature = "tauri-runtime")]
    Tauri(tauri::AppHandle),
    /// Standalone server runtime. Carries the broadcaster (transport-bound
    /// JSON delivery to WS clients on non-ACP channels) and the internal
    /// bus (typed envelope delivery to in-process subscribers).
    WebOnly {
        broadcaster: Arc<WebEventBroadcaster>,
        bus: Arc<InternalEventBus>,
    },
    /// Silent no-op emitter — drops all events. Used when streaming progress
    /// is not needed (e.g. legacy non-streaming call paths).
    Noop,
}

impl EventEmitter {
    /// Convenience constructor for the standalone server runtime path.
    /// Mirrors how `Tauri` resolves the same two pieces of state via
    /// `app.try_state`.
    pub fn web_only(broadcaster: Arc<WebEventBroadcaster>, bus: Arc<InternalEventBus>) -> Self {
        EventEmitter::WebOnly { broadcaster, bus }
    }

    /// Resolve the `InternalEventBus` for ACP-typed event delivery.
    ///
    /// In Tauri mode, looks up `Arc<InternalEventBus>` registered with
    /// `app.manage` during setup. Returns `None` if the bus isn't
    /// registered (only happens in degraded test setups) — the caller
    /// treats this as "no in-process consumers wired".
    pub fn acp_event_bus(&self) -> Option<Arc<InternalEventBus>> {
        match self {
            #[cfg(feature = "tauri-runtime")]
            EventEmitter::Tauri(app) => {
                use tauri::Manager;
                app.try_state::<Arc<InternalEventBus>>()
                    .map(|s| Arc::clone(&s))
            }
            EventEmitter::WebOnly { bus, .. } => Some(Arc::clone(bus)),
            EventEmitter::Noop => None,
        }
    }

    /// Resolve the `EventBusMetrics` handle. Same lookup rules as
    /// `acp_event_bus()`.
    pub fn metrics(&self) -> Option<Arc<EventBusMetrics>> {
        self.acp_event_bus().map(|bus| Arc::clone(bus.metrics()))
    }

    /// Test-only convenience: build a `WebOnly` emitter with a fresh,
    /// orphan `InternalEventBus`. Tests that assert against the
    /// broadcaster don't need to wire the bus through their own setup.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn test_web_only(broadcaster: Arc<WebEventBroadcaster>) -> Self {
        let metrics = Arc::new(EventBusMetrics::default());
        let bus = Arc::new(InternalEventBus::new(metrics));
        EventEmitter::WebOnly { broadcaster, bus }
    }
}

/// Global side-channel for cross-client conversation list/status sync.
pub const CONVERSATION_CHANGED_EVENT: &str = "conversation://changed";

/// Global side-channel announcing a live-feedback enable/disable. The settings
/// UI runs in a SEPARATE window (`openSettingsWindow`), so the conversation
/// feedback bar can't learn about a save through any frontend-only cache — it
/// relies on this backend broadcast to converge across every window / WS client,
/// exactly like [`CONVERSATION_CHANGED_EVENT`]. Payload: `FeedbackSettings`
/// (`{ "enabled": bool }`).
pub const FEEDBACK_SETTINGS_CHANGED_EVENT: &str = "feedback-settings://changed";

/// Payload for the global [`CONVERSATION_CHANGED_EVENT`] side-channel. Drives
/// cross-client sidebar sync (membership + status) independent of the
/// per-connection ACP attach protocol, so clients that are NOT attached to a
/// conversation's connection still see it appear / update / disappear / change
/// state.
///
/// Delivered via [`emit_event`], so in desktop mode a single emit reaches both
/// the Tauri webview (`app.emit`) and every WebSocket browser
/// (`WebEventBroadcaster`); in standalone server mode it reaches all browsers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationChange {
    /// Insert-or-replace by id. Covers create and field updates (e.g. title).
    /// Carries the full summary so the frontend renders without a re-fetch.
    /// Root conversations omit `parent_id` (serde skips `None`); delegation
    /// children carry it and the frontend filters them out of the sidebar.
    Upsert {
        summary: crate::models::DbConversationSummary,
    },
    /// Remove by id (soft delete).
    Deleted { id: i32 },
    /// Lightweight running-state change. Emitted centrally from
    /// [`emit_with_state`] whenever a `ConversationStatusChanged` ACP event
    /// flows through, so the sidebar's status reaches every client (not just
    /// those attached to the connection).
    Status { id: i32, status: String },
}

/// Global side-channel for cross-client open-tab sync. Mirrors
/// [`CONVERSATION_CHANGED_EVENT`]: a single [`emit_event`] reaches the Tauri
/// webview and every WebSocket client.
pub const TABS_CHANGED_EVENT: &str = "tabs://changed";

/// Payload for the [`TABS_CHANGED_EVENT`] side-channel. Carries the full
/// conversation-bound tab set (a snapshot, not a delta) so every client
/// converges idempotently — matching the full-replacement save semantics.
///
/// - `version` — workspace-global logical clock; clients drop events at or
///   below their last-applied version (except `origin == "server"`).
/// - `origin` — the originating client's id, echoed back so the originator can
///   ignore its own broadcast; the sentinel `"server"` marks cascade-originated
///   changes (folder removal, conversation deletion) that every client applies.
/// - `tabs` — the canonical persisted set; `is_active` marks the focused tab,
///   which is mirrored across clients.
#[derive(Debug, Clone, Serialize)]
pub struct TabsChanged {
    pub version: i64,
    pub origin: String,
    pub tabs: Vec<crate::models::OpenedTab>,
}

/// Unified event emission: serializes the payload exactly once and dispatches
/// the shared `Arc<Value>` to both the Tauri webview and the web broadcaster.
pub fn emit_event(emitter: &EventEmitter, event: &str, payload: impl Serialize) {
    match emitter {
        #[cfg(feature = "tauri-runtime")]
        EventEmitter::Tauri(app) => {
            use tauri::{Emitter, Manager};
            let Ok(value) = serde_json::to_value(&payload) else {
                return;
            };
            let shared = Arc::new(value);
            // `&Value` is Copy, so Tauri's `Clone` bound is satisfied without
            // copying the payload — Tauri serializes through the reference.
            let _ = app.emit(event, shared.as_ref());
            if let Some(web) = app.try_state::<Arc<WebEventBroadcaster>>() {
                web.send_value(event, shared);
            }
        }
        EventEmitter::WebOnly { broadcaster, .. } => {
            let _ = broadcaster.send(event, &payload);
        }
        EventEmitter::Noop => {}
    }
}

/// 统一 ACP 事件发射入口。
///
/// 流程：
/// 1. 写锁拿到 `SessionState`
/// 2. `apply_event` 把事件应用到 state（也更新 `last_activity_at`）
/// 3. `event_seq += 1`
/// 4. 用新 seq 构造 `EventEnvelope`，推入 ring buffer，记录淘汰计数
/// 5. 释放写锁
/// 6. 分发到三条路径：
///    - 每连接 `ConnectionEventStream`（WS attach 协议主路径）
///    - 进程内 `InternalEventBus`（lifecycle / pet / chat-channel 订阅者）
///    - Tauri 模式下额外 `app.emit("acp://event", ...)` 给 webview
///
/// 不再向 `WebEventBroadcaster` 上的 `acp://event` 频道广播——所有 ACP
/// 事件消费者要么走 per-connection stream（WS 客户端），要么走
/// InternalEventBus（进程内订阅者），要么走 Tauri `app.emit`（桌面 webview）。
/// 删除该全局广播是 Phase 5 架构清理的核心：它消除了 WS 客户端 receiver-side
/// 去重 (`attachManagedConnectionIdsRef`) 的必要性。
pub async fn emit_with_state(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    payload: AcpEvent,
) {
    emit_with_state_gated(state, emitter, payload, |_| true).await;
}

/// Like [`emit_with_state`], but a `gate` predicate — evaluated under the SAME
/// write lock, BEFORE `apply_event` — can veto the emit: returning `false`
/// aborts with no mutation, no seq bump, no broadcast, and returns `false`.
///
/// The point is atomicity: the gate, the state mutation, and the seq assignment
/// all happen in one critical section, so no other event can interleave between
/// "decide to accept" and "apply + sequence". Used by feedback submit to gate on
/// `turn_in_flight` together with the append (a `TurnComplete`/`UserMessage`
/// can't slip in between to strand or re-add the note), and to assign the
/// `FeedbackSubmitted` seq atomically with the append.
pub async fn emit_with_state_gated<F>(
    state: &Arc<RwLock<SessionState>>,
    emitter: &EventEmitter,
    payload: AcpEvent,
    gate: F,
) -> bool
where
    F: FnOnce(&SessionState) -> bool,
{
    let (envelope_arc, stream, evicted) = {
        let mut s = state.write().await;
        if !gate(&s) {
            return false;
        }
        s.apply_event(&payload);
        s.event_seq += 1;
        let envelope = Arc::new(EventEnvelope {
            seq: s.event_seq,
            connection_id: s.connection_id.clone(),
            payload,
        });
        let evicted = s.push_recent_event(Arc::clone(&envelope));
        (envelope, s.event_stream(), evicted)
    };

    // Per-connection broadcaster — primary delivery path for web/remote-
    // desktop transports (they use Subscribe-with-Snapshot attach for ACP
    // events).
    stream.send(Arc::clone(&envelope_arc));

    // In-process consumers (lifecycle, pet, chat-channel). Typed envelope —
    // no JSON parse on the receiver side. Plus surface ring-buffer pressure
    // and bus emit-rate via metrics so operators can see when things drift.
    match emitter {
        #[cfg(feature = "tauri-runtime")]
        EventEmitter::Tauri(app) => {
            use tauri::{Emitter, Manager};
            // Tauri webview listener is the desktop frontend's only ACP path
            // (it subscribes via `app.listen`, not the WS attach protocol).
            let _ = app.emit("acp://event", envelope_arc.as_ref());
            if let Some(bus) = app.try_state::<Arc<InternalEventBus>>() {
                bus.send(Arc::clone(&envelope_arc));
                if evicted > 0 {
                    bus.metrics()
                        .ring_buffer_evict_count
                        .fetch_add(evicted as u64, Ordering::Relaxed);
                }
            }
        }
        EventEmitter::WebOnly { bus, .. } => {
            bus.send(Arc::clone(&envelope_arc));
            if evicted > 0 {
                bus.metrics()
                    .ring_buffer_evict_count
                    .fetch_add(evicted as u64, Ordering::Relaxed);
            }
        }
        EventEmitter::Noop => {}
    }

    // Bridge conversation status transitions onto the global
    // `conversation://changed` side-channel so clients NOT attached to this
    // connection (only showing the sidebar, or a different browser entirely)
    // still observe running-state changes live — the per-connection delivery
    // above only reaches attached clients. One central hook here covers every
    // `ConversationStatusChanged` emit site (manager + lifecycle). `status`
    // serializes to the same snake_case string the DB stores (e.g.
    // "in_progress"), matching `DbConversationSummary.status`.
    if let AcpEvent::ConversationStatusChanged {
        conversation_id,
        status,
    } = &envelope_arc.payload
    {
        if let Ok(serde_json::Value::String(status_str)) = serde_json::to_value(status) {
            emit_event(
                emitter,
                CONVERSATION_CHANGED_EVENT,
                ConversationChange::Status {
                    id: *conversation_id,
                    status: status_str,
                },
            );
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::conversation::ConversationStatus;
    use crate::models::AgentType;

    fn fresh_state() -> Arc<RwLock<SessionState>> {
        Arc::new(RwLock::new(SessionState::new(
            "conn-test".to_string(),
            AgentType::ClaudeCode,
            None,
            "win-test".to_string(),
            None,
        )))
    }

    #[tokio::test]
    async fn emit_with_state_bridges_status_change_to_global_channel() {
        // A ConversationStatusChanged ACP event must ALSO surface on the global
        // `conversation://changed` channel so clients NOT attached to this
        // connection (e.g. only viewing the sidebar) still observe the flip.
        let state = fresh_state();
        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let mut rx = broadcaster.subscribe();
        let emitter = EventEmitter::test_web_only(broadcaster.clone());

        emit_with_state(
            &state,
            &emitter,
            AcpEvent::ConversationStatusChanged {
                conversation_id: 7,
                status: ConversationStatus::PendingReview,
            },
        )
        .await;

        let evt = rx
            .try_recv()
            .expect("status change should bridge to the global channel");
        let p = &*evt.payload;
        assert_eq!(evt.channel, CONVERSATION_CHANGED_EVENT);
        assert_eq!(p["kind"], "status");
        assert_eq!(p["id"], 7);
        assert_eq!(p["status"], "pending_review");
    }

    #[tokio::test]
    async fn emit_with_state_non_status_event_does_not_touch_global_channel() {
        // High-frequency stream events (deltas, etc.) must NOT spam the global
        // sidebar channel — only status transitions bridge.
        let state = fresh_state();
        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let mut rx = broadcaster.subscribe();
        let emitter = EventEmitter::test_web_only(broadcaster.clone());

        emit_with_state(
            &state,
            &emitter,
            AcpEvent::ContentDelta {
                text: "hello".to_string(),
            },
        )
        .await;

        assert!(
            rx.try_recv().is_err(),
            "non-status ACP events must not emit on conversation://changed"
        );
    }

    #[test]
    fn emit_event_broadcasts_tabs_changed_snapshot() {
        // The open-tab set syncs via the same global side-channel as the
        // sidebar: one `emit_event` on `tabs://changed` reaches every client,
        // carrying version + origin (for echo suppression) + the full set.
        let broadcaster = Arc::new(WebEventBroadcaster::new());
        let mut rx = broadcaster.subscribe();
        let emitter = EventEmitter::test_web_only(broadcaster.clone());

        emit_event(
            &emitter,
            TABS_CHANGED_EVENT,
            TabsChanged {
                version: 6,
                origin: "win-abc".to_string(),
                tabs: vec![],
            },
        );

        let evt = rx.try_recv().expect("tabs change should broadcast");
        let p = &*evt.payload;
        assert_eq!(evt.channel, TABS_CHANGED_EVENT);
        assert_eq!(p["version"], 6);
        assert_eq!(p["origin"], "win-abc");
        assert!(p["tabs"].is_array(), "tabs must serialize as an array");
    }
}
