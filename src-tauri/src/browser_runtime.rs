use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
#[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use async_trait::async_trait;
#[cfg(feature = "tauri-runtime")]
use futures_util::StreamExt;
use sacp::schema::{HttpHeader, McpServer, McpServerHttp};
use serde::{Deserialize, Serialize};
#[cfg(feature = "tauri-runtime")]
use tauri::ipc::Channel;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader, Lines};
use tokio::process::{Child, ChildStdout};
use tokio::sync::{oneshot, Mutex};

use crate::acp::connection::BrowserMcpProvider;
use crate::db::service::app_metadata_service;
use crate::db::AppDatabase;
#[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
use crate::native_browser_runtime::{
    NativeBrowserError, NativeBrowserRuntime, NativeBrowserSnapshot, NativeHistoryDirection,
    NativeSurfaceBounds,
};
use crate::web::event_bridge::{emit_event, EventEmitter};

const SETTINGS_KEY: &str = "browser_runtime_settings";
const STATUS_EVENT: &str = "browser://status";
const READY_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(65);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(2);
const HEALTH_POLL_TIMEOUT: Duration = Duration::from_secs(5);
const LOG_LIMIT: usize = 100;
const EXTERNAL_BACKEND_ID: &str = "external_chromium_cdp";
const EMBEDDED_BACKEND_ID: &str = "embedded_webview2";
const SURFACE_EVENT: &str = "browser://session-activity";
#[cfg(feature = "tauri-runtime")]
const MAX_SURFACE_EVENT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRuntimeSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub browser_path: Option<String>,
    #[serde(default)]
    pub backend: BrowserRuntimeBackend,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRuntimeBackend {
    Embedded,
    #[default]
    External,
}

impl BrowserRuntimeBackend {
    fn id(self) -> &'static str {
        match self {
            Self::Embedded => EMBEDDED_BACKEND_ID,
            Self::External => EXTERNAL_BACKEND_ID,
        }
    }

    fn cli_value(self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::External => "external",
        }
    }
}

