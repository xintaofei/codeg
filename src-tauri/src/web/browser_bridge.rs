//! Web-mode port bridge: shows a dev server that runs on the codeg host
//! inside the workbench when the workbench itself runs in a browser.
//!
//! In server / Docker deployments an agent's `http://localhost:3000` means the
//! *server's* loopback, which the user's browser cannot reach. The bridge
//! listens on extra ports next to codeg's own and forwards each of them to one
//! loopback port on the host, so the workbench can put the page in an iframe.
//!
//! ## One listener per target port — why not one shared listener
//!
//! A page served through a shared listener under a path prefix
//! (`/{cap}/{port}/…`) breaks as soon as it uses root-absolute URLs, which
//! every module-based dev server does (`/src/main.tsx`, `/_next/…`,
//! `/@vite/client`). Those requests arrive without the prefix and nothing in
//! them says which port they belong to: an iframe without `allow-same-origin`
//! runs in an opaque origin, which sends no `Referer` and attaches no cookies
//! to module scripts, `fetch` or XHR. Giving the frame `allow-same-origin`
//! would let two proxied pages read each other. So each target port gets its
//! own origin (its own bridge port), the frame may keep its origin, and the
//! page is served at `/` exactly as it would be on the host: no rewriting of
//! bodies, `history.pushState` routers work, HMR websockets connect.
//!
//! ## Authentication: a same-site cookie, not codeg's token
//!
//! An iframe navigation cannot carry a bearer header, and the page's own
//! requests must not carry codeg's token either. The workbench asks the API
//! (with the token) for a grant; the grant is a random capability tied to one
//! listener. The frame first loads the listener's entry URL carrying that
//! capability, which sets an `HttpOnly; SameSite=Lax` cookie named after the
//! bridge port and redirects to the page. Every later request — documents,
//! modules, fetch, websocket upgrades — carries the cookie: the bridge is a
//! different port on the same host as the workbench, which is the same *site*,
//! so browsers treat those cookies as first-party (including Safari). The
//! cookie is sent to codeg's own port too, where nothing reads cookies; the
//! workbench's own cookies (locale preferences) reach the bridge the same way
//! and are stripped before a request goes on to the dev server.
//!
//! Cookies ignore ports, so a page on one bridge port could make the browser
//! attach another listener's cookie to a request it sends there. Two things
//! keep the listeners apart: the entry answers with a small page that
//! navigates itself to the target (so the first document request, like every
//! request the page makes afterwards, is same-origin with the listener), and
//! every other request must be same-origin — `Sec-Fetch-Site: same-origin`
//! (or `none`, a navigation the user typed), or, for a browser without Fetch
//! Metadata, an `Origin` on the listener's own port. A request one proxied
//! page aims at another listener is `same-site`, and is refused.
//!
//! A capability stays valid for the life of its listener. A listener lives
//! while a workbench tab holds it, closes a minute after the last hold is
//! released, and closes after two hours without a request even when held (a
//! browser tab closed without notice); reopening the page mints a new grant.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use axum::body::{Body, HttpBody};
use axum::extract::ws::{CloseFrame, Message as DownMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Path as AxumPath, RawQuery, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as UpCloseFrame;
use tokio_tungstenite::tungstenite::Message as UpMessage;

/// Entry URL prefix on a bridge listener: `/__codeg_bridge/enter/{cap}?to=/path`.
pub const ENTER_PREFIX: &str = "/__codeg_bridge/enter/";
/// Unauthenticated reachability probe the workbench calls before showing
/// the frame, so an unmapped port is reported instead of a blank frame.
pub const PING_PATH: &str = "/__codeg_bridge/ping";
const COOKIE_PREFIX: &str = "codeg-bridge-";
/// The workbench's own cookies (locale preferences) live on the same host
/// and are not the dev server's business either.
const WORKBENCH_COOKIE_PREFIX: &str = "codeg.";
/// Ports above codeg's own that the bridge takes when `CODEG_BRIDGE_PORTS`
/// is not set.
pub const DEFAULT_POOL_SIZE: u16 = 10;
/// A listener nobody holds closes after this long without a request.
const UNHELD_IDLE: Duration = Duration::from_secs(60);
/// A held listener closes after this long without a request.
const HELD_IDLE: Duration = Duration::from_secs(2 * 60 * 60);
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// How long a closing listener waits for its connections before they are cut.
const CLOSE_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    /// Address the listeners bind to: the same one codeg's API listener uses.
    pub bind_host: String,
    /// Ports a listener may take, in order of preference; `0` means any free
    /// port (each listener its own).
    pub ports: Vec<u16>,
    /// Hostname the browser should use for the bridge when it differs from
    /// the one the workbench was loaded from (a reverse proxy in front).
    pub public_host: Option<String>,
    /// Ports the bridge refuses to forward to: codeg's own listener.
    pub reserved: Vec<u16>,
}

