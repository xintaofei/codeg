use std::collections::HashMap;
use std::time::Duration;

use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::Mutex;

pub const WEB_CLIENT_ID_HEADER: &str = "x-codeg-client-id";
pub const WEB_CLIENT_CLEANUP_DELAY: Duration = Duration::from_secs(10);

const WEB_OWNER_FALLBACK: &str = "web";
const WEB_OWNER_PREFIX: &str = "web:";
const MAX_WEB_CLIENT_ID_LEN: usize = 128;

#[derive(Clone, Debug, Serialize)]
pub struct WebClientInfo {
    pub client_id: String,
    pub active_sockets: usize,
    pub connected_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
struct ClientLeaseState {
    active_sockets: usize,
    generation: u64,
    connected_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl Default for ClientLeaseState {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            active_sockets: 0,
            generation: 0,
            connected_at: now,
            updated_at: now,
        }
    }
}

#[derive(Default)]
pub struct WebClientRegistry {
    inner: Mutex<HashMap<String, ClientLeaseState>>,
}

#[derive(Clone, Debug)]
pub struct CleanupLease {
    pub client_id: String,
    generation: u64,
}

impl WebClientRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register_socket(&self, client_id: &str) {
        let mut inner = self.inner.lock().await;
        let entry = inner.entry(client_id.to_string()).or_default();
        if entry.active_sockets == 0 {
            entry.connected_at = Utc::now();
        }
        entry.active_sockets += 1;
        entry.generation = entry.generation.saturating_add(1);
        entry.updated_at = Utc::now();
    }

    pub async fn unregister_socket(&self, client_id: &str) -> Option<CleanupLease> {
        let mut inner = self.inner.lock().await;
        let entry = inner.get_mut(client_id)?;
        if entry.active_sockets > 0 {
            entry.active_sockets -= 1;
        }
        entry.generation = entry.generation.saturating_add(1);
        entry.updated_at = Utc::now();
        if entry.active_sockets == 0 {
            Some(CleanupLease {
                client_id: client_id.to_string(),
                generation: entry.generation,
            })
        } else {
            None
        }
    }

    pub async fn should_cleanup(&self, cleanup_lease: &CleanupLease) -> bool {
        let inner = self.inner.lock().await;
        inner.get(&cleanup_lease.client_id).is_some_and(|entry| {
            entry.active_sockets == 0 && entry.generation == cleanup_lease.generation
        })
    }

    pub async fn finish_cleanup(&self, cleanup_lease: &CleanupLease) {
        let mut inner = self.inner.lock().await;
        let should_remove = inner.get(&cleanup_lease.client_id).is_some_and(|entry| {
            entry.active_sockets == 0 && entry.generation == cleanup_lease.generation
        });
        if should_remove {
            inner.remove(&cleanup_lease.client_id);
        }
    }

    pub async fn has_active_client(&self, client_id: &str) -> bool {
        let inner = self.inner.lock().await;
        inner.get(client_id).is_some_and(|entry| entry.active_sockets > 0)
    }

    pub async fn list_clients(&self) -> Vec<WebClientInfo> {
        let inner = self.inner.lock().await;
        inner
            .iter()
            .map(|(client_id, entry)| WebClientInfo {
                client_id: client_id.clone(),
                active_sockets: entry.active_sockets,
                connected_at: entry.connected_at,
                updated_at: entry.updated_at,
            })
            .collect()
    }
}

pub fn owner_label_from_headers(headers: &HeaderMap) -> String {
    extract_web_client_id(headers)
        .map(|client_id| owner_label_for_client(&client_id))
        .unwrap_or_else(|| WEB_OWNER_FALLBACK.to_string())
}

pub fn owner_label_for_client(client_id: &str) -> String {
    format!("{WEB_OWNER_PREFIX}{client_id}")
}

pub fn extract_web_client_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get(WEB_CLIENT_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(normalize_web_client_id)
}

pub fn normalize_web_client_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_WEB_CLIENT_ID_LEN {
        return None;
    }

    if trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}
