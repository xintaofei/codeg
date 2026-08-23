use super::security::{
    completed_download_path_for_root, intercepted_request_id, managed_download_path_for_root,
    sanitize_redirect_headers, valid_request_id, validate_navigation_url,
    validate_network_request_url, NativeRequestKey, RequestUrlTracker,
};
use super::*;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{mpsc::sync_channel, Arc, Mutex};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl};
use tokio::sync::{broadcast, oneshot, Mutex as AsyncMutex};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_14, ICoreWebView2_2, ICoreWebView2_5, COREWEBVIEW2_PERMISSION_STATE_DENY,
    COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT, COREWEBVIEW2_SCRIPT_DIALOG_KIND_BEFOREUNLOAD,
    COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL,
};
use webview2_com::{
    take_pwstr, CallDevToolsProtocolMethodCompletedHandler, ClientCertificateRequestedEventHandler,
    CoTaskMemPWSTR, DevToolsProtocolEventReceivedEventHandler, HistoryChangedEventHandler,
    PermissionRequestedEventHandler, ProcessFailedEventHandler, ScriptDialogOpeningEventHandler,
    ServerCertificateErrorDetectedEventHandler, SourceChangedEventHandler,
};
use windows::core::{Interface, BOOL, PWSTR};

use crate::web::event_bridge::{emit_event, EventEmitter};

const UPDATE_EVENT: &str = "browser://session-activity";
const EVENT_LIMIT: usize = 100;
const MAX_CDP_EVENT_BYTES: usize = 256 * 1024;
const MAX_CDP_EVENTS_PER_SUBSCRIPTION: usize = 256;
const MAX_CDP_DRAIN: usize = 100;
const MAX_DOWNLOAD_RECORDS_PER_SESSION: usize = 100;
const CONTROLLER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct NativeBridgeCredentials {
    pub endpoint: String,
    pub token: String,
}