impl BridgeConfig {
    /// Read `CODEG_BRIDGE_PORTS` / `CODEG_BRIDGE_PUBLIC_HOST`. `None` when the
    /// bridge is switched off (`CODEG_BRIDGE_PORTS=off`).
    pub fn from_env(bind_host: &str, codeg_port: u16) -> Option<Self> {
        let ports = match std::env::var("CODEG_BRIDGE_PORTS") {
            Ok(raw) => parse_ports(&raw, codeg_port)?,
            Err(_) => default_ports(codeg_port),
        };
        let public_host = std::env::var("CODEG_BRIDGE_PUBLIC_HOST")
            .ok()
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty());
        Some(Self {
            bind_host: bind_host.to_string(),
            ports,
            public_host,
            reserved: vec![codeg_port],
        })
    }
}

/// The ten ports after codeg's own, stopping at the end of the port space.
pub fn default_ports(codeg_port: u16) -> Vec<u16> {
    (1..=DEFAULT_POOL_SIZE)
        .filter_map(|offset| codeg_port.checked_add(offset))
        .collect()
}

/// `3081-3090`, `3081,3082,3090`, a mix of both, `auto` (any free port), or
/// `off` / `none` / `disabled` / empty (no bridge; returns `None`). Codeg's
/// own port is never part of the pool. An unparseable value is `None` too:
/// a typo must not silently bind ten ports the operator did not choose.
pub fn parse_ports(raw: &str, codeg_port: u16) -> Option<Vec<u16>> {
    let raw = raw.trim();
    if raw.is_empty() || matches!(raw.to_ascii_lowercase().as_str(), "off" | "none" | "disabled") {
        return None;
    }
    if raw.eq_ignore_ascii_case("auto") {
        return Some(vec![0]);
    }
    let mut ports = Vec::new();
    for item in raw.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let range = match item.split_once('-') {
            Some((lo, hi)) => (lo.trim().parse::<u16>().ok()?, hi.trim().parse::<u16>().ok()?),
            None => {
                let port = item.parse::<u16>().ok()?;
                (port, port)
            }
        };
        if range.0 == 0 || range.1 < range.0 {
            return None;
        }
        for port in range.0..=range.1 {
            if port != codeg_port && !ports.contains(&port) {
                ports.push(port);
            }
        }
    }
    if ports.is_empty() {
        None
    } else {
        Some(ports)
    }
}

/// What `browser_bridge_status` tells the workbench.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    pub enabled: bool,
    /// Ports a listener may take (`0` = any free port).
    pub ports: Vec<u16>,
    pub public_host: Option<String>,
}

/// A tab's ticket into one listener.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeGrant {
    pub target_port: u16,
    pub bridge_port: u16,
    /// Path on the bridge origin that sets the cookie and redirects to the
    /// page (`?to=/path` chooses where).
    pub entry_path: String,
    pub public_host: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("the port bridge is off on this server")]
    Disabled,
    #[error("port {0} is codeg's own listener")]
    Reserved(u16),
    #[error("no bridge port is free: {0}")]
    NoPort(String),
}

