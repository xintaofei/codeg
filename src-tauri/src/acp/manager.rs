use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::acp::connection::{spawn_agent_connection, AgentConnection, ConnectionCommand};
use crate::acp::error::AcpError;
use crate::acp::types::{ConnectionInfo, ForkResultInfo, PromptInputBlock};
use crate::models::agent::AgentType;
use crate::runtime_monitor::RuntimeMonitor;
use crate::web::client_owner::WebClientRegistry;
use crate::web::event_bridge::EventEmitter;

const DEFAULT_MAX_CONNECTIONS: usize = 16;
const DEFAULT_MAX_CONNECTIONS_PER_OWNER: usize = 6;
const DEFAULT_MAX_CONNECTIONS_PER_AGENT: usize = 8;
const DEFAULT_BREAKER_THRESHOLD: usize = 4;
const DEFAULT_BREAKER_WINDOW_SECONDS: u64 = 180;
const DEFAULT_BREAKER_COOLDOWN_SECONDS: u64 = 120;
const DEFAULT_ORPHAN_GRACE_SECONDS: u64 = 60;
const DEFAULT_CONNECTING_TIMEOUT_SECONDS: u64 = 180;
const WATCHDOG_INTERVAL_SECONDS: u64 = 30;

#[derive(Debug, Clone)]
struct ConnectionLimits {
    max_connections: usize,
    max_connections_per_owner: usize,
    max_connections_per_agent: usize,
    breaker_threshold: usize,
    breaker_window: Duration,
    breaker_cooldown: Duration,
    orphan_grace: Duration,
    connecting_timeout: Duration,
}

impl ConnectionLimits {
    fn from_env() -> Self {
        Self {
            max_connections: parse_env_usize(
                "CODEG_ACP_MAX_CONNECTIONS",
                DEFAULT_MAX_CONNECTIONS,
            ),
            max_connections_per_owner: parse_env_usize(
                "CODEG_ACP_MAX_CONNECTIONS_PER_OWNER",
                DEFAULT_MAX_CONNECTIONS_PER_OWNER,
            ),
            max_connections_per_agent: parse_env_usize(
                "CODEG_ACP_MAX_CONNECTIONS_PER_AGENT",
                DEFAULT_MAX_CONNECTIONS_PER_AGENT,
            ),
            breaker_threshold: parse_env_usize(
                "CODEG_ACP_BREAKER_THRESHOLD",
                DEFAULT_BREAKER_THRESHOLD,
            ),
            breaker_window: Duration::from_secs(parse_env_u64(
                "CODEG_ACP_BREAKER_WINDOW_SECONDS",
                DEFAULT_BREAKER_WINDOW_SECONDS,
            )),
            breaker_cooldown: Duration::from_secs(parse_env_u64(
                "CODEG_ACP_BREAKER_COOLDOWN_SECONDS",
                DEFAULT_BREAKER_COOLDOWN_SECONDS,
            )),
            orphan_grace: Duration::from_secs(parse_env_u64(
                "CODEG_ACP_ORPHAN_GRACE_SECONDS",
                DEFAULT_ORPHAN_GRACE_SECONDS,
            )),
            connecting_timeout: Duration::from_secs(parse_env_u64(
                "CODEG_ACP_CONNECTING_TIMEOUT_SECONDS",
                DEFAULT_CONNECTING_TIMEOUT_SECONDS,
            )),
        }
    }
}

struct CircuitBreakerState {
    failure_timestamps: VecDeque<Instant>,
    open_until: Option<Instant>,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            failure_timestamps: VecDeque::new(),
            open_until: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionLimitSnapshot {
    pub current_total: usize,
    pub global_limit: usize,
    pub per_owner_limit: usize,
    pub per_agent_limit: usize,
    pub by_agent: BTreeMap<String, usize>,
    pub recent_failures: usize,
    pub breaker_threshold: usize,
    pub breaker_window_seconds: u64,
    pub breaker_cooldown_seconds: u64,
    pub breaker_open: bool,
    pub breaker_open_until: Option<String>,
    pub orphan_grace_seconds: u64,
    pub connecting_timeout_seconds: u64,
}

#[derive(Clone)]
pub struct ConnectionManager {
    connections: Arc<Mutex<HashMap<String, AgentConnection>>>,
    breaker: Arc<Mutex<CircuitBreakerState>>,
    limits: ConnectionLimits,
    runtime_monitor: Arc<StdMutex<Option<Arc<RuntimeMonitor>>>>,
    watchdog_started: Arc<AtomicBool>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            breaker: Arc::new(Mutex::new(CircuitBreakerState::default())),
            limits: ConnectionLimits::from_env(),
            runtime_monitor: Arc::new(StdMutex::new(None)),
            watchdog_started: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns a shallow clone sharing the same underlying connection state.
    pub fn clone_ref(&self) -> Self {
        self.clone()
    }