struct NativeBridgeHandle {
    credentials: NativeBridgeCredentials,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeRuntimeEvent {
    kind: &'static str,
    connection_id: String,
    tab_id: Option<String>,
    generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CdpEventKey {
    connection_id: String,
    tab_id: String,
    generation: u64,
    event: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PausedRequestEvent {
    request_id: String,
    #[serde(default)]
    redirected_request_id: Option<String>,
    request: PausedNetworkRequest,
}

#[derive(Debug, Deserialize)]
struct PausedNetworkRequest {
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct NativeDownloadRecord {
    snapshot: NativeBrowserDownloadSnapshot,
    source_url: String,
    pending_path: PathBuf,
}

impl CdpEventKey {
    fn new(connection_id: &str, tab_id: &str, generation: u64, event: &str) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            tab_id: tab_id.to_string(),
            generation,
            event: event.to_string(),
        }
    }
}

#[derive(Default)]
struct NativeRuntimeState {
    registry: NativeBrowserRegistry,
    controllers: HashMap<String, Webview>,
    environment_identity: Option<usize>,
    controller_identities: HashMap<String, usize>,
    recent_events: VecDeque<NativeRuntimeEvent>,
    cdp_subscriptions: HashSet<CdpEventKey>,
    cdp_events: HashMap<CdpEventKey, VecDeque<Value>>,
    request_urls: RequestUrlTracker,
    downloads: HashMap<String, VecDeque<NativeDownloadRecord>>,
}

impl NativeRuntimeState {
    fn remove_cdp_for_tab(&mut self, connection_id: &str, tab_id: &str) {
        self.cdp_subscriptions
            .retain(|key| key.connection_id != connection_id || key.tab_id != tab_id);
        self.cdp_events
            .retain(|key, _| key.connection_id != connection_id || key.tab_id != tab_id);
        self.request_urls.remove_tab(connection_id, tab_id);
    }

    fn remove_cdp_for_session(&mut self, connection_id: &str) {
        self.cdp_subscriptions
            .retain(|key| key.connection_id != connection_id);
        self.cdp_events
            .retain(|key, _| key.connection_id != connection_id);
        self.request_urls.remove_session(connection_id);
        self.downloads.remove(connection_id);
    }
}

#[derive(Clone)]
pub struct NativeBrowserRuntime {
    app: AppHandle,
    profile_dir: PathBuf,
    download_dir: PathBuf,
    emitter: EventEmitter,
    state: Arc<Mutex<NativeRuntimeState>>,
    lifecycle_gate: Arc<AsyncMutex<()>>,
    bridge: Arc<AsyncMutex<Option<NativeBridgeHandle>>>,
    updates: broadcast::Sender<String>,
}

impl NativeBrowserRuntime {
    pub fn new(app: AppHandle, data_dir: &Path, emitter: EventEmitter) -> Self {
        let (updates, _) = broadcast::channel(128);
        Self {
            app,
            profile_dir: data_dir.join("browser").join("profile"),
            download_dir: data_dir.join("browser").join("downloads"),
            emitter,
            state: Arc::new(Mutex::new(NativeRuntimeState::default())),
            lifecycle_gate: Arc::new(AsyncMutex::new(())),
            bridge: Arc::new(AsyncMutex::new(None)),
            updates,
        }
    }

    pub async fn start(&self) -> Result<NativeBridgeCredentials, NativeBrowserError> {
        fs::create_dir_all(&self.profile_dir).map_err(|_| NativeBrowserError::BridgeFailed)?;
        fs::create_dir_all(&self.download_dir).map_err(|_| NativeBrowserError::BridgeFailed)?;
        if let Some(credentials) = self.bridge_credentials().await {
            return Ok(credentials);
        }

        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .map_err(|_| NativeBrowserError::BridgeFailed)?;
        let address = listener
            .local_addr()
            .map_err(|_| NativeBrowserError::BridgeFailed)?;
        if !address.ip().is_loopback() {
            return Err(NativeBrowserError::BridgeFailed);
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let credentials = NativeBridgeCredentials {
            endpoint: format!("http://127.0.0.1:{}", address.port()),
            token: token.clone(),
        };
        let bridge_state = NativeBridgeState {
            runtime: self.clone(),
            token: Arc::<str>::from(token),
        };
        let router = Router::new()
            .route("/v1/health", get(native_bridge_health))
            .route("/v1/command", post(native_bridge_command))
            .layer(DefaultBodyLimit::max(NATIVE_BRIDGE_MAX_BODY_BYTES))
            .with_state(bridge_state);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        *self.bridge.lock().await = Some(NativeBridgeHandle {
            credentials: credentials.clone(),
            shutdown: Some(shutdown_tx),
            task,
        });
        Ok(credentials)
    }

    pub async fn stop(&self) {
        let _gate = self.lifecycle_gate.lock().await;
        self.close_all_controllers();
        let handle = self.bridge.lock().await.take();
        if let Some(mut handle) = handle {
            if let Some(shutdown) = handle.shutdown.take() {
                let _ = shutdown.send(());
            }
            let mut task = handle.task;
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }

    pub async fn bridge_credentials(&self) -> Option<NativeBridgeCredentials> {
        self.bridge
            .lock()
            .await
            .as_ref()
            .map(|handle| handle.credentials.clone())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.updates.subscribe()
    }

    pub fn diagnostics(&self) -> Value {
        let bridge_running = self
            .bridge
            .try_lock()
            .ok()
            .is_some_and(|bridge| bridge.is_some());
        let Ok(state) = self.state.lock() else {
            return json!({
                "backend": EMBEDDED_WEBVIEW2_BACKEND_ID,
                "state": "error",
                "errorCode": "NATIVE_REGISTRY_POISONED"
            });
        };
        let tab_count = state
            .registry
            .sessions
            .values()
            .map(|session| session.tabs.len())
            .sum::<usize>();
        json!({
            "backend": EMBEDDED_WEBVIEW2_BACKEND_ID,
            "bridgeProtocolVersion": NATIVE_BRIDGE_PROTOCOL_VERSION,
            "bridgeListening": bridge_running,
            "profilePath": self.profile_dir,
            "downloadPath": self.download_dir,
            "sessionCount": state.registry.sessions.len(),
            "tabCount": tab_count,
            "controllerCount": state.controllers.len(),
            "sharedEnvironmentCount": usize::from(state.environment_identity.is_some()),
            "cdpSubscriptionCount": state.cdp_subscriptions.len(),
            "queuedCdpEventCount": state.cdp_events.values().map(VecDeque::len).sum::<usize>(),
            "trackedRequestCount": state.request_urls.len(),
            "registryConsistent": registry_is_consistent(&state),
            "recentEvents": state.recent_events,
        })
    }

    pub fn doctor(&self) -> Value {
        let diagnostics = self.diagnostics();
        json!({
            "ok": diagnostics["bridgeListening"] == Value::Bool(true)
                && diagnostics["registryConsistent"] == Value::Bool(true),
            "backend": EMBEDDED_WEBVIEW2_BACKEND_ID,
            "bridgeProtocolVersion": NATIVE_BRIDGE_PROTOCOL_VERSION,
            "checks": {
                "loopbackOnly": true,
                "bodyLimitBytes": NATIVE_BRIDGE_MAX_BODY_BYTES,
                "requestTimeoutMs": NATIVE_BRIDGE_REQUEST_TIMEOUT.as_millis(),
                "methodAllowlistSize": allowed_cdp_methods().len(),
                "eventAllowlistSize": allowed_cdp_events().len(),
                "requestInterception": true,
                "permissionDefaultDeny": true,
                "certificateDefaultDeny": true,
                "fileChooserDefaultDeny": true,
                "downloadRootControlled": true,
                "registryConsistent": diagnostics["registryConsistent"],
            }
        })
    }

    pub async fn ensure_session(
        &self,
        connection_id: &str,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        validate_connection_id(connection_id)?;
        let _gate = self.lifecycle_gate.lock().await;
        if self
            .state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?
            .registry
            .has_tabs(connection_id)
        {
            return self.snapshot(connection_id);
        }
        self.create_tab_locked(connection_id, None).await
    }

    pub async fn create_tab(
        &self,
        connection_id: &str,
        url: Option<&str>,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        validate_connection_id(connection_id)?;
        let _gate = self.lifecycle_gate.lock().await;
        self.create_tab_locked(connection_id, url).await
    }

    async fn create_tab_locked(
        &self,
        connection_id: &str,
        url: Option<&str>,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        let requested_url = validate_navigation_url(url.unwrap_or(INITIAL_URL))?;
        let pending = {
            self.state
                .lock()
                .map_err(|_| NativeBrowserError::ControllerFailed)?
                .registry
                .plan_tab(connection_id, None, &requested_url)?
        };
        let controller = match self.build_controller(&pending) {
            Ok(controller) => controller,
            Err(error) => return Err(error),
        };
        if let Err(error) = self.commit_controller(&pending, controller) {
            if let Some(webview) = self.app.get_webview(&pending.webview_label) {
                let _ = webview.close();
            }
            return Err(error);
        }
        if requested_url != INITIAL_URL {
            self.navigate(
                connection_id,
                &pending.tab_id,
                pending.generation,
                &requested_url,
            )?;
        }
        self.reconcile_visibility(None)?;
        self.record_event(
            "tab_created",
            connection_id,
            Some(&pending.tab_id),
            Some(pending.generation),
        );
        self.notify(connection_id);
        self.snapshot(connection_id)
    }

    pub fn snapshot(
        &self,
        connection_id: &str,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        self.state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?
            .registry
            .snapshot(connection_id)
    }

    pub fn active_identity(
        &self,
        connection_id: &str,
    ) -> Result<(String, u64), NativeBrowserError> {
        self.state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?
            .registry
            .active_identity(connection_id)
    }

    pub fn focus_tab(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        let snapshot = self
            .state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?
            .registry
            .focus_tab(connection_id, tab_id, generation)?;
        self.reconcile_visibility(Some(tab_id))?;
        self.record_event("tab_focused", connection_id, Some(tab_id), Some(generation));
        self.notify(connection_id);
        Ok(snapshot)
    }

    pub async fn close_tab(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        let _gate = self.lifecycle_gate.lock().await;
        let (closed, webview, snapshot) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let closed = state
                .registry
                .close_tab(connection_id, tab_id, generation)?;
            state.remove_cdp_for_tab(connection_id, tab_id);
            let webview = state.controllers.remove(&closed.webview_label);
            state.controller_identities.remove(&closed.webview_label);
            let snapshot = state.registry.snapshot(connection_id)?;
            (closed, webview, snapshot)
        };
        if let Some(webview) = webview {
            webview
                .close()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
        }
        self.reconcile_visibility(closed.was_active.then_some(tab_id))?;
        self.record_event("tab_closed", connection_id, Some(tab_id), Some(generation));
        self.notify(connection_id);
        Ok(snapshot)
    }

    pub async fn release_session(&self, connection_id: &str) {
        let _gate = self.lifecycle_gate.lock().await;
        let controllers = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            let labels = state.registry.release_session(connection_id);
            state.remove_cdp_for_session(connection_id);
            labels
                .into_iter()
                .filter_map(|label| {
                    state.controller_identities.remove(&label);
                    state.controllers.remove(&label)
                })
                .collect::<Vec<_>>()
        };
        for webview in controllers {
            let _ = webview.close();
        }
        self.record_event("session_released", connection_id, None, None);
        self.notify(connection_id);
    }

    pub fn navigate(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        url: &str,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        let url = validate_navigation_url(url)?;
        let webview = self.controller_for(connection_id, tab_id, generation)?;
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            state.registry.update_loading(
                connection_id,
                tab_id,
                generation,
                Some(url.clone()),
                true,
            );
        }
        webview
            .navigate(
                url.parse()
                    .map_err(|_| NativeBrowserError::NavigationBlocked)?,
            )
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
        self.record_event(
            "navigation_requested",
            connection_id,
            Some(tab_id),
            Some(generation),
        );
        self.notify(connection_id);
        self.snapshot(connection_id)
    }

    pub fn go_history(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        direction: NativeHistoryDirection,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        let webview = self.controller_for(connection_id, tab_id, generation)?;
        with_core_webview(&webview, move |_, core| unsafe {
            match direction {
                NativeHistoryDirection::Back => core.GoBack(),
                NativeHistoryDirection::Forward => core.GoForward(),
            }
            .map_err(|_| NativeBrowserError::ControllerFailed)
        })?;
        self.record_event(
            "history_requested",
            connection_id,
            Some(tab_id),
            Some(generation),
        );
        self.snapshot(connection_id)
    }

    pub fn reload(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        let webview = self.controller_for(connection_id, tab_id, generation)?;
        with_core_webview(&webview, |_, core| unsafe {
            core.Reload()
                .map_err(|_| NativeBrowserError::ControllerFailed)
        })?;
        self.snapshot(connection_id)
    }

    pub fn stop_loading(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        let webview = self.controller_for(connection_id, tab_id, generation)?;
        with_core_webview(&webview, |_, core| unsafe {
            core.Stop()
                .map_err(|_| NativeBrowserError::ControllerFailed)
        })?;
        self.snapshot(connection_id)
    }

    pub fn set_surface(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        bounds: NativeSurfaceBounds,
        visible: bool,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        let snapshot = self
            .state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?
            .registry
            .set_surface(connection_id, tab_id, generation, bounds, visible)?;
        self.reconcile_visibility(visible.then_some(tab_id))?;
        self.record_event(
            if visible {
                "surface_shown"
            } else {
                "surface_hidden"
            },
            connection_id,
            Some(tab_id),
            Some(generation),
        );
        self.notify(connection_id);
        Ok(snapshot)
    }

    pub fn hide_session(&self, connection_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.registry.hide_session(connection_id);
        }
        let _ = self.reconcile_visibility(None);
        self.notify(connection_id);
    }

