use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as AxumPath, Query, State,
    },
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

pub const PROTOCOL_VERSION: u8 = 1;
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1024 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const FRAMES_PER_SECOND: u32 = 120;

#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub credential_file: PathBuf,
    pub desktop_tokens: HashMap<String, String>,
    pub max_frame_bytes: usize,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = env::var("CODEG_RELAY_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8787".to_string())
            .parse()
            .context("CODEG_RELAY_BIND must be a socket address")?;
        let credential_file = env::var_os("CODEG_RELAY_CREDENTIAL_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data/device-credentials.json"));
        let raw_tokens = env::var("CODEG_RELAY_DESKTOP_TOKENS")
            .context("CODEG_RELAY_DESKTOP_TOKENS must be a JSON object")?;
        let desktop_tokens: HashMap<String, String> = serde_json::from_str(&raw_tokens)
            .context("CODEG_RELAY_DESKTOP_TOKENS must be a JSON object")?;
        if desktop_tokens.is_empty() || desktop_tokens.values().any(|token| token.len() < 32) {
            bail!("at least one desktop token of 32+ characters is required");
        }
        let max_frame_bytes = env::var("CODEG_RELAY_MAX_FRAME_BYTES")
            .ok()
            .map(|value| value.parse())
            .transpose()
            .context("CODEG_RELAY_MAX_FRAME_BYTES must be an integer")?
            .unwrap_or(DEFAULT_MAX_FRAME_BYTES);
        if !(4096..=4 * 1024 * 1024).contains(&max_frame_bytes) {
            bail!("CODEG_RELAY_MAX_FRAME_BYTES must be between 4096 and 4194304");
        }
        Ok(Self {
            bind,
            credential_file,
            desktop_tokens,
            max_frame_bytes,
        })
    }
}

#[derive(Default)]
struct Metrics {
    active_desktops: AtomicU64,
    active_mobiles: AtomicU64,
    forwarded_frames: AtomicU64,
    rejected_frames: AtomicU64,
    auth_failures: AtomicU64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DeviceCredential {
    desktop_id: String,
    device_id: String,
    token_sha256: String,
    created_at: u64,
    #[serde(default)]
    last_seen_at: Option<u64>,
    revoked_at: Option<u64>,
}

#[derive(Default, Serialize, Deserialize)]
struct CredentialFile {
    devices: Vec<DeviceCredential>,
}

#[derive(Clone)]
struct CredentialStore {
    path: Arc<PathBuf>,
    devices: Arc<RwLock<HashMap<(String, String), DeviceCredential>>>,
}

impl CredentialStore {
    async fn load(path: PathBuf) -> anyhow::Result<Self> {
        let file = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice::<CredentialFile>(&bytes)
                .with_context(|| format!("invalid credential file {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => CredentialFile::default(),
            Err(error) => return Err(error).context("failed to read credential file"),
        };
        let devices = file
            .devices
            .into_iter()
            .map(|record| {
                (
                    (record.desktop_id.clone(), record.device_id.clone()),
                    record,
                )
            })
            .collect();
        Ok(Self {
            path: Arc::new(path),
            devices: Arc::new(RwLock::new(devices)),
        })
    }

    async fn issue(&self, desktop_id: &str, device_id: &str) -> anyhow::Result<String> {
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).context("OS random generator failed")?;
        let token = format!("mrt_{}", URL_SAFE_NO_PAD.encode(random));
        let record = DeviceCredential {
            desktop_id: desktop_id.to_owned(),
            device_id: device_id.to_owned(),
            token_sha256: token_hash_hex(&token),
            created_at: unix_seconds(),
            last_seen_at: None,
            revoked_at: None,
        };
        let snapshot = {
            let mut devices = self.devices.write().await;
            devices.insert((desktop_id.to_owned(), device_id.to_owned()), record);
            devices.values().cloned().collect::<Vec<_>>()
        };
        self.persist(snapshot).await?;
        Ok(token)
    }

