use std::path::PathBuf;
use std::sync::Arc;

use codeg_lib::app_state::AppState;
use codeg_lib::build_info;
use codeg_lib::runtime_monitor::RuntimeMonitor;
use codeg_lib::web::client_owner::WebClientRegistry;
use codeg_lib::web::event_bridge::{EventEmitter, WebEventBroadcaster};
use codeg_lib::web::{
    find_static_dir_standalone, generate_random_token, get_local_addresses, WebServerState,
};

fn main() {
    // Support --version flag
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }

    // PATH initialisation MUST happen before the tokio runtime is created.
    // std::env::set_var is not thread-safe (unsafe in Rust edition 2024);
    // #[tokio::main] would spawn worker threads before we reach this point.
    codeg_lib::process::ensure_node_in_path();
    codeg_lib::process::ensure_user_npm_prefix_in_path();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime")
        .block_on(async_main());
}

async fn async_main() {
    let port: u16 = std::env::var("CODEG_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3080);
    let host = std::env::var("CODEG_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let disable_auth = std::env::var("CODEG_DISABLE_AUTH")
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let token = if disable_auth {
        String::new()
    } else {
        std::env::var("CODEG_TOKEN").unwrap_or_else(|_| generate_random_token())
    };
    let data_dir = std::env::var("CODEG_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_data_dir());
    let static_dir_env = std::env::var("CODEG_STATIC_DIR").ok();

    let static_dir = find_static_dir_standalone(static_dir_env.as_deref());
    let app_version = env!("CARGO_PKG_VERSION");
    let runtime_monitor = Arc::new(RuntimeMonitor::new());

    let build_consistency = match build_info::enforce_build_consistency(&static_dir) {
        Ok(info) => info,
        Err(error) => {
            eprintln!("[SERVER] {}", error);
            if let Some(detail) = error.detail {
                eprintln!("[SERVER] {detail}");
            }
            std::process::exit(1);
        }
    };
    runtime_monitor.set_build_consistency(build_consistency.clone());
    runtime_monitor.record(
        if build_consistency.status == "ok" {
            "info"
        } else {
            "warn"
        },
        "startup",
        build_consistency.message.clone(),
        None,
    );

    let security_info = match build_info::enforce_startup_security(
        "standalone",
        &host,
        !disable_auth,
        Some(&static_dir),
        Some(&data_dir),
    ) {
        Ok(info) => info,
        Err(error) => {
            eprintln!("[SERVER] {}", error);
            if let Some(detail) = error.detail {
                eprintln!("[SERVER] {detail}");
            }
            std::process::exit(1);
        }
    };
    runtime_monitor.set_security(security_info.clone());
    if security_info.insecure && security_info.override_active {
        runtime_monitor.record(
            "warn",
            "startup",
            "Insecure remote startup override is active",
            None,
        );
    }

    eprintln!("[SERVER] codeg-server v{}", app_version);
    eprintln!("[SERVER] Data directory: {}", data_dir.display());
    eprintln!("[SERVER] Static directory: {}", static_dir.display());

    // Initialize database
    let db = codeg_lib::db::init_database(&data_dir, app_version)
        .await
        .expect("Failed to initialize database");

    // Create shared broadcaster
    let broadcaster = Arc::new(WebEventBroadcaster::new());
    let emitter = EventEmitter::WebOnly(broadcaster.clone());
    let connection_manager = codeg_lib::app_state::default_connection_manager();
    let web_client_registry = Arc::new(WebClientRegistry::new());
    connection_manager.attach_runtime_monitor(runtime_monitor.clone());
    connection_manager.start_orphan_watchdog(web_client_registry.clone());

    // Build AppState
    let state = Arc::new(AppState {
        db,
        connection_manager,
        terminal_manager: codeg_lib::app_state::default_terminal_manager(),
        web_client_registry,
        event_broadcaster: broadcaster,
        emitter,
        data_dir,
        runtime_monitor,
        web_server_state: WebServerState::new(),
        chat_channel_manager: codeg_lib::app_state::default_chat_channel_manager(),
        task_tracker: tokio_util::task::TaskTracker::new(),
    });

    // Install bundled expert skills into the central store
    // (`~/.codeg/skills/`). Runs in the background; failures are logged
    // but non-fatal.
    tokio::spawn(async move {
        let report = codeg_lib::commands::experts::ensure_central_experts_installed().await;
        if !report.errors.is_empty() {
            eprintln!(
                "[Experts] install finished with {} error(s): {:?}",
                report.errors.len(),
                report.errors
            );
        } else {
            eprintln!(
                "[Experts] install ok: installed={} updated={} pending_review={}",
                report.installed_count,
                report.updated_count,
                report.pending_user_review.len()
            );
        }
    });

    // Start chat channel background tasks (event subscriber, command dispatcher, scheduler, auto-connect)
    state
        .chat_channel_manager
        .start_background(
            state.event_broadcaster.clone(),
            state.db.conn.clone(),
            state.connection_manager.clone_ref(),
            state.emitter.clone(),
        )
        .await;

    // Build router
    let router = codeg_lib::web::router::build_router(state, token.clone(), static_dir);

    // Bind
    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("[SERVER] Failed to bind {}: {}", addr, e);
            std::process::exit(1);
        });

    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let addresses = get_local_addresses(actual_port);

    if token.is_empty() {
        eprintln!("[SERVER] Auth: disabled");
    } else {
        eprintln!("[SERVER] Token: {}", token);
    }
    eprintln!("[SERVER] Listening on:");
    for addr in &addresses {
        eprintln!("  {}", addr);
    }

    // Start serving
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("[SERVER] Server error: {}", e);
        std::process::exit(1);
    }
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("codeg"))
        .unwrap_or_else(|| PathBuf::from(".codeg-data"))
}
