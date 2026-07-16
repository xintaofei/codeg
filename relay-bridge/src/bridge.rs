use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot, Mutex, Notify, RwLock};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{header::SEC_WEBSOCKET_PROTOCOL, HeaderValue},
        Message,
    },
};
use tracing::{info, warn};

use crate::{
    config::BridgeConfig,
    crypto::SessionCrypto,
    protocol::{
        valid_command, valid_id, EncryptedPayload, IncomingEnvelope, PairEnvelope, RelayChunk,
        RelayFrame, RelayRequest, PROTOCOL_VERSION,
    },
};

const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(16);
const OUTBOUND_CAPACITY: usize = 256;
const IDEMPOTENCY_CAPACITY: usize = 512;
const DIRECT_PAYLOAD_BYTES: usize = 512 * 1024;
const CHUNK_BYTES: usize = 256 * 1024;
const MAX_CHUNKED_PAYLOAD_BYTES: usize = 128 * 1024 * 1024;
const CHUNK_SEND_INTERVAL: Duration = Duration::from_millis(12);
const MAX_RELAY_TEXT_BYTES: usize = 1024 * 1024;
const MOBILE_CHUNK_TTL: Duration = Duration::from_secs(120);
const MAX_MOBILE_CHUNKS: usize = 512;
const UPLOAD_MAX_BYTES: usize = 2 * 1024 * 1024;
const UPLOAD_MAX_BASE64_BYTES: usize = UPLOAD_MAX_BYTES.div_ceil(3) * 4 + 4;

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<BridgeInner>,
}

struct BridgeInner {
    config: BridgeConfig,
    http: reqwest::Client,
    sessions: RwLock<HashMap<String, Arc<SessionCrypto>>>,
    relay_outbound: RwLock<Option<mpsc::Sender<String>>>,
    local_ws_outbound: RwLock<Option<mpsc::Sender<String>>>,
    local_ready: AtomicBool,
    idempotency: Mutex<IdempotencyCache>,
    in_flight: Mutex<HashMap<String, Arc<CancelSignal>>>,
    mobile_chunks: Mutex<HashMap<(String, String), MobileChunkAssembly>>,
}

#[derive(Default)]
struct CancelSignal {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelSignal {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn cancelled(&self) {
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            let notified = self.notify.notified();
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

struct MobileChunkAssembly {
    request_id: String,
    total: usize,
    total_bytes: usize,
    sha256: String,
    parts: Vec<Vec<u8>>,
    received_bytes: usize,
    expires_at: Instant,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelayUploadAttachment {
    file_name: String,
    mime_type: Option<String>,
    session_id: Option<String>,
    data_base64: String,
}

fn sanitize_upload_file_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| match character {
            '"' | '\\' | '/' => '_',
            other => other,
        })
        .collect();
    let limited: String = cleaned
        .trim_matches(char::is_whitespace)
        .chars()
        .take(255)
        .collect();
    if limited.is_empty() {
        "file".to_owned()
    } else {
        limited
    }
}

#[derive(Clone)]
struct CommandOutcome {
    ok: bool,
    body: Value,
}

fn cancelled_outcome() -> CommandOutcome {
    CommandOutcome {
        ok: false,
        body: json!({
            "code": "request_cancelled",
            "message": "Relay request was cancelled"
        }),
    }
}

#[derive(Default)]
struct IdempotencyCache {
    completed: HashMap<String, CommandOutcome>,
    order: VecDeque<String>,
    in_flight: HashMap<String, Vec<oneshot::Sender<CommandOutcome>>>,
}

enum CacheLookup {
    Cached(CommandOutcome),
    Leader,
    Wait(oneshot::Receiver<CommandOutcome>),
}

impl IdempotencyCache {
    fn begin(&mut self, key: &str) -> CacheLookup {
        if let Some(outcome) = self.completed.get(key) {
            return CacheLookup::Cached(outcome.clone());
        }
        if let Some(waiters) = self.in_flight.get_mut(key) {
            let (sender, receiver) = oneshot::channel();
            waiters.push(sender);
            return CacheLookup::Wait(receiver);
        }
        self.in_flight.insert(key.to_owned(), Vec::new());
        CacheLookup::Leader
    }

    fn complete(&mut self, key: String, outcome: CommandOutcome) {
        if !self.completed.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.completed.insert(key.clone(), outcome.clone());
        while self.order.len() > IDEMPOTENCY_CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.completed.remove(&expired);
            }
        }
        for waiter in self.in_flight.remove(&key).unwrap_or_default() {
            let _ = waiter.send(outcome.clone());
        }
    }
}

