use std::collections::HashMap;
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tokio::sync::{mpsc, Mutex};

use super::error::ChatChannelError;
use super::session_bridge::SessionBridge;
use super::traits::ChatChannelBackend;
use super::types::*;
use crate::acp::manager::ConnectionManager;
use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};

struct ActiveChannel {
    id: i32,
    name: String,
    channel_type: ChannelType,
    backend: Arc<dyn ChatChannelBackend>,
}

/// Inner state shared across clones.
struct Inner {
    channels: Mutex<HashMap<i32, ActiveChannel>>,
    command_tx: mpsc::Sender<IncomingCommand>,
    command_rx: Mutex<Option<mpsc::Receiver<IncomingCommand>>>,
    broadcaster: Mutex<Option<Arc<WebEventBroadcaster>>>,
}

pub struct ChatChannelManager {
    inner: Arc<Inner>,
}

impl Default for ChatChannelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatChannelManager {
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel(256);
        Self {
            inner: Arc::new(Inner {
                channels: Mutex::new(HashMap::new()),
                command_tx,
                command_rx: Mutex::new(Some(command_rx)),
                broadcaster: Mutex::new(None),
            }),
        }
    }

    /// Shallow clone sharing the same state (like ConnectionManager::clone_ref).
    pub fn clone_ref(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }

    pub fn command_sender(&self) -> mpsc::Sender<IncomingCommand> {
        self.inner.command_tx.clone()
    }

    /// Take the command receiver (can only be called once, at startup).
    pub async fn take_command_receiver(&self) -> Option<mpsc::Receiver<IncomingCommand>> {
        self.inner.command_rx.lock().await.take()
    }

    /// Emit a status change event to the frontend via broadcaster.
    async fn emit_status_event(&self, channel_id: i32, status: &str) {
        if let Some(broadcaster) = self.inner.broadcaster.lock().await.as_ref() {
            broadcaster.send(
                "chat-channel://status",
                &serde_json::json!({
                    "channel_id": channel_id,
                    "status": status,
                }),
            );
        }
    }

    pub async fn add_channel(
        &self,
        id: i32,
        name: String,
        channel_type: ChannelType,
        backend: Box<dyn ChatChannelBackend>,
    ) -> Result<(), ChatChannelError> {
        let backend: Arc<dyn ChatChannelBackend> = Arc::from(backend);

        // Stop existing channel if present (prevents task leak on duplicate connect)
        let old = self.inner.channels.lock().await.remove(&id);
        if let Some(existing) = old {
            let _ = existing.backend.stop().await;
        }

        let command_tx = self.inner.command_tx.clone();
        backend.start(command_tx).await?;

        let channel = ActiveChannel {
            id,
            name,
            channel_type,
            backend,
        };

        self.inner.channels.lock().await.insert(id, channel);
        self.emit_status_event(id, "connected").await;
        Ok(())
    }

    pub async fn remove_channel(&self, id: i32) -> Result<(), ChatChannelError> {
        let removed = self.inner.channels.lock().await.remove(&id);
        if let Some(channel) = removed {
            channel.backend.stop().await?;
            self.emit_status_event(id, "disconnected").await;
        }
        Ok(())
    }

    pub async fn stop_all(&self) {
        let drained: Vec<ActiveChannel> = {
            let mut channels = self.inner.channels.lock().await;
            channels.drain().map(|(_, ch)| ch).collect()
        };
        for channel in drained {
            let _ = channel.backend.stop().await;
        }
    }

    pub async fn send_to_channel(
        &self,
        channel_id: i32,
        message: &RichMessage,
    ) -> Result<SentMessageId, ChatChannelError> {
        let backend = {
            let channels = self.inner.channels.lock().await;
            channels
                .get(&channel_id)
                .ok_or(ChatChannelError::NotFound(channel_id))?
                .backend
                .clone()
        };
        backend.send_rich_message(message).await
    }

    pub async fn send_to_all(&self, message: &RichMessage) {
        let backends: Vec<Arc<dyn ChatChannelBackend>> = {
            let channels = self.inner.channels.lock().await;
            channels.values().map(|ch| ch.backend.clone()).collect()
        };
        for backend in backends {
            let _ = backend.send_rich_message(message).await;
        }
    }

    pub async fn get_status(&self) -> Vec<crate::models::ChannelStatusInfo> {
        let entries: Vec<(i32, String, String, Arc<dyn ChatChannelBackend>)> = {
            let channels = self.inner.channels.lock().await;
            channels
                .values()
                .map(|ch| {
                    (
                        ch.id,
                        ch.name.clone(),
                        ch.channel_type.to_string(),
                        ch.backend.clone(),
                    )
                })
                .collect()
        };
        let mut result = Vec::with_capacity(entries.len());
        for (id, name, ct, backend) in entries {
            let status = backend.status().await;
            result.push(crate::models::ChannelStatusInfo {
                channel_id: id,
                name,
                channel_type: ct,
                status: serde_json::to_value(status)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| "unknown".to_string()),
            });
        }
        result
    }

    pub async fn test_channel(&self, id: i32) -> Result<(), ChatChannelError> {
        let backend = {
            let channels = self.inner.channels.lock().await;
            channels
                .get(&id)
                .ok_or(ChatChannelError::NotFound(id))?
                .backend
                .clone()
        };
        backend.test_connection().await
    }

    pub async fn is_connected(&self, id: i32) -> bool {
        let backend = {
            let channels = self.inner.channels.lock().await;
            channels.get(&id).map(|ch| ch.backend.clone())
        };
        if let Some(b) = backend {
            b.status().await == ChannelConnectionStatus::Connected
        } else {
            false
        }
    }

    /// Start background tasks (event subscriber + command dispatcher) and
    /// auto-connect all enabled channels from DB.
    ///
    /// `broadcaster` continues to back the `*Status* / *Inbound*` JSON
    /// events the ChatChannel itself emits (still consumed by the WS
    /// firehose). `bus` carries typed `Arc<EventEnvelope>` to the two
    /// ACP-event-driven subscribers (`event_subscriber`,
    /// `session_event_subscriber`). Phase 5 split: ACP-shaped data goes
    /// through the typed bus; chat-channel-shaped data stays on the JSON
    /// broadcaster.
    pub async fn start_background(
        &self,
        broadcaster: Arc<WebEventBroadcaster>,
        bus: Arc<crate::acp::InternalEventBus>,
        db_conn: DatabaseConnection,
        conn_mgr: ConnectionManager,
        emitter: EventEmitter,
    ) {
        // Store broadcaster for status event emission
        *self.inner.broadcaster.lock().await = Some(broadcaster.clone());

        let db_conn2 = db_conn.clone();

        // Create shared session bridge
        let bridge = Arc::new(Mutex::new(SessionBridge::new()));

        // Spawn event subscriber
        let manager_for_events = self.clone_ref();
        super::event_subscriber::spawn_event_subscriber(
            bus.clone(),
            manager_for_events,
            db_conn.clone(),
            bridge.clone(),
        );

        // Spawn session event subscriber (ACP event routing to channels)
        let manager_for_session_events = self.clone_ref();
        super::session_event_subscriber::spawn_session_event_subscriber(
            bus,
            bridge.clone(),
            manager_for_session_events,
            conn_mgr.clone_ref(),
            db_conn.clone(),
        );

        // Spawn command dispatcher
        if let Some(command_rx) = self.take_command_receiver().await {
            eprintln!("[ChatChannel] command dispatcher started");
            let manager_for_cmds = self.clone_ref();
            super::command_dispatcher::spawn_command_dispatcher(
                command_rx,
                manager_for_cmds,
                db_conn.clone(),
                conn_mgr,
                emitter,
                bridge,
            );
        } else {
            eprintln!("[ChatChannel] WARNING: command_rx already taken, dispatcher NOT started");
        }

        // Spawn daily report scheduler
        let manager_for_scheduler = self.clone_ref();
        super::scheduler::spawn_daily_report_scheduler(manager_for_scheduler, db_conn.clone());

        // Auto-connect enabled channels
        self.auto_connect_channels(&db_conn2).await;
    }

    async fn auto_connect_channels(&self, db_conn: &DatabaseConnection) {
        let channels = match crate::db::service::chat_channel_service::list_enabled(db_conn).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[ChatChannel] failed to load enabled channels: {e}");
                return;
            }
        };

        for ch in channels {
            let channel_type: ChannelType =
                match serde_json::from_value(serde_json::Value::String(ch.channel_type.clone())) {
                    Ok(t) => t,
                    Err(_) => {
                        eprintln!(
                            "[ChatChannel] unknown channel type '{}' for '{}' (id={}), skipping",
                            ch.channel_type, ch.name, ch.id
                        );
                        continue;
                    }
                };

            let config: serde_json::Value = match serde_json::from_str(&ch.config_json) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "[ChatChannel] invalid config for '{}' (id={}): {e}, skipping",
                        ch.name, ch.id
                    );
                    continue;
                }
            };

            let token = match crate::keyring_store::get_channel_token(ch.id) {
                Some(t) => t,
                None => {
                    eprintln!(
                        "[ChatChannel] no token found for '{}' (id={}), skipping auto-connect",
                        ch.name, ch.id
                    );
                    continue;
                }
            };

            let backend = match super::backends::create_backend(ch.id, channel_type, &config, token)
            {
                Ok(b) => b,
                Err(e) => {
                    eprintln!(
                        "[ChatChannel] failed to create backend for '{}' (id={}): {e}",
                        ch.name, ch.id
                    );
                    continue;
                }
            };

            if let Err(e) = self
                .add_channel(ch.id, ch.name.clone(), channel_type, backend)
                .await
            {
                eprintln!(
                    "[ChatChannel] failed to auto-connect '{}' (id={}): {e}",
                    ch.name, ch.id
                );
            } else {
                eprintln!("[ChatChannel] auto-connected '{}' (id={})", ch.name, ch.id);
            }
        }
    }
}
