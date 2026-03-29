pub mod auth;
pub mod event_bridge;
pub mod handlers;
pub mod router;
pub mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::app_error::{AppCommandError, AppErrorCode};
use crate::app_state::AppState;

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

#[cfg(feature = "tauri-runtime")]
pub(crate) fn find_static_dir_tauri(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager;
    // 1. Production: bundle.resources copies out/ → web/ inside the resource directory.
    let resource = app.path().resource_dir().ok();
    if let Some(ref dir) = resource {
        let web = dir.join("web");
        if web.join("index.html").exists() {
            eprintln!("[WEB] Serving static files from resource/web: {}", web.display());
            return web;
        }
        // Fallback: files at resource root.
        if dir.join("index.html").exists() {
            eprintln!("[WEB] Serving static files from resource dir: {}", dir.display());
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
            eprintln!("[WEB] Serving static files from project out/: {}", out.display());
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
            eprintln!("[WEB] Serving static files from CODEG_STATIC_DIR: {}", p.display());
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
) -> Result<WebServerInfo, AppCommandError> {
    let ws = &app_state.web_server_state;
    if ws.running.load(Ordering::Relaxed) {
        return Err(AppCommandError::new(
            AppErrorCode::AlreadyExists,
            "Web server is already running",
        ));
    }

    let port = port.unwrap_or(3080);
    let host = host.unwrap_or_else(|| "0.0.0.0".to_string());
    let token = generate_random_token();

    let router = router::build_router(app_state.clone(), token.clone(), static_dir);

    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .map_err(|e: std::net::AddrParseError| {
            AppCommandError::invalid_input("Invalid host/port").with_detail(e.to_string())
        })?;

    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        AppCommandError::io_error("Failed to bind address").with_detail(e.to_string())
    })?;

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
    ws.running.store(true, Ordering::Relaxed);

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
) -> Result<WebServerInfo, AppCommandError> {
    // In Tauri mode, we still need to start via the legacy path because
    // the full AppState isn't easily available from tauri::State here.
    // The embedded web server uses Tauri's resource directory for static files.
    use tauri::Manager;

    let ws = &*state;
    if ws.running.load(Ordering::Relaxed) {
        return Err(AppCommandError::new(
            AppErrorCode::AlreadyExists,
            "Web server is already running",
        ));
    }

    let port_val = port.unwrap_or(3080);
    let host_val = host.unwrap_or_else(|| "0.0.0.0".to_string());
    let token = generate_random_token();

    let static_dir = find_static_dir_tauri(&app);

    // Build AppState for the router
    let app_state = Arc::new(AppState {
        db: crate::db::AppDatabase {
            conn: app.state::<crate::db::AppDatabase>().conn.clone(),
        },
        connection_manager: (*app.state::<crate::acp::manager::ConnectionManager>()).clone_ref(),
        terminal_manager: (*app.state::<crate::terminal::manager::TerminalManager>()).clone_ref(),
        event_broadcaster: app.state::<Arc<crate::web::event_bridge::WebEventBroadcaster>>().inner().clone(),
        emitter: crate::web::event_bridge::EventEmitter::Tauri(app.clone()),
        data_dir: app.path().app_data_dir().unwrap_or_default(),
        web_server_state: WebServerState::new(), // placeholder; not used by handlers
    });

    let router = router::build_router(app_state, token.clone(), static_dir);

    let addr: SocketAddr = format!("{}:{}", host_val, port_val)
        .parse()
        .map_err(|e: std::net::AddrParseError| {
            AppCommandError::invalid_input("Invalid host/port").with_detail(e.to_string())
        })?;

    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        AppCommandError::io_error("Failed to bind address").with_detail(e.to_string())
    })?;

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
    ws.running.store(true, Ordering::Relaxed);

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