impl Bridge {
    pub fn new(config: BridgeConfig) -> anyhow::Result<Self> {
        config.validate()?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .build()
            .context("failed to create local Codeg HTTP client")?;
        Ok(Self {
            inner: Arc::new(BridgeInner {
                config,
                http,
                sessions: RwLock::new(HashMap::new()),
                relay_outbound: RwLock::new(None),
                local_ws_outbound: RwLock::new(None),
                local_ready: AtomicBool::new(false),
                idempotency: Mutex::new(IdempotencyCache::default()),
                in_flight: Mutex::new(HashMap::new()),
                mobile_chunks: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let relay = self.clone();
        let events = self.clone();
        tokio::select! {
            result = relay.run_relay_forever() => result,
            _ = events.run_local_events_forever() => Ok(()),
        }
    }

    async fn run_relay_forever(&self) -> anyhow::Result<()> {
        let mut delay = RECONNECT_MIN;
        loop {
            match self.run_relay_once().await {
                Ok(()) => warn!("Relay connection closed"),
                Err(error) => {
                    warn!(error = %format!("{error:#}"), "Relay connection failed")
                }
            }
            self.inner.sessions.write().await.clear();
            *self.inner.relay_outbound.write().await = None;
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(RECONNECT_MAX);
        }
    }

    async fn run_relay_once(&self) -> anyhow::Result<()> {
        let (socket, _) = connect_async(&self.inner.config.relay_url)
            .await
            .context("failed to connect Relay WebSocket")?;
        let (mut sink, mut stream) = socket.split();
        sink.send(Message::Text(
            json!({
                "v": PROTOCOL_VERSION,
                "type": "hello",
                "role": "desktop",
                "desktop_id": self.inner.config.desktop_id,
                "token": self.inner.config.relay_token
            })
            .to_string()
            .into(),
        ))
        .await
        .context("failed to authenticate Relay WebSocket")?;

        self.inner.sessions.write().await.clear();
        let (sender, mut receiver) = mpsc::channel::<String>(OUTBOUND_CAPACITY);
        *self.inner.relay_outbound.write().await = Some(sender);
        info!(
            desktop_id = %self.inner.config.desktop_id,
            "Relay bridge connected"
        );

        loop {
            tokio::select! {
                outgoing = receiver.recv() => {
                    let Some(outgoing) = outgoing else { break };
                    sink.send(Message::Text(outgoing.into()))
                        .await
                        .context("failed to send Relay frame")?;
                }
                incoming = stream.next() => {
                    let Some(message) = incoming else { break };
                    match message.context("Relay WebSocket read failed")? {
                        Message::Text(text) => self.handle_relay_text(&text).await,
                        Message::Ping(bytes) => sink.send(Message::Pong(bytes)).await?,
                        Message::Pong(_) => {}
                        Message::Close(_) => break,
                        Message::Binary(_) | Message::Frame(_) => {
                            warn!("Relay sent a non-text application frame");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_relay_text(&self, text: &str) {
        let envelope = match serde_json::from_str::<IncomingEnvelope>(text) {
            Ok(envelope) => envelope,
            Err(_) => {
                warn!("Relay message did not match protocol v1");
                return;
            }
        };
        match envelope {
            IncomingEnvelope::Pair(pair) => {
                if let Err(error) = self.handle_mobile_hello(pair).await {
                    warn!(error = %error, "Mobile session handshake rejected");
                }
            }
            IncomingEnvelope::Frame(frame) => {
                if let Err(error) = self.handle_mobile_frame(frame).await {
                    warn!(error = %error, "Encrypted mobile frame rejected");
                }
            }
            IncomingEnvelope::Error { code } => warn!(%code, "Relay routing error"),
            IncomingEnvelope::Other => {}
        }
    }

    async fn handle_mobile_hello(&self, hello: PairEnvelope) -> anyhow::Result<()> {
        if !valid_id(&hello.device_id) || !valid_id(&hello.connection_id) {
            bail!("invalid mobile handshake identifiers");
        }
        let pairing_root = self
            .inner
            .config
            .pairing_root(&hello.device_id)
            .context("device is not paired")?;
        let (session, response) =
            SessionCrypto::from_mobile_hello(&self.inner.config.desktop_id, &pairing_root, &hello)?;
        let session = Arc::new(session);
        self.inner
            .sessions
            .write()
            .await
            .insert(hello.device_id.clone(), session.clone());
        self.send_pair_response(&response).await?;
        if self.inner.local_ready.load(Ordering::Acquire) {
            self.send_encrypted(&session, &json!({"kind": "ready"}))
                .await?;
        }
        info!(device_id = %hello.device_id, "Mobile encrypted session established");
        Ok(())
    }

    async fn send_pair_response(&self, response: &PairEnvelope) -> anyhow::Result<()> {
        let message = json!({
            "v": response.v,
            "type": "pair",
            "phase": response.phase,
            "desktop_id": response.desktop_id,
            "device_id": response.device_id,
            "connection_id": response.connection_id,
            "public_key": response.public_key,
            "proof": response.proof
        });
        self.send_to_relay(message.to_string()).await
    }

    async fn handle_mobile_frame(&self, frame: RelayFrame) -> anyhow::Result<()> {
        if frame.desktop_id != self.inner.config.desktop_id {
            bail!("frame targets another desktop");
        }
        let device_id = frame.device_id.clone();
        let session = self
            .inner
            .sessions
            .read()
            .await
            .get(&frame.device_id)
            .cloned()
            .context("encrypted session is not established")?;
        let plaintext = session.open_mobile_frame(&frame)?;
        let payload: EncryptedPayload =
            serde_json::from_slice(&plaintext).context("invalid encrypted payload")?;
        self.dispatch_mobile_payload(session, &device_id, payload)
            .await
    }

    async fn dispatch_mobile_payload(
        &self,
        session: Arc<SessionCrypto>,
        device_id: &str,
        payload: EncryptedPayload,
    ) -> anyhow::Result<()> {
        match payload {
            EncryptedPayload::Request(request) => {
                self.start_request(session, request).await;
            }
            EncryptedPayload::WsFrame { frame } => {
                self.send_local_ws(frame.to_string()).await?;
            }
            EncryptedPayload::Cancel { request_id } => {
                self.inner
                    .mobile_chunks
                    .lock()
                    .await
                    .retain(|(owner, _), assembly| {
                        owner != device_id || assembly.request_id != request_id
                    });
                if let Some(cancel) = self.inner.in_flight.lock().await.get(&request_id).cloned() {
                    cancel.cancel();
                }
            }
            EncryptedPayload::Chunk(chunk) => {
                self.handle_mobile_chunk(session, device_id, chunk).await?;
            }
            EncryptedPayload::Other => bail!("unsupported encrypted payload kind"),
        }
        Ok(())
    }

    async fn start_request(&self, session: Arc<SessionCrypto>, request: RelayRequest) {
        let cancel = {
            let mut in_flight = self.inner.in_flight.lock().await;
            in_flight
                .entry(request.request_id.clone())
                .or_insert_with(|| Arc::new(CancelSignal::default()))
                .clone()
        };
        let bridge = self.clone();
        tokio::spawn(async move {
            bridge.handle_request(session, request, cancel).await;
        });
    }

    async fn handle_mobile_chunk(
        &self,
        session: Arc<SessionCrypto>,
        device_id: &str,
        chunk: RelayChunk,
    ) -> anyhow::Result<()> {
        if !valid_id(&chunk.chunk_id)
            || !valid_id(&chunk.request_id)
            || !(2..=MAX_MOBILE_CHUNKS).contains(&chunk.total)
            || chunk.index >= chunk.total
            || chunk.total_bytes == 0
            || chunk.total_bytes > MAX_CHUNKED_PAYLOAD_BYTES
        {
            bail!("invalid mobile chunk metadata");
        }
        let checksum = URL_SAFE_NO_PAD
            .decode(&chunk.sha256)
            .context("invalid mobile chunk checksum")?;
        if checksum.len() != 32 {
            bail!("invalid mobile chunk checksum length");
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&chunk.data)
            .context("invalid mobile chunk data")?;
        if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
            bail!("invalid mobile chunk size");
        }

        let key = (device_id.to_owned(), chunk.chunk_id.clone());
        let mut completed = None;
        let next_index;
        {
            let now = Instant::now();
            let mut assemblies = self.inner.mobile_chunks.lock().await;
            assemblies.retain(|_, assembly| assembly.expires_at > now);
            if !assemblies.contains_key(&key) && chunk.index != 0 {
                // The bridge may have restarted after the phone received a
                // previous acknowledgement. Ask it to resume from zero; the
                // phone keeps the same request/idempotency identifiers.
                next_index = 0;
            } else {
                let assembly =
                    assemblies
                        .entry(key.clone())
                        .or_insert_with(|| MobileChunkAssembly {
                            request_id: chunk.request_id.clone(),
                            total: chunk.total,
                            total_bytes: chunk.total_bytes,
                            sha256: chunk.sha256.clone(),
                            parts: Vec::with_capacity(chunk.total),
                            received_bytes: 0,
                            expires_at: now + MOBILE_CHUNK_TTL,
                        });
                if assembly.request_id != chunk.request_id
                    || assembly.total != chunk.total
                    || assembly.total_bytes != chunk.total_bytes
                    || assembly.sha256 != chunk.sha256
                {
                    bail!("inconsistent mobile chunk stream");
                }
                if chunk.index < assembly.parts.len() {
                    if assembly.parts[chunk.index] != bytes {
                        bail!("mobile chunk retry changed payload bytes");
                    }
                } else if chunk.index == assembly.parts.len() {
                    assembly.received_bytes = assembly
                        .received_bytes
                        .checked_add(bytes.len())
                        .context("mobile chunk byte count overflow")?;
                    if assembly.received_bytes > assembly.total_bytes {
                        bail!("mobile chunk stream exceeds declared size");
                    }
                    assembly.parts.push(bytes);
                    assembly.expires_at = now + MOBILE_CHUNK_TTL;
                }
                // A gap means the bridge has fewer durable plaintext chunks
                // than the phone expected. Report the authoritative cursor.
                next_index = assembly.parts.len();
                if next_index == assembly.total {
                    completed = assemblies.remove(&key);
                }
            }
        }

        let completed_request = if let Some(assembly) = completed {
            if assembly.received_bytes != assembly.total_bytes {
                bail!("mobile chunk stream has the wrong final size");
            }
            let mut joined = Vec::with_capacity(assembly.total_bytes);
            for part in assembly.parts {
                joined.extend_from_slice(&part);
            }
            let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(&joined));
            if digest != assembly.sha256 {
                bail!("mobile chunk checksum mismatch");
            }
            let payload: EncryptedPayload =
                serde_json::from_slice(&joined).context("invalid reassembled mobile payload")?;
            match payload {
                EncryptedPayload::Request(request) if request.request_id == assembly.request_id => {
                    Some(request)
                }
                _ => bail!("chunked mobile payload is not the declared request"),
            }
        } else {
            None
        };

        self.send_encrypted(
            &session,
            &json!({
                "kind": "chunk_ack",
                "chunk_id": chunk.chunk_id,
                "next_index": next_index
            }),
        )
        .await?;
        if let Some(request) = completed_request {
            self.start_request(session, request).await;
        }
        Ok(())
    }

    async fn handle_request(
        &self,
        session: Arc<SessionCrypto>,
        request: RelayRequest,
        cancel: Arc<CancelSignal>,
    ) {
        let started = Instant::now();
        let outcome = match self.cached_or_execute(&request, &cancel).await {
            Ok(outcome) => outcome,
            Err(error) => CommandOutcome {
                ok: false,
                body: json!({"code": "bridge_error", "message": error.to_string()}),
            },
        };
        let response = if outcome.ok {
            json!({
                "kind": "response",
                "request_id": request.request_id,
                "ok": true,
                "result": outcome.body
            })
        } else {
            json!({
                "kind": "response",
                "request_id": request.request_id,
                "ok": false,
                "error": outcome.body
            })
        };
        let elapsed = started.elapsed();
        let response_bytes = serde_json::to_vec(&response)
            .map(|bytes| bytes.len())
            .unwrap_or_default();
        if elapsed >= Duration::from_millis(500) || response_bytes > DIRECT_PAYLOAD_BYTES {
            warn!(
                command = %request.command,
                elapsed_ms = elapsed.as_millis(),
                response_bytes,
                "Relay command required slow or chunked handling"
            );
        }
        if let Err(error) = self.send_encrypted(&session, &response).await {
            warn!(error = %error, "Failed to send encrypted command response");
        }
        self.inner
            .in_flight
            .lock()
            .await
            .remove(&request.request_id);
    }

    async fn cached_or_execute(
        &self,
        request: &RelayRequest,
        cancel: &CancelSignal,
    ) -> anyhow::Result<CommandOutcome> {
        if !valid_id(&request.request_id) || !valid_id(&request.idempotency_key) {
            bail!("invalid request identifiers");
        }
        if !valid_command(&request.command) {
            bail!("invalid Codeg command");
        }
        let lookup = self
            .inner
            .idempotency
            .lock()
            .await
            .begin(&request.idempotency_key);
        match lookup {
            CacheLookup::Cached(outcome) => return Ok(outcome),
            CacheLookup::Wait(receiver) => {
                return tokio::select! {
                    outcome = receiver => outcome.context("idempotent request leader was cancelled"),
                    _ = cancel.cancelled() => Ok(cancelled_outcome()),
                }
            }
            CacheLookup::Leader => {}
        }

        let outcome = self.execute_local_command(request, cancel).await;
        self.inner
            .idempotency
            .lock()
            .await
            .complete(request.idempotency_key.clone(), outcome.clone());
        Ok(outcome)
    }

    async fn execute_local_command(
        &self,
        request: &RelayRequest,
        cancel: &CancelSignal,
    ) -> CommandOutcome {
        let timeout =
            Duration::from_millis(request.timeout_ms.unwrap_or(60_000).clamp(1_000, 300_000));
        if request.command == "relay_upload_attachment" {
            return self
                .execute_relay_upload_attachment(request, cancel, timeout)
                .await;
        }
        let endpoint = format!(
            "{}/api/{}",
            self.inner.config.local_url.trim_end_matches('/'),
            request.command
        );
        let request_future = self
            .inner
            .http
            .post(endpoint)
            .bearer_auth(&self.inner.config.codeg_token)
            .json(&request.args)
            .timeout(timeout)
            .send();
        let response = tokio::select! {
            response = request_future => response,
            _ = cancel.cancelled() => return cancelled_outcome(),
        };
        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return CommandOutcome {
                    ok: false,
                    body: json!({"code": "request_timeout", "message": "Local Codeg request timed out"}),
                }
            }
            Err(_) => {
                return CommandOutcome {
                    ok: false,
                    body: json!({"code": "codeg_unreachable", "message": "Local Codeg is unavailable"}),
                }
            }
        };
        let status = response.status();
        let body = response.json::<Value>().await.unwrap_or_else(
            |_| json!({"code": "invalid_response", "message": format!("HTTP {}", status.as_u16())}),
        );
        if status.is_success() {
            CommandOutcome { ok: true, body }
        } else if status == StatusCode::UNAUTHORIZED {
            CommandOutcome {
                ok: false,
                body: json!({"code": "codeg_unauthorized", "message": "Local Codeg token was rejected"}),
            }
        } else {
            CommandOutcome { ok: false, body }
        }
    }

    async fn execute_relay_upload_attachment(
        &self,
        request: &RelayRequest,
        cancel: &CancelSignal,
        timeout: Duration,
    ) -> CommandOutcome {
        let args = match serde_json::from_value::<RelayUploadAttachment>(request.args.clone()) {
            Ok(args) => args,
            Err(_) => {
                return CommandOutcome {
                    ok: false,
                    body: json!({
                        "code": "invalid_upload",
                        "message": "Relay attachment metadata is invalid"
                    }),
                }
            }
        };
        if args.data_base64.len() > UPLOAD_MAX_BASE64_BYTES {
            return CommandOutcome {
                ok: false,
                body: json!({
                    "code": "attachment_too_large",
                    "message": "Attachment exceeds the 2 MiB limit"
                }),
            };
        }
        let bytes = match STANDARD.decode(args.data_base64.as_bytes()) {
            Ok(bytes) if !bytes.is_empty() && bytes.len() <= UPLOAD_MAX_BYTES => bytes,
            _ => {
                return CommandOutcome {
                    ok: false,
                    body: json!({
                        "code": "invalid_upload",
                        "message": "Relay attachment payload is invalid"
                    }),
                }
            }
        };
        let mime = args
            .mime_type
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        let part = match reqwest::multipart::Part::bytes(bytes)
            .file_name(sanitize_upload_file_name(&args.file_name))
            .mime_str(&mime)
        {
            Ok(part) => part,
            Err(_) => {
                return CommandOutcome {
                    ok: false,
                    body: json!({
                        "code": "invalid_upload",
                        "message": "Attachment MIME type is invalid"
                    }),
                }
            }
        };
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if let Some(session_id) = args.session_id.filter(|value| !value.is_empty()) {
            form = form.text("session_id", session_id);
        }
        let endpoint = format!(
            "{}/api/upload_attachment",
            self.inner.config.local_url.trim_end_matches('/')
        );
        let request_future = self
            .inner
            .http
            .post(endpoint)
            .bearer_auth(&self.inner.config.codeg_token)
            .multipart(form)
            .timeout(timeout)
            .send();
        let response = tokio::select! {
            response = request_future => response,
            _ = cancel.cancelled() => return cancelled_outcome(),
        };
        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return CommandOutcome {
                    ok: false,
                    body: json!({"code": "request_timeout", "message": "Attachment upload timed out"}),
                }
            }
            Err(_) => {
                return CommandOutcome {
                    ok: false,
                    body: json!({"code": "codeg_unreachable", "message": "Local Codeg is unavailable"}),
                }
            }
        };
        let status = response.status();
        let body = response.json::<Value>().await.unwrap_or_else(
            |_| json!({"code": "invalid_response", "message": format!("HTTP {}", status.as_u16())}),
        );
        if status.is_success() {
            CommandOutcome { ok: true, body }
        } else if status == StatusCode::UNAUTHORIZED {
            CommandOutcome {
                ok: false,
                body: json!({"code": "codeg_unauthorized", "message": "Local Codeg token was rejected"}),
            }
        } else {
            CommandOutcome { ok: false, body }
        }
    }

    async fn run_local_events_forever(&self) {
        let mut delay = RECONNECT_MIN;
        loop {
            match self.run_local_events_once().await {
                Ok(()) => warn!("Local Codeg event stream closed"),
                Err(error) => warn!(error = %error, "Local Codeg event stream failed"),
            }
            *self.inner.local_ws_outbound.write().await = None;
            self.inner.local_ready.store(false, Ordering::Release);
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(RECONNECT_MAX);
        }
    }

    async fn run_local_events_once(&self) -> anyhow::Result<()> {
        let mut url = url::Url::parse(&self.inner.config.local_url)?;
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|_| anyhow::anyhow!("failed to create local WebSocket URL"))?;
        url.set_path("/ws/events");
        url.set_query(None);
        url.set_fragment(None);

        let encoded_token = URL_SAFE_NO_PAD.encode(self.inner.config.codeg_token.as_bytes());
        let mut request = url.as_str().into_client_request()?;
        request.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_str(&format!("codeg-events, codeg-token.{encoded_token}"))?,
        );
        let (socket, _) = connect_async(request)
            .await
            .context("failed to connect local Codeg event WebSocket")?;
        let (mut sink, mut stream) = socket.split();
        let (sender, mut receiver) = mpsc::channel::<String>(OUTBOUND_CAPACITY);
        *self.inner.local_ws_outbound.write().await = Some(sender);
        info!("Local Codeg event stream connected");

        loop {
            tokio::select! {
                outgoing = receiver.recv() => {
                    let Some(outgoing) = outgoing else { break };
                    sink.send(Message::Text(outgoing.into())).await?;
                }
                incoming = stream.next() => {
                    let Some(message) = incoming else { break };
                    match message? {
                        Message::Text(text) => self.forward_local_ws_frame(&text).await,
                        Message::Ping(bytes) => sink.send(Message::Pong(bytes)).await?,
                        Message::Pong(_) => {}
                        Message::Close(_) => break,
                        Message::Binary(_) | Message::Frame(_) => {
                            warn!("Local Codeg sent a non-text event frame");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn forward_local_ws_frame(&self, text: &str) {
        let Ok(frame) = serde_json::from_str::<Value>(text) else {
            warn!("Local Codeg sent invalid JSON event frame");
            return;
        };
        if text.len() > 512 * 1024 {
            warn!(
                bytes = text.len(),
                channel = frame
                    .get("channel")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown"),
                frame_type = frame
                    .get("type")
                    .and_then(|value| value.as_str())
                    .or_else(|| {
                        frame
                            .get("payload")
                            .and_then(|payload| payload.get("type"))
                            .and_then(|value| value.as_str())
                    })
                    .unwrap_or("unknown"),
                "Local Codeg emitted a large event frame"
            );
        }
        let payload = if frame.get("channel").and_then(Value::as_str) == Some("__ready__") {
            self.inner.local_ready.store(true, Ordering::Release);
            json!({"kind": "ready"})
        } else {
            json!({"kind": "ws_frame", "frame": frame})
        };
        let sessions = self
            .inner
            .sessions
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            if let Err(error) = self.send_encrypted(&session, &payload).await {
                warn!(error = %error, "Failed to forward encrypted Codeg event");
            }
        }
    }

    async fn send_encrypted(&self, session: &SessionCrypto, payload: &Value) -> anyhow::Result<()> {
        // Command responses and event forwarding run concurrently. Protect
        // sequence allocation plus queue insertion as one operation so frames
        // cannot reach the mobile out of sequence.
        let _send_guard = session.lock_send().await;
        let plaintext = serde_json::to_vec(payload)?;
        if plaintext.len() <= DIRECT_PAYLOAD_BYTES {
            return self.send_encrypted_bytes(session, &plaintext).await;
        }
        if plaintext.len() > MAX_CHUNKED_PAYLOAD_BYTES {
            bail!(
                "encrypted payload exceeds the {} byte reassembly limit",
                MAX_CHUNKED_PAYLOAD_BYTES
            );
        }

        let chunk_id = format!("ch_{}", uuid::Uuid::new_v4().simple());
        let total = plaintext.len().div_ceil(CHUNK_BYTES);
        let checksum = URL_SAFE_NO_PAD.encode(Sha256::digest(&plaintext));
        for (index, chunk) in plaintext.chunks(CHUNK_BYTES).enumerate() {
            let envelope = json!({
                "kind": "chunk",
                "chunk_id": chunk_id,
                "index": index,
                "total": total,
                "total_bytes": plaintext.len(),
                "sha256": checksum,
                "data": URL_SAFE_NO_PAD.encode(chunk)
            });
            self.send_encrypted_bytes(session, &serde_json::to_vec(&envelope)?)
                .await?;
            if index + 1 < total {
                // Relay enforces a per-socket frame rate. Pacing large payloads
                // keeps chunks below that bound while allowing other peers to
                // remain responsive.
                tokio::time::sleep(CHUNK_SEND_INTERVAL).await;
            }
        }
        Ok(())
    }

    async fn send_encrypted_bytes(
        &self,
        session: &SessionCrypto,
        plaintext: &[u8],
    ) -> anyhow::Result<()> {
        let frame = session.seal_desktop_payload(&self.inner.config.desktop_id, plaintext)?;
        let encoded = serde_json::to_string(&frame)?;
        if encoded.len() > MAX_RELAY_TEXT_BYTES {
            bail!("encrypted frame exceeds the Relay text frame limit");
        }
        self.send_to_relay(encoded).await
    }

    async fn send_to_relay(&self, message: String) -> anyhow::Result<()> {
        let sender = self
            .inner
            .relay_outbound
            .read()
            .await
            .clone()
            .context("Relay is disconnected")?;
        sender
            .send(message)
            .await
            .map_err(|_| anyhow::anyhow!("Relay connection closed"))
    }

    async fn send_local_ws(&self, message: String) -> anyhow::Result<()> {
        let sender = self
            .inner
            .local_ws_outbound
            .read()
            .await
            .clone()
            .context("Local Codeg event stream is disconnected")?;
        sender
            .send(message)
            .await
            .map_err(|_| anyhow::anyhow!("Local Codeg event stream closed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Multipart, State},
        http::{HeaderMap, StatusCode as AxumStatusCode},
        routing::post,
        Json, Router,
    };
    use std::sync::atomic::AtomicUsize;

    type CapturedUpload = Option<(String, Vec<u8>)>;

    #[derive(Clone)]
    struct UploadCapture {
        count: Arc<AtomicUsize>,
        result: Arc<Mutex<CapturedUpload>>,
    }

    async fn capture_upload(
        State(capture): State<UploadCapture>,
        headers: HeaderMap,
        mut multipart: Multipart,
    ) -> Result<Json<Value>, AxumStatusCode> {
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            != Some("Bearer local-secret")
        {
            return Err(AxumStatusCode::UNAUTHORIZED);
        }
        let mut session_id = String::new();
        let mut bytes = Vec::new();
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|_| AxumStatusCode::BAD_REQUEST)?
        {
            match field.name() {
                Some("file") => {
                    bytes = field
                        .bytes()
                        .await
                        .map_err(|_| AxumStatusCode::BAD_REQUEST)?
                        .to_vec();
                }
                Some("session_id") => {
                    session_id = field
                        .text()
                        .await
                        .map_err(|_| AxumStatusCode::BAD_REQUEST)?;
                }
                _ => {}
            }
        }
        capture.count.fetch_add(1, Ordering::AcqRel);
        *capture.result.lock().await = Some((session_id, bytes));
        Ok(Json(json!({"path": "/tmp/codeg-upload/test.bin"})))
    }

    async fn slow_upload() -> Json<Value> {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Json(json!({"path": "/tmp/codeg-upload/too-late.bin"}))
    }

    async fn local_server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("listener should have an address");
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should run");
        });
        (format!("http://{address}"), task)
    }

    fn test_bridge(local_url: String) -> Bridge {
        Bridge::new(BridgeConfig {
            relay_url: "wss://relay.example.test/v1/ws".into(),
            desktop_id: "d_test".into(),
            relay_token: "r".repeat(32),
            local_url,
            codeg_token: "local-secret".into(),
            devices: HashMap::from([(
                "m_phone".into(),
                crate::config::DeviceConfig {
                    pairing_root: URL_SAFE_NO_PAD.encode([7_u8; 32]),
                },
            )]),
        })
        .expect("test bridge config should be valid")
    }

    fn upload_request(idempotency_key: &str, bytes: &[u8]) -> RelayRequest {
        RelayRequest {
            request_id: format!("req_{idempotency_key}"),
            command: "relay_upload_attachment".into(),
            args: json!({
                "fileName": "test.bin",
                "mimeType": "application/octet-stream",
                "sessionId": "session-test",
                "dataBase64": STANDARD.encode(bytes),
            }),
            idempotency_key: idempotency_key.into(),
            timeout_ms: Some(2_000),
        }
    }

    #[tokio::test]
    async fn cancel_signal_wakes_current_and_late_waiters() {
        let signal = Arc::new(CancelSignal::default());
        let waiter = {
            let signal = signal.clone();
            tokio::spawn(async move { signal.cancelled().await })
        };
        signal.cancel();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("current waiter should wake")
            .expect("waiter task should finish");
        tokio::time::timeout(Duration::from_secs(1), signal.cancelled())
            .await
            .expect("late waiter should observe cancellation");
    }

    #[test]
    fn idempotency_cache_fans_out_and_evicts() {
        let mut cache = IdempotencyCache::default();
        assert!(matches!(cache.begin("key"), CacheLookup::Leader));
        let waiter = match cache.begin("key") {
            CacheLookup::Wait(waiter) => waiter,
            _ => panic!("duplicate in-flight request must wait"),
        };
        let outcome = CommandOutcome {
            ok: true,
            body: json!({"value": 7}),
        };
        cache.complete("key".into(), outcome.clone());
        assert_eq!(waiter.blocking_recv().unwrap().body, outcome.body);
        assert!(matches!(cache.begin("key"), CacheLookup::Cached(_)));

        for index in 0..=IDEMPOTENCY_CAPACITY {
            let key = format!("key-{index}");
            assert!(matches!(cache.begin(&key), CacheLookup::Leader));
            cache.complete(key, outcome.clone());
        }
        assert!(!cache.completed.contains_key("key"));
    }

    #[tokio::test]
    async fn relay_attachment_upload_is_multipart_and_idempotent() {
        let capture = UploadCapture {
            count: Arc::new(AtomicUsize::new(0)),
            result: Arc::new(Mutex::new(None)),
        };
        let router = Router::new()
            .route("/api/upload_attachment", post(capture_upload))
            .with_state(capture.clone());
        let (local_url, server) = local_server(router).await;
        let bridge = test_bridge(local_url);
        let payload = vec![0x5a; 700_000];

        let first = bridge
            .cached_or_execute(
                &upload_request("upload_once", &payload),
                &CancelSignal::default(),
            )
            .await
            .expect("first upload should execute");
        let second = bridge
            .cached_or_execute(
                &upload_request("upload_once", &payload),
                &CancelSignal::default(),
            )
            .await
            .expect("retry should use the cached result");

        assert!(first.ok);
        assert_eq!(first.body, json!({"path": "/tmp/codeg-upload/test.bin"}));
        assert!(second.ok);
        assert_eq!(capture.count.load(Ordering::Acquire), 1);
        let captured = capture
            .result
            .lock()
            .await
            .take()
            .expect("server should capture one upload");
        assert_eq!(captured.0, "session-test");
        assert_eq!(captured.1, payload);
        server.abort();
    }

    #[tokio::test]
    async fn relay_attachment_upload_can_be_cancelled() {
        let router = Router::new().route("/api/upload_attachment", post(slow_upload));
        let (local_url, server) = local_server(router).await;
        let bridge = test_bridge(local_url);
        let signal = Arc::new(CancelSignal::default());
        let task = {
            let bridge = bridge.clone();
            let signal = signal.clone();
            tokio::spawn(async move {
                bridge
                    .cached_or_execute(&upload_request("cancel_me", &[1; 1024]), &signal)
                    .await
                    .expect("cancelled upload should return a protocol outcome")
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        signal.cancel();
        let outcome = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancellation should not wait for the HTTP timeout")
            .expect("upload task should finish");
        assert!(!outcome.ok);
        assert_eq!(outcome.body["code"], "request_cancelled");
        server.abort();
    }
}