    async fn authenticate(&self, desktop_id: &str, device_id: &str, token: &str) -> bool {
        let snapshot = {
            let mut devices = self.devices.write().await;
            let Some(record) = devices.get_mut(&(desktop_id.to_owned(), device_id.to_owned()))
            else {
                return false;
            };
            if record.revoked_at.is_some()
                || !constant_time_hash_matches(&record.token_sha256, token)
            {
                return false;
            }
            record.last_seen_at = Some(unix_seconds());
            devices.values().cloned().collect::<Vec<_>>()
        };
        if let Err(error) = self.persist(snapshot).await {
            warn!(error = %error, "failed to persist Relay device activity");
        }
        true
    }

    async fn revoke(&self, desktop_id: &str, device_id: &str) -> anyhow::Result<bool> {
        let (found, snapshot) = {
            let mut devices = self.devices.write().await;
            let found = if let Some(record) =
                devices.get_mut(&(desktop_id.to_owned(), device_id.to_owned()))
            {
                record.revoked_at = Some(unix_seconds());
                true
            } else {
                false
            };
            (found, devices.values().cloned().collect::<Vec<_>>())
        };
        if found {
            self.persist(snapshot).await?;
        }
        Ok(found)
    }

    async fn list(&self, desktop_id: &str) -> Vec<DeviceSummary> {
        self.devices
            .read()
            .await
            .values()
            .filter(|record| record.desktop_id == desktop_id)
            .map(|record| DeviceSummary {
                device_id: record.device_id.clone(),
                created_at: record.created_at,
                last_seen_at: record.last_seen_at,
                revoked_at: record.revoked_at,
            })
            .collect()
    }

    async fn persist(&self, devices: Vec<DeviceCredential>) -> anyhow::Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        tokio::fs::create_dir_all(parent).await?;
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(&CredentialFile { devices })?;
        tokio::fs::write(&tmp, bytes).await?;
        set_private_permissions(&tmp).await?;
        tokio::fs::rename(&tmp, self.path.as_ref()).await?;
        Ok(())
    }
}

#[cfg(unix)]
async fn set_private_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await
}

#[cfg(not(unix))]
async fn set_private_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[derive(Clone, Default)]
struct Hub {
    desktops: Arc<RwLock<HashMap<String, PeerSender>>>,
    mobiles: Arc<RwLock<MobilePeers>>,
}

type PeerSender = mpsc::Sender<Message>;
type MobilePeers = HashMap<(String, String), PeerSender>;

impl Hub {
    async fn register_desktop(&self, id: String, sender: mpsc::Sender<Message>) -> bool {
        let previous = self.desktops.write().await.insert(id, sender);
        if let Some(previous) = previous.as_ref() {
            let _ = previous.send(Message::Close(None)).await;
        }
        previous.is_none()
    }

    async fn register_mobile(
        &self,
        desktop_id: String,
        device_id: String,
        sender: mpsc::Sender<Message>,
    ) -> bool {
        let previous = self
            .mobiles
            .write()
            .await
            .insert((desktop_id, device_id), sender);
        if let Some(previous) = previous.as_ref() {
            let _ = previous.send(Message::Close(None)).await;
        }
        previous.is_none()
    }

    async fn remove_desktop(&self, id: &str, sender: &mpsc::Sender<Message>) -> bool {
        let mut desktops = self.desktops.write().await;
        if desktops
            .get(id)
            .is_some_and(|current| current.same_channel(sender))
        {
            desktops.remove(id);
            return true;
        }
        false
    }

    async fn remove_mobile(
        &self,
        desktop_id: &str,
        device_id: &str,
        sender: &mpsc::Sender<Message>,
    ) -> bool {
        let key = (desktop_id.to_owned(), device_id.to_owned());
        let mut mobiles = self.mobiles.write().await;
        if mobiles
            .get(&key)
            .is_some_and(|current| current.same_channel(sender))
        {
            mobiles.remove(&key);
            return true;
        }
        false
    }

    async fn disconnect_mobile(&self, desktop_id: &str, device_id: &str) {
        if let Some(sender) = self
            .mobiles
            .write()
            .await
            .remove(&(desktop_id.to_owned(), device_id.to_owned()))
        {
            let _ = sender.send(Message::Close(None)).await;
        }
    }
}