impl Default for BrowserRuntimeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_start: false,
            browser_path: None,
            backend: BrowserRuntimeBackend::External,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRuntimeState {
    NotInstalled,
    Stopped,
    Starting,
    Ready,
    Error,
    Recovering,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRuntimeStatus {
    pub state: BrowserRuntimeState,
    pub installed: bool,
    pub enabled: bool,
    pub auto_start: bool,
    pub browser_path: Option<String>,
    pub sidecar_pid: Option<u32>,
    pub browser_pid: Option<u32>,
    pub runtime_version: Option<String>,
    pub backend: String,
    pub browser_name: Option<String>,
    pub browser_version: Option<String>,
    pub profile_path: String,
    pub download_path: String,
    pub recovery_attempt: u8,
    pub last_error_code: Option<String>,
    pub recent_logs: Vec<BrowserLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLogEntry {
    pub at: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSurfaceTab {
    pub id: String,
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSurfacePageState {
    pub tab: BrowserSurfaceTab,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSurfaceKind {
    #[default]
    Frame,
    Native,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSurfaceSnapshot {
    pub session_id: String,
    #[serde(default)]
    pub surface_kind: BrowserSurfaceKind,
    pub tabs: Vec<BrowserSurfaceTab>,
    pub active_target_id: Option<String>,
    pub active: Option<BrowserSurfacePageState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSurfaceFrame {
    pub target_id: String,
    pub data: String,
    pub mime_type: String,
    pub device_width: u32,
    pub device_height: u32,
    pub page_scale_factor: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserSurfaceEvent {
    Snapshot {
        snapshot: BrowserSurfaceSnapshot,
    },
    Frame {
        frame: BrowserSurfaceFrame,
    },
    Error {
        code: String,
        message: String,
        retryable: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSessionActivity {
    connection_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BrowserRuntimeError {
    #[error("Browser runtime is only available in the Windows desktop client")]
    UnsupportedPlatform,
    #[error("Browser runtime is disabled in settings")]
    Disabled,
    #[error("Browser runtime sidecar is not installed")]
    NotInstalled,
    #[error("Browser runtime failed to start")]
    StartFailed,
    #[error("Browser runtime did not become ready in time")]
    ReadyTimeout,
    #[error("Browser runtime control request failed")]
    ControlFailed,
    #[error("Native Browser runtime operation failed")]
    NativeFailed,
    #[error("Browser runtime settings could not be saved")]
    SettingsFailed,
}

struct BrowserRuntimePrivate {
    status: BrowserRuntimeStatus,
    settings: BrowserRuntimeSettings,
    sidecar_installed: bool,
    sidecar_backend: Option<BrowserRuntimeBackend>,
    child: Option<Child>,
    endpoint: Option<String>,
    token: Option<String>,
}

struct BrowserSurfaceAttachment {
    #[cfg_attr(not(feature = "tauri-runtime"), allow(dead_code))]
    generation: u64,
    cancel: oneshot::Sender<()>,
    done: oneshot::Receiver<()>,
}

#[derive(Default)]
struct BrowserSurfaceEntry {
    generation: u64,
    attachment: Option<BrowserSurfaceAttachment>,
}

#[derive(Default)]
struct BrowserSurfaceRegistry {
    entries: HashMap<String, BrowserSurfaceEntry>,
}

impl BrowserSurfaceRegistry {
    fn ensure(&mut self, connection_id: &str) -> u64 {
        self.entries
            .entry(connection_id.to_string())
            .or_default()
            .generation
    }

    #[cfg_attr(not(feature = "tauri-runtime"), allow(dead_code))]
    fn next_generation(&mut self, connection_id: &str) -> u64 {
        let entry = self.entries.entry(connection_id.to_string()).or_default();
        entry.generation = entry.generation.saturating_add(1);
        entry.generation
    }

    #[cfg(test)]
    fn contains(&self, connection_id: &str) -> bool {
        self.entries.contains_key(connection_id)
    }

    fn remove(&mut self, connection_id: &str) -> Option<BrowserSurfaceAttachment> {
        self.entries
            .remove(connection_id)
            .and_then(|entry| entry.attachment)
    }
}

#[derive(Clone)]
pub struct BrowserRuntimeManager {
    inner: Arc<Mutex<BrowserRuntimePrivate>>,
    data_dir: PathBuf,
    emitter: EventEmitter,
    client: reqwest::Client,
    #[cfg(feature = "tauri-runtime")]
    stream_client: reqwest::Client,
    surface_registry: Arc<Mutex<BrowserSurfaceRegistry>>,
    surface_attach_gate: Arc<Mutex<()>>,
    #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
    native_runtime: Arc<StdMutex<Option<NativeBrowserRuntime>>>,
}

impl BrowserRuntimeManager {
    pub fn new(data_dir: PathBuf, emitter: EventEmitter) -> Self {
        let installed = locate_sidecar().is_some();
        let profile_path = data_dir
            .join("browser")
            .join("profile")
            .to_string_lossy()
            .into_owned();
        let download_path = data_dir
            .join("browser")
            .join("downloads")
            .to_string_lossy()
            .into_owned();
        let state = if installed {
            BrowserRuntimeState::Stopped
        } else {
            BrowserRuntimeState::NotInstalled
        };
        Self {
            inner: Arc::new(Mutex::new(BrowserRuntimePrivate {
                status: BrowserRuntimeStatus {
                    state,
                    installed,
                    enabled: false,
                    auto_start: false,
                    browser_path: None,
                    sidecar_pid: None,
                    browser_pid: None,
                    runtime_version: None,
                    backend: BrowserRuntimeBackend::External.id().to_string(),
                    browser_name: None,
                    browser_version: None,
                    profile_path,
                    download_path,
                    recovery_attempt: 0,
                    last_error_code: None,
                    recent_logs: Vec::new(),
                },
                settings: BrowserRuntimeSettings::default(),
                sidecar_installed: installed,
                sidecar_backend: None,
                child: None,
                endpoint: None,
                token: None,
            })),
            data_dir,
            emitter,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("Browser runtime HTTP client configuration is valid"),
            #[cfg(feature = "tauri-runtime")]
            stream_client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("Browser surface HTTP client configuration is valid"),
            surface_registry: Arc::new(Mutex::new(BrowserSurfaceRegistry::default())),
            surface_attach_gate: Arc::new(Mutex::new(())),
            #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
            native_runtime: Arc::new(StdMutex::new(None)),
        }
    }

    #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
    pub fn install_native_runtime(&self, app: tauri::AppHandle) {
        if let Ok(mut slot) = self.native_runtime.lock() {
            *slot = Some(NativeBrowserRuntime::new(
                app,
                &self.data_dir,
                self.emitter.clone(),
            ));
        }
    }

    pub async fn load_settings(
        &self,
        db: &AppDatabase,
    ) -> Result<BrowserRuntimeSettings, BrowserRuntimeError> {
        let saved = app_metadata_service::get_value(&db.conn, SETTINGS_KEY)
            .await
            .map_err(|_| BrowserRuntimeError::SettingsFailed)?;
        let settings = normalize_settings(
            saved
                .and_then(|raw| serde_json::from_str::<BrowserRuntimeSettings>(&raw).ok())
                .unwrap_or_default(),
        );
        self.apply_settings_snapshot(settings.clone()).await;
        Ok(settings)
    }

    pub async fn settings(&self) -> BrowserRuntimeSettings {
        self.inner.lock().await.settings.clone()
    }

    pub async fn update_settings(
        &self,
        db: &AppDatabase,
        settings: BrowserRuntimeSettings,
    ) -> Result<BrowserRuntimeSettings, BrowserRuntimeError> {
        let settings = normalize_settings(settings);
        let previous = self.settings().await;
        let was_running = matches!(
            self.inner.lock().await.status.state,
            BrowserRuntimeState::Starting
                | BrowserRuntimeState::Ready
                | BrowserRuntimeState::Recovering
        );
        let backend_changed = settings.backend != previous.backend;

        if backend_changed && was_running {
            let _ = self.stop().await;
            self.apply_settings_snapshot(settings.clone()).await;
            if self.start().await.is_err() {
                self.rollback_settings(previous, was_running).await;
                return Err(BrowserRuntimeError::ControlFailed);
            }
            if persist_settings(db, &settings).await.is_err() {
                self.rollback_settings(previous, was_running).await;
                return Err(BrowserRuntimeError::SettingsFailed);
            }
            return Ok(settings);
        }

        persist_settings(db, &settings).await?;
        self.apply_settings_snapshot(settings.clone()).await;
        Ok(settings)
    }

    async fn rollback_settings(&self, settings: BrowserRuntimeSettings, restart: bool) {
        let _ = self.stop().await;
        self.apply_settings_snapshot(settings).await;
        if restart {
            let _ = self.start().await;
        }
    }

    pub async fn status(&self) -> BrowserRuntimeStatus {
        self.inner.lock().await.status.clone()
    }

    pub async fn start(&self) -> Result<BrowserRuntimeStatus, BrowserRuntimeError> {
        let (already_running, backend) = {
            let mut inner = self.inner.lock().await;
            if inner.status.state == BrowserRuntimeState::Ready {
                return Ok(inner.status.clone());
            }
            inner.status.state = BrowserRuntimeState::Starting;
            inner.status.last_error_code = None;
            (inner.endpoint.is_some(), inner.settings.backend)
        };
        self.emit_status().await;

        if backend == BrowserRuntimeBackend::Embedded {
            if let Err(error) = self.start_embedded().await {
                self.set_error(error_code(&error)).await;
                return Err(error);
            }
            return Ok(self.status().await);
        }

        if !already_running {
            if let Err(error) = self.spawn_sidecar(None).await {
                self.set_error(error_code(&error)).await;
                return Err(error);
            }
        }
        if let Err(error) = self.control("start").await {
            self.set_error(error_code(&error)).await;
            return Err(error);
        }
        self.refresh_health().await?;
        Ok(self.status().await)
    }

    pub async fn stop(&self) -> Result<BrowserRuntimeStatus, BrowserRuntimeError> {
        let backend = self.inner.lock().await.settings.backend;
        if backend == BrowserRuntimeBackend::Embedded {
            self.detach_all_surfaces(true).await;
            if self.inner.lock().await.endpoint.is_some() {
                let _ = self.control("stop").await;
            }
            self.terminate_sidecar().await;
            self.stop_embedded().await;
            let mut inner = self.inner.lock().await;
            inner.endpoint = None;
            inner.token = None;
            inner.sidecar_backend = None;
            inner.status.sidecar_pid = None;
            inner.status.browser_pid = None;
            inner.status.runtime_version = None;
            inner.status.browser_name = None;
            inner.status.browser_version = None;
            inner.status.recovery_attempt = 0;
            inner.status.last_error_code = None;
            inner.status.state = if inner.status.installed {
                BrowserRuntimeState::Stopped
            } else {
                BrowserRuntimeState::NotInstalled
            };
            drop(inner);
            self.emit_status().await;
            return Ok(self.status().await);
        }
        self.detach_all_surfaces(false).await;
        let has_endpoint = self.inner.lock().await.endpoint.is_some();
        if has_endpoint {
            let _ = self.control("stop").await;
        }
        let child = {
            let mut inner = self.inner.lock().await;
            inner.endpoint = None;
            inner.token = None;
            inner.sidecar_backend = None;
            inner.status.sidecar_pid = None;
            inner.status.browser_pid = None;
            inner.status.runtime_version = None;
            inner.status.browser_name = None;
            inner.status.browser_version = None;
            inner.status.recovery_attempt = 0;
            inner.status.last_error_code = None;
            inner.status.state = if inner.status.installed {
                BrowserRuntimeState::Stopped
            } else {
                BrowserRuntimeState::NotInstalled
            };
            inner.child.take()
        };
        if let Some(mut child) = child {
            if let Some(pid) = child.id() {
                let _ = kill_tree::tokio::kill_tree(pid).await;
            } else {
                let _ = child.start_kill();
            }
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        }
        self.emit_status().await;
        Ok(self.status().await)
    }

    pub async fn recover(&self) -> Result<BrowserRuntimeStatus, BrowserRuntimeError> {
        let backend = self.inner.lock().await.settings.backend;
        if backend == BrowserRuntimeBackend::Embedded {
            {
                let mut inner = self.inner.lock().await;
                inner.status.state = BrowserRuntimeState::Recovering;
                inner.status.last_error_code = None;
            }
            self.emit_status().await;
            if let Err(error) = self.recover_embedded().await {
                self.set_error(error_code(&error)).await;
                return Err(error);
            }
            return Ok(self.status().await);
        }
        self.detach_all_surfaces(false).await;
        {
            let mut inner = self.inner.lock().await;
            inner.status.state = BrowserRuntimeState::Recovering;
            inner.status.last_error_code = None;
        }
        self.emit_status().await;
        if self.inner.lock().await.endpoint.is_none() {
            self.spawn_sidecar(None).await?;
        }
        if let Err(error) = self.control("recover").await {
            self.set_error(error_code(&error)).await;
            return Err(error);
        }
        self.refresh_health().await?;
        Ok(self.status().await)
    }

    pub async fn doctor(&self) -> Result<serde_json::Value, BrowserRuntimeError> {
        if self.inner.lock().await.settings.backend == BrowserRuntimeBackend::Embedded {
            return self.embedded_doctor();
        }
        if self.inner.lock().await.endpoint.is_none() {
            self.spawn_sidecar(None).await?;
        }
        self.control("doctor").await
    }

    pub async fn diagnostics(&self) -> Result<serde_json::Value, BrowserRuntimeError> {
        if self.inner.lock().await.settings.backend == BrowserRuntimeBackend::Embedded {
            return self.embedded_diagnostics();
        }
        if self.inner.lock().await.endpoint.is_none() {
            return Ok(serde_json::json!({
                "status": self.status().await,
                "audit": [],
            }));
        }
        self.request(reqwest::Method::GET, "/admin/diagnostics", None)
            .await
    }

    pub async fn release_session(&self, session_id: &str) {
        let _ = self.detach_surface(session_id).await;
        self.surface_registry
            .lock()
            .await
            .entries
            .remove(session_id);
        if self.inner.lock().await.settings.backend == BrowserRuntimeBackend::Embedded {
            let released = self
                .request(
                    reqwest::Method::POST,
                    "/admin/release-session",
                    Some(serde_json::json!({ "sessionId": session_id })),
                )
                .await
                .is_ok();
            if !released {
                self.release_embedded_session(session_id).await;
            }
            return;
        }
        let _ = self
            .request(
                reqwest::Method::POST,
                "/admin/release-session",
                Some(serde_json::json!({ "sessionId": session_id })),
            )
            .await;
    }

    pub async fn ensure_surface(
        &self,
        connection_id: &str,
    ) -> Result<BrowserSurfaceSnapshot, BrowserRuntimeError> {
        validate_connection_id(connection_id)?;
        self.surface_registry.lock().await.ensure(connection_id);
        if self.inner.lock().await.settings.backend == BrowserRuntimeBackend::Embedded {
            return self.ensure_embedded_surface(connection_id).await;
        }
        let value = self
            .request(
                reqwest::Method::POST,
                "/admin/surface/ensure",
                Some(serde_json::json!({ "sessionId": connection_id })),
            )
            .await?;
        parse_surface_snapshot(value, connection_id)
    }

    pub async fn surface_action(
        &self,
        connection_id: &str,
        action: serde_json::Value,
    ) -> Result<BrowserSurfaceSnapshot, BrowserRuntimeError> {
        validate_connection_id(connection_id)?;
        self.surface_registry.lock().await.ensure(connection_id);
        if self.inner.lock().await.settings.backend == BrowserRuntimeBackend::Embedded {
            return self.embedded_surface_action(connection_id, action).await;
        }
        let mut body = action
            .as_object()
            .cloned()
            .ok_or(BrowserRuntimeError::ControlFailed)?;
        body.insert(
            "sessionId".to_string(),
            serde_json::Value::String(connection_id.to_string()),
        );
        let value = self
            .request(
                reqwest::Method::POST,
                "/admin/surface/action",
                Some(serde_json::Value::Object(body)),
            )
            .await?;
        parse_surface_snapshot(value, connection_id)
    }

    #[cfg(feature = "tauri-runtime")]
    pub async fn attach_surface(
        &self,
        connection_id: &str,
        channel: Channel<BrowserSurfaceEvent>,
    ) -> Result<BrowserSurfaceSnapshot, BrowserRuntimeError> {
        validate_connection_id(connection_id)?;
        let _gate = self.surface_attach_gate.lock().await;
        self.stop_surface_attachment(connection_id).await;
        let snapshot = self.ensure_surface(connection_id).await?;
        let embedded = self.inner.lock().await.settings.backend == BrowserRuntimeBackend::Embedded;
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (done_tx, done_rx) = oneshot::channel();
        let generation = {
            let mut registry = self.surface_registry.lock().await;
            let generation = registry.next_generation(connection_id);
            registry
                .entries
                .get_mut(connection_id)
                .expect("surface entry was ensured")
                .attachment = Some(BrowserSurfaceAttachment {
                generation,
                cancel: cancel_tx,
                done: done_rx,
            });
            generation
        };
        let manager = self.clone();
        let owned_connection_id = connection_id.to_string();
        tokio::spawn(async move {
            let stream_result = if embedded {
                manager
                    .run_native_surface_stream(&owned_connection_id, channel.clone(), cancel_rx)
                    .await
            } else {
                manager
                    .run_surface_stream(&owned_connection_id, channel.clone(), cancel_rx)
                    .await
            };
            if stream_result.is_err() {
                let _ = channel.send(BrowserSurfaceEvent::Error {
                    code: "SURFACE_STREAM_FAILED".to_string(),
                    message: "Browser surface stream disconnected".to_string(),
                    retryable: true,
                });
            }
            let _ = done_tx.send(());
            let mut registry = manager.surface_registry.lock().await;
            let should_clear = registry
                .entries
                .get(&owned_connection_id)
                .and_then(|entry| entry.attachment.as_ref())
                .is_some_and(|attachment| attachment.generation == generation);
            if should_clear {
                if let Some(entry) = registry.entries.get_mut(&owned_connection_id) {
                    entry.attachment = None;
                }
            }
        });
        Ok(snapshot)
    }

    pub async fn detach_surface(&self, connection_id: &str) -> Result<(), BrowserRuntimeError> {
        validate_connection_id(connection_id)?;
        let _gate = self.surface_attach_gate.lock().await;
        self.stop_surface_attachment(connection_id).await;
        self.hide_embedded_surface(connection_id);
        Ok(())
    }

    pub async fn close_surface(&self, connection_id: &str) -> Result<(), BrowserRuntimeError> {
        validate_connection_id(connection_id)?;
        let _gate = self.surface_attach_gate.lock().await;
        self.stop_surface_attachment(connection_id).await;
        self.surface_registry.lock().await.remove(connection_id);
        if self.inner.lock().await.settings.backend == BrowserRuntimeBackend::Embedded {
            let released = self
                .request(
                    reqwest::Method::POST,
                    "/admin/release-session",
                    Some(serde_json::json!({ "sessionId": connection_id })),
                )
                .await
                .is_ok();
            if !released {
                self.release_embedded_session(connection_id).await;
            }
            return Ok(());
        }
        self.request(
            reqwest::Method::POST,
            "/admin/release-session",
            Some(serde_json::json!({ "sessionId": connection_id })),
        )
        .await?;
        Ok(())
    }

    async fn stop_surface_attachment(&self, connection_id: &str) {
        let attachment = {
            let mut registry = self.surface_registry.lock().await;
            registry
                .entries
                .get_mut(connection_id)
                .and_then(|entry| entry.attachment.take())
        };
        if let Some(attachment) = attachment {
            let _ = attachment.cancel.send(());
            let _ = tokio::time::timeout(Duration::from_secs(2), attachment.done).await;
        }
    }

    async fn detach_all_surfaces(&self, remove_entries: bool) {
        let _gate = self.surface_attach_gate.lock().await;
        let connection_ids = {
            let registry = self.surface_registry.lock().await;
            registry.entries.keys().cloned().collect::<Vec<_>>()
        };
        for connection_id in connection_ids {
            self.stop_surface_attachment(&connection_id).await;
        }
        if remove_entries {
            self.surface_registry.lock().await.entries.clear();
        }
        self.hide_all_embedded_surfaces();
    }

    #[cfg(feature = "tauri-runtime")]
    async fn run_surface_stream(
        &self,
        connection_id: &str,
        channel: Channel<BrowserSurfaceEvent>,
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<(), BrowserRuntimeError> {
        let (endpoint, token) = self.private_endpoint().await?;
        let response = self
            .stream_client
            .get(format!("{endpoint}/admin/surface/stream"))
            .query(&[("sessionId", connection_id)])
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "application/x-ndjson")
            .send()
            .await
            .map_err(|_| BrowserRuntimeError::ControlFailed)?;
        if !response.status().is_success() {
            return Err(BrowserRuntimeError::ControlFailed);
        }
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::<u8>::new();
        let mut mapped_targets = std::collections::HashSet::<String>::new();
        loop {
            let chunk = tokio::select! {
                _ = &mut cancel => return Ok(()),
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = chunk else {
                return Err(BrowserRuntimeError::ControlFailed);
            };
            let chunk = chunk.map_err(|_| BrowserRuntimeError::ControlFailed)?;
            buffer.extend_from_slice(&chunk);
            if buffer.len() > MAX_SURFACE_EVENT_BYTES {
                let _ = self.release_surface_remote(connection_id).await;
                return Err(BrowserRuntimeError::ControlFailed);
            }
            while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = buffer.drain(..=newline).collect();
                if line.len() <= 1 {
                    continue;
                }
                let event = serde_json::from_slice::<BrowserSurfaceEvent>(&line[..line.len() - 1])
                    .map_err(|_| BrowserRuntimeError::ControlFailed)?;
                match &event {
                    BrowserSurfaceEvent::Snapshot { snapshot } => {
                        validate_surface_snapshot(snapshot, connection_id)?;
                        mapped_targets = snapshot.tabs.iter().map(|tab| tab.id.clone()).collect();
                    }
                    BrowserSurfaceEvent::Frame { frame } => {
                        if !mapped_targets.contains(&frame.target_id) {
                            let _ = self.release_surface_remote(connection_id).await;
                            return Err(BrowserRuntimeError::ControlFailed);
                        }
                    }
                    BrowserSurfaceEvent::Error { .. } => {}
                }
                if channel.send(event).is_err() {
                    return Ok(());
                }
            }
        }
    }

    #[cfg(feature = "tauri-runtime")]
    async fn private_endpoint(&self) -> Result<(String, String), BrowserRuntimeError> {
        let inner = self.inner.lock().await;
        Ok((
            inner
                .endpoint
                .clone()
                .ok_or(BrowserRuntimeError::ControlFailed)?,
            inner
                .token
                .clone()
                .ok_or(BrowserRuntimeError::ControlFailed)?,
        ))
    }

    #[cfg(feature = "tauri-runtime")]
    async fn release_surface_remote(&self, connection_id: &str) -> Result<(), BrowserRuntimeError> {
        self.request(
            reqwest::Method::POST,
            "/admin/release-session",
            Some(serde_json::json!({ "sessionId": connection_id })),
        )
        .await?;
        Ok(())
    }

    #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
    fn native_runtime(&self) -> Result<NativeBrowserRuntime, BrowserRuntimeError> {
        self.native_runtime
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .ok_or(BrowserRuntimeError::NativeFailed)
    }

    async fn start_embedded(&self) -> Result<(), BrowserRuntimeError> {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        {
            let runtime = self.native_runtime()?;
            let credentials = runtime.start().await.map_err(map_native_error)?;
            let needs_sidecar = {
                let inner = self.inner.lock().await;
                inner.child.is_none()
                    || inner.endpoint.is_none()
                    || inner.sidecar_backend != Some(BrowserRuntimeBackend::Embedded)
            };
            if needs_sidecar {
                if let Err(error) = self
                    .spawn_sidecar(Some((credentials.endpoint, credentials.token)))
                    .await
                {
                    runtime.stop().await;
                    return Err(error);
                }
            }
            if let Err(error) = self.control("start").await {
                self.terminate_sidecar().await;
                runtime.stop().await;
                return Err(error);
            }
            if let Err(error) = self.refresh_health().await {
                self.terminate_sidecar().await;
                runtime.stop().await;
                return Err(error);
            }
            let mut inner = self.inner.lock().await;
            inner.status.installed = true;
            inner.status.browser_pid = None;
            let sidecar_version = inner
                .status
                .runtime_version
                .clone()
                .unwrap_or_else(|| "sidecar".to_string());
            inner.status.runtime_version = Some(format!(
                "{}+native-bridge-v{}",
                sidecar_version,
                crate::native_browser_runtime::NATIVE_BRIDGE_PROTOCOL_VERSION,
            ));
            inner.status.backend = EMBEDDED_BACKEND_ID.to_string();
            inner.status.browser_name = Some("Microsoft Edge WebView2".to_string());
            inner.status.browser_version = None;
            inner.status.recovery_attempt = 0;
            inner.status.last_error_code = None;
            inner.status.state = BrowserRuntimeState::Ready;
            push_log(&mut inner.status, "native_bridge_ready");
            drop(inner);
            self.emit_status().await;
            Ok(())
        }
        #[cfg(not(all(feature = "tauri-runtime", target_os = "windows")))]
        {
            Err(BrowserRuntimeError::UnsupportedPlatform)
        }
    }

    async fn stop_embedded(&self) {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        if let Ok(runtime) = self.native_runtime() {
            runtime.stop().await;
        }
    }

    async fn recover_embedded(&self) -> Result<(), BrowserRuntimeError> {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        {
            let runtime = self.native_runtime()?;
            let credentials = runtime.start().await.map_err(map_native_error)?;
            runtime
                .recover_failed_tabs()
                .await
                .map_err(map_native_error)?;
            let needs_sidecar = {
                let inner = self.inner.lock().await;
                inner.child.is_none()
                    || inner.endpoint.is_none()
                    || inner.sidecar_backend != Some(BrowserRuntimeBackend::Embedded)
            };
            if needs_sidecar {
                self.spawn_sidecar(Some((credentials.endpoint, credentials.token)))
                    .await?;
            }
            self.control("recover").await?;
            self.refresh_health().await?;
            let mut inner = self.inner.lock().await;
            inner.status.installed = true;
            inner.status.state = BrowserRuntimeState::Ready;
            inner.status.last_error_code = None;
            inner.status.recovery_attempt = 0;
            push_log(&mut inner.status, "native_recovered");
            drop(inner);
            self.emit_status().await;
            Ok(())
        }
        #[cfg(not(all(feature = "tauri-runtime", target_os = "windows")))]
        {
            Err(BrowserRuntimeError::UnsupportedPlatform)
        }
    }

    fn embedded_doctor(&self) -> Result<serde_json::Value, BrowserRuntimeError> {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        {
            Ok(self.native_runtime()?.doctor())
        }
        #[cfg(not(all(feature = "tauri-runtime", target_os = "windows")))]
        {
            Err(BrowserRuntimeError::UnsupportedPlatform)
        }
    }

    fn embedded_diagnostics(&self) -> Result<serde_json::Value, BrowserRuntimeError> {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        {
            Ok(self.native_runtime()?.diagnostics())
        }
        #[cfg(not(all(feature = "tauri-runtime", target_os = "windows")))]
        {
            Err(BrowserRuntimeError::UnsupportedPlatform)
        }
    }

    async fn release_embedded_session(&self, _connection_id: &str) {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        if let Ok(runtime) = self.native_runtime() {
            runtime.release_session(_connection_id).await;
        }
    }

    async fn ensure_embedded_surface(
        &self,
        connection_id: &str,
    ) -> Result<BrowserSurfaceSnapshot, BrowserRuntimeError> {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        {
            let snapshot = self
                .native_runtime()?
                .ensure_session(connection_id)
                .await
                .map_err(map_native_error)?;
            Ok(native_snapshot_to_surface(snapshot))
        }
        #[cfg(not(all(feature = "tauri-runtime", target_os = "windows")))]
        {
            let _ = connection_id;
            Err(BrowserRuntimeError::UnsupportedPlatform)
        }
    }

    async fn embedded_surface_action(
        &self,
        connection_id: &str,
        action: serde_json::Value,
    ) -> Result<BrowserSurfaceSnapshot, BrowserRuntimeError> {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        {
            let runtime = self.native_runtime()?;
            let object = action
                .as_object()
                .ok_or(BrowserRuntimeError::ControlFailed)?;
            let action_name = object
                .get("action")
                .and_then(serde_json::Value::as_str)
                .ok_or(BrowserRuntimeError::ControlFailed)?;
            let snapshot = match action_name {
                "open" => {
                    let url = object.get("url").and_then(serde_json::Value::as_str);
                    runtime
                        .create_tab(connection_id, url)
                        .await
                        .map_err(map_native_error)?
                }
                "focus" | "close" => {
                    let tab_id = object
                        .get("targetId")
                        .and_then(serde_json::Value::as_str)
                        .ok_or(BrowserRuntimeError::ControlFailed)?;
                    let snapshot = runtime.snapshot(connection_id).map_err(map_native_error)?;
                    let generation = snapshot
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .map(|tab| tab.generation)
                        .ok_or(BrowserRuntimeError::ControlFailed)?;
                    if action_name == "focus" {
                        runtime
                            .focus_tab(connection_id, tab_id, generation)
                            .map_err(map_native_error)?
                    } else {
                        runtime
                            .close_tab(connection_id, tab_id, generation)
                            .await
                            .map_err(map_native_error)?
                    }
                }
                "surface" => {
                    let bounds = object
                        .get("bounds")
                        .and_then(serde_json::Value::as_object)
                        .ok_or(BrowserRuntimeError::ControlFailed)?;
                    let x = bounds
                        .get("x")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or(BrowserRuntimeError::ControlFailed)?;
                    let y = bounds
                        .get("y")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or(BrowserRuntimeError::ControlFailed)?;
                    let width = bounds
                        .get("width")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or(BrowserRuntimeError::ControlFailed)?;
                    let height = bounds
                        .get("height")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or(BrowserRuntimeError::ControlFailed)?;
                    let visible = object
                        .get("visible")
                        .and_then(serde_json::Value::as_bool)
                        .ok_or(BrowserRuntimeError::ControlFailed)?;
                    let (tab_id, generation) = runtime
                        .active_identity(connection_id)
                        .map_err(map_native_error)?;
                    runtime
                        .set_surface(
                            connection_id,
                            &tab_id,
                            generation,
                            NativeSurfaceBounds {
                                x,
                                y,
                                width,
                                height,
                            },
                            visible,
                        )
                        .map_err(map_native_error)?
                }
                "navigate" => {
                    let url = object
                        .get("url")
                        .and_then(serde_json::Value::as_str)
                        .ok_or(BrowserRuntimeError::ControlFailed)?;
                    let (tab_id, generation) = runtime
                        .active_identity(connection_id)
                        .map_err(map_native_error)?;
                    runtime
                        .navigate(connection_id, &tab_id, generation, url)
                        .map_err(map_native_error)?
                }
                "back" | "forward" => {
                    let (tab_id, generation) = runtime
                        .active_identity(connection_id)
                        .map_err(map_native_error)?;
                    runtime
                        .go_history(
                            connection_id,
                            &tab_id,
                            generation,
                            if action_name == "back" {
                                NativeHistoryDirection::Back
                            } else {
                                NativeHistoryDirection::Forward
                            },
                        )
                        .map_err(map_native_error)?
                }
                "reload" | "stop" => {
                    let (tab_id, generation) = runtime
                        .active_identity(connection_id)
                        .map_err(map_native_error)?;
                    if action_name == "reload" {
                        runtime
                            .reload(connection_id, &tab_id, generation)
                            .map_err(map_native_error)?
                    } else {
                        runtime
                            .stop_loading(connection_id, &tab_id, generation)
                            .map_err(map_native_error)?
                    }
                }
                // Native WebView2 receives user input directly. Keeping the old
                // host-coordinate input action alive would recreate the split
                // interaction path B2-18 is replacing.
                "resize" | "input" => return Err(BrowserRuntimeError::ControlFailed),
                _ => return Err(BrowserRuntimeError::ControlFailed),
            };
            Ok(native_snapshot_to_surface(snapshot))
        }
        #[cfg(not(all(feature = "tauri-runtime", target_os = "windows")))]
        {
            let _ = (connection_id, action);
            Err(BrowserRuntimeError::UnsupportedPlatform)
        }
    }

    #[cfg(feature = "tauri-runtime")]
    async fn run_native_surface_stream(
        &self,
        connection_id: &str,
        channel: Channel<BrowserSurfaceEvent>,
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<(), BrowserRuntimeError> {
        #[cfg(target_os = "windows")]
        {
            let runtime = self.native_runtime()?;
            let mut updates = runtime.subscribe();
            let initial = native_snapshot_to_surface(
                runtime.snapshot(connection_id).map_err(map_native_error)?,
            );
            if channel
                .send(BrowserSurfaceEvent::Snapshot { snapshot: initial })
                .is_err()
            {
                return Ok(());
            }
            loop {
                tokio::select! {
                    _ = &mut cancel => return Ok(()),
                    update = updates.recv() => {
                        match update {
                            Ok(updated_connection) if updated_connection == connection_id => {
                                let snapshot = native_snapshot_to_surface(
                                    runtime.snapshot(connection_id).map_err(map_native_error)?,
                                );
                                if channel.send(BrowserSurfaceEvent::Snapshot { snapshot }).is_err() {
                                    return Ok(());
                                }
                            }
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                return Err(BrowserRuntimeError::NativeFailed);
                            }
                        }
                    }
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (connection_id, channel, &mut cancel);
            Err(BrowserRuntimeError::UnsupportedPlatform)
        }
    }

    fn hide_embedded_surface(&self, _connection_id: &str) {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        if let Ok(runtime) = self.native_runtime() {
            runtime.hide_session(_connection_id);
        }
    }

    fn hide_all_embedded_surfaces(&self) {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        if let Ok(runtime) = self.native_runtime() {
            runtime.hide_all();
        }
    }

    #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
    pub fn sync_native_surface_geometry(&self) {
        if let Ok(runtime) = self.native_runtime() {
            runtime.reapply_surface_geometry();
        }
    }

    async fn spawn_sidecar(
        &self,
        native_bridge: Option<(String, String)>,
    ) -> Result<(), BrowserRuntimeError> {
        let executable = locate_sidecar().ok_or({
            if cfg!(windows) {
                BrowserRuntimeError::NotInstalled
            } else {
                BrowserRuntimeError::UnsupportedPlatform
            }
        })?;
        let (browser_path, backend, profile_dir, download_dir) = {
            let inner = self.inner.lock().await;
            (
                inner.settings.browser_path.clone(),
                inner.settings.backend,
                self.data_dir.join("browser").join("profile"),
                self.data_dir.join("browser").join("downloads"),
            )
        };
        std::fs::create_dir_all(&profile_dir).map_err(|_| BrowserRuntimeError::StartFailed)?;
        std::fs::create_dir_all(&download_dir).map_err(|_| BrowserRuntimeError::StartFailed)?;
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut command = crate::process::tokio_command(&executable);
        command
            .arg("--profile-dir")
            .arg(&profile_dir)
            .arg("--download-dir")
            .arg(&download_dir)
            .arg("--parent-pid")
            .arg(std::process::id().to_string())
            .arg("--backend")
            .arg(backend.cli_value())
            .env("CODEG_BROWSER_TOKEN", &token)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        match (backend, native_bridge) {
            (BrowserRuntimeBackend::Embedded, Some((endpoint, token))) => {
                command
                    .env("CODEG_BROWSER_NATIVE_BRIDGE_ENDPOINT", endpoint)
                    .env("CODEG_BROWSER_NATIVE_BRIDGE_TOKEN", token);
            }
            (BrowserRuntimeBackend::Embedded, None) => {
                return Err(BrowserRuntimeError::NativeFailed);
            }
            (BrowserRuntimeBackend::External, _) => {}
        }
        if let Some(path) = browser_path {
            command.arg("--browser-path").arg(path);
        }
        let mut child = command
            .spawn()
            .map_err(|_| BrowserRuntimeError::StartFailed)?;
        let sidecar_pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or(BrowserRuntimeError::StartFailed)?;
        let stderr = child.stderr.take();
        let mut lines = BufReader::new(stdout).lines();
        let ready = tokio::time::timeout(READY_TIMEOUT, read_ready(&mut lines))
            .await
            .map_err(|_| BrowserRuntimeError::ReadyTimeout)??;
        let endpoint = format!("http://127.0.0.1:{}", ready.port);
        {
            let mut inner = self.inner.lock().await;
            inner.child = Some(child);
            inner.endpoint = Some(endpoint);
            inner.token = Some(token);
            inner.sidecar_backend = Some(backend);
            inner.status.sidecar_pid = sidecar_pid;
            inner.status.runtime_version = Some(ready.version);
            inner.status.installed = true;
            inner.sidecar_installed = true;
            push_log(&mut inner.status, "sidecar_ready");
        }
        self.spawn_stdout_drain(lines);
        if let Some(stderr) = stderr {
            self.spawn_stderr_drain(stderr);
        }
        if let Some(pid) = sidecar_pid {
            self.spawn_process_monitor(pid);
            self.spawn_health_monitor(pid);
        }
        Ok(())
    }

    async fn terminate_sidecar(&self) {
        let child = {
            let mut inner = self.inner.lock().await;
            inner.endpoint = None;
            inner.token = None;
            inner.sidecar_backend = None;
            inner.status.sidecar_pid = None;
            inner.status.runtime_version = None;
            inner.child.take()
        };
        if let Some(mut child) = child {
            if let Some(pid) = child.id() {
                let _ = kill_tree::tokio::kill_tree(pid).await;
            } else {
                let _ = child.start_kill();
            }
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        }
    }

    fn spawn_stdout_drain(&self, mut lines: Lines<BufReader<ChildStdout>>) {
        let manager = self.clone();
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if event.get("event").and_then(serde_json::Value::as_str) == Some("browser-status")
                {
                    if let Some(status) = event.get("status") {
                        manager.apply_runtime_status(status).await;
                    }
                    continue;
                }
                if event.get("event").and_then(serde_json::Value::as_str)
                    == Some("browser-session-activity")
                {
                    let Some(connection_id) = event
                        .get("sessionId")
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| validate_connection_id(value).is_ok())
                    else {
                        continue;
                    };
                    manager.surface_registry.lock().await.ensure(connection_id);
                    emit_event(
                        &manager.emitter,
                        SURFACE_EVENT,
                        BrowserSessionActivity {
                            connection_id: connection_id.to_string(),
                        },
                    );
                }
            }
        });
    }

    fn spawn_stderr_drain(&self, stderr: tokio::process::ChildStderr) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let code = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("code")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "sidecar_stderr".to_string());
                let mut inner = manager.inner.lock().await;
                push_log(&mut inner.status, &code);
            }
        });
    }

    fn spawn_process_monitor(&self, expected_pid: u32) {
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let exited = {
                    let mut inner = manager.inner.lock().await;
                    let Some(child) = inner.child.as_mut() else {
                        break;
                    };
                    if child.id() != Some(expected_pid) {
                        break;
                    }
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            inner.child = None;
                            inner.endpoint = None;
                            inner.token = None;
                            inner.sidecar_backend = None;
                            inner.status.sidecar_pid = None;
                            inner.status.browser_pid = None;
                            inner.status.runtime_version = None;
                            inner.status.state = BrowserRuntimeState::Error;
                            inner.status.last_error_code = Some("SIDECAR_EXITED".to_string());
                            push_log(&mut inner.status, "sidecar_exited");
                            true
                        }
                        _ => false,
                    }
                };
                if exited {
                    manager.detach_all_surfaces(false).await;
                    manager.emit_status().await;
                    break;
                }
            }
        });
    }

    fn spawn_health_monitor(&self, expected_pid: u32) {
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
                let should_poll = {
                    let inner = manager.inner.lock().await;
                    let same_process = inner.child.as_ref().and_then(tokio::process::Child::id)
                        == Some(expected_pid);
                    if !same_process {
                        break;
                    }
                    matches!(
                        inner.status.state,
                        BrowserRuntimeState::Starting
                            | BrowserRuntimeState::Ready
                            | BrowserRuntimeState::Recovering
                    )
                };
                if !should_poll {
                    continue;
                }
                let healthy = matches!(
                    tokio::time::timeout(HEALTH_POLL_TIMEOUT, manager.refresh_health()).await,
                    Ok(Ok(()))
                );
                if healthy {
                    continue;
                }
                let same_process = manager
                    .inner
                    .lock()
                    .await
                    .child
                    .as_ref()
                    .and_then(tokio::process::Child::id)
                    == Some(expected_pid);
                if same_process {
                    manager.set_error("CONTROL_REQUEST_FAILED").await;
                }
            }
        });
    }

    async fn control(&self, action: &str) -> Result<serde_json::Value, BrowserRuntimeError> {
        self.request(
            reqwest::Method::POST,
            &format!("/admin/{action}"),
            Some(serde_json::json!({})),
        )
        .await
    }

    async fn refresh_health(&self) -> Result<(), BrowserRuntimeError> {
        let response = self.request(reqwest::Method::GET, "/health", None).await?;
        let runtime = response
            .get("runtime")
            .ok_or(BrowserRuntimeError::ControlFailed)?;
        self.apply_runtime_status(runtime).await;
        Ok(())
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, BrowserRuntimeError> {
        let (endpoint, token) = {
            let inner = self.inner.lock().await;
            (
                inner
                    .endpoint
                    .clone()
                    .ok_or(BrowserRuntimeError::ControlFailed)?,
                inner
                    .token
                    .clone()
                    .ok_or(BrowserRuntimeError::ControlFailed)?,
            )
        };
        let mut request = self
            .client
            .request(method, format!("{endpoint}{path}"))
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| BrowserRuntimeError::ControlFailed)?;
        let status = response.status();
        let value = response
            .json::<serde_json::Value>()
            .await
            .map_err(|_| BrowserRuntimeError::ControlFailed)?;
        if !status.is_success() {
            return Err(BrowserRuntimeError::ControlFailed);
        }
        Ok(value)
    }

    async fn apply_runtime_status(&self, value: &serde_json::Value) {
        let runtime_state = value.get("state").and_then(serde_json::Value::as_str);
        let next_state = match runtime_state {
            Some("ready") => BrowserRuntimeState::Ready,
            Some("starting") => BrowserRuntimeState::Starting,
            Some("recovering") => BrowserRuntimeState::Recovering,
            Some("error") => BrowserRuntimeState::Error,
            _ => BrowserRuntimeState::Stopped,
        };
        let next_browser_pid = value
            .get("browserPid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok());
        let next_browser_name = value
            .get("browserName")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let next_browser_version = value
            .get("browserVersion")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let next_recovery_attempt = value
            .get("recoveryAttempt")
            .and_then(serde_json::Value::as_u64)
            .and_then(|attempt| u8::try_from(attempt).ok())
            .unwrap_or(0);
        let next_error_code = value
            .get("lastErrorCode")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let mut inner = self.inner.lock().await;
        let changed = inner.status.state != next_state
            || inner.status.browser_pid != next_browser_pid
            || inner.status.browser_name != next_browser_name
            || inner.status.browser_version != next_browser_version
            || inner.status.recovery_attempt != next_recovery_attempt
            || inner.status.last_error_code != next_error_code;
        inner.status.state = next_state;
        inner.status.browser_pid = next_browser_pid;
        inner.status.browser_name = next_browser_name;
        inner.status.browser_version = next_browser_version;
        inner.status.recovery_attempt = next_recovery_attempt;
        inner.status.last_error_code = next_error_code;
        drop(inner);
        if changed {
            self.emit_status().await;
        }
    }

    async fn apply_settings_snapshot(&self, settings: BrowserRuntimeSettings) {
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        let native_installed = self
            .native_runtime
            .lock()
            .ok()
            .is_some_and(|slot| slot.is_some());
        #[cfg(not(all(feature = "tauri-runtime", target_os = "windows")))]
        let native_installed = false;
        let mut inner = self.inner.lock().await;
        inner.settings = settings.clone();
        inner.status.enabled = settings.enabled;
        inner.status.auto_start = settings.auto_start;
        inner.status.browser_path = settings.browser_path;
        inner.status.backend = settings.backend.id().to_string();
        inner.status.installed = match settings.backend {
            BrowserRuntimeBackend::Embedded => native_installed && inner.sidecar_installed,
            BrowserRuntimeBackend::External => inner.sidecar_installed,
        };
        if matches!(
            inner.status.state,
            BrowserRuntimeState::Stopped | BrowserRuntimeState::NotInstalled
        ) {
            inner.status.state = if inner.status.installed {
                BrowserRuntimeState::Stopped
            } else {
                BrowserRuntimeState::NotInstalled
            };
        }
        drop(inner);
        self.emit_status().await;
    }

    async fn set_error(&self, code: &str) {
        let mut inner = self.inner.lock().await;
        inner.status.state = BrowserRuntimeState::Error;
        inner.status.last_error_code = Some(code.to_string());
        push_log(&mut inner.status, code);
        drop(inner);
        self.emit_status().await;
    }

    async fn emit_status(&self) {
        emit_event(&self.emitter, STATUS_EVENT, self.status().await);
    }
}

