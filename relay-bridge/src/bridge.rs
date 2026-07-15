use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{bail, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
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
        valid_command, valid_id, EncryptedPayload, IncomingEnvelope, PairEnvelope, RelayFrame,
        RelayRequest, PROTOCOL_VERSION,
    },
};

const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(16);
const OUTBOUND_CAPACITY: usize = 256;
const IDEMPOTENCY_CAPACITY: usize = 512;

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
}

#[derive(Clone)]
struct CommandOutcome {
    ok: bool,
    body: Value,
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
            }),
        })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let events = self.clone();
        tokio::spawn(async move {
            events.run_local_events_forever().await;
        });

        let mut delay = RECONNECT_MIN;
        loop {
            match self.run_relay_once().await {
                Ok(()) => warn!("Relay connection closed"),
                Err(error) => warn!(error = %error, "Relay connection failed"),
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
        match payload {
            EncryptedPayload::Request(request) => {
                let bridge = self.clone();
                tokio::spawn(async move {
                    bridge.handle_request(session, request).await;
                });
            }
            EncryptedPayload::WsFrame { frame } => {
                self.send_local_ws(frame.to_string()).await?;
            }
            EncryptedPayload::Cancel { request_id } => {
                let response = json!({
                    "kind": "response",
                    "request_id": request_id,
                    "ok": false,
                    "error": {"code": "cancel_not_supported", "message": "Request cancellation is not available"}
                });
                self.send_encrypted(&session, &response).await?;
            }
            EncryptedPayload::Other => bail!("unsupported encrypted payload kind"),
        }
        Ok(())
    }

    async fn handle_request(&self, session: Arc<SessionCrypto>, request: RelayRequest) {
        let outcome = match self.cached_or_execute(&request).await {
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
        if let Err(error) = self.send_encrypted(&session, &response).await {
            warn!(error = %error, "Failed to send encrypted command response");
        }
    }

    async fn cached_or_execute(&self, request: &RelayRequest) -> anyhow::Result<CommandOutcome> {
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
                return receiver
                    .await
                    .context("idempotent request leader was cancelled")
            }
            CacheLookup::Leader => {}
        }

        let outcome = self.execute_local_command(request).await;
        self.inner
            .idempotency
            .lock()
            .await
            .complete(request.idempotency_key.clone(), outcome.clone());
        Ok(outcome)
    }

    async fn execute_local_command(&self, request: &RelayRequest) -> CommandOutcome {
        let endpoint = format!(
            "{}/api/{}",
            self.inner.config.local_url.trim_end_matches('/'),
            request.command
        );
        let timeout =
            Duration::from_millis(request.timeout_ms.unwrap_or(60_000).clamp(1_000, 300_000));
        let response = self
            .inner
            .http
            .post(endpoint)
            .bearer_auth(&self.inner.config.codeg_token)
            .json(&request.args)
            .timeout(timeout)
            .send()
            .await;
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
        let plaintext = serde_json::to_vec(payload)?;
        let frame = session.seal_desktop_payload(&self.inner.config.desktop_id, &plaintext)?;
        self.send_to_relay(serde_json::to_string(&frame)?).await
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
}