pub struct AppState {
    desktop_token_hashes: HashMap<String, [u8; 32]>,
    credentials: CredentialStore,
    hub: Hub,
    metrics: Metrics,
    max_frame_bytes: usize,
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let desktop_token_hashes = config
            .desktop_tokens
            .into_iter()
            .map(|(id, token)| (id, token_hash(&token)))
            .collect();
        Ok(Self {
            desktop_token_hashes,
            credentials: CredentialStore::load(config.credential_file).await?,
            hub: Hub::default(),
            metrics: Metrics::default(),
            max_frame_bytes: config.max_frame_bytes,
        })
    }

    fn authenticate_desktop(&self, desktop_id: &str, token: &str) -> bool {
        self.desktop_token_hashes
            .get(desktop_id)
            .or_else(|| self.desktop_token_hashes.get("*"))
            .is_some_and(|expected| expected.ct_eq(&token_hash(token)).into())
    }
}

pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/v1/ws", get(websocket))
        .route("/v1/devices", get(list_devices).post(issue_device))
        .route("/v1/devices/{device_id}", delete(revoke_device))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status":"ok","protocol":PROTOCOL_VERSION}))
}

async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let metrics = &state.metrics;
    format!(
        concat!(
            "codeg_relay_active_desktops {}\n",
            "codeg_relay_active_mobiles {}\n",
            "codeg_relay_forwarded_frames_total {}\n",
            "codeg_relay_rejected_frames_total {}\n",
            "codeg_relay_auth_failures_total {}\n"
        ),
        metrics.active_desktops.load(Ordering::Relaxed),
        metrics.active_mobiles.load(Ordering::Relaxed),
        metrics.forwarded_frames.load(Ordering::Relaxed),
        metrics.rejected_frames.load(Ordering::Relaxed),
        metrics.auth_failures.load(Ordering::Relaxed),
    )
}

async fn websocket(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.max_message_size(state.max_frame_bytes + 4096)
        .max_frame_size(state.max_frame_bytes + 4096)
        .on_upgrade(move |socket| handle_socket(socket, state))
}

#[derive(Deserialize)]
struct Hello {
    v: u8,
    #[serde(rename = "type")]
    message_type: String,
    role: String,
    desktop_id: String,
    device_id: Option<String>,
    token: String,
}

enum Session {
    Desktop {
        desktop_id: String,
    },
    Mobile {
        desktop_id: String,
        device_id: String,
    },
}