#[async_trait]
impl BrowserMcpProvider for BrowserRuntimeManager {
    async fn enabled_for_new_session(&self) -> bool {
        self.inner.lock().await.settings.enabled
    }

    async fn server_for_session(&self, session_id: &str) -> Option<McpServer> {
        validate_connection_id(session_id).ok()?;
        let inner = self.inner.lock().await;
        if inner.status.state != BrowserRuntimeState::Ready
            || inner.sidecar_backend != Some(inner.settings.backend)
        {
            return None;
        }
        let endpoint = inner.endpoint.as_ref()?;
        let token = inner.token.as_ref()?;
        let server = McpServerHttp::new("codeg-browser", format!("{endpoint}/mcp")).headers(vec![
            HttpHeader::new("Authorization", format!("Bearer {token}")),
            HttpHeader::new("X-Codeg-Browser-Session", session_id),
        ]);
        drop(inner);
        self.surface_registry.lock().await.ensure(session_id);
        Some(McpServer::Http(server))
    }

    async fn release_session(&self, session_id: &str) {
        BrowserRuntimeManager::release_session(self, session_id).await;
    }
}

#[derive(Debug, Deserialize)]
struct ReadyEvent {
    event: String,
    port: u16,
    version: String,
}

async fn read_ready<R: AsyncRead + Unpin>(
    lines: &mut Lines<BufReader<R>>,
) -> Result<ReadyEvent, BrowserRuntimeError> {
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|_| BrowserRuntimeError::StartFailed)?
    {
        let Ok(event) = serde_json::from_str::<ReadyEvent>(&line) else {
            continue;
        };
        if event.event == "ready" && event.port > 0 {
            return Ok(event);
        }
    }
    Err(BrowserRuntimeError::StartFailed)
}