struct Listener {
    target_port: u16,
    bridge_port: u16,
    caps: Mutex<Vec<String>>,
    /// Workbench tabs holding this listener open.
    holds: Mutex<HashSet<String>>,
    last_seen: Mutex<Instant>,
    shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Listener {
    fn has_cap(&self, cap: &str) -> bool {
        let caps = lock(&self.caps);
        caps.iter().any(|known| constant_time_eq(known.as_bytes(), cap.as_bytes()))
    }

    fn touch(&self) {
        *lock(&self.last_seen) = Instant::now();
    }

    fn grant(&self, tab_id: &str, public_host: Option<String>) -> BridgeGrant {
        let cap = uuid::Uuid::new_v4().simple().to_string();
        lock(&self.caps).push(cap.clone());
        lock(&self.holds).insert(tab_id.to_string());
        self.touch();
        BridgeGrant {
            target_port: self.target_port,
            bridge_port: self.bridge_port,
            entry_path: format!("{ENTER_PREFIX}{cap}"),
            public_host,
        }
    }

    fn cookie_name(&self) -> String {
        format!("{COOKIE_PREFIX}{}", self.bridge_port)
    }

    /// Stop accepting; connections still open get `CLOSE_GRACE`, then are cut.
    fn close(&self) {
        if let Some(tx) = lock(&self.shutdown).take() {
            let _ = tx.send(());
        }
        if let Some(task) = lock(&self.task).take() {
            if tokio::runtime::Handle::try_current().is_ok() {
                tokio::spawn(async move {
                    let abort = task.abort_handle();
                    if tokio::time::timeout(CLOSE_GRACE, task).await.is_err() {
                        abort.abort();
                    }
                });
            } else {
                task.abort();
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

struct Bridge {
    config: Mutex<Option<BridgeConfig>>,
    /// Bumped by every `configure`, so an `open` that started under an
    /// earlier configuration cannot publish a listener after the bridge was
    /// switched off (or re-pointed) while it was binding.
    generation: AtomicU64,
    /// Live listeners by target port.
    listeners: Mutex<HashMap<u16, Arc<Listener>>>,
}

static BRIDGE: LazyLock<Bridge> = LazyLock::new(|| Bridge {
    config: Mutex::new(None),
    generation: AtomicU64::new(0),
    listeners: Mutex::new(HashMap::new()),
});

static SWEEPER: OnceLock<()> = OnceLock::new();

/// Set (or, with `None`, switch off) the bridge. Switching off closes every
/// listener; changing the configuration keeps the listeners already bound.
pub fn configure(config: Option<BridgeConfig>) {
    let off = config.is_none();
    {
        let mut current = lock(&BRIDGE.config);
        *current = config;
        BRIDGE.generation.fetch_add(1, Ordering::AcqRel);
    }
    if off {
        shutdown_all();
    }
}

pub fn status() -> BridgeStatus {
    match lock(&BRIDGE.config).as_ref() {
        Some(config) => BridgeStatus {
            enabled: true,
            ports: config.ports.clone(),
            public_host: config.public_host.clone(),
        },
        None => BridgeStatus {
            enabled: false,
            ports: Vec::new(),
            public_host: None,
        },
    }
}

/// Number of live listeners.
pub fn listener_count() -> usize {
    lock(&BRIDGE.listeners).len()
}

/// Let `tab_id` reach `127.0.0.1:{target_port}` through a bridge listener,
/// binding one when the port has none yet.
pub async fn open(target_port: u16, tab_id: &str) -> Result<BridgeGrant, BridgeError> {
    let (config, generation) = {
        let config = lock(&BRIDGE.config);
        let generation = BRIDGE.generation.load(Ordering::Acquire);
        (config.clone().ok_or(BridgeError::Disabled)?, generation)
    };
    if config.reserved.contains(&target_port) {
        return Err(BridgeError::Reserved(target_port));
    }
    if let Some(existing) = lock(&BRIDGE.listeners).get(&target_port).cloned() {
        return Ok(existing.grant(tab_id, config.public_host.clone()));
    }

    let used: HashSet<u16> = lock(&BRIDGE.listeners).values().map(|l| l.bridge_port).collect();
    let mut last_error = String::from("no ports configured");
    for &port in &config.ports {
        if port != 0 && used.contains(&port) {
            continue;
        }
        let socket = match bind(&config.bind_host, port).await {
            Ok(socket) => socket,
            Err(err) => {
                last_error = format!("{}:{port}: {err}", config.bind_host);
                continue;
            }
        };
        let bridge_port = socket.local_addr().map(|a| a.port()).unwrap_or(port);
        let listener = Arc::new(Listener {
            target_port,
            bridge_port,
            caps: Mutex::new(Vec::new()),
            holds: Mutex::new(HashSet::new()),
            last_seen: Mutex::new(Instant::now()),
            shutdown: Mutex::new(None),
            task: Mutex::new(None),
        });
        // Another task may have bound this target while we were binding, and
        // the bridge may have been switched off: a listener is published only
        // under the configuration it was opened for.
        let winner = {
            let config_now = lock(&BRIDGE.config);
            if config_now.is_none() || BRIDGE.generation.load(Ordering::Acquire) != generation {
                return Err(BridgeError::Disabled);
            }
            let mut listeners = lock(&BRIDGE.listeners);
            match listeners.get(&target_port) {
                Some(existing) => existing.clone(),
                None => {
                    listeners.insert(target_port, listener.clone());
                    listener.clone()
                }
            }
        };
        if Arc::ptr_eq(&winner, &listener) {
            serve(socket, listener.clone());
            SWEEPER.get_or_init(|| {
                tokio::spawn(sweep_task());
            });
            tracing::info!(
                "[bridge] port {bridge_port} now forwards to 127.0.0.1:{target_port}"
            );
        }
        return Ok(winner.grant(tab_id, config.public_host.clone()));
    }
    Err(BridgeError::NoPort(last_error))
}

/// `tab_id` no longer needs any listener.
pub fn close(tab_id: &str) {
    for listener in lock(&BRIDGE.listeners).values() {
        lock(&listener.holds).remove(tab_id);
    }
}

/// Close listeners nobody has used for a while; returns how many closed.
pub fn sweep(now: Instant) -> usize {
    let stale: Vec<Arc<Listener>> = {
        let mut listeners = lock(&BRIDGE.listeners);
        let stale: Vec<u16> = listeners
            .values()
            .filter(|l| {
                let idle = now.saturating_duration_since(*lock(&l.last_seen));
                let held = !lock(&l.holds).is_empty();
                idle >= if held { HELD_IDLE } else { UNHELD_IDLE }
            })
            .map(|l| l.target_port)
            .collect();
        stale.iter().filter_map(|port| listeners.remove(port)).collect()
    };
    for listener in &stale {
        tracing::info!(
            "[bridge] port {} closed (127.0.0.1:{} idle)",
            listener.bridge_port,
            listener.target_port
        );
        listener.close();
    }
    stale.len()
}

pub fn shutdown_all() {
    let all: Vec<Arc<Listener>> = lock(&BRIDGE.listeners).drain().map(|(_, l)| l).collect();
    for listener in all {
        listener.close();
    }
}

async fn sweep_task() {
    loop {
        tokio::time::sleep(SWEEP_INTERVAL).await;
        sweep(Instant::now());
    }
}

/// Binds like the API listener does: an IP literal (bare or bracketed IPv6)
/// or a hostname such as `localhost`, resolved here.
async fn bind(host: &str, port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    let socket = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => tokio::net::TcpListener::bind(SocketAddr::new(ip, port)).await?,
        Err(_) => tokio::net::TcpListener::bind((host, port)).await?,
    };
    if let Err(err) = super::socket_inherit::mark_listener_non_inheritable(&socket) {
        tracing::warn!("[bridge] failed to mark listener non-inheritable: {err}");
    }
    Ok(socket)
}

fn serve(socket: tokio::net::TcpListener, listener: Arc<Listener>) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let router = Router::new()
        .route(PING_PATH, get(ping))
        .route("/__codeg_bridge/enter/{cap}", get(enter))
        .fallback(any(forward))
        .with_state(listener.clone());
    let task = tokio::spawn(async move {
        let serve = axum::serve(socket, router).with_graceful_shutdown(async move {
            let _ = rx.await;
        });
        if let Err(err) = serve.await {
            tracing::error!("[bridge] listener error: {err}");
        }
    });
    *lock(&listener.shutdown) = Some(tx);
    *lock(&listener.task) = Some(task);
}

// ─── Listener routes ───────────────────────────────────────────────────

async fn ping() -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (header::CACHE_CONTROL, "no-store"),
        ],
    )
        .into_response()
}