async fn authenticate_socket(socket: &mut WebSocket, state: &AppState) -> Option<Session> {
    let message = tokio::time::timeout(AUTH_TIMEOUT, socket.recv())
        .await
        .ok()??
        .ok()?;
    let Message::Text(text) = message else {
        return None;
    };
    if text.len() > 8192 {
        return None;
    }
    let hello: Hello = serde_json::from_str(&text).ok()?;
    if hello.v != PROTOCOL_VERSION || hello.message_type != "hello" || !valid_id(&hello.desktop_id)
    {
        return None;
    }
    match hello.role.as_str() {
        "desktop" if state.authenticate_desktop(&hello.desktop_id, &hello.token) => {
            Some(Session::Desktop {
                desktop_id: hello.desktop_id,
            })
        }
        "mobile" => {
            let device_id = hello.device_id.filter(|id| valid_id(id))?;
            state
                .credentials
                .authenticate(&hello.desktop_id, &device_id, &hello.token)
                .await
                .then_some(Session::Mobile {
                    desktop_id: hello.desktop_id,
                    device_id,
                })
        }
        _ => None,
    }
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let Some(session) = authenticate_socket(&mut socket, &state).await else {
        state.metrics.auth_failures.fetch_add(1, Ordering::Relaxed);
        let _ = socket.send(Message::Close(None)).await;
        return;
    };

    let (mut sink, mut stream) = socket.split();
    let (sender, mut receiver) = mpsc::channel::<Message>(256);
    let writer = tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            let closing = matches!(message, Message::Close(_));
            if sink.send(message).await.is_err() || closing {
                break;
            }
        }
    });

    match &session {
        Session::Desktop { desktop_id } => {
            let inserted = state
                .hub
                .register_desktop(desktop_id.clone(), sender.clone())
                .await;
            if inserted {
                state
                    .metrics
                    .active_desktops
                    .fetch_add(1, Ordering::Relaxed);
            }
            info!(role = "desktop", %desktop_id, "relay peer connected");
        }
        Session::Mobile {
            desktop_id,
            device_id,
        } => {
            let inserted = state
                .hub
                .register_mobile(desktop_id.clone(), device_id.clone(), sender.clone())
                .await;
            if inserted {
                state.metrics.active_mobiles.fetch_add(1, Ordering::Relaxed);
            }
            info!(role = "mobile", %desktop_id, %device_id, "relay peer connected");
        }
    }

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    let mut last_seen = Instant::now();
    let mut rate = FrameRate::default();

    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(message)) = incoming else { break };
                last_seen = Instant::now();
                match message {
                    Message::Text(text) => {
                        if text.len() > state.max_frame_bytes || !rate.accept() {
                            state.metrics.rejected_frames.fetch_add(1, Ordering::Relaxed);
                            let _ = sender.send(error_message(if text.len() > state.max_frame_bytes { "frame_too_large" } else { "rate_limited" })).await;
                            continue;
                        }
                        if forward_text(&state, &session, text).await {
                            state.metrics.forwarded_frames.fetch_add(1, Ordering::Relaxed);
                        } else {
                            state.metrics.rejected_frames.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Message::Ping(bytes) => { let _ = sender.send(Message::Pong(bytes)).await; }
                    Message::Pong(_) => {}
                    Message::Close(_) => break,
                    Message::Binary(_) => {
                        state.metrics.rejected_frames.fetch_add(1, Ordering::Relaxed);
                        let _ = sender.send(error_message("protocol_violation")).await;
                    }
                }
            }
            _ = heartbeat.tick() => {
                if last_seen.elapsed() > IDLE_TIMEOUT { break; }
                if sender.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
        }
    }

    match &session {
        Session::Desktop { desktop_id } => {
            if state.hub.remove_desktop(desktop_id, &sender).await {
                state
                    .metrics
                    .active_desktops
                    .fetch_sub(1, Ordering::Relaxed);
            }
            info!(role = "desktop", %desktop_id, "relay peer disconnected");
        }
        Session::Mobile {
            desktop_id,
            device_id,
        } => {
            let removed = state
                .hub
                .remove_mobile(desktop_id, device_id, &sender)
                .await;
            if removed {
                state.metrics.active_mobiles.fetch_sub(1, Ordering::Relaxed);
            }
            info!(role = "mobile", %desktop_id, %device_id, "relay peer disconnected");
        }
    }
    let _ = sender.send(Message::Close(None)).await;
    let _ = writer.await;
}

#[derive(Deserialize)]
struct RoutingEnvelope {
    v: u8,
    desktop_id: String,
    device_id: String,
}

async fn forward_text(
    state: &AppState,
    session: &Session,
    text: axum::extract::ws::Utf8Bytes,
) -> bool {
    let Ok(route) = serde_json::from_str::<RoutingEnvelope>(&text) else {
        return false;
    };
    if route.v != PROTOCOL_VERSION || !valid_id(&route.desktop_id) || !valid_id(&route.device_id) {
        return false;
    }

    let target = match session {
        Session::Desktop { desktop_id } if desktop_id == &route.desktop_id => state
            .hub
            .mobiles
            .read()
            .await
            .get(&(route.desktop_id, route.device_id))
            .cloned(),
        Session::Mobile {
            desktop_id,
            device_id,
        } if desktop_id == &route.desktop_id && device_id == &route.device_id => {
            state.hub.desktops.read().await.get(desktop_id).cloned()
        }
        _ => None,
    };

    let Some(target) = target else {
        return false;
    };
    target.send(Message::Text(text)).await.is_ok()
}

#[derive(Default)]
struct FrameRate {
    window_started: Option<Instant>,
    count: u32,
}

