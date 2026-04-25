pub mod auth;
pub mod client_owner;
pub mod event_bridge;
pub mod handlers;
pub mod router;
pub mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use sea_orm::{DatabaseConnection, TransactionError, TransactionTrait};
use serde::Serialize;

use crate::app_error::{AppCommandError, AppErrorCode};
use crate::app_state::AppState;
use crate::db::service::app_metadata_service;

const WEB_SERVICE_TOKEN_KEY: &str = "web_service_token";
const WEB_SERVICE_PORT_KEY: &str = "web_service_port";
pub const DEFAULT_WEB_SERVICE_PORT: u16 = 3080;

pub struct WebServerState {
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    port: AtomicU16,
    token: Mutex<String>,
    running: std::sync::atomic::AtomicBool,
}

impl Default for WebServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl WebServerState {
    pub fn new() -> Self {
        Self {
            handle: Mutex::new(None),
            port: AtomicU16::new(0),
            token: Mutex::new(String::new()),
            running: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerInfo {
    pub port: u16,
    pub token: String,
    pub addresses: Vec<String>,
}

pub fn generate_random_token() -> String {
    uuid::Uuid::new_v4().to_string().replace('-', "")
}

/// Resolve the token to use when starting the Web server:
/// 1. use the explicit override if non-empty;
/// 2. fall back to the persisted value in `AppMetadata`;
/// 3. otherwise generate a fresh random token.
async fn resolve_web_service_token(
    conn: &DatabaseConnection,
    override_token: Option<String>,
) -> Result<String, AppCommandError> {
    let trimmed_override = override_token
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(value) = trimmed_override {
        return Ok(value);
    }

    match app_metadata_service::get_value(conn, WEB_SERVICE_TOKEN_KEY)
        .await
        .map_err(AppCommandError::from)?
    {
        Some(saved) if !saved.trim().is_empty() => Ok(saved),
        _ => Ok(generate_random_token()),
    }
}

async fn resolve_web_service_port(
    conn: &DatabaseConnection,
    override_port: Option<u16>,
) -> Result<u16, AppCommandError> {
    if let Some(port) = override_port {
        return Ok(port);
    }
    let saved = app_metadata_service::get_value(conn, WEB_SERVICE_PORT_KEY)
        .await
        .map_err(AppCommandError::from)?;
    let port = saved
        .as_deref()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_WEB_SERVICE_PORT);
    Ok(port)
}

/// Persist token and port atomically so a partial failure cannot leave
/// `app_metadata` in a mixed old/new state.
async fn persist_web_service_config(
    conn: &DatabaseConnection,
    token: &str,
    port: u16,
) -> Result<(), AppCommandError> {
    // Own the values so the inner future is 'static (required by transaction).
    let token_owned = token.to_string();
    let port_str = port.to_string();
    conn.transaction::<_, (), AppCommandError>(move |txn| {
        Box::pin(async move {
            app_metadata_service::upsert_value(txn, WEB_SERVICE_TOKEN_KEY, &token_owned)
                .await
                .map_err(AppCommandError::from)?;
            app_metadata_service::upsert_value(txn, WEB_SERVICE_PORT_KEY, &port_str)
                .await
                .map_err(AppCommandError::from)?;
            Ok(())
        })
    })
    .await
    .map_err(|e: TransactionError<AppCommandError>| match e {
        TransactionError::Connection(db) => {
            AppCommandError::new(AppErrorCode::DatabaseError, "Database transaction failed")
                .with_detail(db.to_string())
        }
        TransactionError::Transaction(inner) => inner,
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServiceConfig {
    pub token: Option<String>,
    pub port: Option<u16>,
}

pub async fn load_web_service_config(
    conn: &DatabaseConnection,
) -> Result<WebServiceConfig, AppCommandError> {
    let token = app_metadata_service::get_value(conn, WEB_SERVICE_TOKEN_KEY)
        .await
        .map_err(AppCommandError::from)?;
    let port = app_metadata_service::get_value(conn, WEB_SERVICE_PORT_KEY)
        .await
        .map_err(AppCommandError::from)?
        .as_deref()
        .and_then(|s| s.trim().parse::<u16>().ok());
    Ok(WebServiceConfig { token, port })
}

/// Stable i18n-key prefixes — the frontend maps these to localized text.
const ERR_ALREADY_RUNNING: &str = "web_server.already_running";
const ERR_INVALID_ADDRESS: &str = "web_server.invalid_address";
const ERR_PORT_IN_USE: &str = "web_server.port_in_use";
const ERR_PERMISSION_DENIED: &str = "web_server.permission_denied";
const ERR_ADDRESS_UNAVAILABLE: &str = "web_server.address_unavailable";
const ERR_BIND_FAILED: &str = "web_server.bind_failed";

fn classify_bind_error(err: std::io::Error) -> AppCommandError {
    use std::io::ErrorKind;
    let (code, key) = match err.kind() {
        ErrorKind::AddrInUse => (AppErrorCode::AlreadyExists, ERR_PORT_IN_USE),
        ErrorKind::PermissionDenied => (AppErrorCode::PermissionDenied, ERR_PERMISSION_DENIED),
        ErrorKind::AddrNotAvailable => (AppErrorCode::InvalidInput, ERR_ADDRESS_UNAVAILABLE),
        _ => (AppErrorCode::IoError, ERR_BIND_FAILED),
    };
    AppCommandError::new(code, key).with_detail(err.to_string())
}

#[cfg(feature = "tauri-runtime")]
pub(crate) fn find_static_dir_tauri(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager;
    // 1. Production: bundle.resources copies out/ → web/ inside the resource directory.
    let resource = app.path().resource_dir().ok();
    if let Some(ref dir) = resource {
        let web = dir.join("web");
        if web.join("index.html").exists() {
            eprintln!(
                "[WEB] Serving static files from resource/web: {}",
                web.display()
            );
            return web;
        }
        // Fallback: files at resource root.
        if dir.join("index.html").exists() {
            eprintln!(
                "[WEB] Serving static files from resource dir: {}",
                dir.display()
            );
            return dir.clone();
        }
    }

    find_static_dir_fallback()
}

pub(crate) fn find_static_dir_fallback() -> PathBuf {
    // Dev mode: "out/" is at the project root, which is one level above src-tauri/.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let project_out = manifest_dir.parent().map(|p| p.join("out"));
    if let Some(ref out) = project_out {
        if out.join("index.html").exists() {
            eprintln!(
                "[WEB] Serving static files from project out/: {}",
                out.display()
            );
            return out.clone();
        }
    }

    // Fallback: current working directory / out
    let cwd_out = std::env::current_dir()
        .map(|d| d.join("out"))
        .unwrap_or_else(|_| PathBuf::from("out"));
    eprintln!(
        "[WEB] Fallback static dir (may not exist): {}",
        cwd_out.display()
    );
    cwd_out
}

pub fn find_static_dir_standalone(explicit: Option<&str>) -> PathBuf {
    if let Some(dir) = explicit {
        let p = PathBuf::from(dir);
        if p.join("index.html").exists() {
            eprintln!(
                "[WEB] Serving static files from CODEG_STATIC_DIR: {}",
                p.display()
            );
            return p;
        }
    }

    // Try ./web/
    let web = PathBuf::from("web");
    if web.join("index.html").exists() {
        eprintln!("[WEB] Serving static files from ./web/: {}", web.display());
        return web;
    }

    find_static_dir_fallback()
}

/// RAII guard that resets `running` back to `false` on drop unless disarmed.
/// Used to guarantee the flag is released on any error during start.
struct RunningGuard<'a> {
    running: &'a std::sync::atomic::AtomicBool,
    armed: bool,
}

impl<'a> RunningGuard<'a> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl<'a> Drop for RunningGuard<'a> {
    fn drop(&mut self) {
        if self.armed {
            self.running.store(false, Ordering::Release);
        }
    }
}

pub fn get_local_addresses(port: u16) -> Vec<String> {
    let mut addrs = vec![format!("http://127.0.0.1:{}", port)];
    // Try to get LAN IPs
    if let Ok(interfaces) = std::net::UdpSocket::bind("0.0.0.0:0") {
        // Connect to a public DNS to determine local IP
        if interfaces.connect("8.8.8.8:80").is_ok() {
            if let Ok(local_addr) = interfaces.local_addr() {
                addrs.push(format!("http://{}:{}", local_addr.ip(), port));
            }
        }
    }
    addrs
}

// ── Core logic (shared by Tauri commands and web handlers) ──

#[allow(dead_code)]
pub(crate) async fn do_start_web_server_with_state(
    app_state: Arc<AppState>,
    static_dir: PathBuf,
    port: Option<u16>,
    host: Option<String>,
    token: Option<String>,
) -> Result<WebServerInfo, AppCommandError> {
    let ws = &app_state.web_server_state;

    // Atomically claim the running flag; concurrent starts see AlreadyExists.
    ws.running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| AppCommandError::new(AppErrorCode::AlreadyExists, ERR_ALREADY_RUNNING))?;
    let mut guard = RunningGuard {
        running: &ws.running,
        armed: true,
    };

    let port = resolve_web_service_port(&app_state.db.conn, port).await?;
    let host = host.unwrap_or_else(|| "0.0.0.0".to_string());
    let token = resolve_web_service_token(&app_state.db.conn, token).await?;
    let auth_enabled = !token.is_empty();

    let build_consistency = crate::build_info::enforce_build_consistency(&static_dir)?;
    app_state
        .runtime_monitor
        .set_build_consistency(build_consistency.clone());
    app_state.runtime_monitor.record(
        if build_consistency.status == "ok" {
            "info"
        } else {
            "warn"
        },
        "web",
        build_consistency.message.clone(),
        None,
    );

    let security = crate::build_info::enforce_startup_security(
        "embedded_web",
        &host,
        auth_enabled,
        Some(&static_dir),
        Some(&app_state.data_dir),
    )?;
    app_state.runtime_monitor.set_security(security.clone());
    if security.insecure && security.override_active {
        app_state.runtime_monitor.record(
            "warn",
            "web",
            "Insecure remote web-server override is active",
            None,
        );
    }

    let addr: SocketAddr =
        format!("{}:{}", host, port)
            .parse()
            .map_err(|e: std::net::AddrParseError| {
                AppCommandError::new(AppErrorCode::InvalidInput, ERR_INVALID_ADDRESS)
                    .with_detail(e.to_string())
            })?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(classify_bind_error)?;

    // Persist only after a successful bind so a failed attempt doesn't overwrite saved state.
    persist_web_service_config(&app_state.db.conn, &token, port).await?;

    let router = router::build_router(app_state.clone(), token.clone(), static_dir);

    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    eprintln!("[WEB] Starting web server on {}", addr);

    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("[WEB] Server error: {}", e);
        }
    });

