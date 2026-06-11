pub mod acp;
pub use acp::{
    idle_sweep_task, idle_timeout_from_env, lifecycle_subscriber_task, SWEEP_INTERVAL_SECS,
};
pub use network::proxy::init_proxy_from_db;
mod app_error;
pub mod app_state;
pub mod chat_channel;
pub mod commands;
pub mod db;
pub mod git_credential;
pub mod git_repo;
pub mod keyring_store;
pub mod models;
mod network;
pub mod parsers;
pub mod paths;
pub mod pet_sessions;
pub mod pet_state_mapper;
pub mod pets;
#[cfg(feature = "tauri-runtime")]
pub mod preferences;
pub mod process;
pub mod supervise;
mod terminal;
pub mod update;
pub mod web;
pub mod workspace_state;
pub mod workspace_transfer;

/// Sweep stale ACP binary cache trash created by the rename-aside fallback in
/// `acp::binary_cache::clear_agent_cache`. Safe to call any time; intended to
/// be invoked once at startup from a detached OS thread. Does not block, does
/// not panic, errors are silently dropped.
pub fn sweep_acp_binary_trash() {
    crate::acp::binary_cache::sweep_trash();
}

#[cfg(feature = "tauri-runtime")]
mod tauri_app {
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::acp::manager::ConnectionManager;
    use crate::chat_channel::manager::ChatChannelManager;
    use crate::commands::{
        acp as acp_commands, app_update as app_update_commands, backup,
        chat_channel as chat_channel_commands, conversations, delegation as delegation_commands,
        experts as experts_commands, feedback as feedback_commands, file_io, folder_commands,
        folders, mcp as mcp_commands,
        model_provider as model_provider_commands, notification, pet as pet_commands, project_boot,
        question as question_commands, quick_messages as quick_messages_commands,
        remote_proxy as remote_proxy_commands,
        remote_workspace as remote_workspace_commands, system_settings, terminal as terminal_commands,
        version_control, windows, workspace_state as workspace_state_commands,
    };
    use crate::terminal::manager::TerminalManager;
    use crate::{db, git_credential, network, paths, process, web};
    use tauri::Manager;

    static APP_QUITTING: AtomicBool = AtomicBool::new(false);

    fn summarize_web_auto_start_error(err: &crate::app_error::AppCommandError) -> String {
        match err
            .detail
            .as_deref()
            .filter(|detail| !detail.trim().is_empty())
        {
            Some(detail) if detail != err.message.as_str() => {
                format!("{}: {}", err.message, detail)
            }
            _ => err.message.clone(),
        }
    }

    fn notify_web_auto_start_failed(
        app: &tauri::AppHandle,
        port: u16,
        err: &crate::app_error::AppCommandError,
    ) {
        let app = app.clone();
        let body = format!(
            "Could not start the Web service on port {}: {}",
            port,
            summarize_web_auto_start_error(err)
        );
        tauri::async_runtime::spawn(async move {
            let _ =
                notification::send_notification(app, "Codeg Web service".to_string(), body).await;
        });
    }

    /// On Windows, opt-out users can disable WebView2 hardware acceleration to
    /// work around AMD/Intel GPU driver bugs that produce a black-screen
    /// webview. The flag is stored in a tiny sidecar file at
    /// `~/.codeg/preferences.json` so it can be read **before** the Tauri
    /// builder, plugins, or tokio runtime start — once a tokio worker is alive,
    /// `std::env::set_var` would race with concurrent `getenv` calls from
    /// libraries like reqwest/rustls that read `HTTP_PROXY` etc.
    #[cfg(target_os = "windows")]
    fn apply_webview2_rendering_override() {
        // Matches the dominant pattern across the Tauri 2 ecosystem (Dorion,
        // Seelen-UI, and most production Tauri 2 apps that ship a "disable
        // hardware acceleration" toggle all use `--disable-gpu`).
        const DISABLE_GPU_ARGS: [&str; 1] = ["--disable-gpu"];
        const ENV_KEY: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

        let prefs = crate::preferences::load();
        if !prefs.disable_hardware_acceleration {
            return;
        }

        let mut tokens: Vec<String> = match std::env::var(ENV_KEY) {
            Ok(prev) => prev.split_whitespace().map(str::to_string).collect(),
            Err(_) => Vec::new(),
        };
        for arg in DISABLE_GPU_ARGS {
            if !tokens.iter().any(|t| t == arg) {
                tokens.push(arg.to_string());
            }
        }
        // SAFETY: called before any tokio worker or plugin thread spawns, so
        // no concurrent `getenv` can race. `set_var` is `unsafe` since Rust 1.82.
        unsafe {
            std::env::set_var(ENV_KEY, tokens.join(" "));
        }
    }