fn push_log(status: &mut BrowserRuntimeStatus, code: &str) {
    let mut entries: VecDeque<BrowserLogEntry> = status.recent_logs.drain(..).collect();
    entries.push_back(BrowserLogEntry {
        at: chrono::Utc::now().to_rfc3339(),
        code: sanitize_log_code(code),
    });
    while entries.len() > LOG_LIMIT {
        entries.pop_front();
    }
    status.recent_logs = entries.into_iter().collect();
}

fn normalize_settings(mut settings: BrowserRuntimeSettings) -> BrowserRuntimeSettings {
    // The main Settings UI intentionally exposes one Browser switch. Keep the
    // legacy field as a serialized compatibility mirror so an older saved
    // autoStart=false value cannot silently block an enabled Browser on launch.
    settings.auto_start = settings.enabled;
    settings.browser_path = settings
        .browser_path
        .take()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    settings
}

async fn persist_settings(
    db: &AppDatabase,
    settings: &BrowserRuntimeSettings,
) -> Result<(), BrowserRuntimeError> {
    let serialized =
        serde_json::to_string(settings).map_err(|_| BrowserRuntimeError::SettingsFailed)?;
    app_metadata_service::upsert_value(&db.conn, SETTINGS_KEY, &serialized)
        .await
        .map_err(|_| BrowserRuntimeError::SettingsFailed)
}

