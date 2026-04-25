//! Centralized reqwest client factory.
//!
//! Every outbound HTTP call inside this crate must go through one of the
//! helpers in this module so that user-configured proxy settings are honored
//! consistently. Previously each call site built its own client and relied on
//! `HTTP_PROXY` / `HTTPS_PROXY` env vars being mutated at runtime — that path
//! is unsound under multi-threading (Rust 2024 marks `set_var` `unsafe`) and
//! never affected long-lived clients which read env vars only at construction.
//!
//! The factory keeps a single `RwLock<SystemProxySettings>` snapshot. Reads
//! are cheap, writes happen only when the user toggles proxy settings or at
//! startup. Each call to [`build_client`] / [`build_client_with`] reads the
//! current snapshot and injects an explicit `reqwest::Proxy` so the resulting
//! client has its proxy baked in.

use std::sync::OnceLock;
use std::sync::RwLock;
use std::time::Duration;

use crate::models::SystemProxySettings;

fn snapshot_lock() -> &'static RwLock<SystemProxySettings> {
    static SNAPSHOT: OnceLock<RwLock<SystemProxySettings>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| RwLock::new(SystemProxySettings::default()))
}

/// Update the global proxy snapshot. Subsequent calls to [`build_client`] will
/// honor the new settings; previously constructed clients keep their old
/// proxy. Callers that want to force-rebuild should hold their own
/// `Arc<reqwest::Client>` and recreate it.
pub fn set_proxy_settings(settings: SystemProxySettings) {
    if let Ok(mut guard) = snapshot_lock().write() {
        *guard = settings;
    }
}

/// Read the current proxy snapshot.
pub fn current_proxy_settings() -> SystemProxySettings {
    snapshot_lock()
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

/// Builder configuration for an HTTP client. Defaults match the previous
/// per-site values used across the codebase.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub connect_timeout: Duration,
    pub timeout: Duration,
    pub user_agent: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            timeout: Duration::from_secs(30),
            user_agent: None,
        }
    }
}

impl ClientConfig {
    pub fn with_timeouts(connect: Duration, total: Duration) -> Self {
        Self {
            connect_timeout: connect,
            timeout: total,
            ..Self::default()
        }
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
}

fn proxy_for(snapshot: &SystemProxySettings) -> Option<reqwest::Proxy> {
    if !snapshot.enabled {
        return None;
    }
    let url = snapshot.proxy_url.as_deref()?.trim();
    if url.is_empty() {
        return None;
    }
    reqwest::Proxy::all(url).ok()
}

/// Build a `reqwest::Client` using the given config and the current proxy
/// snapshot. Falls back to an unconfigured client if the builder fails — the
/// same behavior the prior `unwrap_or_default()` callers relied on.
pub fn build_client_with(config: ClientConfig) -> reqwest::Client {
    let snapshot = current_proxy_settings();
    let mut builder = reqwest::Client::builder()
        .connect_timeout(config.connect_timeout)
        .timeout(config.timeout);
    if let Some(ua) = &config.user_agent {
        builder = builder.user_agent(ua);
    }
    if let Some(proxy) = proxy_for(&snapshot) {
        builder = builder.proxy(proxy);
    }
    builder.build().unwrap_or_default()
}

/// Convenience helper for ad-hoc one-shot requests (used to be
/// `reqwest::Client::new()` at call sites).
pub fn build_client() -> reqwest::Client {
    build_client_with(ClientConfig::default())
}