async fn enter(
    State(listener): State<Arc<Listener>>,
    AxumPath(cap): AxumPath<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if !listener.has_cap(&cap) {
        return forbidden_page();
    }
    listener.touch();
    let to = query
        .as_deref()
        .and_then(|q| query_param(q, "to"))
        .filter(|to| is_local_path(to))
        .unwrap_or_else(|| "/".to_string());
    let secure = if forwarded_https(&headers) { "; Secure" } else { "" };
    let cookie = format!(
        "{}={cap}; Path=/; HttpOnly; SameSite=Lax{secure}",
        listener.cookie_name()
    );
    // Not a redirect: a redirected request keeps the workbench as its
    // initiator and arrives `same-site`, which is exactly what `forward`
    // refuses. A page that navigates itself makes the next request
    // same-origin with this listener.
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::SET_COOKIE, cookie)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bounce_page(&to)))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// The entry's answer: a page whose only job is to go to `to` on this
/// origin, by script or, failing that, by meta refresh. Replaces itself in
/// the history, so the frame's back button does not return here.
fn bounce_page(to: &str) -> String {
    let attribute = escape_html(to);
    let script = json_for_script(to);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"0;url={attribute}\"><title>codeg</title></head>\
         <body><script>location.replace({script})</script></body></html>"
    )
}

async fn forward(State(listener): State<Arc<Listener>>, request: Request) -> Response {
    let (mut parts, body) = request.into_parts();
    if parts.uri.path().starts_with("/__codeg_bridge/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let cookie_name = listener.cookie_name();
    let presented = cookie_value(&parts.headers, &cookie_name);
    if !presented.is_some_and(|cap| listener.has_cap(&cap)) {
        return forbidden_page();
    }
    if !same_origin_initiator(&parts.headers, listener.bridge_port) {
        return cross_origin_page();
    }
    listener.touch();
    if is_websocket_upgrade(&parts.headers) {
        return proxy_websocket(&listener, &mut parts).await;
    }
    proxy_http(&listener, parts, body).await
}

// ─── HTTP forwarding ───────────────────────────────────────────────────

/// Never follows redirects (the browser must see them), never decodes bodies
/// (they pass through byte for byte, `Content-Length` intact), never goes
/// through a proxy, and has no overall timeout: a dev server's SSE / long
/// poll stays open as long as the page wants.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("failed to build the bridge client")
});

/// Request headers that describe this connection, not the request, plus the
/// ones the bridge sets itself.
fn drop_request_header(name: &str) -> bool {
    matches!(
        name,
        "host"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "expect"
            | "content-length"
            | "cookie"
            | "origin"
            | "referer"
    )
}

/// Response headers that must not reach the browser: connection-level ones,
/// and `X-Frame-Options` — the page is being shown in the user's own workbench
/// on purpose (a CSP `frame-ancestors` directive goes the same way, see
/// `without_frame_ancestors`).
fn drop_response_header(name: &str) -> bool {
    matches!(
        name,
        "connection" | "keep-alive" | "transfer-encoding" | "trailer" | "upgrade" | "x-frame-options"
    )
}