    pub fn hide_all(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.registry.hide_all();
        }
        let _ = self.reconcile_visibility(None);
    }

    pub fn reapply_surface_geometry(&self) {
        let _ = self.reconcile_visibility(None);
    }

    pub async fn recover_tab(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<NativeBrowserSnapshot, NativeBrowserError> {
        let _gate = self.lifecycle_gate.lock().await;
        let (pending, old) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let old_label = state
                .registry
                .validate_identity(connection_id, tab_id, generation)?
                .webview_label
                .clone();
            let pending = state
                .registry
                .begin_recover(connection_id, tab_id, generation)?;
            state.remove_cdp_for_tab(connection_id, tab_id);
            state.controller_identities.remove(&old_label);
            (pending, state.controllers.remove(&old_label))
        };
        if let Some(webview) = old {
            let _ = webview.close();
        }
        let controller = match self.build_controller(&pending) {
            Ok(controller) => controller,
            Err(error) => {
                self.mark_process_failed(
                    connection_id,
                    tab_id,
                    pending.generation,
                    "WEBVIEW2_RECOVERY_FAILED",
                );
                return Err(error);
            }
        };
        if let Err(error) = self.commit_recovered_controller(&pending, controller) {
            if let Some(webview) = self.app.get_webview(&pending.webview_label) {
                let _ = webview.close();
            }
            self.mark_process_failed(
                connection_id,
                tab_id,
                pending.generation,
                "WEBVIEW2_RECOVERY_FAILED",
            );
            return Err(error);
        }
        if pending.url != INITIAL_URL {
            self.navigate(connection_id, tab_id, pending.generation, &pending.url)?;
        }
        self.reconcile_visibility(Some(tab_id))?;
        self.record_event(
            "tab_recovered",
            connection_id,
            Some(tab_id),
            Some(pending.generation),
        );
        self.notify(connection_id);
        self.snapshot(connection_id)
    }

    pub async fn recover_failed_tabs(&self) -> Result<(), NativeBrowserError> {
        let failed = {
            let state = self
                .state
                .lock()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            state
                .registry
                .sessions
                .iter()
                .flat_map(|(connection_id, session)| {
                    session
                        .tabs
                        .values()
                        .filter(|record| record.snapshot.state == NativeTabState::Error)
                        .map(move |record| {
                            (
                                connection_id.clone(),
                                record.snapshot.id.clone(),
                                record.snapshot.generation,
                            )
                        })
                })
                .collect::<Vec<_>>()
        };
        for (connection_id, tab_id, generation) in failed {
            self.recover_tab(&connection_id, &tab_id, generation)
                .await?;
        }
        Ok(())
    }

    pub async fn call_cdp(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, NativeBrowserError> {
        if !allowed_cdp_methods().contains(method) {
            return Err(NativeBrowserError::CommandNotAllowed);
        }
        let webview = self.controller_for(connection_id, tab_id, generation)?;
        let method = method.to_string();
        let response =
            tokio::task::spawn_blocking(move || call_devtools_protocol(&webview, &method, params))
                .await
                .map_err(|_| NativeBrowserError::ControllerFailed)??;
        self.state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?
            .registry
            .validate_identity(connection_id, tab_id, generation)?;
        Ok(response)
    }

    pub fn subscribe_cdp(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        event: &str,
    ) -> Result<bool, NativeBrowserError> {
        if !allowed_cdp_events().contains(event) {
            return Err(NativeBrowserError::CommandNotAllowed);
        }
        let key = CdpEventKey::new(connection_id, tab_id, generation, event);
        let webview = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let webview_label = state
                .registry
                .validate_identity(connection_id, tab_id, generation)?
                .webview_label
                .clone();
            if state.cdp_subscriptions.contains(&key) {
                return Ok(false);
            }
            let webview = state
                .controllers
                .get(&webview_label)
                .cloned()
                .ok_or(NativeBrowserError::ControllerFailed)?;
            state.cdp_subscriptions.insert(key.clone());
            state.cdp_events.entry(key.clone()).or_default();
            webview
        };

        let handler_runtime = self.clone();
        let handler_key = key.clone();
        let event_name = event.to_string();
        let attached = with_core_webview(&webview, move |_, core| {
            let event_name = CoTaskMemPWSTR::from(event_name.as_str());
            let receiver =
                unsafe { core.GetDevToolsProtocolEventReceiver(*event_name.as_ref().as_pcwstr()) }
                    .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let mut token = 0_i64;
            unsafe {
                receiver.add_DevToolsProtocolEventReceived(
                    &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                        if let Some(args) = args {
                            let mut raw = PWSTR::null();
                            args.ParameterObjectAsJson(&mut raw)?;
                            handler_runtime.enqueue_cdp_event(&handler_key, take_pwstr(raw));
                        }
                        Ok(())
                    })),
                    &mut token,
                )
            }
            .map_err(|_| NativeBrowserError::ControllerFailed)
        });
        if let Err(error) = attached {
            if let Ok(mut state) = self.state.lock() {
                state.cdp_subscriptions.remove(&key);
                state.cdp_events.remove(&key);
            }
            return Err(error);
        }

        let state = self
            .state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
        state
            .registry
            .validate_identity(connection_id, tab_id, generation)?;
        if !state.cdp_subscriptions.contains(&key) {
            return Err(NativeBrowserError::ControllerFailed);
        }
        Ok(true)
    }

    pub fn drain_cdp_events(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        event: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Value>, NativeBrowserError> {
        if !allowed_cdp_events().contains(event) {
            return Err(NativeBrowserError::CommandNotAllowed);
        }
        let key = CdpEventKey::new(connection_id, tab_id, generation, event);
        let mut state = self
            .state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
        state
            .registry
            .validate_identity(connection_id, tab_id, generation)?;
        if !state.cdp_subscriptions.contains(&key) {
            return Err(NativeBrowserError::CommandNotAllowed);
        }
        let limit = limit.unwrap_or(MAX_CDP_DRAIN).clamp(1, MAX_CDP_DRAIN);
        let queue = state.cdp_events.entry(key).or_default();
        let mut events = Vec::with_capacity(limit.min(queue.len()));
        while events.len() < limit {
            let Some(event) = queue.pop_front() else {
                break;
            };
            events.push(event);
        }
        Ok(events)
    }

    fn enqueue_cdp_event(&self, key: &CdpEventKey, raw: String) {
        if raw.len() > MAX_CDP_EVENT_BYTES {
            return;
        }
        let Ok(params) = serde_json::from_str::<Value>(&raw) else {
            return;
        };
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state
            .registry
            .validate_identity(&key.connection_id, &key.tab_id, key.generation)
            .is_err()
            || !state.cdp_subscriptions.contains(key)
        {
            return;
        }
        let queue = state.cdp_events.entry(key.clone()).or_default();
        queue.push_back(json!({
            "method": key.event,
            "params": params,
        }));
        while queue.len() > MAX_CDP_EVENTS_PER_SUBSCRIPTION {
            queue.pop_front();
        }
    }

    pub fn downloads(
        &self,
        connection_id: &str,
    ) -> Result<Vec<NativeBrowserDownloadSnapshot>, NativeBrowserError> {
        validate_connection_id(connection_id)?;
        let state = self
            .state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
        Ok(state
            .downloads
            .get(connection_id)
            .into_iter()
            .flat_map(|records| records.iter())
            .map(|record| record.snapshot.clone())
            .collect())
    }

    async fn execute_bridge_command(
        &self,
        command: NativeBridgeCommand,
    ) -> Result<Value, NativeBrowserError> {
        match command {
            NativeBridgeCommand::Ensure { connection_id } => Ok(json!({
                "snapshot": self.ensure_session(&connection_id).await?
            })),
            NativeBridgeCommand::Snapshot { connection_id } => Ok(json!({
                "snapshot": self.snapshot(&connection_id)?
            })),
            NativeBridgeCommand::Downloads { connection_id } => Ok(json!({
                "downloads": self.downloads(&connection_id)?
            })),
            NativeBridgeCommand::Release { connection_id } => {
                validate_connection_id(&connection_id)?;
                self.release_session(&connection_id).await;
                Ok(json!({ "released": true }))
            }
            NativeBridgeCommand::Create { connection_id, url } => Ok(json!({
                "snapshot": self.create_tab(&connection_id, url.as_deref()).await?
            })),
            NativeBridgeCommand::Focus {
                connection_id,
                tab_id,
                generation,
            } => Ok(json!({
                "snapshot": self.focus_tab(&connection_id, &tab_id, generation)?
            })),
            NativeBridgeCommand::Close {
                connection_id,
                tab_id,
                generation,
            } => Ok(json!({
                "snapshot": self.close_tab(&connection_id, &tab_id, generation).await?
            })),
            NativeBridgeCommand::Navigate {
                connection_id,
                tab_id,
                generation,
                url,
            } => Ok(json!({
                "snapshot": self.navigate(&connection_id, &tab_id, generation, &url)?
            })),
            NativeBridgeCommand::History {
                connection_id,
                tab_id,
                generation,
                direction,
            } => Ok(json!({
                "snapshot": self.go_history(&connection_id, &tab_id, generation, direction)?
            })),
            NativeBridgeCommand::Reload {
                connection_id,
                tab_id,
                generation,
            } => Ok(json!({
                "snapshot": self.reload(&connection_id, &tab_id, generation)?
            })),
            NativeBridgeCommand::Stop {
                connection_id,
                tab_id,
                generation,
            } => Ok(json!({
                "snapshot": self.stop_loading(&connection_id, &tab_id, generation)?
            })),
            NativeBridgeCommand::Surface {
                connection_id,
                tab_id,
                generation,
                bounds,
                visible,
            } => Ok(json!({
                "snapshot": self.set_surface(
                    &connection_id,
                    &tab_id,
                    generation,
                    bounds,
                    visible,
                )?
            })),
            NativeBridgeCommand::Recover {
                connection_id,
                tab_id,
                generation,
            } => Ok(json!({
                "snapshot": self.recover_tab(&connection_id, &tab_id, generation).await?
            })),
            NativeBridgeCommand::Cdp {
                connection_id,
                tab_id,
                generation,
                method,
                params,
            } => Ok(json!({
                "result": self.call_cdp(
                    &connection_id,
                    &tab_id,
                    generation,
                    &method,
                    params,
                ).await?
            })),
            NativeBridgeCommand::CdpSubscribe {
                connection_id,
                tab_id,
                generation,
                event,
            } => Ok(json!({
                "subscribed": self.subscribe_cdp(
                    &connection_id,
                    &tab_id,
                    generation,
                    &event,
                )?
            })),
            NativeBridgeCommand::CdpDrain {
                connection_id,
                tab_id,
                generation,
                event,
                limit,
            } => Ok(json!({
                "events": self.drain_cdp_events(
                    &connection_id,
                    &tab_id,
                    generation,
                    &event,
                    limit,
                )?
            })),
        }
    }

    fn build_controller(
        &self,
        pending: &PendingNativeTab,
    ) -> Result<BuiltController, NativeBrowserError> {
        let connection_id = pending.connection_id.clone();
        let tab_id = pending.tab_id.clone();
        let generation = pending.generation;

        let navigation_runtime = self.clone();
        let page_runtime = self.clone();
        let page_connection = connection_id.clone();
        let page_tab = tab_id.clone();
        let title_runtime = self.clone();
        let title_connection = connection_id.clone();
        let title_tab = tab_id.clone();
        let popup_runtime = self.clone();
        let popup_connection = connection_id.clone();
        let download_runtime = self.clone();
        let download_connection = connection_id.clone();
        let download_tab = tab_id.clone();

        let builder = WebviewBuilder::new(
            pending.webview_label.clone(),
            WebviewUrl::External(INITIAL_URL.parse().expect("valid about:blank URL")),
        )
        .data_directory(self.profile_dir.clone())
        .focused(false)
        .devtools(false)
        .browser_extensions_enabled(false)
        .disable_drag_drop_handler()
        .on_navigation(move |url| {
            let allowed = validate_navigation_url(url.as_str()).is_ok();
            if !allowed {
                navigation_runtime.record_event(
                    "navigation_blocked",
                    &connection_id,
                    Some(&tab_id),
                    Some(generation),
                );
            }
            allowed
        })
        .on_page_load(move |_, payload| {
            let loading = matches!(payload.event(), PageLoadEvent::Started);
            page_runtime.update_page_load(
                &page_connection,
                &page_tab,
                generation,
                payload.url().to_string(),
                loading,
            );
        })
        .on_document_title_changed(move |_, title| {
            title_runtime.update_title(&title_connection, &title_tab, generation, title);
        })
        .on_new_window(move |url, _| {
            if validate_navigation_url(url.as_str()).is_ok() {
                let runtime = popup_runtime.clone();
                let connection_id = popup_connection.clone();
                let url = url.to_string();
                tauri::async_runtime::spawn(async move {
                    let _ = runtime.create_tab(&connection_id, Some(&url)).await;
                });
            } else {
                popup_runtime.record_event("popup_blocked", &popup_connection, None, None);
            }
            NewWindowResponse::Deny
        })
        .on_download(move |_, event| {
            download_runtime.handle_download(&download_connection, &download_tab, generation, event)
        });

        let parent = self
            .app
            .get_window("main")
            .ok_or(NativeBrowserError::NotInitialized)?;
        let child = parent
            .add_child(
                builder,
                LogicalPosition::new(0.0, 0.0),
                LogicalSize::new(1.0, 1.0),
            )
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
        let configured = (|| {
            child
                .hide()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let identities = self.attach_native_handlers(
                &child,
                &pending.connection_id,
                &pending.tab_id,
                pending.generation,
            )?;
            self.attach_request_interceptor(
                &child,
                &pending.connection_id,
                &pending.tab_id,
                pending.generation,
            )?;
            Ok(identities)
        })();
        match configured {
            Ok((controller_identity, environment_identity)) => Ok(BuiltController {
                webview: child,
                controller_identity,
                environment_identity,
            }),
            Err(error) => {
                let _ = child.close();
                Err(error)
            }
        }
    }

    fn commit_controller(
        &self,
        pending: &PendingNativeTab,
        controller: BuiltController,
    ) -> Result<(), NativeBrowserError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
        if state
            .environment_identity
            .is_some_and(|identity| identity != controller.environment_identity)
        {
            return Err(NativeBrowserError::ControllerFailed);
        }
        state.environment_identity = Some(controller.environment_identity);
        state.registry.commit_tab(pending.clone());
        state.controller_identities.insert(
            pending.webview_label.clone(),
            controller.controller_identity,
        );
        state
            .controllers
            .insert(pending.webview_label.clone(), controller.webview);
        Ok(())
    }

    fn commit_recovered_controller(
        &self,
        pending: &PendingNativeTab,
        controller: BuiltController,
    ) -> Result<(), NativeBrowserError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
        state.registry.validate_identity(
            &pending.connection_id,
            &pending.tab_id,
            pending.generation,
        )?;
        if state
            .environment_identity
            .is_some_and(|identity| identity != controller.environment_identity)
        {
            return Err(NativeBrowserError::ControllerFailed);
        }
        state.environment_identity = Some(controller.environment_identity);
        state.controller_identities.insert(
            pending.webview_label.clone(),
            controller.controller_identity,
        );
        state
            .controllers
            .insert(pending.webview_label.clone(), controller.webview);
        Ok(())
    }

    fn controller_for(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<Webview, NativeBrowserError> {
        let state = self
            .state
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
        let record = state
            .registry
            .validate_identity(connection_id, tab_id, generation)?;
        state
            .controllers
            .get(&record.webview_label)
            .cloned()
            .ok_or(NativeBrowserError::ControllerFailed)
    }

    fn reconcile_visibility(&self, focus_tab_id: Option<&str>) -> Result<(), NativeBrowserError> {
        let actions = {
            let state = self
                .state
                .lock()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let mut actions = Vec::new();
            for session in state.registry.sessions.values() {
                for record in session.tabs.values() {
                    let Some(webview) = state.controllers.get(&record.webview_label) else {
                        continue;
                    };
                    let visible = session.surface_visible
                        && session.active_tab_id.as_deref() == Some(record.snapshot.id.as_str());
                    actions.push((
                        webview.clone(),
                        record.snapshot.id.clone(),
                        session.surface_bounds,
                        visible,
                    ));
                }
            }
            actions
        };
        for (webview, tab_id, bounds, visible) in actions {
            if visible {
                webview
                    .set_position(LogicalPosition::new(bounds.x, bounds.y))
                    .and_then(|_| webview.set_size(LogicalSize::new(bounds.width, bounds.height)))
                    .and_then(|_| webview.show())
                    .map_err(|_| NativeBrowserError::ControllerFailed)?;
                if focus_tab_id == Some(tab_id.as_str()) {
                    webview
                        .set_focus()
                        .map_err(|_| NativeBrowserError::ControllerFailed)?;
                }
            } else {
                webview
                    .hide()
                    .map_err(|_| NativeBrowserError::ControllerFailed)?;
            }
        }
        Ok(())
    }

    fn attach_native_handlers(
        &self,
        webview: &Webview,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<(usize, usize), NativeBrowserError> {
        let permission_runtime = self.clone();
        let permission_connection = connection_id.to_string();
        let permission_tab = tab_id.to_string();
        let dialog_runtime = self.clone();
        let dialog_connection = connection_id.to_string();
        let dialog_tab = tab_id.to_string();
        let process_runtime = self.clone();
        let process_connection = connection_id.to_string();
        let process_tab = tab_id.to_string();
        let history_runtime = self.clone();
        let history_connection = connection_id.to_string();
        let history_tab = tab_id.to_string();
        let source_runtime = self.clone();
        let source_connection = connection_id.to_string();
        let source_tab = tab_id.to_string();
        let client_certificate_runtime = self.clone();
        let client_certificate_connection = connection_id.to_string();
        let client_certificate_tab = tab_id.to_string();
        let server_certificate_runtime = self.clone();
        let server_certificate_connection = connection_id.to_string();
        let server_certificate_tab = tab_id.to_string();

        with_core_webview(webview, move |controller, core| {
            unsafe {
                core.Settings()
                    .and_then(|settings| settings.SetAreDefaultScriptDialogsEnabled(false))
                    .map_err(|_| NativeBrowserError::ControllerFailed)?;
            }

            let mut permission_token = 0_i64;
            unsafe {
                core.add_PermissionRequested(
                    &PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                        if let Some(args) = args {
                            args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                        }
                        permission_runtime.record_event(
                            "permission_denied",
                            &permission_connection,
                            Some(&permission_tab),
                            Some(generation),
                        );
                        Ok(())
                    })),
                    &mut permission_token,
                )
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            }

            let mut dialog_token = 0_i64;
            unsafe {
                core.add_ScriptDialogOpening(
                    &ScriptDialogOpeningEventHandler::create(Box::new(move |_, args| {
                        if let Some(args) = args {
                            let mut kind = Default::default();
                            args.Kind(&mut kind)?;
                            if kind == COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT
                                || kind == COREWEBVIEW2_SCRIPT_DIALOG_KIND_BEFOREUNLOAD
                            {
                                args.Accept()?;
                            }
                        }
                        dialog_runtime.record_event(
                            "script_dialog_handled",
                            &dialog_connection,
                            Some(&dialog_tab),
                            Some(generation),
                        );
                        Ok(())
                    })),
                    &mut dialog_token,
                )
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            }

            let mut process_token = 0_i64;
            unsafe {
                core.add_ProcessFailed(
                    &ProcessFailedEventHandler::create(Box::new(move |_, _| {
                        process_runtime.mark_process_failed(
                            &process_connection,
                            &process_tab,
                            generation,
                            "WEBVIEW2_PROCESS_FAILED",
                        );
                        Ok(())
                    })),
                    &mut process_token,
                )
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            }

            let mut history_token = 0_i64;
            unsafe {
                core.add_HistoryChanged(
                    &HistoryChangedEventHandler::create(Box::new(move |webview, _| {
                        if let Some(webview) = webview {
                            let mut can_go_back: BOOL = false.into();
                            let mut can_go_forward: BOOL = false.into();
                            webview.CanGoBack(&mut can_go_back)?;
                            webview.CanGoForward(&mut can_go_forward)?;
                            history_runtime.update_history(
                                &history_connection,
                                &history_tab,
                                generation,
                                can_go_back.as_bool(),
                                can_go_forward.as_bool(),
                            );
                        }
                        Ok(())
                    })),
                    &mut history_token,
                )
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            }

            let mut source_token = 0_i64;
            unsafe {
                core.add_SourceChanged(
                    &SourceChangedEventHandler::create(Box::new(move |webview, _| {
                        if let Some(webview) = webview {
                            let mut source = PWSTR::null();
                            webview.Source(&mut source)?;
                            source_runtime.update_source(
                                &source_connection,
                                &source_tab,
                                generation,
                                take_pwstr(source),
                            );
                        }
                        Ok(())
                    })),
                    &mut source_token,
                )
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            }

            let webview5: ICoreWebView2_5 = core
                .cast()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let mut client_certificate_token = 0_i64;
            unsafe {
                webview5
                    .add_ClientCertificateRequested(
                        &ClientCertificateRequestedEventHandler::create(Box::new(
                            move |_, args| {
                                if let Some(args) = args {
                                    args.SetCancel(true)?;
                                    args.SetHandled(true)?;
                                }
                                client_certificate_runtime.record_event(
                                    "client_certificate_denied",
                                    &client_certificate_connection,
                                    Some(&client_certificate_tab),
                                    Some(generation),
                                );
                                Ok(())
                            },
                        )),
                        &mut client_certificate_token,
                    )
                    .map_err(|_| NativeBrowserError::ControllerFailed)?;
            }

            let webview14: ICoreWebView2_14 = core
                .cast()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let mut server_certificate_token = 0_i64;
            unsafe {
                webview14
                    .add_ServerCertificateErrorDetected(
                        &ServerCertificateErrorDetectedEventHandler::create(Box::new(
                            move |_, args| {
                                if let Some(args) = args {
                                    args.SetAction(
                                        COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL,
                                    )?;
                                }
                                server_certificate_runtime.record_event(
                                    "server_certificate_denied",
                                    &server_certificate_connection,
                                    Some(&server_certificate_tab),
                                    Some(generation),
                                );
                                Ok(())
                            },
                        )),
                        &mut server_certificate_token,
                    )
                    .map_err(|_| NativeBrowserError::ControllerFailed)?;
            }

            let controller_identity = controller.as_raw() as usize;
            let webview2: ICoreWebView2_2 = core
                .cast()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let environment_identity = unsafe {
                webview2
                    .Environment()
                    .map_err(|_| NativeBrowserError::ControllerFailed)?
                    .as_raw() as usize
            };
            Ok((controller_identity, environment_identity))
        })
    }

    fn attach_request_interceptor(
        &self,
        webview: &Webview,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
    ) -> Result<(), NativeBrowserError> {
        let paused_runtime = self.clone();
        let paused_webview = webview.clone();
        let paused_connection = connection_id.to_string();
        let paused_tab = tab_id.to_string();
        let auth_runtime = self.clone();
        let auth_webview = webview.clone();
        let auth_connection = connection_id.to_string();
        let auth_tab = tab_id.to_string();

        with_core_webview(webview, move |_, core| {
            let request_paused = CoTaskMemPWSTR::from("Fetch.requestPaused");
            let paused_receiver = unsafe {
                core.GetDevToolsProtocolEventReceiver(*request_paused.as_ref().as_pcwstr())
            }
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let mut paused_token = 0_i64;
            unsafe {
                paused_receiver.add_DevToolsProtocolEventReceived(
                    &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                        if let Some(args) = args {
                            let mut raw = PWSTR::null();
                            args.ParameterObjectAsJson(&mut raw)?;
                            let runtime = paused_runtime.clone();
                            let webview = paused_webview.clone();
                            let connection_id = paused_connection.clone();
                            let tab_id = paused_tab.clone();
                            let raw = take_pwstr(raw);
                            tauri::async_runtime::spawn(async move {
                                runtime
                                    .handle_paused_request(
                                        webview,
                                        connection_id,
                                        tab_id,
                                        generation,
                                        raw,
                                    )
                                    .await;
                            });
                        }
                        Ok(())
                    })),
                    &mut paused_token,
                )
            }
            .map_err(|_| NativeBrowserError::ControllerFailed)?;

            let auth_required = CoTaskMemPWSTR::from("Fetch.authRequired");
            let auth_receiver = unsafe {
                core.GetDevToolsProtocolEventReceiver(*auth_required.as_ref().as_pcwstr())
            }
            .map_err(|_| NativeBrowserError::ControllerFailed)?;
            let mut auth_token = 0_i64;
            unsafe {
                auth_receiver.add_DevToolsProtocolEventReceived(
                    &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                        if let Some(args) = args {
                            let mut raw = PWSTR::null();
                            args.ParameterObjectAsJson(&mut raw)?;
                            let runtime = auth_runtime.clone();
                            let webview = auth_webview.clone();
                            let connection_id = auth_connection.clone();
                            let tab_id = auth_tab.clone();
                            let raw = take_pwstr(raw);
                            tauri::async_runtime::spawn(async move {
                                runtime
                                    .handle_auth_required(
                                        webview,
                                        connection_id,
                                        tab_id,
                                        generation,
                                        raw,
                                    )
                                    .await;
                            });
                        }
                        Ok(())
                    })),
                    &mut auth_token,
                )
            }
            .map_err(|_| NativeBrowserError::ControllerFailed)
        })?;

        call_devtools_protocol(
            webview,
            "Fetch.enable",
            json!({
                "patterns": [
                    { "urlPattern": "http://*/*", "requestStage": "Request" },
                    { "urlPattern": "https://*/*", "requestStage": "Request" }
                ],
                "handleAuthRequests": true
            }),
        )?;
        call_devtools_protocol(
            webview,
            "Page.setInterceptFileChooserDialog",
            json!({ "enabled": true, "cancel": true }),
        )?;
        self.record_event(
            "security_handlers_attached",
            connection_id,
            Some(tab_id),
            Some(generation),
        );
        Ok(())
    }

    async fn handle_paused_request(
        &self,
        webview: Webview,
        connection_id: String,
        tab_id: String,
        generation: u64,
        raw: String,
    ) {
        let fallback_request_id = intercepted_request_id(&raw, MAX_CDP_EVENT_BYTES);
        let event = if raw.len() <= MAX_CDP_EVENT_BYTES {
            serde_json::from_str::<PausedRequestEvent>(&raw).ok()
        } else {
            None
        };
        let Some(event) = event else {
            if let Some(request_id) = fallback_request_id {
                let _ = fail_intercepted_request(webview, request_id).await;
            }
            self.record_event(
                "request_interceptor_failed",
                &connection_id,
                Some(&tab_id),
                Some(generation),
            );
            return;
        };
        if !valid_request_id(&event.request_id)
            || event
                .redirected_request_id
                .as_deref()
                .is_some_and(|value| !valid_request_id(value))
        {
            let _ = fail_intercepted_request(webview, event.request_id).await;
            self.record_event(
                "request_blocked",
                &connection_id,
                Some(&tab_id),
                Some(generation),
            );
            return;
        }

        let validated_url = validate_network_request_url(&event.request.url).await;
        let headers = validated_url.and_then(|canonical_url| {
            let mut state = self
                .state
                .lock()
                .map_err(|_| NativeBrowserError::ControllerFailed)?;
            state
                .registry
                .validate_identity(&connection_id, &tab_id, generation)?;
            let headers = if let Some(previous_id) = event.redirected_request_id.as_deref() {
                let previous_key =
                    NativeRequestKey::new(&connection_id, &tab_id, generation, previous_id);
                let previous_url = state
                    .request_urls
                    .get(&previous_key)
                    .ok_or(NativeBrowserError::NavigationBlocked)?;
                sanitize_redirect_headers(previous_url, &canonical_url, &event.request.headers)?
            } else {
                None
            };
            state.request_urls.remember(
                NativeRequestKey::new(&connection_id, &tab_id, generation, &event.request_id),
                canonical_url,
            );
            Ok(headers)
        });

        let allowed = headers.is_ok();
        let result = match headers {
            Ok(headers) => {
                continue_intercepted_request(webview.clone(), event.request_id.clone(), headers)
                    .await
            }
            Err(_) => fail_intercepted_request(webview.clone(), event.request_id.clone()).await,
        };
        let kind = match (allowed, result.is_ok()) {
            (true, true) => return,
            (false, true) => "request_blocked",
            _ => "request_interceptor_failed",
        };
        self.record_event(kind, &connection_id, Some(&tab_id), Some(generation));
    }

    async fn handle_auth_required(
        &self,
        webview: Webview,
        connection_id: String,
        tab_id: String,
        generation: u64,
        raw: String,
    ) {
        let Some(request_id) = intercepted_request_id(&raw, MAX_CDP_EVENT_BYTES) else {
            self.record_event(
                "request_interceptor_failed",
                &connection_id,
                Some(&tab_id),
                Some(generation),
            );
            return;
        };
        let result = call_internal_cdp(
            webview,
            "Fetch.continueWithAuth",
            json!({
                "requestId": request_id,
                "authChallengeResponse": { "response": "CancelAuth" }
            }),
        )
        .await;
        self.record_event(
            if result.is_ok() {
                "http_auth_denied"
            } else {
                "request_interceptor_failed"
            },
            &connection_id,
            Some(&tab_id),
            Some(generation),
        );
    }

    fn update_page_load(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        url: String,
        loading: bool,
    ) {
        let changed = self.state.lock().ok().is_some_and(|mut state| {
            state
                .registry
                .update_loading(connection_id, tab_id, generation, Some(url), loading)
        });
        if changed {
            self.notify(connection_id);
        }
    }

    fn update_source(&self, connection_id: &str, tab_id: &str, generation: u64, url: String) {
        let changed = self.state.lock().ok().is_some_and(|mut state| {
            state
                .registry
                .update_url(connection_id, tab_id, generation, url)
        });
        if changed {
            self.notify(connection_id);
        }
    }

    fn update_title(&self, connection_id: &str, tab_id: &str, generation: u64, title: String) {
        let changed = self.state.lock().ok().is_some_and(|mut state| {
            state
                .registry
                .update_title(connection_id, tab_id, generation, title)
        });
        if changed {
            self.notify(connection_id);
        }
    }

    fn update_history(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        can_go_back: bool,
        can_go_forward: bool,
    ) {
        let changed = self.state.lock().ok().is_some_and(|mut state| {
            state.registry.update_history(
                connection_id,
                tab_id,
                generation,
                can_go_back,
                can_go_forward,
            )
        });
        if changed {
            self.notify(connection_id);
        }
    }

    fn mark_process_failed(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        error_code: &str,
    ) {
        let webview = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if !state
                .registry
                .mark_failed(connection_id, tab_id, generation, error_code)
            {
                return;
            }
            state.remove_cdp_for_tab(connection_id, tab_id);
            let label = state
                .registry
                .validate_identity(connection_id, tab_id, generation)
                .ok()
                .map(|record| record.webview_label.clone());
            label.and_then(|label| state.controllers.get(&label).cloned())
        };
        if let Some(webview) = webview {
            let _ = webview.hide();
        }
        self.record_event(
            "process_failed",
            connection_id,
            Some(tab_id),
            Some(generation),
        );
        self.notify(connection_id);
    }

    fn handle_download(
        &self,
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        event: DownloadEvent<'_>,
    ) -> bool {
        match event {
            DownloadEvent::Requested { url, destination } => {
                let filename = destination
                    .file_name()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty())
                    .unwrap_or("download")
                    .to_string();
                let Ok(path) = self.managed_download_path(connection_id, destination) else {
                    self.record_event(
                        "download_blocked",
                        connection_id,
                        Some(tab_id),
                        Some(generation),
                    );
                    return false;
                };
                let record = NativeDownloadRecord {
                    snapshot: NativeBrowserDownloadSnapshot {
                        guid: uuid::Uuid::new_v4().to_string(),
                        state: NativeDownloadState::InProgress,
                        filename,
                        received_bytes: 0,
                        total_bytes: 0,
                        path: None,
                        error_code: None,
                    },
                    source_url: url.to_string(),
                    pending_path: path.clone(),
                };
                let Ok(mut state) = self.state.lock() else {
                    return false;
                };
                let records = state
                    .downloads
                    .entry(connection_id.to_string())
                    .or_default();
                records.push_back(record);
                while records.len() > MAX_DOWNLOAD_RECORDS_PER_SESSION {
                    records.pop_front();
                }
                drop(state);
                *destination = path;
                self.record_event(
                    "download_started",
                    connection_id,
                    Some(tab_id),
                    Some(generation),
                );
                true
            }
            DownloadEvent::Finished { url, path, success } => {
                let pending = self.state.lock().ok().and_then(|state| {
                    state.downloads.get(connection_id).and_then(|records| {
                        records
                            .iter()
                            .rev()
                            .find(|record| {
                                record.snapshot.state == NativeDownloadState::InProgress
                                    && (path
                                        .as_ref()
                                        .is_some_and(|value| value == &record.pending_path)
                                        || record.source_url == url.as_str())
                            })
                            .map(|record| {
                                (
                                    record.snapshot.guid.clone(),
                                    path.clone().unwrap_or_else(|| record.pending_path.clone()),
                                )
                            })
                    })
                });
                if let Some((guid, candidate)) = pending {
                    let completed =
                        success.then(|| self.completed_download_path(connection_id, &candidate));
                    if let Ok(mut state) = self.state.lock() {
                        if let Some(record) =
                            state.downloads.get_mut(connection_id).and_then(|records| {
                                records
                                    .iter_mut()
                                    .find(|record| record.snapshot.guid == guid)
                            })
                        {
                            match completed {
                                Some(Ok(path)) => {
                                    let bytes = fs::metadata(&candidate)
                                        .map(|metadata| metadata.len())
                                        .unwrap_or(0);
                                    record.snapshot.state = NativeDownloadState::Completed;
                                    record.snapshot.received_bytes = bytes;
                                    record.snapshot.total_bytes = bytes;
                                    record.snapshot.path = Some(path);
                                    record.snapshot.error_code = None;
                                }
                                Some(Err(_)) => {
                                    record.snapshot.state = NativeDownloadState::Canceled;
                                    record.snapshot.error_code = Some("PATH_ESCAPE".to_string());
                                }
                                None => {
                                    record.snapshot.state = NativeDownloadState::Canceled;
                                    record.snapshot.error_code =
                                        Some("DOWNLOAD_FAILED".to_string());
                                }
                            }
                        }
                    }
                }
                self.record_event(
                    if success {
                        "download_finished"
                    } else {
                        "download_failed"
                    },
                    connection_id,
                    Some(tab_id),
                    Some(generation),
                );
                true
            }
            _ => false,
        }
    }

    fn managed_download_path(
        &self,
        connection_id: &str,
        proposed: &Path,
    ) -> Result<PathBuf, NativeBrowserError> {
        managed_download_path_for_root(&self.download_dir, connection_id, proposed)
    }

    fn completed_download_path(
        &self,
        connection_id: &str,
        candidate: &Path,
    ) -> Result<String, NativeBrowserError> {
        completed_download_path_for_root(&self.download_dir, connection_id, candidate)
    }

    fn close_all_controllers(&self) {
        let controllers = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            state.registry.sessions.clear();
            state.controller_identities.clear();
            state.environment_identity = None;
            state.cdp_subscriptions.clear();
            state.cdp_events.clear();
            state.request_urls.clear();
            state.downloads.clear();
            state
                .controllers
                .drain()
                .map(|(_, webview)| webview)
                .collect::<Vec<_>>()
        };
        for webview in controllers {
            let _ = webview.close();
        }
    }

    fn record_event(
        &self,
        kind: &'static str,
        connection_id: &str,
        tab_id: Option<&str>,
        generation: Option<u64>,
    ) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.recent_events.push_back(NativeRuntimeEvent {
            kind,
            connection_id: connection_id.to_string(),
            tab_id: tab_id.map(str::to_string),
            generation,
        });
        while state.recent_events.len() > EVENT_LIMIT {
            state.recent_events.pop_front();
        }
    }

    fn notify(&self, connection_id: &str) {
        let _ = self.updates.send(connection_id.to_string());
        emit_event(
            &self.emitter,
            UPDATE_EVENT,
            json!({ "connectionId": connection_id }),
        );
    }
}

