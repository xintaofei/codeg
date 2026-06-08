pub mod binary_cache;
pub mod connection;
pub mod delegation;
pub mod error;
pub mod event_stream;
pub mod feedback;
pub mod file_system_runtime;
pub mod fork;
pub mod idle_sweep;
pub mod internal_bus;
pub mod lifecycle;
pub mod manager;
pub mod opencode_plugins;
pub mod preflight;
pub mod registry;
pub mod session_state;
pub mod terminal_runtime;
pub mod types;

pub use idle_sweep::{idle_sweep_task, idle_timeout_from_env, SWEEP_INTERVAL_SECS};
pub use internal_bus::{EventBusMetrics, EventBusMetricsSnapshot, InternalEventBus};
pub use lifecycle::lifecycle_subscriber_task;
pub use session_state::{LiveSessionSnapshot, SessionState};
// Re-export the inner types of LiveSessionSnapshot for downstream consumers; not all are
// directly named in Rust today (they ride along through the snapshot struct), so silence
// dead-import warnings rather than dropping them.
#[allow(unused_imports)]
pub use session_state::{
    LiveContentBlock, LiveMessage, PendingPermissionState, ToolCallOutput, ToolCallState,
    ToolCallStatus, ToolKind, UsageInfo,
};
pub use types::{
    user_blocks_from_prompt, AcpEvent, ConversationConnectionInfo, EventEnvelope, UserMessageBlock,
};