fn sanitize_log_code(code: &str) -> String {
    const ALLOWED: &[&str] = &[
        "sidecar_ready",
        "sidecar_exited",
        "sidecar_stderr",
        "native_bridge_ready",
        "native_recovered",
        "missing_or_short_control_token",
        "profile_and_download_directories_are_required",
        "invalid_argument",
        "invalid_parent_pid",
        "invalid_backend",
        "unsupported_platform",
        "unknown_fatal_error",
        "UNSUPPORTED_PLATFORM",
        "RUNTIME_DISABLED",
        "SIDECAR_NOT_INSTALLED",
        "SIDECAR_START_FAILED",
        "SIDECAR_READY_TIMEOUT",
        "CONTROL_REQUEST_FAILED",
        "NATIVE_RUNTIME_FAILED",
        "SETTINGS_FAILED",
    ];
    if ALLOWED.contains(&code) {
        code.to_string()
    } else {
        "sidecar_error".to_string()
    }
}

fn validate_connection_id(connection_id: &str) -> Result<(), BrowserRuntimeError> {
    if connection_id.is_empty()
        || connection_id.len() > 256
        || !connection_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        return Err(BrowserRuntimeError::ControlFailed);
    }
    Ok(())
}

fn parse_surface_snapshot(
    value: serde_json::Value,
    connection_id: &str,
) -> Result<BrowserSurfaceSnapshot, BrowserRuntimeError> {
    let snapshot = serde_json::from_value::<BrowserSurfaceSnapshot>(value)
        .map_err(|_| BrowserRuntimeError::ControlFailed)?;
    validate_surface_snapshot(&snapshot, connection_id)?;
    Ok(snapshot)
}