struct BuiltController {
    webview: Webview,
    controller_identity: usize,
    environment_identity: usize,
}

#[derive(Clone)]
struct NativeBridgeState {
    runtime: NativeBrowserRuntime,
    token: Arc<str>,
}

async fn native_bridge_health(
    State(state): State<NativeBridgeState>,
    headers: HeaderMap,
) -> Response {
    if !bridge_authorized(&headers, &state.token) {
        return bridge_error(StatusCode::UNAUTHORIZED, NativeBrowserError::Unauthorized);
    }
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "backend": EMBEDDED_WEBVIEW2_BACKEND_ID,
            "protocolVersion": NATIVE_BRIDGE_PROTOCOL_VERSION,
        })),
    )
        .into_response()
}

async fn native_bridge_command(
    State(state): State<NativeBridgeState>,
    headers: HeaderMap,
    Json(command): Json<NativeBridgeCommand>,
) -> Response {
    if !bridge_authorized(&headers, &state.token) {
        return bridge_error(StatusCode::UNAUTHORIZED, NativeBrowserError::Unauthorized);
    }
    match tokio::time::timeout(
        NATIVE_BRIDGE_REQUEST_TIMEOUT,
        state.runtime.execute_bridge_command(command),
    )
    .await
    {
        Ok(Ok(value)) => {
            (StatusCode::OK, Json(json!({ "ok": true, "value": value }))).into_response()
        }
        Ok(Err(error)) => bridge_error(status_for_native_error(&error), error),
        Err(_) => bridge_error(
            StatusCode::GATEWAY_TIMEOUT,
            NativeBrowserError::BridgeFailed,
        ),
    }
}