async fn proxy_http(listener: &Listener, parts: Parts, body: Body) -> Response {
    let target = listener.target_port;
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let upstream = format!("http://127.0.0.1:{target}{path_and_query}");
    let mut builder = CLIENT.request(parts.method.clone(), &upstream);
    for (name, value) in parts.headers.iter() {
        if !drop_request_header(name.as_str()) {
            builder = builder.header(name, value);
        }
    }
    for (name, value) in rewritten_request_headers(&parts.headers, target) {
        builder = builder.header(name, value);
    }
    // Anything the browser sent as a body goes on as a stream (chunked
    // upstream when the length is unknown); a body-less GET stays body-less.
    if !body.is_end_stream() {
        builder = builder.body(reqwest::Body::wrap_stream(body.into_data_stream()));
    }

    let response = match builder.send().await {
        Ok(response) => response,
        Err(err) => return bad_gateway_page(target, &err),
    };

    let mut out = Response::builder().status(response.status().as_u16());
    for (name, value) in response.headers().iter() {
        if drop_response_header(name.as_str()) {
            continue;
        }
        if name == header::LOCATION {
            if let Some(rewritten) = rewrite_location(value, target) {
                out = out.header(name, rewritten);
                continue;
            }
        }
        if name == header::CONTENT_SECURITY_POLICY
            || name == header::CONTENT_SECURITY_POLICY_REPORT_ONLY
        {
            if let Some(kept) = without_frame_ancestors(value) {
                out = out.header(name, kept);
            }
            continue;
        }
        out = out.header(name, value);
    }
    out.body(Body::from_stream(response.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `Cookie` without the bridge's own cookies, and `Origin` / `Referer`
/// pointing at the target as the page would if it ran there directly — dev
/// servers compare them with `Host` before they answer a websocket or a
/// module request.
fn rewritten_request_headers(headers: &HeaderMap, target: u16) -> Vec<(HeaderName, HeaderValue)> {
    let mut out = Vec::new();
    let cookies = foreign_cookies(headers);
    if !cookies.is_empty() {
        if let Ok(value) = HeaderValue::from_str(&cookies) {
            out.push((header::COOKIE, value));
        }
    }
    if headers.contains_key(header::ORIGIN) {
        if let Ok(value) = HeaderValue::from_str(&format!("http://127.0.0.1:{target}")) {
            out.push((header::ORIGIN, value));
        }
    }
    if let Some(referer) = headers.get(header::REFERER).and_then(|v| v.to_str().ok()) {
        if let Ok(url) = reqwest::Url::parse(referer) {
            let mut rewritten = format!("http://127.0.0.1:{target}{}", url.path());
            if let Some(query) = url.query() {
                rewritten.push('?');
                rewritten.push_str(query);
            }
            if let Ok(value) = HeaderValue::from_str(&rewritten) {
                out.push((header::REFERER, value));
            }
        }
    }
    out
}

/// An absolute `Location` on the target itself becomes a path on the bridge
/// origin; anything else (another host, a relative path) passes unchanged.
fn rewrite_location(value: &HeaderValue, target: u16) -> Option<HeaderValue> {
    let raw = value.to_str().ok()?;
    let url = reqwest::Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    if !is_loopback_host(host) || url.port_or_known_default() != Some(target) {
        return None;
    }
    let mut path = url.path().to_string();
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        path.push('#');
        path.push_str(fragment);
    }
    HeaderValue::from_str(&path).ok()
}

// ─── WebSocket forwarding ──────────────────────────────────────────────

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    let upgrade = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let connection = headers
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|part| part.trim().eq_ignore_ascii_case("upgrade")));
    upgrade && connection
}

async fn proxy_websocket(listener: &Listener, parts: &mut Parts) -> Response {
    let target = listener.target_port;
    let upgrade = match WebSocketUpgrade::from_request_parts(parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let mut request = match format!("ws://127.0.0.1:{target}{path_and_query}").into_client_request()
    {
        Ok(request) => request,
        Err(err) => return bad_gateway_page(target, &err),
    };
    for name in [header::SEC_WEBSOCKET_PROTOCOL, header::USER_AGENT, header::ACCEPT_LANGUAGE] {
        if let Some(value) = parts.headers.get(&name) {
            request.headers_mut().insert(name, value.clone());
        }
    }
    for (name, value) in rewritten_request_headers(&parts.headers, target) {
        request.headers_mut().insert(name, value);
    }

    let (upstream, response) = match tokio_tungstenite::connect_async(request).await {
        Ok(connected) => connected,
        Err(err) => return bad_gateway_page(target, &err),
    };
    let mut upgrade = upgrade;
    if let Some(protocol) = response
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
    {
        upgrade = upgrade.protocols([protocol.to_string()]);
    }
    upgrade.on_upgrade(move |socket| pump(socket, upstream))
}

async fn pump(
    downstream: WebSocket,
    upstream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let (mut down_tx, mut down_rx) = downstream.split();
    let (mut up_tx, mut up_rx) = upstream.split();
    let to_upstream = async {
        while let Some(Ok(message)) = down_rx.next().await {
            let close = matches!(message, DownMessage::Close(_));
            if up_tx.send(downstream_to_upstream(message)).await.is_err() || close {
                break;
            }
        }
        let _ = up_tx.close().await;
    };
    let to_downstream = async {
        while let Some(Ok(message)) = up_rx.next().await {
            let Some(message) = upstream_to_downstream(message) else {
                continue;
            };
            let close = matches!(message, DownMessage::Close(_));
            if down_tx.send(message).await.is_err() || close {
                break;
            }
        }
        let _ = down_tx.close().await;
    };
    tokio::select! {
        _ = to_upstream => {}
        _ = to_downstream => {}
    }
}

fn downstream_to_upstream(message: DownMessage) -> UpMessage {
    match message {
        DownMessage::Text(text) => UpMessage::Text(text.as_str().into()),
        DownMessage::Binary(bytes) => UpMessage::Binary(bytes),
        DownMessage::Ping(bytes) => UpMessage::Ping(bytes),
        DownMessage::Pong(bytes) => UpMessage::Pong(bytes),
        DownMessage::Close(frame) => UpMessage::Close(frame.map(|f| UpCloseFrame {
            code: f.code.into(),
            reason: f.reason.as_str().into(),
        })),
    }
}

fn upstream_to_downstream(message: UpMessage) -> Option<DownMessage> {
    Some(match message {
        UpMessage::Text(text) => DownMessage::Text(text.as_str().into()),
        UpMessage::Binary(bytes) => DownMessage::Binary(bytes),
        UpMessage::Ping(bytes) => DownMessage::Ping(bytes),
        UpMessage::Pong(bytes) => DownMessage::Pong(bytes),
        UpMessage::Close(frame) => DownMessage::Close(frame.map(|f| CloseFrame {
            code: u16::from(f.code),
            reason: f.reason.as_str().into(),
        })),
        UpMessage::Frame(_) => return None,
    })
}

// ─── Small helpers ─────────────────────────────────────────────────────

fn query_param(raw: &str, key: &str) -> Option<String> {
    raw.split('&').find_map(|segment| {
        let (name, value) = segment.split_once('=').unwrap_or((segment, ""));
        if name != key {
            return None;
        }
        Some(
            urlencoding::decode(value)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| value.to_string()),
        )
    })
}

