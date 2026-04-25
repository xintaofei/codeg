use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::task::TaskTracker;

use crate::acp::manager::ConnectionManager;
use crate::chat_channel::manager::ChatChannelManager;
use crate::db::AppDatabase;
use crate::runtime_monitor::RuntimeMonitor;
use crate::terminal::manager::TerminalManager;
use crate::web::client_owner::WebClientRegistry;
use crate::web::event_bridge::{EventEmitter, WebEventBroadcaster};
use crate::web::WebServerState;

pub struct AppState {
    pub db: AppDatabase,
    pub connection_manager: ConnectionManager,
    pub terminal_manager: TerminalManager,
    pub web_client_registry: Arc<WebClientRegistry>,
    pub event_broadcaster: Arc<WebEventBroadcaster>,
    pub emitter: EventEmitter,
    pub data_dir: PathBuf,
    pub runtime_monitor: Arc<RuntimeMonitor>,
    pub web_server_state: WebServerState,
    pub chat_channel_manager: ChatChannelManager,
    /// Tracks background tasks spawned by web request handlers (e.g. WS
    /// cleanup timers). Allows graceful shutdown to await pending work.
    pub task_tracker: TaskTracker,
}

pub fn default_connection_manager() -> ConnectionManager {
    ConnectionManager::new()
}

pub fn default_terminal_manager() -> TerminalManager {
    TerminalManager::new()
}

pub fn default_chat_channel_manager() -> ChatChannelManager {
    ChatChannelManager::new()
}