fn bridge_authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.as_bytes().strip_prefix(b"Bearer "))
    else {
        return false;
    };
    fixed_time_token_eq(value, token.as_bytes())
}

fn bridge_error(status: StatusCode, error: NativeBrowserError) -> Response {
    (
        status,
        Json(json!({
            "ok": false,
            "error": {
                "code": error.code(),
                "message": error.to_string(),
                "retryable": error.retryable(),
            }
        })),
    )
        .into_response()
}

fn status_for_native_error(error: &NativeBrowserError) -> StatusCode {
    match error {
        NativeBrowserError::Unauthorized => StatusCode::UNAUTHORIZED,
        NativeBrowserError::SessionNotFound | NativeBrowserError::TabNotFound => {
            StatusCode::NOT_FOUND
        }
        NativeBrowserError::GenerationMismatch => StatusCode::CONFLICT,
        NativeBrowserError::InvalidConnection
        | NativeBrowserError::InvalidBounds
        | NativeBrowserError::NavigationBlocked
        | NativeBrowserError::CommandNotAllowed => StatusCode::BAD_REQUEST,
        NativeBrowserError::UnsupportedPlatform
        | NativeBrowserError::NotInitialized
        | NativeBrowserError::ControllerFailed
        | NativeBrowserError::BridgeFailed => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn continue_intercepted_request(
    webview: Webview,
    request_id: String,
    headers: Option<Vec<Value>>,
) -> Result<(), NativeBrowserError> {
    let mut params = json!({ "requestId": request_id });
    if let Some(headers) = headers {
        params["headers"] = Value::Array(headers);
    }
    call_internal_cdp(webview, "Fetch.continueRequest", params)
        .await
        .map(|_| ())
}

async fn fail_intercepted_request(
    webview: Webview,
    request_id: String,
) -> Result<(), NativeBrowserError> {
    call_internal_cdp(
        webview,
        "Fetch.failRequest",
        json!({ "requestId": request_id, "errorReason": "BlockedByClient" }),
    )
    .await
    .map(|_| ())
}

async fn call_internal_cdp(
    webview: Webview,
    method: &'static str,
    params: Value,
) -> Result<Value, NativeBrowserError> {
    tokio::task::spawn_blocking(move || call_devtools_protocol(&webview, method, params))
        .await
        .map_err(|_| NativeBrowserError::ControllerFailed)?
}

fn registry_is_consistent(state: &NativeRuntimeState) -> bool {
    let labels = state
        .registry
        .sessions
        .values()
        .flat_map(|session| session.tabs.values())
        .map(|record| record.webview_label.as_str())
        .collect::<std::collections::HashSet<_>>();
    labels.len() == state.controllers.len()
        && labels
            .iter()
            .all(|label| state.controllers.contains_key(*label))
        && state.registry.sessions.values().all(|session| {
            session.tab_order.len() == session.tabs.len()
                && session
                    .tab_order
                    .iter()
                    .all(|tab_id| session.tabs.contains_key(tab_id))
                && session
                    .active_tab_id
                    .as_ref()
                    .is_none_or(|tab_id| session.tabs.contains_key(tab_id))
        })
}

fn with_core_webview<T, F>(webview: &Webview, operation: F) -> Result<T, NativeBrowserError>
where
    T: Send + 'static,
    F: FnOnce(
            webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
            webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
        ) -> Result<T, NativeBrowserError>
        + Send
        + 'static,
{
    let (tx, rx) = sync_channel(1);
    webview
        .with_webview(move |platform| {
            let controller = platform.controller();
            let result = unsafe { controller.CoreWebView2() }
                .map_err(|_| NativeBrowserError::ControllerFailed)
                .and_then(|core| operation(controller, core));
            let _ = tx.send(result);
        })
        .map_err(|_| NativeBrowserError::ControllerFailed)?;
    rx.recv_timeout(CONTROLLER_TIMEOUT)
        .map_err(|_| NativeBrowserError::ControllerFailed)?
}

fn call_devtools_protocol(
    webview: &Webview,
    method: &str,
    params: Value,
) -> Result<Value, NativeBrowserError> {
    let method = method.to_string();
    let params =
        serde_json::to_string(&params).map_err(|_| NativeBrowserError::CommandNotAllowed)?;
    let body = with_core_webview(webview, move |_, core| {
        let response = Arc::new(Mutex::new(None::<String>));
        let completed_response = response.clone();
        CallDevToolsProtocolMethodCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                let method = CoTaskMemPWSTR::from(method.as_str());
                let params = CoTaskMemPWSTR::from(params.as_str());
                core.CallDevToolsProtocolMethod(
                    *method.as_ref().as_pcwstr(),
                    *params.as_ref().as_pcwstr(),
                    &handler,
                )
                .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(move |status, body| {
                status?;
                if let Ok(mut slot) = completed_response.lock() {
                    *slot = Some(body);
                }
                Ok(())
            }),
        )
        .map_err(|_| NativeBrowserError::ControllerFailed)?;
        let body = response
            .lock()
            .map_err(|_| NativeBrowserError::ControllerFailed)?
            .take()
            .ok_or(NativeBrowserError::ControllerFailed)?;
        Ok(body)
    })?;
    let value =
        serde_json::from_str::<Value>(&body).map_err(|_| NativeBrowserError::ControllerFailed)?;
    if value.get("error").is_some() {
        return Err(NativeBrowserError::ControllerFailed);
    }
    Ok(value)
}