/// A path the entry may redirect to: root-relative on this origin only, so
/// the entry URL cannot be used to send the browser somewhere else.
fn is_local_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.starts_with("/\\")
        && !path.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Whether the browser says this request comes from the listener's own
/// page. `Sec-Fetch-Site` is set by the browser and cannot be forged by a
/// page: `same-origin` is the page itself, `none` a navigation the user
/// typed; `same-site` is another port on this host — a different proxied
/// page, or the workbench, neither of which may talk to the dev server
/// directly — and `cross-site` is anyone else. Without Fetch Metadata (older
/// browsers, non-browser clients) an `Origin`, when present, must name this
/// listener's port; websocket handshakes always carry one.
fn same_origin_initiator(headers: &HeaderMap, bridge_port: u16) -> bool {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return matches!(site.trim(), "same-origin" | "none");
    }
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(origin) => origin_port(origin) == Some(bridge_port),
        None => true,
    }
}

/// The port an `Origin` header names, explicit or the scheme's default.
fn origin_port(origin: &str) -> Option<u16> {
    let url = reqwest::Url::parse(origin.trim()).ok()?;
    url.port_or_known_default()
}

/// A CSP without its `frame-ancestors` directive; `None` when nothing else
/// was in it (the header is then dropped).
fn without_frame_ancestors(value: &HeaderValue) -> Option<HeaderValue> {
    let raw = value.to_str().ok()?;
    let kept: Vec<&str> = raw
        .split(';')
        .map(str::trim)
        .filter(|directive| {
            !directive.is_empty()
                && !directive
                    .split_whitespace()
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("frame-ancestors"))
        })
        .collect();
    if kept.is_empty() {
        return None;
    }
    HeaderValue::from_str(&kept.join("; ")).ok()
}

fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').next().is_some_and(|p| p.trim().eq_ignore_ascii_case("https")))
}

fn cookie_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|line| line.split(';'))
        .filter_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    cookie_pairs(headers)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
}

/// The page's own cookies, re-serialized without the bridge's and without
/// the workbench's (both share the host with the page).
fn foreign_cookies(headers: &HeaderMap) -> String {
    cookie_pairs(headers)
        .into_iter()
        .filter(|(name, _)| {
            !name.starts_with(COOKIE_PREFIX) && !name.starts_with(WORKBENCH_COOKIE_PREFIX)
        })
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// `localhost`, `*.localhost`, `127/8`, `::1` and the unspecified addresses,
/// which a browser connects to as local.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if host == "::1" || host == "::" || host == "0.0.0.0" {
        return true;
    }
    let host = host.strip_prefix("::ffff:").unwrap_or(&host);
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip.octets()[0] == 127,
        Err(_) => false,
    }
}

fn html_page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{font:14px/1.5 system-ui,sans-serif;color:#333;margin:0;padding:32px 24px;\
         background:#fafafa}}h1{{font-size:16px;margin:0 0 8px}}p{{margin:0;max-width:52ch}}</style>\
         </head><body><h1>{title}</h1><p>{body}</p></body></html>"
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(html))
        .unwrap_or_else(|_| status.into_response())
}

fn forbidden_page() -> Response {
    html_page(
        StatusCode::FORBIDDEN,
        "This preview is no longer valid",
        "Reopen the page from codeg to start a new preview session.",
    )
}

fn cross_origin_page() -> Response {
    html_page(
        StatusCode::FORBIDDEN,
        "This request did not come from the page itself",
        "A dev server shown through codeg only answers its own page. Open the address from codeg to view it.",
    )
}