    pub fn attach_runtime_monitor(&self, monitor: Arc<RuntimeMonitor>) {
        *self.runtime_monitor.lock().unwrap() = Some(monitor);
    }

    pub fn start_orphan_watchdog(&self, web_client_registry: Arc<WebClientRegistry>) {
        if self
            .watchdog_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let manager = self.clone_ref();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(WATCHDOG_INTERVAL_SECONDS));
            loop {
                interval.tick().await;
                manager.run_watchdog_pass(&web_client_registry).await;
            }
        });
    }

    pub(crate) fn connections_arc(&self) -> Arc<Mutex<HashMap<String, AgentConnection>>> {
        self.connections.clone()
    }

    pub(crate) fn runtime_monitor(&self) -> Option<Arc<RuntimeMonitor>> {
        self.runtime_monitor.lock().unwrap().clone()
    }

    pub(crate) fn log_runtime(
        &self,
        level: &str,
        scope: &str,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) {
        if let Some(monitor) = self.runtime_monitor() {
            monitor.record(level, scope, message, data);
        }
    }

    pub(crate) async fn record_failure(
        &self,
        agent_type: AgentType,
        message: &str,
        connection_id: Option<&str>,
    ) {
        let now = Instant::now();
        let recent_failures = {
            let mut breaker = self.breaker.lock().await;
            trim_failure_window(&mut breaker.failure_timestamps, self.limits.breaker_window, now);
            breaker.failure_timestamps.push_back(now);
            if breaker.failure_timestamps.len() >= self.limits.breaker_threshold {
                breaker.open_until = Some(now + self.limits.breaker_cooldown);
            }
            breaker.failure_timestamps.len()
        };

        self.log_runtime(
            "error",
            "acp",
            format!("[{agent_type}] {message}"),
            Some(serde_json::json!({
                "agent_type": serde_json::to_value(agent_type).ok(),
                "connection_id": connection_id,
                "recent_failures": recent_failures,
            })),
        );
    }

    pub async fn spawn_agent(
        &self,
        agent_type: AgentType,
        working_dir: Option<String>,
        session_id: Option<String>,
        runtime_env: BTreeMap<String, String>,
        owner_window_label: String,
        emitter: EventEmitter,
    ) -> Result<String, AcpError> {
        self.enforce_spawn_guards(agent_type, &owner_window_label).await?;

        let connection_id = uuid::Uuid::new_v4().to_string();
        self.log_runtime(
            "info",
            "acp",
            format!(
                "spawning connection {} for {} ({owner_window_label})",
                connection_id, agent_type
            ),
            None,
        );

        if let Err(error) = spawn_agent_connection(
            self.clone_ref(),
            connection_id.clone(),
            agent_type,
            working_dir,
            session_id,
            runtime_env,
            owner_window_label,
            emitter,
        )
        .await
        {
            self.record_failure(agent_type, &error.to_string(), Some(&connection_id))
                .await;
            return Err(error);
        }

        Ok(connection_id)
    }

    pub async fn send_prompt(
        &self,
        conn_id: &str,
        blocks: Vec<PromptInputBlock>,
    ) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        cmd_tx
            .send(ConnectionCommand::Prompt { blocks })
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    pub async fn set_mode(&self, conn_id: &str, mode_id: String) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        cmd_tx
            .send(ConnectionCommand::SetMode { mode_id })
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    pub async fn set_config_option(
        &self,
        conn_id: &str,
        config_id: String,
        value_id: String,
    ) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        cmd_tx
            .send(ConnectionCommand::SetConfigOption {
                config_id,
                value_id,
            })
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    pub async fn cancel(&self, conn_id: &str) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        cmd_tx
            .send(ConnectionCommand::Cancel)
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    pub async fn respond_permission(
        &self,
        conn_id: &str,
        request_id: &str,
        option_id: &str,
    ) -> Result<(), AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        cmd_tx
            .send(ConnectionCommand::RespondPermission {
                request_id: request_id.into(),
                option_id: option_id.into(),
            })
            .await
            .map_err(|_| AcpError::ProcessExited)
    }

    pub async fn fork_session(&self, conn_id: &str) -> Result<ForkResultInfo, AcpError> {
        let cmd_tx = {
            let connections = self.connections.lock().await;
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| AcpError::ConnectionNotFound(conn_id.into()))?;
            conn.cmd_tx.clone()
        };
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        cmd_tx
            .send(ConnectionCommand::Fork { reply: reply_tx })
            .await
            .map_err(|_| AcpError::ProcessExited)?;
        reply_rx
            .await
            .map_err(|_| AcpError::protocol("Fork reply channel closed".to_string()))?
    }

    pub async fn disconnect(&self, conn_id: &str) -> Result<(), AcpError> {
        let cmd_tx = {
            let mut connections = self.connections.lock().await;
            connections.remove(conn_id).map(|conn| conn.cmd_tx)
        };
        if let Some(cmd_tx) = cmd_tx {
            let _ = cmd_tx.send(ConnectionCommand::Disconnect).await;
            Ok(())
        } else {
            Err(AcpError::ConnectionNotFound(conn_id.into()))
        }
    }

    pub async fn disconnect_by_owner_window(&self, owner_window_label: &str) -> usize {
        let cmd_txs = {
            let mut connections = self.connections.lock().await;
            let ids: Vec<String> = connections
                .iter()
                .filter_map(|(id, conn)| {
                    if conn.owner_window_label == owner_window_label {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect();

            let mut txs = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(conn) = connections.remove(&id) {
                    txs.push(conn.cmd_tx);
                }
            }
            txs
        };

        let disconnected = cmd_txs.len();
        for cmd_tx in cmd_txs {
            let _ = cmd_tx.send(ConnectionCommand::Disconnect).await;
        }
        self.log_runtime(
            "info",
            "acp",
            format!(
                "disconnected {disconnected} connection(s) for owner {owner_window_label}"
            ),
            None,
        );
        disconnected
    }

    pub async fn disconnect_all(&self) -> usize {
        let cmd_txs: Vec<_> = {
            let mut connections = self.connections.lock().await;
            connections.drain().map(|(_, conn)| conn.cmd_tx).collect()
        };
        let disconnected = cmd_txs.len();
        for cmd_tx in cmd_txs {
            let _ = cmd_tx.send(ConnectionCommand::Disconnect).await;
        }
        self.log_runtime(
            "info",
            "acp",
            format!("disconnected all ACP connections ({disconnected})"),
            None,
        );
        disconnected
    }

    pub async fn list_connections(&self) -> Vec<ConnectionInfo> {
        let entries = {
            let connections = self.connections.lock().await;
            connections.values().cloned().collect::<Vec<_>>()
        };

        let mut result = Vec::with_capacity(entries.len());
        for connection in entries {
            result.push(connection.info().await);
        }
        result.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        result
    }

    pub async fn limits_snapshot(&self) -> ConnectionLimitSnapshot {
        let by_agent = {
            let connections = self.connections.lock().await;
            let mut counts = BTreeMap::<String, usize>::new();
            for connection in connections.values() {
                let key = serde_json::to_value(connection.agent_type)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_else(|| connection.agent_type.to_string());
                *counts.entry(key).or_default() += 1;
            }
            (connections.len(), counts)
        };

        let (recent_failures, breaker_open, breaker_open_until) = {
            let now = Instant::now();
            let mut breaker = self.breaker.lock().await;
            trim_failure_window(&mut breaker.failure_timestamps, self.limits.breaker_window, now);
            let breaker_open = breaker.open_until.is_some_and(|until| until > now);
            let breaker_open_until = breaker
                .open_until
                .filter(|until| *until > now)
                .map(|until| {
                    let seconds = until.saturating_duration_since(now).as_secs();
                    (Utc::now() + chrono::Duration::seconds(seconds as i64)).to_rfc3339()
                });
            (breaker.failure_timestamps.len(), breaker_open, breaker_open_until)
        };

        ConnectionLimitSnapshot {
            current_total: by_agent.0,
            global_limit: self.limits.max_connections,
            per_owner_limit: self.limits.max_connections_per_owner,
            per_agent_limit: self.limits.max_connections_per_agent,
            by_agent: by_agent.1,
            recent_failures,
            breaker_threshold: self.limits.breaker_threshold,
            breaker_window_seconds: self.limits.breaker_window.as_secs(),
            breaker_cooldown_seconds: self.limits.breaker_cooldown.as_secs(),
            breaker_open,
            breaker_open_until,
            orphan_grace_seconds: self.limits.orphan_grace.as_secs(),
            connecting_timeout_seconds: self.limits.connecting_timeout.as_secs(),
        }
    }

    async fn enforce_spawn_guards(
        &self,
        agent_type: AgentType,
        owner_window_label: &str,
    ) -> Result<(), AcpError> {
        let now = Instant::now();
        {
            let mut breaker = self.breaker.lock().await;
            trim_failure_window(&mut breaker.failure_timestamps, self.limits.breaker_window, now);
            if let Some(until) = breaker.open_until {
                if until > now {
                    let wait_seconds = until.saturating_duration_since(now).as_secs();
                    let message = format!(
                        "ACP circuit breaker is open after repeated failures. Retry in about {} seconds.",
                        wait_seconds.max(1)
                    );
                    self.log_runtime("warn", "acp", &message, None);
                    return Err(AcpError::CircuitOpen(message));
                }
                breaker.open_until = None;
            }
        }

        let (current_total, owner_total, agent_total) = {
            let connections = self.connections.lock().await;
            let current_total = connections.len();
            let owner_total = connections
                .values()
                .filter(|connection| connection.owner_window_label == owner_window_label)
                .count();
            let agent_total = connections
                .values()
                .filter(|connection| connection.agent_type == agent_type)
                .count();
            (current_total, owner_total, agent_total)
        };

        if current_total >= self.limits.max_connections {
            let message = format!(
                "ACP connection limit reached ({current_total}/{})",
                self.limits.max_connections
            );
            self.log_runtime("warn", "acp", &message, None);
            return Err(AcpError::LimitExceeded(message));
        }

        if owner_total >= self.limits.max_connections_per_owner {
            let message = format!(
                "ACP owner connection limit reached for {} ({owner_total}/{})",
                owner_window_label, self.limits.max_connections_per_owner
            );
            self.log_runtime("warn", "acp", &message, None);
            return Err(AcpError::LimitExceeded(message));
        }

        if agent_total >= self.limits.max_connections_per_agent {
            let message = format!(
                "ACP agent connection limit reached for {} ({agent_total}/{})",
                agent_type, self.limits.max_connections_per_agent
            );
            self.log_runtime("warn", "acp", &message, None);
            return Err(AcpError::LimitExceeded(message));
        }

        Ok(())
    }

    async fn run_watchdog_pass(&self, web_client_registry: &WebClientRegistry) {
        let snapshots = {
            let connections = self.connections.lock().await;
            connections
                .iter()
                .map(|(id, connection)| {
                    (
                        id.clone(),
                        connection.owner_window_label.clone(),
                        connection.metadata.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };

        let mut stale_ids = BTreeSet::new();
        for (connection_id, owner_window_label, metadata) in snapshots {
            let metadata = metadata.read().await.clone();

            if let Some(client_id) = owner_window_label.strip_prefix("web:") {
                let has_active_client = web_client_registry.has_active_client(client_id).await;
                let age = Utc::now()
                    .signed_duration_since(metadata.created_at)
                    .num_seconds()
                    .max(0) as u64;
                if !has_active_client && age >= self.limits.orphan_grace.as_secs() {
                    stale_ids.insert(connection_id.clone());
                    self.log_runtime(
                        "warn",
                        "acp",
                        format!(
                            "orphan watchdog disconnecting {} owned by {}",
                            connection_id, owner_window_label
                        ),
                        None,
                    );
                    continue;
                }
            }

            if metadata.status == crate::acp::types::ConnectionStatus::Connecting {
                let idle_for = Utc::now()
                    .signed_duration_since(metadata.updated_at)
                    .num_seconds()
                    .max(0) as u64;
                if idle_for >= self.limits.connecting_timeout.as_secs() {
                    stale_ids.insert(connection_id.clone());
                    self.log_runtime(
                        "warn",
                        "acp",
                        format!(
                            "watchdog disconnecting {} after {}s stuck in connecting",
                            connection_id, idle_for
                        ),
                        None,
                    );
                }
            }
        }

        for connection_id in stale_ids {
            let _ = self.disconnect(&connection_id).await;
        }
    }
}

fn parse_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn trim_failure_window(
    failures: &mut VecDeque<Instant>,
    window: Duration,
    now: Instant,
) {
    while failures
        .front()
        .is_some_and(|instant| now.saturating_duration_since(*instant) > window)
    {
        failures.pop_front();
    }
}