    #[cfg_attr(mobile, tauri::mobile_entry_point)]
    pub fn run() {
        // Apply the WebView2 rendering override before *any* tokio worker
        // exists or any plugin reads the env. See doc comment above.
        #[cfg(target_os = "windows")]
        apply_webview2_rendering_override();

        if let Err(err) = fix_path_env::fix() {
            eprintln!("[PATH] fix_path_env failed: {err}");
        }
        process::ensure_node_in_path();
        process::ensure_user_npm_prefix_in_path();

        let builder = tauri::Builder::default();

        // Must be the first plugin: it short-circuits second launches by
        // signalling the running instance and exiting before any other
        // initialization. The callback runs in the *original* process.
        //
        // Skipped in debug builds so a locally-built `cargo run` instance
        // can run alongside an installed release build of codeg during
        // development. Debug desktop builds use an isolated SQLite file, but
        // they still share other `app.codeg` data-dir artifacts with release.
        #[cfg(not(debug_assertions))]
        let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::show_main_window(app);
        }));

        builder
            .plugin(tauri_plugin_window_state::Builder::new().build())
            .plugin(tauri_plugin_opener::init())
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init())
            .plugin(tauri_plugin_notification::init())
            .manage(ConnectionManager::new())
            .manage(TerminalManager::new())
            .manage(ChatChannelManager::new())
            .manage(windows::SettingsWindowState::new())
            .manage(windows::CommitWindowState::new())
            .manage(windows::MergeWindowState::new())
            .manage(web::WebServerState::new())
            // Remote-workspace IPC proxy. Routes HTTP / WS for windows
            // opened against a remote codeg-server through Rust so we
            // bypass webview mixed-content blocking and can centrally
            // manage per-window subscriptions.
            .manage(std::sync::Arc::new(
                crate::commands::remote_proxy::RemoteProxyState::new(),
            ))
            .manage(std::sync::Arc::new(
                crate::workspace_transfer::WorkspaceTransferManager::new_from_env(),
            ))
            .manage(std::sync::Arc::new(
                web::event_bridge::WebEventBroadcaster::new(),
            ))
            // In-process ACP event bus — typed `Arc<EventEnvelope>` delivery
            // to lifecycle / pet / chat-channel subscribers. Distinct from
            // the JSON-shape `WebEventBroadcaster` above. The metrics handle
            // lives inside the bus so the `/debug/event_metrics` endpoint
            // and shutdown logs can read it.
            .manage({
                let metrics =
                    std::sync::Arc::new(crate::acp::EventBusMetrics::default());
                std::sync::Arc::new(crate::acp::InternalEventBus::new(metrics))
            })
            .manage(crate::pet_state_mapper::new_pet_state_handle())
            // Source of truth for an in-flight app self-update. Shared with the
            // embedded web server's AppState so HTTP and webview clients see the
            // same download progress; lets the upgrade UI survive navigation.
            .manage(crate::update::new_update_state_handle())
            .setup(|app| {
                let app_data_dir = app.path().app_data_dir()?;

                // Unify the data root across every consumer:
                //   * SQLite database (initialised below)
                //   * `paths::codeg_uploads_root` / `codeg_pets_root`
                //   * `AppState.data_dir` and every desktop command
                //     that injects a git credential helper / askpass
                //     into a subprocess (terminal, ACP, folder ops)
                //
                // The contract is "one effective root, end of story."
                // `paths::resolve_effective_data_dir` is the single
                // source of truth; every desktop call site that
                // historically read `app.path().app_data_dir()` and
                // passed it to a credential helper has been migrated
                // to the same helper so a pre-set `CODEG_DATA_DIR` is
                // honored end-to-end.
                //
                // We also write the absolutized value back to the env,
                // even when the operator pre-set it, so:
                //   * subprocesses inherit an absolute path (a relative
                //     `CODEG_DATA_DIR` would otherwise re-resolve
                //     against the subprocess CWD, which may differ
                //     from ours), and
                //   * any future caller that reaches for the env
                //     directly sees the same value the in-process
                //     resolver returns.
                //
                // `set_var` is `unsafe` in edition 2024. We are still
                // single-threaded at this point: `setup` runs on the
                // main thread before any window or async runtime task
                // reads the var, the Tauri plugins registered above
                // (window state, opener, dialog, updater, process,
                // notification) do not read `CODEG_DATA_DIR`, and the
                // value is never mutated again for the lifetime of the
                // process.
                let effective_data_dir = paths::resolve_effective_data_dir(&app_data_dir);
                // SAFETY: see the rationale block above — still
                // single-threaded at setup; edition 2024 will require
                // the `unsafe` block, mirroring the WebView2 rendering
                // override.
                unsafe {
                    std::env::set_var("CODEG_DATA_DIR", &effective_data_dir);
                }

                // `CODEG_HOME` overrides `CODEG_DATA_DIR` inside
                // `paths::codeg_uploads_root` / `codeg_pets_root` for
                // backwards-compatibility with the legacy `~/.codeg/`
                // layout. If both are set and point at different roots,
                // uploads/pets land on `CODEG_HOME` while the database
                // lands on `CODEG_DATA_DIR` — a silent split. The
                // backup story here is "loud warning, no automatic
                // override": the operator likely meant one of them, but
                // we don't know which.
                if let Some(home) = std::env::var_os("CODEG_HOME").filter(|s| !s.is_empty()) {
                    let home_path = git_credential::absolutize(std::path::Path::new(&home));
                    if home_path != effective_data_dir {
                        eprintln!(
                            "[paths][WARN] CODEG_HOME ({}) and CODEG_DATA_DIR ({}) point at different roots. \
                             Uploads/pets follow CODEG_HOME; the database follows CODEG_DATA_DIR. \
                             Unset one or align them to avoid split state.",
                            home_path.display(),
                            effective_data_dir.display()
                        );
                    }
                }

                let app_version = env!("CARGO_PKG_VERSION");
                let database = tauri::async_runtime::block_on(db::init_database(
                    &effective_data_dir,
                    app_version,
                ))
                .map_err(|e| e.to_string())?;
                app.manage(database);

                // Restore and apply saved system proxy settings before any network operation.
                let db = app.state::<db::AppDatabase>();
                tauri::async_runtime::block_on(network::proxy::init_proxy_from_db(&db.conn));

                // Load saved appearance settings before any window is created.
                tauri::async_runtime::block_on(windows::load_saved_zoom(&db.conn));
                tauri::async_runtime::block_on(windows::load_saved_appearance_mode(&db.conn));

                // System tray: required for the WeChat-style hide-on-close
                // flow on Windows/Linux (no built-in dock to bring the
                // workspace back). Locale comes from the persisted language
                // settings; system mode falls back to English here, which
                // the user can fix by switching to manual mode.
                let tray_locale = tauri::async_runtime::block_on(
                    crate::commands::system_settings::load_system_language_settings(&db.conn),
                )
                .map(|settings| settings.language)
                .unwrap_or_default();
                if let Err(err) = windows::install_tray_icon(app.handle(), tray_locale) {
                    eprintln!("[Tray] failed to install tray icon: {err}");
                }

                // Sweep stale ACP binary cache trash (rename-aside fallback
                // artifacts). Detached OS thread: cannot block startup, panics
                // are caught and dropped, errors are silenced, no subprocesses
                // spawned. Anything still locked is left for next startup.
                std::thread::spawn(|| {
                    let _ = std::panic::catch_unwind(|| {
                        crate::sweep_acp_binary_trash();
                    });
                });

                // Install bundled expert skills into the central store
                // (`~/.codeg/skills/`). Runs in the background and does
                // not block startup; failures are logged but non-fatal.
                tauri::async_runtime::spawn(async move {
                    let report = crate::commands::experts::ensure_central_experts_installed().await;
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

                // Start chat channel background tasks
                {
                    let ccm = app.state::<ChatChannelManager>();
                    let broadcaster =
                        app.state::<std::sync::Arc<web::event_bridge::WebEventBroadcaster>>();
                    let db_conn = app.state::<db::AppDatabase>().conn.clone();
                    let ccm_ref = ccm.clone_ref();
                    let br = broadcaster.inner().clone();
                    let bus = app
                        .state::<std::sync::Arc<crate::acp::InternalEventBus>>()
                        .inner()
                        .clone();
                    let cm = app.state::<ConnectionManager>().clone_ref();
                    let emitter = web::event_bridge::EventEmitter::Tauri(app.handle().clone());
                    tauri::async_runtime::spawn(async move {
                        ccm_ref.start_background(br, bus, db_conn, cm, emitter).await;
                    });
                }

                // Spawn the desktop pet state mapper: subscribes to ACP events
                // (typed envelopes via the in-process bus) AND folder/app
                // side-channel notifications (JSON via the broadcaster), and
                // emits `pet://state` whenever the aggregated pet state
                // changes. The renderer in the floating pet window listens
                // for these events to drive its sprite animation row.
                {
                    let bus = app
                        .state::<std::sync::Arc<crate::acp::InternalEventBus>>()
                        .inner()
                        .clone();
                    let broadcaster = app
                        .state::<std::sync::Arc<web::event_bridge::WebEventBroadcaster>>()
                        .inner()
                        .clone();
                    let emitter = web::event_bridge::EventEmitter::Tauri(app.handle().clone());
                    let pet_state_handle = app
                        .state::<crate::pet_state_mapper::PetStateHandle>()
                        .inner()
                        .clone();
                    tauri::async_runtime::spawn(
                        crate::pet_state_mapper::pet_state_subscriber_task(
                            bus,
                            broadcaster,
                            emitter,
                            pet_state_handle,
                        ),
                    );
                }

                // Spawn the pet panel active-session aggregator: rebuilds the
                // `PetSessionsPayload` (running/waiting/error counts + per-
                // session rows with titles and pending permissions) on ACP
                // lifecycle events and emits `pet://sessions` for the sprite
                // badge + panel window. Shares the same buses as the ambient
                // mapper but is kept separate so the DB-free ambient task stays
                // simple; desktop-only (server mode has no pet window).
                {
                    let bus = app
                        .state::<std::sync::Arc<crate::acp::InternalEventBus>>()
                        .inner()
                        .clone();
                    let broadcaster = app
                        .state::<std::sync::Arc<web::event_bridge::WebEventBroadcaster>>()
                        .inner()
                        .clone();
                    let emitter = web::event_bridge::EventEmitter::Tauri(app.handle().clone());
                    let manager = app.state::<ConnectionManager>().inner().clone_ref();
                    let db_conn = app.state::<db::AppDatabase>().conn.clone();
                    tauri::async_runtime::spawn(
                        crate::pet_sessions::pet_sessions_subscriber_task(
                            bus,
                            broadcaster,
                            emitter,
                            manager,
                            db_conn,
                        ),
                    );
                }

                // Delegation broker + UDS listener. Built from the managed
                // ConnectionManager + DB so spawn / depth-lookup work against
                // live state. Managed alongside the existing per-resource
                // states so commands (Tauri + web) can resolve them by type.
                // MUST run before the LifecycleSubscriber spawn below so the
                // broker handle is available to it.
                let broker_for_lifecycle = {
                    let cm_state = app.state::<ConnectionManager>();
                    let db_conn = app.state::<db::AppDatabase>().conn.clone();
                    let (broker, tokens, socket_path, feedback_config, question_config) =
                        crate::app_state::build_delegation_stack(
                            &cm_state,
                            db_conn.clone(),
                            effective_data_dir.clone(),
                        );
                    app.manage(broker.clone());
                    app.manage(tokens.clone());
                    app.manage(feedback_config.clone());
                    app.manage(question_config.clone());
                    app.manage(crate::commands::delegation::DelegationSocketPath(
                        socket_path.clone(),
                    ));

                    // Push persisted settings into the broker + feedback + question
                    // config before listener accept.
                    let broker_for_init = broker.clone();
                    let db_for_init = db_conn.clone();
                    let feedback_for_init = feedback_config.clone();
                    let question_for_init = question_config.clone();
                    tauri::async_runtime::block_on(async move {
                        delegation_commands::apply_persisted_config(
                            &db_for_init,
                            &broker_for_init,
                        )
                        .await;
                        crate::commands::feedback::apply_persisted_feedback_config(
                            &db_for_init,
                            &feedback_for_init,
                        )
                        .await;
                        crate::commands::question::apply_persisted_question_config(
                            &db_for_init,
                            &question_for_init,
                        )
                        .await;
                    });

                    let listener_broker = broker.clone();
                    let listener = crate::acp::delegation::listener::DelegationListener::new(
                        listener_broker,
                        tokens,
                        std::sync::Arc::new(
                            crate::acp::manager::ConnectionManagerParentLookup {
                                manager: std::sync::Arc::new(cm_state.clone_ref()),
                            },
                        ),
                        std::sync::Arc::new(
                            crate::acp::manager::ConnectionManagerFeedbackLookup {
                                manager: std::sync::Arc::new(cm_state.clone_ref()),
                            },
                        ),
                        std::sync::Arc::new(
                            crate::acp::manager::ConnectionManagerQuestionLookup {
                                manager: std::sync::Arc::new(cm_state.clone_ref()),
                            },
                        ),
                    );
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = listener.run(socket_path).await {
                            eprintln!("[delegation] listener exited: {e}");
                        }
                    });
                    broker
                };

                // Spawn the LifecycleSubscriber: persists cross-connection DB state
                // (currently `external_id` on conversation rows when SessionStarted fires)
                // off the emit hot path. `subscribe()` runs synchronously inside
                // `lifecycle_subscriber_task` before the future is returned, so the
                // subscribe-before-spawn invariant holds. The setup callback runs
                // outside any tokio runtime, so we use `tauri::async_runtime::spawn`.
                {
                    let db_conn = app.state::<db::AppDatabase>().conn.clone();
                    let cm = app.state::<ConnectionManager>().clone_ref();
                    let bus = app
                        .state::<std::sync::Arc<crate::acp::InternalEventBus>>()
                        .inner()
                        .clone();
                    tauri::async_runtime::spawn(crate::acp::lifecycle_subscriber_task(
                        db_conn,
                        cm,
                        bus,
                        Some(broker_for_lifecycle),
                    ));
                }

                match tauri::async_runtime::block_on(web::load_web_service_config(&db.conn)) {
                    Ok(config) if config.auto_start => {
                        let port = config.port.unwrap_or(web::DEFAULT_WEB_SERVICE_PORT);
                        let ws = app.state::<web::WebServerState>();
                        if let Err(err) =
                            tauri::async_runtime::block_on(web::do_start_web_server_tauri(
                                app.handle().clone(),
                                &ws,
                                config.port,
                                None,
                                config.token,
                            ))
                        {
                            eprintln!("[WEB] auto-start failed: {err}");
                            notify_web_auto_start_failed(app.handle(), port, &err);
                        }
                    }
                    Ok(_) => {}
                    Err(err) => eprintln!("[WEB] failed to load auto-start config: {err}"),
                }

                // Spawn the idle sweep so connections abandoned without an
                // explicit disconnect (e.g. window/tab closed without
                // teardown, panic survivors) are reaped. Override the
                // 60-second default via `CODEG_ACP_IDLE_TIMEOUT_SECS`
                // (set to `0` to disable).
                if let Some(idle_timeout) = crate::acp::idle_timeout_from_env() {
                    let cm = app.state::<ConnectionManager>().clone_ref();
                    tauri::async_runtime::spawn(crate::acp::idle_sweep_task(
                        cm,
                        idle_timeout,
                        std::time::Duration::from_secs(crate::acp::SWEEP_INTERVAL_SECS),
                    ));
                }

                // Single-window workspace: ensure the main window exists.
                // Workspace state (open folders, opened tabs, active tab) is
                // restored by the frontend via `list_open_folder_details` /
                // `list_opened_tabs` inside the main window.
                if app.get_webview_window("main").is_none() {
                    let url = tauri::WebviewUrl::App("workspace".into());
                    let builder = tauri::WebviewWindowBuilder::new(app, "main", url)
                        .title("Codeg")
                        .inner_size(1260.0, 860.0)
                        .min_inner_size(400.0, 600.0);
                    if let Ok(w) = windows::apply_platform_window_style(builder).build() {
                        windows::post_window_setup(&w);
                    }
                }

                Ok(())
            })
            .on_menu_event(|app, event| {
                let id = event.id().as_ref().to_string();

                // Tray menu items act in Rust directly: showing the
                // workspace and quitting are both pure runtime concerns
                // with no UI state to coordinate.
                if id.starts_with(windows::TRAY_MENU_ID_PREFIX) {
                    match id.as_str() {
                        windows::TRAY_MENU_ID_SHOW => windows::show_main_window(app),
                        windows::TRAY_MENU_ID_QUIT => app.exit(0),
                        _ => {}
                    }
                    return;
                }

                // Dispatch native pet context-menu actions. Items live under
                // the `pet:` id namespace; everything else (future app
                // menus) flows past untouched. We re-emit a webview event
                // rather than acting in Rust so the existing frontend
                // commands (pet_save_window_state, open_settings_window,
                // close_pet_window) stay the single source of truth — the
                // native menu is just a different *trigger*.
                if !id.starts_with(windows::PET_MENU_ID_PREFIX) {
                    return;
                }
                let payload: serde_json::Value =
                    if let Some(scale) = windows::pet_menu_scale_from_id(&id) {
                        serde_json::json!({ "type": "scale", "value": scale })
                    } else if id == windows::PET_MENU_ID_OPEN_MANAGER {
                        serde_json::json!({ "type": "open_manager" })
                    } else if id == windows::PET_MENU_ID_CLOSE {
                        serde_json::json!({ "type": "close" })
                    } else {
                        // Header / unknown — nothing to do.
                        return;
                    };
                use tauri::Emitter;
                let _ = app.emit_to("pet", "pet://menu-action", payload);
            })
            .on_window_event(|window, event| {
                let label = window.label().to_string();

                if (label == "settings" || label.starts_with("remote-settings-"))
                    && matches!(
                        event,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    )
                {
                    let app = window.app_handle();
                    if let Some(state) = app.try_state::<windows::SettingsWindowState>() {
                        windows::restore_windows_after_settings(app, &state, &label);
                    }
                }

                if (label.starts_with("commit-") || label.starts_with("remote-commit-"))
                    && matches!(
                        event,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    )
                {
                    let app = window.app_handle();
                    if let Some(state) = app.try_state::<windows::CommitWindowState>() {
                        windows::restore_window_after_commit(app, &state, &label);
                    }
                }

                if (label.starts_with("merge-") || label.starts_with("remote-merge-"))
                    && matches!(
                        event,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    )
                {
                    let app = window.app_handle();
                    if let Some(state) = app.try_state::<windows::MergeWindowState>() {
                        windows::restore_window_after_merge(app, &state, &label);
                    }
                    if label.starts_with("merge-") {
                        let app_clone = window.app_handle().clone();
                        let label_clone = label.clone();
                        tauri::async_runtime::spawn(async move {
                            windows::cleanup_dangling_merge(&app_clone, &label_clone).await;
                        });
                    }
                }

                if label == "pet"
                    && matches!(
                        event,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    )
                {
                    // Persist `enabled = false` so the next launch doesn't
                    // race-open the pet before the user asks for it. We
                    // intentionally do NOT clear `active_pet_id` — the user
                    // chose that pet, they want it back next time they open
                    // the window.
                    if let Some(db) = window.app_handle().try_state::<db::AppDatabase>() {
                        let conn = db.conn.clone();
                        let save = async move {
                            let _ = crate::commands::pet::pet_save_window_state_core(
                                &conn,
                                crate::models::pet::PetWindowStatePatch {
                                    x: None,
                                    y: None,
                                    scale: None,
                                    always_on_top: None,
                                    enabled: Some(false),
                                },
                            )
                            .await;
                        };
                        // During app shutdown the runtime is about to be torn
                        // down — a fire-and-forget spawn would lose the save
                        // and `enabled = true` would survive into the next
                        // launch. Block here so the write lands before
                        // ExitRequested returns.
                        if APP_QUITTING.load(Ordering::Relaxed) {
                            tauri::async_runtime::block_on(save);
                        } else {
                            tauri::async_runtime::spawn(save);
                        }
                    }
                }

                if label == windows::PET_PANEL_LABEL
                    && matches!(event, tauri::WindowEvent::Focused(false))
                {
                    // Click-away dismiss for the session panel.
                    windows::close_pet_panel_on_blur(window.app_handle());
                }

                if label == "main" {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        // The close button does one of two things, depending
                        // on whether the platform can keep the workspace
                        // recoverable while it's hidden:
                        //
                        //   * tray usable (macOS, Windows + tray): WeChat-style
                        //     hide. App keeps running, tray brings it back.
                        //   * tray not usable (Linux, tray install failed):
                        //     force a real app exit. Letting only `main`
                        //     close would orphan the desktop pet and other
                        //     aux windows in a process with no workspace and
                        //     no way to bring it back — `pet` runs with
                        //     `skip_taskbar(true)`, and the single-instance
                        //     callback's `show_main_window` is a no-op once
                        //     main is destroyed.
                        //
                        // ExitRequested itself reaches this branch with
                        // APP_QUITTING already set — that's the only path
                        // that should fall through to the cleanup below.
                        if !APP_QUITTING.load(Ordering::Relaxed) {
                            api.prevent_close();
                            if windows::can_hide_to_tray() {
                                let _ = window.hide();
                            } else {
                                window.app_handle().exit(0);
                            }
                            return;
                        }
                        let app = window.app_handle();
                        if let Some(cm) = app.try_state::<ConnectionManager>() {
                            let disconnected = tauri::async_runtime::block_on(
                                cm.disconnect_by_owner_window(&label),
                            );
                            eprintln!(
                                "[ACP] main window closing disconnected_connections={}",
                                disconnected
                            );
                        }
                        if let Some(tm) = app.try_state::<TerminalManager>() {
                            let killed = tm.kill_by_owner_window(&label);
                            eprintln!("[TERM] main window closing killed_terminals={}", killed);
                        }
                    }
                }
            })
            .invoke_handler(tauri::generate_handler![
                conversations::list_conversations,
                conversations::get_conversation,
                conversations::list_all_conversations,
                conversations::list_child_conversations,
                conversations::list_opened_tabs,
                conversations::save_opened_tabs,
                conversations::import_local_conversations,
                conversations::get_folder_conversation,
                conversations::list_folders,
                conversations::get_stats,
                conversations::get_sidebar_data,
                conversations::create_conversation,
                conversations::update_conversation_status,
                conversations::update_conversation_title,
                conversations::update_conversation_pinned,
                conversations::delete_conversation,
                folders::load_folder_history,
                folders::get_folder,
                folders::list_open_folder_details,
                folders::list_all_folder_details,
                folders::open_folder,
                folders::open_worktree_folder,
                folders::resolve_worktree_folder,
                folders::open_folder_in_workspace,
                folders::open_folder_by_id,
                folders::remove_folder_from_workspace,
                folders::reorder_folders,
                folders::update_folder_color,
                folders::update_folder_default_agent,
                folders::add_folder_to_history,
                folders::remove_folder_from_history,
                folders::create_folder_directory,
                folders::clone_repository,
                folders::get_git_branch,
                folders::git_init,
                folders::git_pull,
                folders::git_start_pull_merge,
                folders::git_has_merge_head,
                folders::git_fetch,
                folders::git_push_info,
                folders::git_push,
                folders::git_new_branch,
                folders::git_worktree_add,
                folders::git_checkout,
                folders::git_reset,
                folders::git_list_branches,
                folders::git_stash_push,
                folders::git_stash_pop,
                folders::git_stash_list,
                folders::git_stash_apply,
                folders::git_stash_drop,
                folders::git_stash_clear,
                folders::git_stash_show,
                folders::git_status,
                folders::git_is_tracked,
                folders::git_diff,
                folders::git_diff_with_branch,
                folders::git_show_diff,
                folders::git_show_file,
                folders::git_commit,
                folders::git_rollback_file,
                folders::git_add_files,
                folders::git_list_all_branches,
                folders::git_list_remotes,
                folders::git_fetch_remote,
                folders::git_add_remote,
                folders::git_remove_remote,
                folders::git_set_remote_url,
                folders::git_merge,
                folders::git_rebase,
                folders::git_delete_branch,
                folders::git_delete_remote_branch,
                folders::git_list_conflicts,
                folders::git_conflict_file_versions,
                folders::git_resolve_conflict,
                folders::git_abort_operation,
                folders::git_continue_operation,
                workspace_state_commands::start_workspace_state_stream,
                workspace_state_commands::stop_workspace_state_stream,
                workspace_state_commands::get_workspace_snapshot,
                folders::get_home_directory,
                folders::list_directory_entries,
                folders::list_directory_with_files,
                folders::get_file_tree,
                folders::read_file_base64,
                folders::read_workspace_file_base64,
                folders::read_file_preview,
                folders::read_file_for_edit,
                folders::save_file_content,
                folders::save_file_copy,
                folders::rename_file_tree_entry,
                folders::delete_file_tree_entry,
                folders::create_file_tree_entry,
                folders::git_log,
                folders::git_commit_branches,
                windows::open_folder_window,
                windows::open_commit_window,
                windows::open_settings_window,
                windows::open_merge_window,
                windows::open_stash_window,
                windows::open_push_window,
                windows::open_project_boot_window,
                remote_workspace_commands::list_remote_workspace_connections,
                remote_workspace_commands::create_remote_workspace_connection,
                remote_workspace_commands::update_remote_workspace_connection,
                remote_workspace_commands::delete_remote_workspace_connection,
                remote_workspace_commands::test_remote_workspace_connection,
                remote_workspace_commands::get_remote_workspace_connection,
                remote_workspace_commands::reorder_remote_workspace_connections,
                remote_workspace_commands::open_remote_workspace,
                remote_proxy_commands::remote_http_call,
                remote_proxy_commands::remote_upload_attachment,
                remote_proxy_commands::remote_upload_workspace_paths,
                remote_proxy_commands::remote_cancel_workspace_transfer,
                remote_proxy_commands::remote_download_workspace_file,
                remote_proxy_commands::remote_download_workspace_dir,
                remote_proxy_commands::read_local_file_for_upload,
                remote_proxy_commands::remote_ws_subscribe,
                remote_proxy_commands::remote_ws_unsubscribe,
                remote_proxy_commands::remote_ws_send_text,
                windows::open_pet_window,
                windows::close_pet_window,
                windows::pet_window_record_position,
                windows::pet_show_context_menu,
                windows::toggle_pet_panel,
                windows::close_pet_panel,
                windows::resize_pet_panel,
                windows::focus_conversation,
                windows::update_traffic_light_position,
                windows::update_appearance_mode,
                windows::set_tray_locale,
                pet_commands::pet_list,
                pet_commands::pet_get,
                pet_commands::pet_read_spritesheet,
                pet_commands::pet_add,
                pet_commands::pet_update_meta,
                pet_commands::pet_replace_sprite,
                pet_commands::pet_delete,
                pet_commands::pet_list_importable_codex,
                pet_commands::pet_import_codex,
                pet_commands::pet_codex_import_available,
                pet_commands::pet_get_settings,
                pet_commands::pet_set_active,
                pet_commands::pet_save_window_state,
                pet_commands::pet_marketplace_list,
                pet_commands::pet_marketplace_install,
                pet_commands::pet_celebrate,
                pet_commands::pet_get_current_state,
                pet_commands::pet_list_active_sessions,
                app_update_commands::app_update_state,
                app_update_commands::perform_app_update,
                app_update_commands::restart_app,
                project_boot::detect_package_manager,
                project_boot::create_shadcn_project,
                project_boot::detect_hyperframes_skills,
                project_boot::install_hyperframes_skills,
                project_boot::create_hyperframes_project,
                system_settings::get_system_proxy_settings,
                system_settings::update_system_proxy_settings,
                system_settings::get_system_language_settings,
                system_settings::update_system_language_settings,
                system_settings::get_system_terminal_settings,
                system_settings::update_system_terminal_settings,
                system_settings::get_available_terminal_shells,
                system_settings::probe_terminal_shell_path,
                system_settings::get_system_rendering_settings,
                system_settings::update_system_rendering_settings,
                delegation_commands::get_delegation_settings,
                delegation_commands::set_delegation_settings,
                feedback_commands::get_feedback_settings,
                feedback_commands::set_feedback_settings,
                feedback_commands::submit_session_feedback,
                question_commands::get_question_settings,
                question_commands::set_question_settings,
                version_control::detect_git,
                version_control::test_git_path,
                version_control::get_git_settings,
                version_control::update_git_settings,
                version_control::get_github_accounts,
                version_control::validate_github_token,
                version_control::update_github_accounts,
                version_control::save_account_token,
                version_control::get_account_token,
                version_control::delete_account_token,
                acp_commands::acp_preflight,
                acp_commands::acp_connect,
                acp_commands::acp_prompt,
                acp_commands::acp_set_mode,
                acp_commands::acp_set_config_option,
                acp_commands::acp_describe_agent_options,
                acp_commands::acp_cancel,
                acp_commands::acp_fork,
                acp_commands::acp_respond_permission,
                acp_commands::acp_answer_question,
                acp_commands::acp_disconnect,
                acp_commands::acp_touch_connection,
                acp_commands::acp_list_connections,
                acp_commands::acp_get_session_snapshot,
                acp_commands::acp_get_session_snapshot_by_conversation,
                acp_commands::acp_find_connection_for_conversation,
                acp_commands::acp_list_agents,
                acp_commands::acp_get_agent_status,
                acp_commands::acp_clear_binary_cache,
                acp_commands::acp_download_agent_binary,
                acp_commands::acp_install_uv_tool,
                acp_commands::acp_detect_agent_local_version,
                acp_commands::acp_prepare_npx_agent,
                acp_commands::acp_uninstall_agent,
                acp_commands::acp_update_agent_preferences,
                acp_commands::acp_update_agent_env,
                acp_commands::acp_update_agent_config,
                acp_commands::acp_update_hermes_config,
                acp_commands::acp_open_hermes_setup_terminal,
                acp_commands::acp_reveal_hermes_home,
                acp_commands::acp_reorder_agents,
                acp_commands::acp_list_agent_skills,
                acp_commands::acp_read_agent_skill,
                acp_commands::acp_save_agent_skill,
                acp_commands::acp_delete_agent_skill,
                acp_commands::opencode_list_plugins,
                acp_commands::opencode_install_plugins,
                acp_commands::opencode_uninstall_plugin,
                acp_commands::codex_request_device_code,
                acp_commands::codex_poll_device_code,
                experts_commands::experts_list,
                experts_commands::experts_list_for_agent,
                experts_commands::experts_get_install_status,
                experts_commands::experts_link_to_agent,
                experts_commands::experts_unlink_from_agent,
                experts_commands::experts_read_content,
                experts_commands::experts_open_central_dir,
                folder_commands::list_folder_commands,
                folder_commands::create_folder_command,
                folder_commands::update_folder_command,
                folder_commands::delete_folder_command,
                folder_commands::reorder_folder_commands,
                folder_commands::bootstrap_folder_commands_from_package_json,
                quick_messages_commands::quick_messages_list,
                quick_messages_commands::quick_messages_create,
                quick_messages_commands::quick_messages_update,
                quick_messages_commands::quick_messages_delete,
                quick_messages_commands::quick_messages_reorder,
                terminal_commands::terminal_spawn,
                terminal_commands::terminal_write,
                terminal_commands::terminal_resize,
                terminal_commands::terminal_kill,
                terminal_commands::terminal_list,
                mcp_commands::mcp_scan_local,
                mcp_commands::mcp_list_marketplaces,
                mcp_commands::mcp_search_marketplace,
                mcp_commands::mcp_get_marketplace_server_detail,
                mcp_commands::mcp_install_from_marketplace,
                mcp_commands::mcp_upsert_local_server,
                mcp_commands::mcp_set_server_apps,
                mcp_commands::mcp_remove_server,
                notification::send_notification,
                file_io::save_binary_file,
                file_io::save_text_file,
                backup::backup_create,
                backup::backup_inspect,
                backup::backup_scan_external_conflicts,
                backup::backup_restore_stage,
                backup::backup_cancel,
                chat_channel_commands::list_chat_channels,
                chat_channel_commands::create_chat_channel,
                chat_channel_commands::update_chat_channel,
                chat_channel_commands::delete_chat_channel,
                chat_channel_commands::save_chat_channel_token,
                chat_channel_commands::get_chat_channel_has_token,
                chat_channel_commands::delete_chat_channel_token,
                chat_channel_commands::connect_chat_channel,
                chat_channel_commands::disconnect_chat_channel,
                chat_channel_commands::test_chat_channel,
                chat_channel_commands::get_chat_channel_status,
                chat_channel_commands::list_chat_channel_messages,
                chat_channel_commands::get_chat_command_prefix,
                chat_channel_commands::set_chat_command_prefix,
                chat_channel_commands::get_chat_event_filter,
                chat_channel_commands::set_chat_event_filter,
                chat_channel_commands::get_chat_event_webhooks,
                chat_channel_commands::set_chat_event_webhooks,
                chat_channel_commands::get_chat_message_language,
                chat_channel_commands::set_chat_message_language,
                chat_channel_commands::weixin_get_qrcode,
                chat_channel_commands::weixin_check_qrcode,
                model_provider_commands::list_model_providers,
                model_provider_commands::create_model_provider,
                model_provider_commands::update_model_provider,
                model_provider_commands::delete_model_provider,
                web::start_web_server,
                web::stop_web_server,
                web::get_web_server_status,
                web::get_web_service_config,
                web::update_web_service_config,
                web::probe_web_service_port,
            ])
            .build(tauri::generate_context!())
            .expect("error while building tauri application")
            .run(|app, event| match event {
                tauri::RunEvent::ExitRequested { .. } => {
                    APP_QUITTING.store(true, Ordering::Relaxed);
                    // Drop the desktop pet alongside the workspace so it
                    // never outlives a real quit. Tauri also tears down all
                    // windows on shutdown, but doing it explicitly here lets
                    // the pet's CloseRequested handler persist `enabled = false`
                    // before the runtime races to exit.
                    if let Some(pet) = app.get_webview_window("pet") {
                        let _ = pet.close();
                    }
                    if let Some(ws) = app.try_state::<web::WebServerState>() {
                        tauri::async_runtime::block_on(web::do_stop_web_server(&ws));
                    }
                    if let Some(tm) = app.try_state::<TerminalManager>() {
                        tm.kill_all();
                    }
                    if let Some(cm) = app.try_state::<ConnectionManager>() {
                        tauri::async_runtime::block_on(cm.disconnect_all());
                    }
                }
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    // Dock-icon click: bring the workspace forward
                    // unconditionally. `has_visible_windows` is true
                    // whenever any aux window (pet, settings, commit…)
                    // is alive, so gating on it would suppress recovery
                    // even though `main` itself is hidden.
                    // `show_main_window` is idempotent — already-visible
                    // windows just get re-focused, which is what dock
                    // activation should do anyway.
                    windows::show_main_window(app);
                }
                _ => {}
            });
    }
}

#[cfg(feature = "tauri-runtime")]
pub use tauri_app::run;