    *ws.handle.lock().unwrap() = Some(handle);
    ws.port.store(actual_port, Ordering::Relaxed);
    *ws.token.lock().unwrap() = token.clone();
    // running already true from compare_exchange; disarm guard so it doesn't flip back.
    guard.disarm();

    let addresses = get_local_addresses(actual_port);
    Ok(WebServerInfo {
        port: actual_port,
        token,
        addresses,
    })
}

pub(crate) fn do_stop_web_server(state: &WebServerState) {
    if let Some(handle) = state.handle.lock().unwrap().take() {
        handle.abort();
    }
    state.running.store(false, Ordering::Relaxed);
    state.port.store(0, Ordering::Relaxed);
    *state.token.lock().unwrap() = String::new();
    eprintln!("[WEB] Web server stopped");
}

pub(crate) fn do_get_web_server_status(state: &WebServerState) -> Option<WebServerInfo> {
    if !state.running.load(Ordering::Relaxed) {
        return None;
    }
    let port = state.port.load(Ordering::Relaxed);
    let token = state.token.lock().unwrap().clone();
    let addresses = get_local_addresses(port);
    Some(WebServerInfo {
        port,
        token,
        addresses,
    })
}

// ── Tauri commands (thin wrappers) ──

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn start_web_server(
    app: tauri::AppHandle,
    state: tauri::State<'_, WebServerState>,
    port: Option<u16>,
    host: Option<String>,
    token: Option<String>,
) -> Result<WebServerInfo, AppCommandError> {
    // In Tauri mode, we still need to start via the legacy path because
    // the full AppState isn't easily available from tauri::State here.
    // The embedded web server uses Tauri's resource directory for static files.
    use tauri::Manager;

    let ws = &*state;

    // Atomically claim the running flag; concurrent starts see AlreadyExists.
    ws.running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| AppCommandError::new(AppErrorCode::AlreadyExists, ERR_ALREADY_RUNNING))?;
    let mut guard = RunningGuard {
        running: &ws.running,
        armed: true,
    };

    let db = app.state::<crate::db::AppDatabase>();
    let port_val = resolve_web_service_port(&db.conn, port).await?;
    let host_val = host.unwrap_or_else(|| "0.0.0.0".to_string());
    let token = resolve_web_service_token(&db.conn, token).await?;
    let auth_enabled = !token.is_empty();

    let addr: SocketAddr =
        format!("{}:{}", host_val, port_val)
            .parse()
            .map_err(|e: std::net::AddrParseError| {
                AppCommandError::new(AppErrorCode::InvalidInput, ERR_INVALID_ADDRESS)
                    .with_detail(e.to_string())
            })?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(classify_bind_error)?;

    // Persist only after a successful bind so a failed attempt doesn't overwrite saved state.
    persist_web_service_config(&db.conn, &token, port_val).await?;

    let static_dir = find_static_dir_tauri(&app);
    let runtime_monitor = app
        .state::<std::sync::Arc<crate::runtime_monitor::RuntimeMonitor>>()
        .inner()
        .clone();
    let build_consistency = crate::build_info::enforce_build_consistency(&static_dir)?;
    runtime_monitor.set_build_consistency(build_consistency.clone());
    runtime_monitor.record(
        if build_consistency.status == "ok" {
            "info"
        } else {
            "warn"
        },
        "web",
        build_consistency.message.clone(),
        None,
    );

    let data_dir = app.path().app_data_dir().unwrap_or_default();
    let security = crate::build_info::enforce_startup_security(
        "embedded_web",
        &host_val,
        auth_enabled,
        Some(&static_dir),
        Some(&data_dir),
    )?;
    runtime_monitor.set_security(security.clone());
    if security.insecure && security.override_active {
        runtime_monitor.record(
            "warn",
            "web",
            "Insecure remote web-server override is active",
            None,
        );
    }

    // Build AppState for the router
    let app_state = Arc::new(AppState {
        db: crate::db::AppDatabase {
            conn: app.state::<crate::db::AppDatabase>().conn.clone(),
        },
        connection_manager: (*app.state::<crate::acp::manager::ConnectionManager>()).clone_ref(),
        terminal_manager: (*app.state::<crate::terminal::manager::TerminalManager>()).clone_ref(),
        web_client_registry: app
            .state::<Arc<crate::web::client_owner::WebClientRegistry>>()
            .inner()
            .clone(),
        event_broadcaster: app
            .state::<Arc<crate::web::event_bridge::WebEventBroadcaster>>()
            .inner()
            .clone(),
        emitter: crate::web::event_bridge::EventEmitter::Tauri(app.clone()),
        data_dir,
        runtime_monitor,
        web_server_state: WebServerState::new(), // placeholder; not used by handlers
        chat_channel_manager: crate::app_state::default_chat_channel_manager(),
    });

    let router = router::build_router(app_state, token.clone(), static_dir);

    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port_val);
    eprintln!("[WEB] Starting web server on {}", addr);

    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("[WEB] Server error: {}", e);
        }
    });

    *ws.handle.lock().unwrap() = Some(handle);
    ws.port.store(actual_port, Ordering::Relaxed);
    *ws.token.lock().unwrap() = token.clone();
    // running already true from compare_exchange; disarm guard so it doesn't flip back.
    guard.disarm();

    let addresses = get_local_addresses(actual_port);
    Ok(WebServerInfo {
        port: actual_port,
        token,
        addresses,
    })
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn stop_web_server(
    state: tauri::State<'_, WebServerState>,
) -> Result<(), AppCommandError> {
    do_stop_web_server(&state);
    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn get_web_server_status(
    state: tauri::State<'_, WebServerState>,
) -> Result<Option<WebServerInfo>, AppCommandError> {
    Ok(do_get_web_server_status(&state))
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn get_web_service_config(
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<WebServiceConfig, AppCommandError> {
    load_web_service_config(&db.conn).await
}