impl FrameRate {
    fn accept(&mut self) -> bool {
        let now = Instant::now();
        if self
            .window_started
            .is_none_or(|started| now.duration_since(started) >= Duration::from_secs(1))
        {
            self.window_started = Some(now);
            self.count = 0;
        }
        self.count += 1;
        self.count <= FRAMES_PER_SECOND
    }
}

fn error_message(code: &str) -> Message {
    Message::Text(
        serde_json::json!({"v":PROTOCOL_VERSION,"type":"error","code":code})
            .to_string()
            .into(),
    )
}

#[derive(Deserialize)]
struct DeviceRequest {
    desktop_id: String,
    device_id: String,
}

#[derive(Serialize, Deserialize)]
struct IssuedDevice {
    desktop_id: String,
    device_id: String,
    token: String,
}

#[derive(Serialize)]
struct DeviceSummary {
    device_id: String,
    created_at: u64,
    last_seen_at: Option<u64>,
    revoked_at: Option<u64>,
}

#[derive(Deserialize)]
struct DesktopQuery {
    desktop_id: String,
}

async fn issue_device(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<DeviceRequest>,
) -> Result<Json<IssuedDevice>, ApiError> {
    authorize_desktop(&state, &headers, &request.desktop_id)?;
    if !valid_id(&request.desktop_id) || !valid_id(&request.device_id) {
        return Err(ApiError::bad_request("invalid_device_id"));
    }
    let token = state
        .credentials
        .issue(&request.desktop_id, &request.device_id)
        .await
        .map_err(ApiError::internal)?;
    info!(desktop_id = %request.desktop_id, device_id = %request.device_id, "relay device credential issued");
    Ok(Json(IssuedDevice {
        desktop_id: request.desktop_id,
        device_id: request.device_id,
        token,
    }))
}

async fn list_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<DesktopQuery>,
) -> Result<Json<Vec<DeviceSummary>>, ApiError> {
    authorize_desktop(&state, &headers, &query.desktop_id)?;
    Ok(Json(state.credentials.list(&query.desktop_id).await))
}

async fn revoke_device(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<DesktopQuery>,
    AxumPath(device_id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    authorize_desktop(&state, &headers, &query.desktop_id)?;
    if !state
        .credentials
        .revoke(&query.desktop_id, &device_id)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::not_found("device_not_found"));
    }
    state
        .hub
        .disconnect_mobile(&query.desktop_id, &device_id)
        .await;
    info!(desktop_id = %query.desktop_id, %device_id, "relay device revoked");
    Ok(StatusCode::NO_CONTENT)
}

fn authorize_desktop(
    state: &AppState,
    headers: &HeaderMap,
    desktop_id: &str,
) -> Result<(), ApiError> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("unauthorized"))?;
    if !state.authenticate_desktop(desktop_id, token) {
        return Err(ApiError::unauthorized("unauthorized"));
    }
    Ok(())
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
}

impl ApiError {
    fn bad_request(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
        }
    }

    fn unauthorized(code: &'static str) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code,
        }
    }

    fn not_found(code: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
        }
    }

    fn internal(error: anyhow::Error) -> Self {
        warn!(error = %error, "relay internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({"error": self.code}))).into_response()
    }
}