fn bad_gateway_page(target: u16, err: &dyn std::fmt::Display) -> Response {
    let detail = escape_html(&err.to_string());
    html_page(
        StatusCode::BAD_GATEWAY,
        &format!("codeg cannot reach port {target} on its host"),
        &format!("Is the server still running there? Reload to try again. ({detail})"),
    )
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A JSON string literal safe inside a `<script>` block: `<`, `>` and `&`
/// become escapes so no `</script>` (or comment opener) can end the block.
fn json_for_script(text: &str) -> String {
    serde_json::to_string(text)
        .unwrap_or_else(|_| "\"/\"".to_string())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_pools_parse_ranges_lists_and_switches() {
        assert_eq!(parse_ports("3081-3083", 3080), Some(vec![3081, 3082, 3083]));
        assert_eq!(parse_ports("3081, 3090,3081", 3080), Some(vec![3081, 3090]));
        assert_eq!(parse_ports("3079-3082", 3080), Some(vec![3079, 3081, 3082]));
        assert_eq!(parse_ports("auto", 3080), Some(vec![0]));
        assert_eq!(parse_ports("AUTO", 3080), Some(vec![0]));
        for off in ["", "  ", "off", "none", "Disabled"] {
            assert_eq!(parse_ports(off, 3080), None, "{off:?}");
        }
        // A typo must not turn into a default pool.
        assert_eq!(parse_ports("3081-", 3080), None);
        assert_eq!(parse_ports("abc", 3080), None);
        assert_eq!(parse_ports("3090-3081", 3080), None);
        assert_eq!(parse_ports("0-3", 3080), None);
        // Only codeg's own port: nothing left.
        assert_eq!(parse_ports("3080", 3080), None);
    }

    #[test]
    fn default_pool_is_the_ten_ports_above_codeg() {
        assert_eq!(default_ports(3080), (3081..=3090).collect::<Vec<_>>());
        assert_eq!(default_ports(65533), vec![65534, 65535]);
    }

    #[test]
    fn entry_redirects_stay_on_this_origin() {
        assert!(is_local_path("/"));
        assert!(is_local_path("/docs?x=1#top"));
        assert!(!is_local_path(""));
        assert!(!is_local_path("//evil.example/"));
        assert!(!is_local_path("/\\evil.example/"));
        assert!(!is_local_path("http://evil.example/"));
        assert!(!is_local_path("/a\r\nSet-Cookie: x=y"));
        assert!(!is_local_path("/with space"));
    }

    #[test]
    fn cookies_are_read_and_filtered() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("a=1; codeg-bridge-3081=cap-one; b=2"),
        );
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("codeg-bridge-3082=cap-two; codeg.locale=zh-CN"),
        );
        assert_eq!(cookie_value(&headers, "codeg-bridge-3081").as_deref(), Some("cap-one"));
        assert_eq!(cookie_value(&headers, "codeg-bridge-3082").as_deref(), Some("cap-two"));
        assert_eq!(cookie_value(&headers, "codeg-bridge-3083"), None);
        assert_eq!(foreign_cookies(&headers), "a=1; b=2");
        assert_eq!(foreign_cookies(&HeaderMap::new()), "");
    }

    #[test]
    fn origin_and_referer_point_at_the_target() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://codeg.example:3081"));
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("http://codeg.example:3081/app/page?tab=2"),
        );
        headers.insert(header::COOKIE, HeaderValue::from_static("codeg-bridge-3081=c; sid=9"));
        let rewritten = rewritten_request_headers(&headers, 3000);
        let get = |name: HeaderName| {
            rewritten
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_str().unwrap().to_string())
        };
        assert_eq!(get(header::ORIGIN).as_deref(), Some("http://127.0.0.1:3000"));
        assert_eq!(
            get(header::REFERER).as_deref(),
            Some("http://127.0.0.1:3000/app/page?tab=2")
        );
        assert_eq!(get(header::COOKIE).as_deref(), Some("sid=9"));

        // `Origin: null` (a sandboxed frame) is rewritten too; no Origin at
        // all stays absent.
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("null"));
        let rewritten = rewritten_request_headers(&headers, 3000);
        assert_eq!(rewritten.len(), 1);
        assert!(rewritten_request_headers(&HeaderMap::new(), 3000).is_empty());
    }

    #[test]
    fn location_on_the_target_becomes_a_path() {
        let rewrite = |raw: &str| {
            rewrite_location(&HeaderValue::from_str(raw).unwrap(), 3000)
                .map(|v| v.to_str().unwrap().to_string())
        };
        assert_eq!(rewrite("http://127.0.0.1:3000/login?next=%2F").as_deref(), Some("/login?next=%2F"));
        assert_eq!(rewrite("http://localhost:3000/a#b").as_deref(), Some("/a#b"));
        assert_eq!(rewrite("http://[::1]:3000/").as_deref(), Some("/"));
        assert_eq!(rewrite("http://0.0.0.0:3000/x").as_deref(), Some("/x"));
        // Another port, another host, a relative path: untouched.
        assert_eq!(rewrite("http://127.0.0.1:3001/"), None);
        assert_eq!(rewrite("https://example.com/"), None);
        assert_eq!(rewrite("/relative"), None);
        assert_eq!(rewrite("login"), None);
    }

    #[test]
    fn loopback_hosts() {
        for host in ["localhost", "LOCALHOST", "app.localhost", "127.0.0.1", "127.1.2.3", "::1", "[::1]", "0.0.0.0", "::", "::ffff:127.0.0.1"] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in ["example.com", "10.0.0.1", "192.168.1.5", "128.0.0.1", "localhost.evil", "fe80::1", ""] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn header_filters() {
        for name in ["host", "connection", "cookie", "origin", "referer", "content-length", "upgrade", "expect"] {
            assert!(drop_request_header(name), "{name}");
        }
        for name in ["accept", "authorization", "content-type", "accept-encoding", "sec-fetch-site", "x-requested-with"] {
            assert!(!drop_request_header(name), "{name}");
        }
        for name in ["x-frame-options", "connection", "transfer-encoding"] {
            assert!(drop_response_header(name), "{name}");
        }
        for name in ["content-type", "content-length", "content-encoding", "set-cookie", "location", "content-security-policy", "cache-control"] {
            assert!(!drop_response_header(name), "{name}");
        }
    }

    #[test]
    fn websocket_upgrade_detection() {
        let mut headers = HeaderMap::new();
        assert!(!is_websocket_upgrade(&headers));
        headers.insert(header::UPGRADE, HeaderValue::from_static("WebSocket"));
        assert!(!is_websocket_upgrade(&headers));
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        assert!(is_websocket_upgrade(&headers));
    }

    #[test]
    fn status_and_wire_names() {
        let json = serde_json::to_value(BridgeGrant {
            target_port: 3000,
            bridge_port: 3081,
            entry_path: "/__codeg_bridge/enter/abc".into(),
            public_host: None,
        })
        .unwrap();
        assert_eq!(json["bridgePort"], 3081);
        assert_eq!(json["entryPath"], "/__codeg_bridge/enter/abc");
        assert!(json["publicHost"].is_null());
    }

    #[test]
    fn only_the_listeners_own_page_may_ask() {
        let with = |name: &str, value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(HeaderName::from_bytes(name.as_bytes()).unwrap(), HeaderValue::from_static(value));
            headers
        };
        assert!(same_origin_initiator(&with("sec-fetch-site", "same-origin"), 3081));
        assert!(same_origin_initiator(&with("sec-fetch-site", "none"), 3081));
        // Another port on this host: another proxied page or the workbench.
        assert!(!same_origin_initiator(&with("sec-fetch-site", "same-site"), 3081));
        assert!(!same_origin_initiator(&with("sec-fetch-site", "cross-site"), 3081));
        // Fetch Metadata wins over Origin when both are there.
        let mut both = with("sec-fetch-site", "same-site");
        both.insert(header::ORIGIN, HeaderValue::from_static("http://h:3081"));
        assert!(!same_origin_initiator(&both, 3081));
        // Without it, the Origin's port decides; no Origin at all passes.
        assert!(same_origin_initiator(&with("origin", "http://h:3081"), 3081));
        assert!(same_origin_initiator(&with("origin", "https://codeg.example:3081"), 3081));
        assert!(!same_origin_initiator(&with("origin", "http://h:3082"), 3081));
        assert!(!same_origin_initiator(&with("origin", "http://h"), 3081));
        assert!(!same_origin_initiator(&with("origin", "null"), 3081));
        assert!(same_origin_initiator(&HeaderMap::new(), 3081));
        assert_eq!(origin_port("https://h"), Some(443));
        assert_eq!(origin_port("http://h"), Some(80));
        assert_eq!(origin_port("http://[::1]:3081"), Some(3081));
    }

    #[test]
    fn bounce_page_goes_to_the_path_and_escapes_it() {
        let page = bounce_page("/docs?x=1#top");
        assert!(page.contains("content=\"0;url=/docs?x=1#top\""));
        assert!(page.contains("location.replace(\"/docs?x=1#top\")"));
        let hostile = bounce_page("/a\"><script>alert(1)</script>");
        // The attribute is entity-escaped, the script literal cannot close
        // the block: exactly one `</script>` remains, the page's own.
        assert!(!hostile.contains("<script>alert"));
        assert!(hostile.contains("url=/a&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;\""));
        assert!(hostile.contains("location.replace(\"/a\\\"\\u003e\\u003cscript\\u003ealert(1)\\u003c/script\\u003e\")"));
        assert_eq!(hostile.matches("</script>").count(), 1);
    }

    #[test]
    fn frame_ancestors_is_dropped_from_a_csp() {
        let strip = |raw: &'static str| {
            without_frame_ancestors(&HeaderValue::from_static(raw)).map(|v| v.to_str().unwrap().to_string())
        };
        assert_eq!(
            strip("default-src 'self'; frame-ancestors 'none'; img-src *").as_deref(),
            Some("default-src 'self'; img-src *")
        );
        assert_eq!(strip("FRAME-ANCESTORS 'self'"), None);
        assert_eq!(strip("default-src 'self'").as_deref(), Some("default-src 'self'"));
        // A source expression that merely contains the word stays.
        assert_eq!(
            strip("img-src https://frame-ancestors.example").as_deref(),
            Some("img-src https://frame-ancestors.example")
        );
    }

    #[test]
    fn forwarded_proto_marks_secure_cookies() {
        let mut headers = HeaderMap::new();
        assert!(!forwarded_https(&headers));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https, http"));
        assert!(forwarded_https(&headers));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
        assert!(!forwarded_https(&headers));
    }
}
