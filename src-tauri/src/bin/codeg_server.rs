use std::path::PathBuf;
use std::sync::Arc;

use codeg_lib::app_state::AppState;
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

    // When invoked as a git credential helper (by the script written via
    // `git_credential::create_credential_helper_script`), respond to git's
    // credential protocol on stdin and exit. Mirrors the desktop binary's
    // early-exit in `main.rs` so server deployments don't accidentally try
    // to start a second server instance per `git credential` invocation.
    if args.iter().any(|a| a == "--credential-helper") {
        codeg_lib::git_credential::run_credential_helper();
        return;
    }

    // PATH initialisation MUST happen before the tokio runtime is created.
    // std::env::set_var is not thread-safe (unsafe in Rust edition 2024);
    // #[tokio::main] would spawn worker threads before we reach this point.
    codeg_lib::process::ensure_node_in_path();
    codeg_lib::process::ensure_user_npm_prefix_in_path();

    // Resolve and pin `CODEG_DATA_DIR` before any threads exist.
    //
    // Two things matter here, both single-shot:
    //
    // 1. Absolutize: child processes (notably the credential helper
    //    subprocess invoked by git from inside the user's repo) inherit
    //    the env var and use it via `keyring_store::tokens_file_path` to
    //    find `tokens.json`. A relative `CODEG_DATA_DIR=data` would
    //    otherwise resolve against git's CWD, not the server's startup
    //    CWD, and the helper would silently miss the token file even
    //    though we found the database.
    //
    // 2. Fill in the default if unset, so every downstream resolver —
    //    `paths::codeg_uploads_root`, `paths::codeg_pets_root`,
    //    the credential subprocess — converges on the same root the
    //    server itself chose for the database. Without this, a default
    //    deployment (env var unset) puts the DB under
    //    `dirs::data_dir()/codeg` but uploads under `~/.codeg/uploads`,
    //    splitting the persistent surface across two filesystem roots
    //    and silently breaking single-volume backups, container mounts,
    //    and any `file://` URI in session history that points at an
    //    upload.
    //
    // `std::env::set_var` is not thread-safe (unsafe in Rust edition
    // 2024); doing this before the tokio runtime is built guarantees we
    // are still single-threaded.
    let resolved_data_dir = std::env::var("CODEG_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_data_dir());
    let resolved_data_dir = codeg_lib::git_credential::absolutize(&resolved_data_dir);
    std::env::set_var("CODEG_DATA_DIR", &resolved_data_dir);

    // `CODEG_HOME` overrides `CODEG_DATA_DIR` for uploads/pets inside
    // `paths::codeg_*_root` (legacy `~/.codeg/` layout). If both are set
    // and resolve to different roots, the database and uploads land on
    // different filesystems — a silent split. Warn loudly so the
    // operator notices before relying on a backup or volume mount that
    // only covers one of them.
    if let Some(home) = std::env::var_os("CODEG_HOME").filter(|s| !s.is_empty()) {
        let home_path = codeg_lib::git_credential::absolutize(std::path::Path::new(&home));
        if home_path != resolved_data_dir {
            eprintln!(
                "[paths][WARN] CODEG_HOME ({}) and CODEG_DATA_DIR ({}) point at different roots. \
                 Uploads/pets follow CODEG_HOME; the database follows CODEG_DATA_DIR. \
                 Unset one or align them to avoid split state.",
                home_path.display(),
                resolved_data_dir.display()
            );
        }
    }

    // Strict-mode quota validation runs before any I/O. Failing fast
    // here means a misconfigured strict deployment never reaches the
    // tokio runtime, never binds a port, and never persists config —
    // the operator sees the FATAL line and a clean exit code 2.
    codeg_lib::web::handlers::files::log_upload_quota_config_at_startup();
    if let Err(err) = codeg_lib::web::handlers::files::validate_upload_quota_config() {
        eprintln!("[uploads][FATAL] {err}; aborting startup.");
        std::process::exit(2);
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime")
        .block_on(async_main());
}

async fn async_main() {
    // Sweep stale ACP binary cache trash (rename-aside fallback artifacts).
    // Detached OS thread: cannot block startup, panics are caught and dropped,
    // errors are silenced, no subprocesses spawned.
    std::thread::spawn(|| {
        let _ = std::panic::catch_unwind(|| {
            codeg_lib::sweep_acp_binary_trash();
        });
    });

    let port: u16 = std::env::var("CODEG_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3080);
    let host = std::env::var("CODEG_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let token = std::env::var("CODEG_TOKEN").unwrap_or_else(|_| generate_random_token());
    // CODEG_DATA_DIR was already resolved and absolutized in `main()` so
    // all path resolvers across the process see the same root. Read it
    // back rather than re-deriving the default.
    let data_dir =
        PathBuf::from(std::env::var("CODEG_DATA_DIR").expect("CODEG_DATA_DIR set by main()"));
    let static_dir_env = std::env::var("CODEG_STATIC_DIR").ok();

    let static_dir = find_static_dir_standalone(static_dir_env.as_deref());
    let app_version = env!("CARGO_PKG_VERSION");

    eprintln!("[SERVER] codeg-server v{}", app_version);
    eprintln!("[SERVER] Data directory: {}", data_dir.display());
    eprintln!("[SERVER] Static directory: {}", static_dir.display());

    // Initialize database
    let db = codeg_lib::db::init_database(&data_dir, app_version)
        .await
        .expect("Failed to initialize database");

    // Restore and apply saved system proxy settings before any network operation.
    // reqwest clients (including the LazyLock in check_app_update) cache the proxy
    // config at build time, so this must run before the first one is constructed.
    codeg_lib::init_proxy_from_db(&db.conn).await;

    // Create shared broadcaster + internal ACP event bus.
    let broadcaster = Arc::new(WebEventBroadcaster::new());
    let event_bus_metrics = Arc::new(codeg_lib::acp::EventBusMetrics::default());
    let acp_event_bus = Arc::new(codeg_lib::acp::InternalEventBus::new(
        event_bus_metrics.clone(),
    ));
    let emitter = EventEmitter::web_only(broadcaster.clone(), acp_event_bus.clone());

    // Build AppState
    let pet_state_handle = codeg_lib::pet_state_mapper::new_pet_state_handle();
    let connection_manager = codeg_lib::app_state::default_connection_manager();
    let (delegation_broker, delegation_tokens, delegation_socket_path) =
        codeg_lib::app_state::build_delegation_stack(
            &connection_manager,
            db.conn.clone(),
            data_dir.clone(),
        );
    let state = Arc::new(AppState {
        db,
        connection_manager,
        terminal_manager: codeg_lib::app_state::default_terminal_manager(),
        event_broadcaster: broadcaster,
        acp_event_bus: acp_event_bus.clone(),
        emitter,
        data_dir,
        web_server_state: WebServerState::new(),
        chat_channel_manager: codeg_lib::app_state::default_chat_channel_manager(),
        workspace_transfer: Arc::new(
            codeg_lib::workspace_transfer::WorkspaceTransferManager::new_from_env(),
        ),
        pet_state: pet_state_handle.clone(),
        delegation_broker: delegation_broker.clone(),
        delegation_tokens: delegation_tokens.clone(),
        delegation_socket_path: delegation_socket_path.clone(),
    });

    // Apply persisted delegation settings (depth, enabled) before
    // the listener starts accepting so even the first companion request
    // sees the operator's configured behavior. Cancellation is handled
    // out-of-band via MCP `notifications/cancelled` — no broker-side
    // timeout to apply here.
    codeg_lib::commands::delegation::apply_persisted_config(&state.db.conn, &delegation_broker)
        .await;

    // Spawn the delegation listener so companion processes can round-trip
    // through the broker. Path is PID-scoped, so the listener owns it for
    // the lifetime of the process.
    {
        let listener = codeg_lib::acp::delegation::listener::DelegationListener::new(
            delegation_broker,
            delegation_tokens,
            Arc::new(codeg_lib::acp::manager::ConnectionManagerParentLookup {
                manager: Arc::new(state.connection_manager.clone_ref()),
            }),
        );
        let socket = delegation_socket_path.clone();
        tokio::spawn(async move {
            if let Err(e) = listener.run(socket).await {
                eprintln!("[delegation] listener exited: {e}");
            }
        });
    }

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
            state.acp_event_bus.clone(),
            state.db.conn.clone(),
            state.connection_manager.clone_ref(),
            state.emitter.clone(),
        )
        .await;

    // Spawn the LifecycleSubscriber for cross-connection DB writes. The
    // broker is supplied so TurnComplete on a delegation child resolves the
    // parent's pending `delegate_to_agent` tool_use_id and emits
    // `DelegationCompleted`.
    tokio::spawn(codeg_lib::lifecycle_subscriber_task(
        state.db.conn.clone(),
        state.connection_manager.clone_ref(),
        state.acp_event_bus.clone(),
        Some(state.delegation_broker.clone()),
    ));

    // Spawn the desktop pet state mapper so server-mode browsers viewing
    // /pet receive `pet://state` and `pet://oneshot` over the WebSocket
    // bridge, just like the Tauri webview does in desktop mode. ACP events
    // come through the typed bus; folder/app side-channels stay on the
    // JSON broadcaster.
    tokio::spawn(codeg_lib::pet_state_mapper::pet_state_subscriber_task(
        state.acp_event_bus.clone(),
        state.event_broadcaster.clone(),
        state.emitter.clone(),
        pet_state_handle,
    ));

    // Spawn the idle sweep so connections abandoned without an explicit
    // disconnect (e.g. browser tab closed, panic survivors) are reaped.
    // Override the 60-second default via `CODEG_ACP_IDLE_TIMEOUT_SECS`
    // (set to `0` to disable).
    if let Some(idle_timeout) = codeg_lib::idle_timeout_from_env() {
        tokio::spawn(codeg_lib::idle_sweep_task(
            state.connection_manager.clone_ref(),
            idle_timeout,
            std::time::Duration::from_secs(codeg_lib::SWEEP_INTERVAL_SECS),
        ));
    }

    // Sweep abandoned upload staging files from any prior run before
    // serving the first request. The quota log/validate ran earlier in
    // `main` so strict-mode misconfigurations abort before we touch
    // disk; no second log line here.
    codeg_lib::web::handlers::files::purge_upload_staging().await;

    // Build router
    let shutdown_signal = state.web_server_state.shutdown_signal();
    let router = codeg_lib::web::router::build_router(
        state.clone(),
        token.clone(),
        static_dir,
        shutdown_signal,
    );

    // Bind
    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("[SERVER] Failed to bind {}: {}", addr, e);
            std::process::exit(1);
        });

    if let Err(e) = codeg_lib::web::socket_inherit::mark_listener_non_inheritable(&listener) {
        eprintln!(
            "[SERVER][WARN] failed to mark listener non-inheritable: {}",
            e
        );
    }

    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

    // Publish runtime state so the settings page (served by us) shows
    // the truth — running on `actual_port` with this token — instead of
    // the placeholder "stopped" that triggers the stale-port banner.
    state
        .web_server_state
        .mark_externally_running(actual_port, token.clone());
    let addresses = get_local_addresses(actual_port);

    eprintln!("[SERVER] Token: {}", token);
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