fn valid_id(value: &str) -> bool {
    (3..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn token_hash_hex(token: &str) -> String {
    hex::encode(token_hash(token))
}

fn constant_time_hash_matches(expected_hex: &str, token: &str) -> bool {
    let Ok(expected) = hex::decode(expected_hex) else {
        return false;
    };
    expected.as_slice().ct_eq(&token_hash(token)).into()
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};

    fn test_config(path: PathBuf) -> Config {
        Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            credential_file: path,
            desktop_tokens: HashMap::from([(
                "d_test".to_string(),
                "desktop-token-at-least-thirty-two-characters".to_string(),
            )]),
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        }
    }

    #[tokio::test]
    async fn issued_credentials_are_hashed_persisted_and_revocable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("credentials.json");
        let store = CredentialStore::load(path.clone()).await.unwrap();
        let token = store.issue("d_test", "m_phone").await.unwrap();
        assert!(store.authenticate("d_test", "m_phone", &token).await);

        let serialized = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(!serialized.contains(&token));
        assert!(serialized.contains(&token_hash_hex(&token)));

        let reloaded = CredentialStore::load(path).await.unwrap();
        assert!(reloaded.authenticate("d_test", "m_phone", &token).await);
        assert!(reloaded.revoke("d_test", "m_phone").await.unwrap());
        assert!(!reloaded.authenticate("d_test", "m_phone", &token).await);
    }

    #[tokio::test]
    async fn rejects_wrong_desktop_token_in_constant_time_hash_path() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new(test_config(temp.path().join("credentials.json")))
            .await
            .unwrap();
        assert!(
            state.authenticate_desktop("d_test", "desktop-token-at-least-thirty-two-characters")
        );
        assert!(!state.authenticate_desktop("d_test", "wrong"));

        let wildcard = AppState::new(Config {
            desktop_tokens: HashMap::from([(
                "*".to_string(),
                "shared-token-at-least-thirty-two-characters".to_string(),
            )]),
            ..test_config(temp.path().join("wildcard-credentials.json"))
        })
        .await
        .unwrap();
        assert!(wildcard.authenticate_desktop(
            "d_randomly_generated",
            "shared-token-at-least-thirty-two-characters"
        ));
    }

    #[tokio::test]
    async fn routes_opaque_frames_and_disconnects_a_revoked_mobile() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(
            AppState::new(test_config(temp.path().join("credentials.json")))
                .await
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app(state)).await.unwrap();
        });

        let client = reqwest::Client::new();
        let issued = client
            .post(format!("http://{address}/v1/devices"))
            .bearer_auth("desktop-token-at-least-thirty-two-characters")
            .json(&serde_json::json!({
                "desktop_id": "d_test",
                "device_id": "m_phone"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(issued.status(), StatusCode::OK);
        let issued: IssuedDevice = issued.json().await.unwrap();

        let (mut desktop, _) = connect_async(format!("ws://{address}/v1/ws"))
            .await
            .unwrap();
        desktop
            .send(ClientMessage::Text(
                serde_json::json!({
                    "v": 1,
                    "type": "hello",
                    "role": "desktop",
                    "desktop_id": "d_test",
                    "token": "desktop-token-at-least-thirty-two-characters"
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();

        let (mut mobile, _) = connect_async(format!("ws://{address}/v1/ws"))
            .await
            .unwrap();
        mobile
            .send(ClientMessage::Text(
                serde_json::json!({
                    "v": 1,
                    "type": "hello",
                    "role": "mobile",
                    "desktop_id": "d_test",
                    "device_id": "m_phone",
                    "token": issued.token
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();

        let opaque = serde_json::json!({
            "v": 1,
            "type": "frame",
            "desktop_id": "d_test",
            "device_id": "m_phone",
            "connection_id": "c_test",
            "frame_id": "f_test",
            "seq": 1,
            "ack": 0,
            "nonce": "AAAAAAAAAAAAAAAA",
            "ciphertext": "relay-cannot-read-this"
        })
        .to_string();
        mobile
            .send(ClientMessage::Text(opaque.clone().into()))
            .await
            .unwrap();
        assert_eq!(recv_client_text(&mut desktop).await, opaque);

        let revoked = client
            .delete(format!(
                "http://{address}/v1/devices/m_phone?desktop_id=d_test"
            ))
            .bearer_auth("desktop-token-at-least-thirty-two-characters")
            .send()
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
        let closed = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match mobile.next().await {
                    Some(Ok(ClientMessage::Close(_))) | None => return true,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => return true,
                }
            }
        })
        .await
        .unwrap();
        assert!(closed);

        server.abort();
    }

    async fn recv_client_text<S>(socket: &mut S) -> String
    where
        S: StreamExt<Item = Result<ClientMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        loop {
            match socket.next().await.unwrap().unwrap() {
                ClientMessage::Text(text) => return text.to_string(),
                ClientMessage::Ping(payload) => {
                    let _ = payload;
                }
                _ => {}
            }
        }
    }
}