fn validate_surface_snapshot(
    snapshot: &BrowserSurfaceSnapshot,
    connection_id: &str,
) -> Result<(), BrowserRuntimeError> {
    if snapshot.session_id != connection_id {
        return Err(BrowserRuntimeError::ControlFailed);
    }
    let target_ids = snapshot
        .tabs
        .iter()
        .map(|tab| tab.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if target_ids.len() != snapshot.tabs.len()
        || snapshot.tabs.iter().any(|tab| tab.id.is_empty())
        || snapshot
            .active_target_id
            .as_deref()
            .is_some_and(|target_id| !target_ids.contains(target_id))
        || snapshot.active.as_ref().is_some_and(|active| {
            snapshot.active_target_id.as_deref() != Some(active.tab.id.as_str())
        })
        || snapshot.active.is_none() != snapshot.active_target_id.is_none()
    {
        return Err(BrowserRuntimeError::ControlFailed);
    }
    Ok(())
}

#[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
fn map_native_error(error: NativeBrowserError) -> BrowserRuntimeError {
    if error == NativeBrowserError::UnsupportedPlatform {
        BrowserRuntimeError::UnsupportedPlatform
    } else {
        BrowserRuntimeError::NativeFailed
    }
}

#[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
fn native_snapshot_to_surface(snapshot: NativeBrowserSnapshot) -> BrowserSurfaceSnapshot {
    let tabs = snapshot
        .tabs
        .iter()
        .map(|tab| BrowserSurfaceTab {
            id: tab.id.clone(),
            url: tab.url.clone(),
            title: tab.title.clone(),
        })
        .collect::<Vec<_>>();
    let active = snapshot.active_tab_id.as_ref().and_then(|active_tab_id| {
        snapshot.tabs.iter().find_map(|tab| {
            (tab.id == *active_tab_id).then(|| BrowserSurfacePageState {
                tab: BrowserSurfaceTab {
                    id: tab.id.clone(),
                    url: tab.url.clone(),
                    title: tab.title.clone(),
                },
                loading: tab.loading,
                can_go_back: tab.can_go_back,
                can_go_forward: tab.can_go_forward,
            })
        })
    });
    BrowserSurfaceSnapshot {
        session_id: snapshot.connection_id,
        surface_kind: BrowserSurfaceKind::Native,
        tabs,
        active_target_id: snapshot.active_tab_id,
        active,
    }
}

fn error_code(error: &BrowserRuntimeError) -> &'static str {
    match error {
        BrowserRuntimeError::UnsupportedPlatform => "UNSUPPORTED_PLATFORM",
        BrowserRuntimeError::Disabled => "RUNTIME_DISABLED",
        BrowserRuntimeError::NotInstalled => "SIDECAR_NOT_INSTALLED",
        BrowserRuntimeError::StartFailed => "SIDECAR_START_FAILED",
        BrowserRuntimeError::ReadyTimeout => "SIDECAR_READY_TIMEOUT",
        BrowserRuntimeError::ControlFailed => "CONTROL_REQUEST_FAILED",
        BrowserRuntimeError::NativeFailed => "NATIVE_RUNTIME_FAILED",
        BrowserRuntimeError::SettingsFailed => "SETTINGS_FAILED",
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.len() > 0
}

fn locate_sidecar() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    if let Some(path) = std::env::var_os("CODEG_BROWSER_SIDECAR_BIN").map(PathBuf::from) {
        if is_executable_file(&path) {
            return Some(path);
        }
    }
    if let Some(directory) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        let candidate = directory.join("codeg-browser-sidecar.exe");
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    let triple = match std::env::consts::ARCH {
        "aarch64" => "aarch64-pc-windows-msvc",
        _ => "x86_64-pc-windows-msvc",
    };
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!("codeg-browser-sidecar-{triple}.exe"));
    is_executable_file(&development).then_some(development)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_public_backend_id_names_the_native_runtime() {
        assert_eq!(BrowserRuntimeBackend::Embedded.id(), "embedded_webview2");
        assert_eq!(BrowserRuntimeBackend::External.id(), EXTERNAL_BACKEND_ID);
    }

    #[test]
    fn surface_kind_preserves_legacy_frame_snapshots() {
        let snapshot = parse_surface_snapshot(
            serde_json::json!({
                "sessionId": "connection-a",
                "tabs": [{ "id": "t1", "url": "about:blank", "title": "New Tab" }],
                "activeTargetId": "t1",
                "active": {
                    "tab": { "id": "t1", "url": "about:blank", "title": "New Tab" },
                    "loading": false,
                    "canGoBack": false,
                    "canGoForward": false
                }
            }),
            "connection-a",
        )
        .expect("legacy sidecar snapshot");

        assert_eq!(snapshot.surface_kind, BrowserSurfaceKind::Frame);
        assert_eq!(
            serde_json::to_value(BrowserSurfaceKind::Native).unwrap(),
            serde_json::json!("native")
        );
    }

    #[test]
    fn log_codes_are_bounded_and_cannot_carry_urls_or_tokens() {
        assert_eq!(
            sanitize_log_code("https://example.com/?token=secret"),
            "sidecar_error"
        );
        assert_eq!(sanitize_log_code(&"x".repeat(100)), "sidecar_error");
        assert_eq!(sanitize_log_code("sidecar_ready"), "sidecar_ready");
    }

    #[test]
    fn browser_switch_controls_startup_connection() {
        let disabled = normalize_settings(BrowserRuntimeSettings {
            enabled: false,
            auto_start: true,
            browser_path: Some("  C:\\Chrome\\chrome.exe  ".to_string()),
            backend: BrowserRuntimeBackend::Embedded,
        });

        assert!(!disabled.auto_start);
        assert_eq!(
            disabled.browser_path.as_deref(),
            Some("C:\\Chrome\\chrome.exe")
        );

        let enabled = normalize_settings(BrowserRuntimeSettings {
            enabled: true,
            auto_start: false,
            browser_path: None,
            backend: BrowserRuntimeBackend::Embedded,
        });
        assert!(enabled.auto_start);
    }

    #[test]
    fn legacy_settings_without_backend_keep_the_external_browser() {
        let settings: BrowserRuntimeSettings =
            serde_json::from_str(r#"{"enabled":true,"autoStart":true,"browserPath":null}"#)
                .unwrap();

        assert_eq!(settings.backend, BrowserRuntimeBackend::External);
    }

    #[test]
    fn saved_embedded_backend_migrates_to_the_native_webview_runtime() {
        let settings: BrowserRuntimeSettings = serde_json::from_str(
            r#"{"enabled":true,"autoStart":true,"browserPath":null,"backend":"embedded"}"#,
        )
        .unwrap();

        assert_eq!(settings.backend, BrowserRuntimeBackend::Embedded);
        assert_eq!(settings.backend.id(), EMBEDDED_BACKEND_ID);
    }

    #[test]
    fn public_status_serialization_has_no_endpoint_or_token_fields() {
        let status = BrowserRuntimeStatus {
            state: BrowserRuntimeState::Ready,
            installed: true,
            enabled: true,
            auto_start: false,
            browser_path: None,
            sidecar_pid: Some(1),
            browser_pid: Some(2),
            runtime_version: Some("0.1.0".to_string()),
            backend: EXTERNAL_BACKEND_ID.to_string(),
            browser_name: Some("Chrome".to_string()),
            browser_version: Some("1".to_string()),
            profile_path: "C:\\Codeg\\browser\\profile".to_string(),
            download_path: "C:\\Codeg\\browser\\downloads".to_string(),
            recovery_attempt: 0,
            last_error_code: None,
            recent_logs: Vec::new(),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains("endpoint"));
        assert!(!json.contains("token"));
    }

    #[tokio::test]
    async fn readiness_captures_the_packaged_runtime_version() {
        let input = br#"{"event":"ready","version":"0.1.0","pid":42,"port":43123}
"#;
        let mut lines = BufReader::new(&input[..]).lines();
        let ready = read_ready(&mut lines).await.unwrap();

        assert_eq!(ready.version, "0.1.0");
        assert_eq!(ready.port, 43123);
    }

    #[tokio::test]
    async fn health_projection_surfaces_a_browser_crash() {
        let manager = BrowserRuntimeManager::new(PathBuf::from("unused"), EventEmitter::Noop);
        manager
            .apply_runtime_status(&serde_json::json!({
                "state": "error",
                "browserPid": null,
                "browserName": "Google Chrome",
                "browserVersion": "140.0",
                "recoveryAttempt": 0,
                "lastErrorCode": "BROWSER_CRASHED"
            }))
            .await;

        let status = manager.status().await;
        assert_eq!(status.state, BrowserRuntimeState::Error);
        assert_eq!(status.last_error_code.as_deref(), Some("BROWSER_CRASHED"));
    }

    #[tokio::test]
    async fn runtime_lifecycle_is_independent_from_the_future_session_default() {
        let manager = BrowserRuntimeManager::new(PathBuf::from("unused"), EventEmitter::Noop);
        {
            let mut inner = manager.inner.lock().await;
            inner.endpoint = Some("http://127.0.0.1:1".to_string());
            inner.token = Some("test-token".to_string());
        }

        assert!(matches!(
            manager.start().await,
            Err(BrowserRuntimeError::ControlFailed)
        ));
        assert!(matches!(
            manager.recover().await,
            Err(BrowserRuntimeError::ControlFailed)
        ));
    }

    #[tokio::test]
    async fn provider_is_fail_closed_and_scopes_headers_to_the_session() {
        let manager = BrowserRuntimeManager::new(PathBuf::from("unused"), EventEmitter::Noop);
        assert!(!manager.enabled_for_new_session().await);
        assert!(manager.server_for_session("session-a").await.is_none());

        {
            let mut inner = manager.inner.lock().await;
            inner.status.state = BrowserRuntimeState::Ready;
            inner.endpoint = Some("http://127.0.0.1:12345".to_string());
            inner.token = Some("private-token".to_string());
            inner.sidecar_backend = Some(BrowserRuntimeBackend::External);
        }
        // A provider already frozen into a running Agent session stays usable
        // after the future-session default is switched off.
        let server = manager
            .server_for_session("session-a")
            .await
            .expect("ready provider");
        assert!(manager.surface_registry.lock().await.contains("session-a"));
        let value = serde_json::to_value(server).unwrap();
        assert_eq!(value["name"], "codeg-browser");
        assert_eq!(value["url"], "http://127.0.0.1:12345/mcp");
        assert!(value["headers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|header| header["name"] == "X-Codeg-Browser-Session"
                && header["value"] == "session-a"));
    }

    #[tokio::test]
    async fn embedded_backend_rejects_an_external_sidecar_endpoint() {
        let manager = BrowserRuntimeManager::new(PathBuf::from("unused"), EventEmitter::Noop);
        manager
            .apply_settings_snapshot(BrowserRuntimeSettings {
                enabled: true,
                auto_start: true,
                browser_path: None,
                backend: BrowserRuntimeBackend::Embedded,
            })
            .await;
        {
            let mut inner = manager.inner.lock().await;
            inner.status.state = BrowserRuntimeState::Ready;
            inner.endpoint = Some("http://127.0.0.1:12345".to_string());
            inner.token = Some("stale-managed-chromium-token".to_string());
        }

        assert!(manager.server_for_session("session-a").await.is_none());
    }

    #[tokio::test]
    async fn embedded_backend_advertises_its_native_sidecar_endpoint() {
        let manager = BrowserRuntimeManager::new(PathBuf::from("unused"), EventEmitter::Noop);
        manager
            .apply_settings_snapshot(BrowserRuntimeSettings {
                enabled: true,
                auto_start: true,
                browser_path: None,
                backend: BrowserRuntimeBackend::Embedded,
            })
            .await;
        {
            let mut inner = manager.inner.lock().await;
            inner.status.state = BrowserRuntimeState::Ready;
            inner.endpoint = Some("http://127.0.0.1:12345".to_string());
            inner.token = Some("private-native-sidecar-token".to_string());
            inner.sidecar_backend = Some(BrowserRuntimeBackend::Embedded);
        }

        let server = manager
            .server_for_session("session-a")
            .await
            .expect("ready native provider");
        let value = serde_json::to_value(server).unwrap();
        assert_eq!(value["url"], "http://127.0.0.1:12345/mcp");
    }

    #[tokio::test]
    async fn embedded_start_without_an_installed_native_runtime_fails_before_sidecar_spawn() {
        let manager = BrowserRuntimeManager::new(PathBuf::from("unused"), EventEmitter::Noop);
        manager
            .apply_settings_snapshot(BrowserRuntimeSettings {
                enabled: true,
                auto_start: true,
                browser_path: None,
                backend: BrowserRuntimeBackend::Embedded,
            })
            .await;

        let result = manager.start().await;
        #[cfg(all(feature = "tauri-runtime", target_os = "windows"))]
        assert!(matches!(result, Err(BrowserRuntimeError::NativeFailed)));
        #[cfg(not(all(feature = "tauri-runtime", target_os = "windows")))]
        assert!(matches!(
            result,
            Err(BrowserRuntimeError::UnsupportedPlatform)
        ));

        let inner = manager.inner.lock().await;
        assert!(inner.child.is_none());
        assert!(inner.endpoint.is_none());
        assert!(inner.token.is_none());
        assert_eq!(inner.status.state, BrowserRuntimeState::Error);
    }

    #[test]
    fn surface_registry_is_unique_per_connection_and_isolates_sessions() {
        let mut registry = BrowserSurfaceRegistry::default();
        assert_eq!(registry.ensure("connection-a"), 0);
        assert_eq!(registry.ensure("connection-a"), 0);
        assert_eq!(registry.next_generation("connection-a"), 1);
        assert_eq!(registry.ensure("connection-a"), 1);
        assert_eq!(registry.ensure("connection-b"), 0);
        assert!(registry.contains("connection-a"));
        assert!(registry.contains("connection-b"));
    }

    #[test]
    fn surface_snapshot_rejects_hidden_or_cross_session_targets() {
        let hidden = BrowserSurfaceSnapshot {
            session_id: "connection-a".to_string(),
            surface_kind: BrowserSurfaceKind::Frame,
            tabs: vec![BrowserSurfaceTab {
                id: "t1".to_string(),
                url: "about:blank".to_string(),
                title: "New Tab".to_string(),
            }],
            active_target_id: Some("t2".to_string()),
            active: None,
        };
        assert!(validate_surface_snapshot(&hidden, "connection-a").is_err());
        assert!(validate_surface_snapshot(&hidden, "connection-b").is_err());
    }
}
